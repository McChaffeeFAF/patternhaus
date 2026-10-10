//! `ph.fx.delay`: stereo / ping-pong delay with tempo sync, filtered feedback and mix.
//!
//! Delay times glide (100 ms one-pole on the time in samples, cubic-interpolated reads),
//! like a tape delay. The feedback path has 12 dB/oct high- and low-cut (TPT SVFs) and a
//! soft clipper (linear below ±0.5) so even 100 % feedback stays bounded.

use crate::common::{Params, SilenceTracker, input, output, stereo_desc, switch};
use ph_dsp::delay::DelayLine;
use ph_dsp::fast_tanh;
use ph_dsp::filter::{FilterMode, Svf, tpt_g};
use ph_dsp::lfo::{SYNC_DIVISIONS, sync_division_names};
use ph_dsp::smooth::OnePole;
use ph_node::*;

const MODE: usize = 0;
const SYNC: usize = 1;
const DIV_L: usize = 2;
const DIV_R: usize = 3;
const TIME_L: usize = 4;
const TIME_R: usize = 5;
const FEEDBACK: usize = 6;
const LOW_CUT: usize = 7;
const HIGH_CUT: usize = 8;
const MIX: usize = 9;

const MAX_SECONDS: f64 = 4.0;

/// Linear up to ±0.5, then a tanh knee towards ±1: keeps runaway feedback bounded without
/// colouring normal-level repeats.
#[inline]
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.5 {
        x
    } else {
        (0.5 + 0.5 * fast_tanh((a - 0.5) * 2.0)).copysign(x)
    }
}

pub fn descriptor() -> NodeDescriptor {
    let divs = sync_division_names();
    let params = vec![
        ParamInfo::choice("mode", "Mode", &["Stereo", "Ping-Pong"], 0),
        switch("sync", "Tempo Sync", true),
        ParamInfo::choice("div_l", "Division L", &divs, 8),
        ParamInfo::choice("div_r", "Division R", &divs, 8),
        ParamInfo::new(
            "time_l",
            "Time L",
            1.0,
            2000.0,
            375.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new(
            "time_r",
            "Time R",
            1.0,
            2000.0,
            375.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new("feedback", "Feedback", 0.0, 1.0, 0.4, ParamUnit::Percent),
        ParamInfo::new("low_cut", "Low Cut", 20.0, 2000.0, 100.0, ParamUnit::Hertz),
        ParamInfo::new(
            "high_cut",
            "High Cut",
            1000.0,
            20000.0,
            8000.0,
            ParamUnit::Hertz,
        ),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.3, ParamUnit::Percent),
    ];
    stereo_desc("ph.fx.delay", "Delay", params, false)
}

pub struct Delay {
    p: Params,
    sr: f64,
    bpm: f64,
    lines: [DelayLine; 2],
    time: [OnePole; 2],
    hp: [Svf; 2],
    lp: [Svf; 2],
    mix: OnePole,
    fb: OnePole,
    silence: SilenceTracker,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            bpm: 120.0,
            lines: [DelayLine::new(1), DelayLine::new(1)],
            time: [OnePole::new(1.0), OnePole::new(1.0)],
            hp: [Svf::new(), Svf::new()],
            lp: [Svf::new(), Svf::new()],
            mix: OnePole::new(0.0),
            fb: OnePole::new(0.0),
            silence: SilenceTracker::default(),
        };
        s.update_all();
        s
    }

    fn time_samples(&self, ch: usize) -> f32 {
        let secs = if self.p.on(SYNC) {
            let div = self.p.u(if ch == 0 { DIV_L } else { DIV_R });
            SYNC_DIVISIONS[div.min(SYNC_DIVISIONS.len() - 1)].1 * 60.0 / self.bpm.max(1.0)
        } else {
            self.p.v[if ch == 0 { TIME_L } else { TIME_R }] * 1e-3
        };
        let max = (self.lines[0].max_delay() as f64 - 2.0).max(2.0);
        (secs * self.sr).clamp(2.0, max) as f32
    }

    fn update_times(&mut self) {
        for ch in 0..2 {
            let t = self.time_samples(ch);
            self.time[ch].set_target(t);
        }
    }

    fn update_all(&mut self) {
        let sr = self.sr as f32;
        for i in 0..self.p.len() {
            self.apply(i);
        }
        for s in self.time.iter_mut() {
            s.set_time(0.1, sr);
            let t = s.target();
            s.reset(t);
        }
        let t = self.mix.target();
        self.mix.reset(t);
        let t = self.fb.target();
        self.fb.reset(t);
    }

    fn apply(&mut self, i: usize) {
        let sr = self.sr as f32;
        match i {
            MODE | SYNC | DIV_L | DIV_R | TIME_L | TIME_R => self.update_times(),
            FEEDBACK => {
                self.fb.set_time(0.02, sr);
                self.fb.set_target(self.p.f(FEEDBACK));
            }
            LOW_CUT => {
                let g = tpt_g(self.p.f(LOW_CUT), sr);
                self.hp.iter_mut().for_each(|f| f.set_coefs(g, 1.414));
            }
            HIGH_CUT => {
                let g = tpt_g(self.p.f(HIGH_CUT), sr);
                self.lp.iter_mut().for_each(|f| f.set_coefs(g, 1.414));
            }
            MIX => {
                self.mix.set_time(0.02, sr);
                self.mix.set_target(self.p.f(MIX));
            }
            _ => {}
        }
    }

    #[inline]
    fn loop_filter(&mut self, ch: usize, x: f32) -> f32 {
        let y = self.hp[ch].process(x, FilterMode::HighPass);
        self.lp[ch].process(y, FilterMode::LowPass)
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let ping = self.p.u(MODE) == 1;
        for i in a..b {
            let l = input(io, 0, 0, i);
            let r = input(io, 0, 1, i);
            let tl = self.time[0].next();
            let tr = self.time[1].next();
            let fb = self.fb.next();
            // read before writing: a delay of `t` samples
            let dl = self.lines[0].read_cubic(tl - 1.0);
            let dr = self.lines[1].read_cubic(if ping { tl - 1.0 } else { tr - 1.0 });
            let fl = self.loop_filter(0, dl);
            let fr = self.loop_filter(1, dr);
            if ping {
                self.lines[0].push(0.5 * (l + r) + soft_clip(fb * fr));
                self.lines[1].push(soft_clip(fb * fl));
            } else {
                self.lines[0].push(l + soft_clip(fb * fl));
                self.lines[1].push(r + soft_clip(fb * fr));
            }
            let m = self.mix.next();
            output(io, i, l * (1.0 - m) + dl * m, r * (1.0 - m) + dr * m);
        }
    }
}

impl Processor for Delay {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate;
        let n = (MAX_SECONDS * info.sample_rate) as usize + 8;
        self.lines = [DelayLine::new(n), DelayLine::new(n)];
        self.update_all();
        self.reset();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        if ctx.bpm != self.bpm && ctx.bpm > 0.0 {
            self.bpm = ctx.bpm;
            self.update_times();
        }
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
        for l in &mut self.lines {
            l.clear();
        }
        for f in self.hp.iter_mut().chain(self.lp.iter_mut()) {
            f.reset();
        }
        self.silence.reset();
    }

    /// Longest echo time × repeats until −100 dB.
    fn tail(&self) -> u32 {
        let t = self.time_samples(0).max(self.time_samples(1)) as f64;
        let fb = self.p.v[FEEDBACK];
        if fb >= 0.999 {
            return u32::MAX;
        }
        let repeats = if fb <= 1e-4 {
            1.0
        } else {
            1.0 + (-5.0 / fb.log10()).ceil()
        };
        let mult = if self.p.u(MODE) == 1 { 2.0 } else { 1.0 };
        (t * repeats * mult).min(u32::MAX as f64 - 1.0) as u32
    }
}
