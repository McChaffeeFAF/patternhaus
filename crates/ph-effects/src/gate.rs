//! `ph.fx.gate`: gate / downward expander with sidechain input and lookahead.
//!
//! Below `threshold` the signal is attenuated by `(threshold - level) * (ratio - 1)` dB,
//! limited to `range` dB: a high ratio gives a gate with a depth of `range`, a low ratio a
//! gentle expander (Zolzer, "DAFX", dynamic range control). The attenuation is smoothed in
//! the dB domain: `attack` governs opening, a `hold` time keeps the gate open after the level
//! last exceeded the threshold, and `release` governs closing. The detector can be peak or
//! RMS, stereo-linked, and can listen to the sidechain input (port 1) through a high-pass.
//! Lookahead delays the audio so the gate is open before a transient arrives.

use crate::common::{Params, SilenceTracker, impl_processor, input, output, stereo_desc};
use ph_dsp::delay::DelayLine;
use ph_dsp::dynamics::time_coef;
use ph_dsp::filter::{Svf, tpt_g};
use ph_dsp::{db_to_lin, lin_to_db};
use ph_node::*;

const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const RANGE: usize = 2;
const ATTACK: usize = 3;
const HOLD: usize = 4;
const RELEASE: usize = 5;
const DETECT: usize = 6;
const LOOKAHEAD: usize = 7;
const SIDECHAIN: usize = 8;
const SC_HPF: usize = 9;

const MAX_LOOKAHEAD_MS: f64 = 10.0;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new(
            "threshold",
            "Threshold",
            -80.0,
            0.0,
            -40.0,
            ParamUnit::Decibels,
        ),
        ParamInfo::new("ratio", "Ratio", 1.0, 20.0, 20.0, ParamUnit::Ratio),
        ParamInfo::new("range", "Range", -90.0, 0.0, -60.0, ParamUnit::Decibels),
        ParamInfo::new(
            "attack",
            "Attack",
            0.05,
            100.0,
            1.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new("hold", "Hold", 0.0, 2000.0, 50.0, ParamUnit::Milliseconds),
        ParamInfo::new(
            "release",
            "Release",
            5.0,
            4000.0,
            150.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::choice("detector", "Detector", &["Peak", "RMS"], 0),
        ParamInfo::new(
            "lookahead",
            "Lookahead",
            0.0,
            MAX_LOOKAHEAD_MS,
            0.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::choice("sidechain", "Sidechain", &["Internal", "External"], 0),
        ParamInfo::new(
            "sc_hpf",
            "Sidechain High-Pass",
            20.0,
            2000.0,
            20.0,
            ParamUnit::Hertz,
        ),
    ];
    stereo_desc("ph.fx.gate", "Gate", params, true)
}

/// Static curve: gain in dB (always <= 0) for a detector level `x` (dB).
pub fn static_gain_db(x: f32, threshold: f32, ratio: f32, range: f32) -> f32 {
    if x >= threshold {
        0.0
    } else {
        -((threshold - x) * (ratio - 1.0)).min(-range)
    }
}

pub struct Gate {
    p: Params,
    sr: f32,
    att: f32,
    rel: f32,
    peak_rel: f32,
    rms_coef: f32,
    hold_len: u32,
    hold: u32,
    env: f32,
    ms: f32,
    gr: f32,
    lookahead: usize,
    delay: [DelayLine; 2],
    hpf: [Svf; 2],
    silence: SilenceTracker,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            att: 0.0,
            rel: 0.0,
            peak_rel: 0.0,
            rms_coef: 0.0,
            hold_len: 0,
            hold: 0,
            env: 0.0,
            ms: 0.0,
            gr: 0.0,
            lookahead: 0,
            delay: [DelayLine::new(1), DelayLine::new(1)],
            hpf: [Svf::new(), Svf::new()],
            silence: SilenceTracker::default(),
        };
        s.snap();
        s
    }

    fn snap(&mut self) {
        self.peak_rel = time_coef(0.01, self.sr);
        self.rms_coef = time_coef(0.01, self.sr);
        for i in 0..self.p.len() {
            self.apply(i);
        }
    }

    fn apply(&mut self, i: usize) {
        match i {
            ATTACK => self.att = time_coef(self.p.f(ATTACK) * 1e-3, self.sr),
            RELEASE => self.rel = time_coef(self.p.f(RELEASE) * 1e-3, self.sr),
            HOLD => self.hold_len = (self.p.v[HOLD] * 1e-3 * self.sr as f64) as u32,
            LOOKAHEAD => {
                let max = self.delay[0].max_delay();
                self.lookahead =
                    ((self.p.v[LOOKAHEAD] * 1e-3 * self.sr as f64).round() as usize).min(max);
            }
            SC_HPF => {
                let g = tpt_g(self.p.f(SC_HPF), self.sr);
                for h in &mut self.hpf {
                    h.set_coefs(g, std::f32::consts::SQRT_2);
                }
            }
            _ => {}
        }
    }

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        let max = (MAX_LOOKAHEAD_MS * 1e-3 * info.sample_rate).ceil() as usize + 1;
        self.delay = [DelayLine::new(max), DelayLine::new(max)];
        self.snap();
    }

    fn on_reset(&mut self) {
        self.env = 0.0;
        self.ms = 0.0;
        self.gr = 0.0;
        self.hold = 0;
        for d in &mut self.delay {
            d.clear();
        }
        for h in &mut self.hpf {
            h.reset();
        }
    }

    fn latency_samples(&self) -> u32 {
        self.lookahead as u32
    }

    fn tail_samples(&self) -> u32 {
        // an open gate stays open for the hold time and then closes over the release
        self.lookahead as u32
            + self.hold_len
            + (self.p.v[RELEASE] * 1e-3 * self.sr as f64) as u32
            + 1
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let thr = self.p.f(THRESHOLD);
        let ratio = self.p.f(RATIO);
        let range = self.p.f(RANGE);
        let rms = self.p.u(DETECT) == 1;
        let sc_port = if self.p.u(SIDECHAIN) == 1 { 1 } else { 0 };
        let hpf_on = self.p.f(SC_HPF) > 20.5;
        for i in a..b {
            let l = input(io, 0, 0, i);
            let r = input(io, 0, 1, i);
            let mut dl = input(io, sc_port, 0, i);
            let mut dr = input(io, sc_port, 1, i);
            if hpf_on {
                dl = self.hpf[0].tick(dl).2;
                dr = self.hpf[1].tick(dr).2;
            }
            let level = if rms {
                let m = 0.5 * (dl * dl + dr * dr);
                self.ms = ph_dsp::flush_denormal(m + (self.ms - m) * self.rms_coef);
                self.ms.sqrt()
            } else {
                let p = dl.abs().max(dr.abs());
                self.env = ph_dsp::flush_denormal(p.max(self.env * self.peak_rel));
                self.env
            };
            let x_db = lin_to_db(level).max(-200.0);
            let target = -static_gain_db(x_db, thr, ratio, range); // attenuation, >= 0
            if x_db >= thr {
                self.hold = self.hold_len;
            }
            if target < self.gr {
                // opening
                self.gr = ph_dsp::flush_denormal(target + (self.gr - target) * self.att);
            } else if self.hold > 0 {
                self.hold -= 1;
            } else {
                self.gr = ph_dsp::flush_denormal(target + (self.gr - target) * self.rel);
            }
            let g = db_to_lin(-self.gr);
            self.delay[0].push(l);
            self.delay[1].push(r);
            let ol = self.delay[0].read(self.lookahead);
            let or = self.delay[1].read(self.lookahead);
            output(io, i, ol * g, or * g);
        }
    }
}

impl_processor!(Gate);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_curve_shape() {
        // above threshold: unity
        assert_eq!(static_gain_db(-10.0, -40.0, 4.0, -60.0), 0.0);
        // expander: 3 dB below threshold with ratio 4 -> -9 dB
        assert!((static_gain_db(-43.0, -40.0, 4.0, -60.0) + 9.0).abs() < 1e-5);
        // range limits the attenuation
        assert!((static_gain_db(-100.0, -40.0, 20.0, -30.0) + 30.0).abs() < 1e-5);
        assert_eq!(descriptor().params.len(), 10);
    }
}
