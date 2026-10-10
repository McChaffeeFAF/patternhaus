//! `ph.fx.saturator`: waveshaping distortion with four curves, oversampled 4× (live) or
//! 8× (high quality) through linear-phase halfband cascades.
//!
//! Curves: soft (tanh), hard (clip), tube (biased tanh → even harmonics, DC removed) and
//! tape (algebraic sigmoid with drive-dependent high-frequency loss). On top of
//! oversampling, each curve uses first-order antiderivative anti-aliasing (ADAA: Parker,
//! Zavalishin & Le Bivic, "Reducing the aliasing of nonlinear waveshaping using
//! continuous-time convolution", DAFx-16), which matters most for the hard clipper. A tilt
//! EQ sets the tone; the dry path is delayed by the oversampler's latency so the mix stays
//! phase-true.

use crate::common::{Params, SilenceTracker, input, output, stereo_desc};
use ph_dsp::db_to_lin;
use ph_dsp::delay::DelayLine;
use ph_dsp::filter::{DcBlocker, OnePoleTpt};
use ph_dsp::oversample::Oversampler;
use ph_dsp::smooth::OnePole;
use ph_node::*;

const CURVE: usize = 0;
const DRIVE: usize = 1;
const TONE: usize = 2;
const MIX: usize = 3;
const OUTPUT: usize = 4;

const TUBE_BIAS: f32 = 0.35;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::choice("curve", "Curve", &["Soft", "Hard", "Tube", "Tape"], 0),
        ParamInfo::new("drive", "Drive", 0.0, 36.0, 6.0, ParamUnit::Decibels),
        ParamInfo::new("tone", "Tone", -1.0, 1.0, 0.0, ParamUnit::None),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 1.0, ParamUnit::Percent),
        ParamInfo::new("output", "Output", -24.0, 12.0, 0.0, ParamUnit::Decibels),
    ];
    stereo_desc("ph.fx.saturator", "Saturator", params, false)
}

/// `ln(cosh(x))`, overflow-safe.
#[inline]
fn log_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

/// The static curves (input already multiplied by drive).
#[inline]
pub fn shape(curve: usize, x: f64) -> f64 {
    let b = TUBE_BIAS as f64;
    match curve {
        0 => x.tanh(),
        1 => x.clamp(-1.0, 1.0),
        2 => (x + b).tanh() - b.tanh(),
        _ => x / (1.0 + x * x).sqrt(),
    }
}

/// Antiderivatives of [`shape`].
#[inline]
pub fn shape_antiderivative(curve: usize, x: f64) -> f64 {
    let b = TUBE_BIAS as f64;
    match curve {
        0 => log_cosh(x),
        1 => {
            if x.abs() <= 1.0 {
                0.5 * x * x
            } else {
                x.abs() - 0.5
            }
        }
        2 => log_cosh(x + b) - b.tanh() * x,
        _ => (1.0 + x * x).sqrt(),
    }
}

/// First-order ADAA state for one channel.
#[derive(Clone, Copy, Default)]
struct Adaa {
    x1: f64,
    f1: f64,
}

impl Adaa {
    #[inline]
    fn process(&mut self, curve: usize, x: f64) -> f64 {
        let f = shape_antiderivative(curve, x);
        let dx = x - self.x1;
        let y = if dx.abs() > 1e-6 {
            (f - self.f1) / dx
        } else {
            shape(curve, 0.5 * (x + self.x1))
        };
        self.x1 = x;
        self.f1 = f;
        y
    }

    fn rebase(&mut self, curve: usize) {
        self.f1 = shape_antiderivative(curve, self.x1);
    }
}

pub struct Saturator {
    p: Params,
    sr: f64,
    os: [Oversampler; 2],
    tmp: [Vec<f32>; 2],
    adaa: [Adaa; 2],
    dry: [DelayLine; 2],
    tape_lp: [OnePoleTpt; 2],
    tilt: [OnePoleTpt; 2],
    dc: [DcBlocker; 2],
    drive: OnePole,
    tone: OnePole,
    mix: OnePole,
    out: OnePole,
    silence: SilenceTracker,
}

impl Default for Saturator {
    fn default() -> Self {
        Self::new()
    }
}

impl Saturator {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            os: [Oversampler::new(4, 1), Oversampler::new(4, 1)],
            tmp: [Vec::new(), Vec::new()],
            adaa: [Adaa::default(); 2],
            dry: [DelayLine::new(1), DelayLine::new(1)],
            tape_lp: Default::default(),
            tilt: Default::default(),
            dc: Default::default(),
            drive: OnePole::new(1.0),
            tone: OnePole::new(0.0),
            mix: OnePole::new(1.0),
            out: OnePole::new(1.0),
            silence: SilenceTracker::default(),
        };
        s.update_all();
        s
    }

    fn update_all(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let hs = (self.sr * self.os[0].factor() as f64) as f32;
        self.drive.set_time(0.01, hs);
        for s in [&mut self.tone, &mut self.mix, &mut self.out] {
            s.set_time(0.01, self.sr as f32);
        }
        for s in [
            &mut self.drive,
            &mut self.tone,
            &mut self.mix,
            &mut self.out,
        ] {
            let t = s.target();
            s.reset(t);
        }
        let sr = self.sr as f32;
        for c in 0..2 {
            self.tilt[c].set(800.0, sr);
            self.dc[c] = DcBlocker::new(8.0, sr);
        }
        self.update_tape();
    }

    fn update_tape(&mut self) {
        // more drive → darker, as tape saturates its highs first
        let hs = (self.sr * self.os[0].factor() as f64) as f32;
        let fc = 18000.0 / (1.0 + self.p.f(DRIVE) / 18.0);
        for f in &mut self.tape_lp {
            f.set(fc, hs);
        }
    }

    fn apply(&mut self, i: usize) {
        match i {
            DRIVE => {
                self.drive.set_target(db_to_lin(self.p.f(DRIVE)));
                self.update_tape();
            }
            CURVE => {
                let c = self.p.u(CURVE);
                self.adaa.iter_mut().for_each(|a| a.rebase(c));
            }
            TONE => self.tone.set_target(self.p.f(TONE)),
            MIX => self.mix.set_target(self.p.f(MIX)),
            OUTPUT => self.out.set_target(db_to_lin(self.p.f(OUTPUT))),
            _ => {}
        }
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let n = b - a;
        let curve = self.p.u(CURVE);
        for ch in 0..2 {
            for i in 0..n {
                self.tmp[ch][i] = input(io, 0, ch, a + i);
            }
        }
        let f = self.os[0].factor();
        let [os0, os1] = &mut self.os;
        let h0 = os0.up(&self.tmp[0][..n]);
        let h1 = os1.up(&self.tmp[1][..n]);
        for j in 0..n * f {
            let d = self.drive.next() as f64;
            let x0 = self.adaa[0].process(curve, h0[j] as f64 * d) as f32;
            let x1 = self.adaa[1].process(curve, h1[j] as f64 * d) as f32;
            if curve == 3 {
                h0[j] = self.tape_lp[0].lp(x0);
                h1[j] = self.tape_lp[1].lp(x1);
            } else {
                h0[j] = x0;
                h1[j] = x1;
            }
        }
        let [t0, t1] = &mut self.tmp;
        os0.down(&mut t0[..n]);
        os1.down(&mut t1[..n]);
        let lat = os0.latency() as usize;
        for i in 0..n {
            let tone = self.tone.next();
            let m = self.mix.next();
            let g = self.out.next();
            let lo_g = db_to_lin(-6.0 * tone);
            let hi_g = db_to_lin(6.0 * tone);
            let mut o = [0.0f32; 2];
            for (c, oc) in o.iter_mut().enumerate() {
                let mut w = self.tmp[c][i];
                if curve == 2 {
                    w = self.dc[c].process(w);
                }
                let lo = self.tilt[c].lp(w);
                w = lo * lo_g + (w - lo) * hi_g;
                self.dry[c].push(input(io, 0, c, a + i));
                let d = self.dry[c].read(lat);
                *oc = (d * (1.0 - m) + w * m) * g;
            }
            output(io, a + i, o[0], o[1]);
        }
    }
}

impl Processor for Saturator {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate;
        let factor = match info.quality {
            Quality::Live => 4,
            Quality::High => 8,
        };
        let mf = info.max_frames.max(1);
        self.os = [Oversampler::new(factor, mf), Oversampler::new(factor, mf)];
        let lat = self.os[0].latency() as usize;
        self.dry = [DelayLine::new(lat + 1), DelayLine::new(lat + 1)];
        self.tmp = [vec![0.0; mf], vec![0.0; mf]];
        self.update_all();
        self.reset();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        let n = ctx.frames;
        let mut pos = 0;
        for ev in io.events {
            let f = (ev.frame as usize).min(n);
            if f > pos {
                self.render(io, pos, f);
                pos = f;
            }
            if let EventKind::Param { id, value } = ev.kind {
                self.set_param(id, value);
            }
        }
        if pos < n {
            self.render(io, pos, n);
        }
        if self.silence.update(io, n, self.tail()) {
            ProcessStatus::Sleep
        } else {
            ProcessStatus::Continue
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        if let Some(i) = self.p.set(id, value) {
            self.apply(i);
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.p.get(id)
    }

    fn reset(&mut self) {
        for c in 0..2 {
            self.os[c].reset();
            self.dry[c].clear();
            self.tape_lp[c].reset();
            self.tilt[c].reset();
            self.dc[c].reset();
            self.adaa[c] = Adaa::default();
            self.adaa[c].rebase(self.p.u(CURVE));
        }
        self.silence.reset();
    }

    fn latency(&self) -> u32 {
        self.os[0].latency()
    }

    fn tail(&self) -> u32 {
        self.latency() + (self.sr * 0.1) as u32
    }
}
