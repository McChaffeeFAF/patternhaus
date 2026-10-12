//! `ph.fx.flanger`: feedback flanger.
//!
//! A short modulated delay (`manual` centre, swept by `depth` around it with a sine LFO)
//! summed with the dry signal, with signed feedback from the delay output back to its input
//! (Dattorro, "Effect Design Part 2: Delay-Line Modulation and Chorus", JAES 1997). The
//! right channel's LFO leads the left by `stereo` degrees. The read is compensated for the
//! one-sample feedback loop so the delay time equals the set value.

use crate::common::{
    Params, SilenceTracker, crossfade, impl_processor, input, output, stereo_desc, wrap01,
};
use ph_dsp::delay::DelayLine;
use ph_dsp::flush_denormal;
use ph_dsp::sin_turns;
use ph_dsp::smooth::OnePole;
use ph_node::*;

const RATE: usize = 0;
const DEPTH: usize = 1;
const FEEDBACK: usize = 2;
const MANUAL: usize = 3;
const STEREO: usize = 4;
const MIX: usize = 5;

const MAX_MS: f64 = 20.0;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new("rate", "Rate", 0.02, 10.0, 0.3, ParamUnit::Hertz),
        ParamInfo::new("depth", "Depth", 0.0, 1.0, 0.7, ParamUnit::Percent),
        ParamInfo::new("feedback", "Feedback", -0.95, 0.95, 0.5, ParamUnit::Percent),
        ParamInfo::new("manual", "Manual", 0.1, 10.0, 2.0, ParamUnit::Milliseconds),
        ParamInfo::new("stereo", "Stereo Phase", 0.0, 180.0, 90.0, ParamUnit::None),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.5, ParamUnit::Percent),
    ];
    stereo_desc("ph.fx.flanger", "Flanger", params, false)
}

pub struct Flanger {
    p: Params,
    sr: f32,
    phase: f64,
    inc: f64,
    line: [DelayLine; 2],
    depth: OnePole,
    manual: OnePole,
    fb: OnePole,
    mix: OnePole,
    silence: SilenceTracker,
}

impl Default for Flanger {
    fn default() -> Self {
        Self::new()
    }
}

impl Flanger {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            phase: 0.0,
            inc: 0.0,
            line: [DelayLine::new(1), DelayLine::new(1)],
            depth: OnePole::new(0.0),
            manual: OnePole::new(0.0),
            fb: OnePole::new(0.0),
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
            &mut self.manual,
            &mut self.fb,
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
            MANUAL => {
                self.manual.set_time(0.02, sr);
                self.manual.set_target(self.p.f(MANUAL) * 1e-3 * sr);
            }
            FEEDBACK => {
                self.fb.set_time(0.02, sr);
                self.fb.set_target(self.p.f(FEEDBACK));
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
        // feedback tail: loops until -90 dB, capped at 10 s
        let fb = self.p.v[FEEDBACK].abs().min(0.95);
        let loops = if fb < 1e-3 {
            1.0
        } else {
            90.0 / (-20.0 * fb.log10())
        };
        let d = self.p.v[MANUAL] * 2e-3 * self.sr as f64;
        ((d * loops) as u32).min(10 * self.sr as u32) + 4
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let off = self.p.v[STEREO] / 360.0;
        let max_d = (self.line[0].max_delay() - 2) as f32;
        for i in a..b {
            let depth = self.depth.next();
            let manual = self.manual.next();
            let fb = self.fb.next();
            let m = self.mix.next();
            let x = [input(io, 0, 0, i), input(io, 0, 1, i)];
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                let lfo = sin_turns(wrap01(self.phase + off * c as f64));
                let d = (manual * (1.0 + 0.9 * depth * lfo)).clamp(2.0, max_d);
                // reading before the push adds one sample of delay
                let y = self.line[c].read_cubic(d - 1.0);
                self.line[c].push(flush_denormal(x[c] + fb * y));
                out[c] = crossfade(x[c], y, m);
            }
            self.phase = wrap01(self.phase + self.inc);
            output(io, i, out[0], out[1]);
        }
    }
}

impl_processor!(Flanger);
