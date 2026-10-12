//! `ph.fx.bitcrush`: bit-depth and sample-rate reduction.
//!
//! Amplitude is quantised to a mid-tread grid of step `2^(1 - bits)` (so `bits` bits span
//! -1..1 and silence stays exactly zero), optionally with TPDF dither of up to one step
//! (Lipshitz, Wannamaker & Vanderkooy, "Quantization and Dither", JAES 1992). Sample-rate
//! reduction is a sample-and-hold at `rate` Hz; with `antialias` on, a 24 dB/oct low-pass at
//! 0.45 x `rate` precedes the hold so the fold-back of the reduced rate is suppressed.

use crate::common::{
    Params, SilenceTracker, crossfade, impl_processor, input, output, stereo_desc, switch, whole,
};
use ph_dsp::Rng;
use ph_dsp::filter::{Svf, tpt_g};
use ph_dsp::smooth::OnePole;
use ph_node::*;

const BITS: usize = 0;
const RATE: usize = 1;
const ANTIALIAS: usize = 2;
const DITHER: usize = 3;
const MIX: usize = 4;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        whole(ParamInfo::new(
            "bits",
            "Bit Depth",
            1.0,
            24.0,
            8.0,
            ParamUnit::None,
        )),
        ParamInfo::new(
            "rate",
            "Sample Rate",
            100.0,
            48000.0,
            48000.0,
            ParamUnit::Hertz,
        ),
        switch("antialias", "Anti-Alias", true),
        ParamInfo::new("dither", "Dither", 0.0, 1.0, 0.0, ParamUnit::Percent),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 1.0, ParamUnit::Percent),
    ];
    stereo_desc("ph.fx.bitcrush", "Bitcrusher", params, false)
}

/// Quantise `x` to the grid of `bits` bits (step `2^(1 - bits)`), clipped to -1..1.
#[inline]
pub fn quantise(x: f32, bits: u32) -> f32 {
    let step = (1.0 - bits as f32).exp2();
    ((x / step).round() * step).clamp(-1.0, 1.0)
}

pub struct Bitcrush {
    p: Params,
    sr: f32,
    seed: u64,
    rng: Rng,
    phase: f64,
    held: [f32; 2],
    aa: [[Svf; 2]; 2],
    mix: OnePole,
    silence: SilenceTracker,
}

impl Default for Bitcrush {
    fn default() -> Self {
        Self::new()
    }
}

impl Bitcrush {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            seed: 0,
            rng: Rng::new(0),
            phase: 1.0,
            held: [0.0; 2],
            aa: [[Svf::new(), Svf::new()], [Svf::new(), Svf::new()]],
            mix: OnePole::new(1.0),
            silence: SilenceTracker::default(),
        };
        s.snap();
        s
    }

    fn snap(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let v = self.mix.target();
        self.mix.reset(v);
    }

    fn apply(&mut self, i: usize) {
        match i {
            RATE => {
                let fc = (0.45 * self.p.f(RATE)).min(0.45 * self.sr);
                for ch in &mut self.aa {
                    for s in ch.iter_mut() {
                        s.set_coefs(tpt_g(fc, self.sr), std::f32::consts::SQRT_2);
                    }
                }
            }
            MIX => {
                self.mix.set_time(0.02, self.sr);
                self.mix.set_target(self.p.f(MIX));
            }
            _ => {}
        }
    }

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        self.seed = info.seed;
        self.snap();
        self.apply(RATE);
    }

    fn on_reset(&mut self) {
        self.rng = Rng::new(self.seed ^ 0xB17C_2A5B);
        self.phase = 1.0;
        self.held = [0.0; 2];
        for ch in &mut self.aa {
            for s in ch.iter_mut() {
                s.reset();
            }
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        // dither makes noise out of silence, so the effect never sleeps then
        if self.p.v[DITHER] > 0.0 { u32::MAX } else { 0 }
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let bits = self.p.u(BITS).clamp(1, 24) as u32;
        let step = (1.0 - bits as f32).exp2();
        let dither = self.p.f(DITHER) * step;
        let ratio = (self.p.v[RATE] / self.sr as f64).min(1.0);
        let aa = self.p.on(ANTIALIAS) && ratio < 1.0;
        for i in a..b {
            let m = self.mix.next();
            let x = [input(io, 0, 0, i), input(io, 0, 1, i)];
            let mut v = x;
            if aa {
                for c in 0..2 {
                    let y = self.aa[c][0].tick(x[c]).0;
                    v[c] = self.aa[c][1].tick(y).0;
                }
            }
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                for c in 0..2 {
                    let d = if dither > 0.0 {
                        dither * (self.rng.next_f32() - self.rng.next_f32())
                    } else {
                        0.0
                    };
                    self.held[c] = quantise(v[c] + d, bits);
                }
            }
            self.phase += ratio;
            output(
                io,
                i,
                crossfade(x[0], self.held[0], m),
                crossfade(x[1], self.held[1], m),
            );
        }
    }
}

impl_processor!(Bitcrush);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiser_grid() {
        assert_eq!(quantise(0.3, 2), 0.5);
        assert_eq!(quantise(-0.3, 2), -0.5);
        assert_eq!(quantise(0.1, 2), 0.0);
        assert_eq!(quantise(1.0, 1), 1.0);
        assert_eq!(quantise(0.2, 8), 26.0 / 128.0);
        assert_eq!(quantise(5.0, 8), 1.0);
    }
}
