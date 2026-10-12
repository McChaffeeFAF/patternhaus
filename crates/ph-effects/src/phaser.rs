//! `ph.fx.phaser`: all-pass chain phaser.
//!
//! `stages` first-order all-pass filters in series (coefficient `a = (t - 1)/(t + 1)` with
//! `t = tan(pi f / fs)`, Smith, "Introduction to Digital Filters") whose break frequency is
//! swept exponentially around `centre` by a sine LFO. The chain output is mixed with the dry
//! signal, giving a notch for every pair of stages, and optionally fed back (signed). The
//! right channel's LFO leads the left by `stereo` degrees.

use crate::common::{
    Params, SilenceTracker, crossfade, impl_processor, input, output, stereo_desc, whole, wrap01,
};
use ph_dsp::flush_denormal;
use ph_dsp::sin_turns;
use ph_dsp::smooth::OnePole;
use ph_node::*;

const STAGES: usize = 0;
const RATE: usize = 1;
const DEPTH: usize = 2;
const FEEDBACK: usize = 3;
const CENTRE: usize = 4;
const STEREO: usize = 5;
const MIX: usize = 6;

const MAX_STAGES: usize = 12;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        whole(ParamInfo::new(
            "stages",
            "Stages",
            2.0,
            12.0,
            6.0,
            ParamUnit::None,
        )),
        ParamInfo::new("rate", "Rate", 0.02, 10.0, 0.4, ParamUnit::Hertz),
        ParamInfo::new("depth", "Depth", 0.0, 1.0, 0.7, ParamUnit::Percent),
        ParamInfo::new("feedback", "Feedback", -0.95, 0.95, 0.4, ParamUnit::Percent),
        ParamInfo::new("centre", "Centre", 100.0, 8000.0, 1000.0, ParamUnit::Hertz),
        ParamInfo::new("stereo", "Stereo Phase", 0.0, 180.0, 90.0, ParamUnit::None),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.5, ParamUnit::Percent),
    ];
    stereo_desc("ph.fx.phaser", "Phaser", params, false)
}

pub struct Phaser {
    p: Params,
    sr: f32,
    phase: f64,
    inc: f64,
    z: [[f32; MAX_STAGES]; 2],
    y: [f32; 2],
    depth: OnePole,
    fb: OnePole,
    centre: OnePole,
    mix: OnePole,
    silence: SilenceTracker,
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new()
    }
}

impl Phaser {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            phase: 0.0,
            inc: 0.0,
            z: [[0.0; MAX_STAGES]; 2],
            y: [0.0; 2],
            depth: OnePole::new(0.0),
            fb: OnePole::new(0.0),
            centre: OnePole::new(1000.0),
            mix: OnePole::new(0.0),
            silence: SilenceTracker::default(),
        };
        s.snap();
        s
    }

    fn snap(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        for s in [
            &mut self.depth,
            &mut self.fb,
            &mut self.centre,
            &mut self.mix,
        ] {
            let v = s.target();
            s.reset(v);
        }
    }

    fn apply(&mut self, i: usize) {
        let sr = self.sr;
        match i {
            RATE => self.inc = self.p.v[RATE] / sr as f64,
            DEPTH => {
                self.depth.set_time(0.02, sr);
                self.depth.set_target(self.p.f(DEPTH));
            }
            FEEDBACK => {
                self.fb.set_time(0.02, sr);
                self.fb.set_target(self.p.f(FEEDBACK));
            }
            CENTRE => {
                self.centre.set_time(0.02, sr);
                self.centre.set_target(self.p.f(CENTRE));
            }
            MIX => {
                self.mix.set_time(0.02, sr);
                self.mix.set_target(self.p.f(MIX));
            }
            _ => {}
        }
    }

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        self.snap();
    }

    fn on_reset(&mut self) {
        self.phase = 0.0;
        self.z = [[0.0; MAX_STAGES]; 2];
        self.y = [0.0; 2];
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        // the chain rings for tens of ms; with feedback, until -90 dB (capped at 10 s)
        let fb = self.p.v[FEEDBACK].abs().min(0.95);
        let loops = if fb < 1e-3 {
            1.0
        } else {
            90.0 / (-20.0 * fb.log10())
        };
        ((0.05 * loops * self.sr as f64) as u32).min(10 * self.sr as u32) + 4
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let stages = self.p.u(STAGES).clamp(2, MAX_STAGES);
        let off = self.p.v[STEREO] / 360.0;
        let pi_sr = std::f32::consts::PI / self.sr;
        let top = 0.45 * self.sr;
        for i in a..b {
            let depth = self.depth.next();
            let fb = self.fb.next();
            let centre = self.centre.next();
            let m = self.mix.next();
            let x = [input(io, 0, 0, i), input(io, 0, 1, i)];
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                let lfo = sin_turns(wrap01(self.phase + off * c as f64));
                let f = (centre * (2.0 * depth * lfo).exp2()).clamp(20.0, top);
                let t = (pi_sr * f).tan();
                let coef = (t - 1.0) / (t + 1.0);
                let mut v = x[c] + fb * self.y[c];
                for s in 0..stages {
                    let y = coef * v + self.z[c][s];
                    self.z[c][s] = flush_denormal(v - coef * y);
                    v = y;
                }
                let v = flush_denormal(v);
                self.y[c] = v;
                out[c] = crossfade(x[c], v, m);
            }
            self.phase = wrap01(self.phase + self.inc);
            output(io, i, out[0], out[1]);
        }
    }
}

impl_processor!(Phaser);
