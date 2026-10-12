//! `ph.fx.chorus`: multi-voice chorus.
//!
//! Each voice reads a short delay line whose length is swept by a sine LFO; the voices share
//! one rate and are spaced evenly in phase around the cycle, then panned across the stereo
//! field by `spread` (equal-power). Fractional delays use cubic Hermite interpolation
//! (Smith, "Physical Audio Signal Processing"). Depth, delay and mix are smoothed.

use crate::common::{
    Params, SilenceTracker, crossfade, impl_processor, input, output, stereo_desc, whole, wrap01,
};
use ph_dsp::delay::DelayLine;
use ph_dsp::sin_turns;
use ph_dsp::smooth::OnePole;
use ph_node::*;

const VOICES: usize = 0;
const RATE: usize = 1;
const DEPTH: usize = 2;
const DELAY: usize = 3;
const SPREAD: usize = 4;
const MIX: usize = 5;

const MAX_MS: f64 = 60.0;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        whole(ParamInfo::new(
            "voices",
            "Voices",
            1.0,
            6.0,
            3.0,
            ParamUnit::None,
        )),
        ParamInfo::new("rate", "Rate", 0.05, 10.0, 0.8, ParamUnit::Hertz),
        ParamInfo::new("depth", "Depth", 0.0, 10.0, 2.0, ParamUnit::Milliseconds),
        ParamInfo::new("delay", "Delay", 1.0, 40.0, 12.0, ParamUnit::Milliseconds),
        ParamInfo::new("spread", "Spread", 0.0, 1.0, 0.8, ParamUnit::Percent),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.5, ParamUnit::Percent),
    ];
    stereo_desc("ph.fx.chorus", "Chorus", params, false)
}

pub struct Chorus {
    p: Params,
    sr: f32,
    phase: f64,
    inc: f64,
    line: [DelayLine; 2],
    depth: OnePole,
    delay: OnePole,
    mix: OnePole,
    silence: SilenceTracker,
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl Chorus {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            phase: 0.0,
            inc: 0.0,
            line: [DelayLine::new(1), DelayLine::new(1)],
            depth: OnePole::new(0.0),
            delay: OnePole::new(0.0),
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
        for s in [&mut self.depth, &mut self.delay, &mut self.mix] {
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
                self.depth.set_target(self.p.f(DEPTH) * 1e-3 * sr);
            }
            DELAY => {
                self.delay.set_time(0.02, sr);
                self.delay.set_target(self.p.f(DELAY) * 1e-3 * sr);
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
        let n = (MAX_MS * 1e-3 * info.sample_rate) as usize + 8;
        self.line = [DelayLine::new(n), DelayLine::new(n)];
        self.snap();
    }

    fn on_reset(&mut self) {
        self.phase = 0.0;
        for l in &mut self.line {
            l.clear();
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        ((self.p.v[DELAY] + self.p.v[DEPTH]) * 1e-3 * self.sr as f64) as u32 + 4
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let n = self.p.u(VOICES).clamp(1, 6);
        let spread = self.p.f(SPREAD);
        let max_d = (self.line[0].max_delay() - 2) as f32;
        let norm = 1.0 / (n as f32).sqrt();
        for i in a..b {
            let depth = self.depth.next();
            let delay = self.delay.next();
            let m = self.mix.next();
            let (l, r) = (input(io, 0, 0, i), input(io, 0, 1, i));
            self.line[0].push(l);
            self.line[1].push(r);
            let (mut wl, mut wr) = (0.0f32, 0.0f32);
            for v in 0..n {
                let lfo = sin_turns(wrap01(self.phase + v as f64 / n as f64));
                let d = (delay + depth * lfo).clamp(1.0, max_d);
                let pos = if n == 1 {
                    0.0
                } else {
                    -1.0 + 2.0 * v as f32 / (n - 1) as f32
                };
                let gl = (1.0 - spread * pos).sqrt() * norm;
                let gr = (1.0 + spread * pos).sqrt() * norm;
                wl += gl * self.line[0].read_cubic(d);
                wr += gr * self.line[1].read_cubic(d);
            }
            self.phase = wrap01(self.phase + self.inc);
            output(io, i, crossfade(l, wl, m), crossfade(r, wr, m));
        }
    }
}

impl_processor!(Chorus);
