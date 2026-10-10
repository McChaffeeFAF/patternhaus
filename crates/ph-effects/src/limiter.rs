//! `ph.fx.limiter`: true-peak brickwall limiter with lookahead.
//!
//! Detection runs on a 4× oversampled (windowed-sinc polyphase, parabolic refinement)
//! estimate of each input sample's true peak, stereo-linked. The required gain is held
//! over the lookahead window plus one sample (running minimum), released with a one-pole
//! (instant attack), then smoothed by a moving average over the lookahead window. Because every value in the averaging window is at most the
//! gain the upcoming peak needs, the output reaches that gain exactly when the delayed
//! peak arrives: no overshoot by construction, and no gain step (no clicks).

use crate::common::{Params, SilenceTracker, input, output, stereo_desc};
use ph_dsp::db_to_lin;
use ph_dsp::delay::DelayLine;
use ph_dsp::dynamics::time_coef;
use ph_dsp::oversample::TruePeak4x;
use ph_dsp::smooth::OnePole;
use ph_node::*;

const INPUT_GAIN: usize = 0;
const CEILING: usize = 1;
const RELEASE: usize = 2;
const LOOKAHEAD: usize = 3;

const MAX_LOOKAHEAD_MS: f64 = 10.0;
/// Safety margin below the ceiling for gain-modulation side bands (−0.02 dB).
const MARGIN: f32 = 0.997_7;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new(
            "input_gain",
            "Input Gain",
            0.0,
            24.0,
            0.0,
            ParamUnit::Decibels,
        ),
        ParamInfo::new("ceiling", "Ceiling", -24.0, 0.0, -1.0, ParamUnit::Decibels),
        ParamInfo::new(
            "release",
            "Release",
            1.0,
            1000.0,
            100.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new(
            "lookahead",
            "Lookahead",
            0.5,
            MAX_LOOKAHEAD_MS,
            2.0,
            ParamUnit::Milliseconds,
        ),
    ];
    stereo_desc("ph.fx.limiter", "Limiter", params, false)
}

pub struct Limiter {
    p: Params,
    sr: f32,
    in_gain: OnePole,
    ceiling: f32,
    rel: f32,
    window: usize,
    tp: [TruePeak4x; 2],
    delay: [DelayLine; 2],
    // running minimum (monotonic deque in a ring)
    dq_idx: Vec<u64>,
    dq_val: Vec<f32>,
    dq_head: usize,
    dq_len: usize,
    t: u64,
    // release
    r: f32,
    // moving average
    avg: Vec<f64>,
    avg_pos: usize,
    sum: f64,
    silence: SilenceTracker,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            in_gain: OnePole::new(1.0),
            ceiling: 1.0,
            rel: 0.0,
            window: 1,
            tp: [TruePeak4x::new(), TruePeak4x::new()],
            delay: [DelayLine::new(1), DelayLine::new(1)],
            dq_idx: Vec::new(),
            dq_val: Vec::new(),
            dq_head: 0,
            dq_len: 0,
            t: 0,
            r: 1.0,
            avg: Vec::new(),
            avg_pos: 0,
            sum: 0.0,
            silence: SilenceTracker::default(),
        };
        s.alloc(48000.0);
        s.update_all();
        s
    }

    fn alloc(&mut self, sr: f64) {
        let max_w = (MAX_LOOKAHEAD_MS * 1e-3 * sr).ceil() as usize + 1;
        self.dq_idx = vec![0; max_w + 2];
        self.dq_val = vec![0.0; max_w + 2];
        self.avg = vec![1.0; max_w];
        let d = TruePeak4x::DELAY + max_w;
        self.delay = [DelayLine::new(d), DelayLine::new(d)];
    }

    fn update_all(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let t = self.in_gain.target();
        self.in_gain.reset(t);
    }

    fn apply(&mut self, i: usize) {
        match i {
            INPUT_GAIN => {
                self.in_gain.set_time(0.01, self.sr);
                self.in_gain.set_target(db_to_lin(self.p.f(INPUT_GAIN)));
            }
            CEILING => self.ceiling = db_to_lin(self.p.f(CEILING)) * MARGIN,
            RELEASE => self.rel = time_coef(self.p.f(RELEASE) * 1e-3, self.sr),
            LOOKAHEAD => {
                let w = ((self.p.v[LOOKAHEAD] * 1e-3 * self.sr as f64).round() as usize)
                    .clamp(1, self.avg.len());
                if w != self.window {
                    self.window = w;
                    self.restart_window();
                }
            }
            _ => {}
        }
    }

    /// Re-seed the running min and moving average with the current release value.
    fn restart_window(&mut self) {
        self.dq_len = 0;
        self.dq_head = 0;
        for v in &mut self.avg[..self.window] {
            *v = self.r as f64;
        }
        self.avg_pos = 0;
        self.sum = self.r as f64 * self.window as f64;
    }

    #[inline]
    fn push_min(&mut self, v: f32) -> f32 {
        let cap = self.dq_idx.len();
        // pop larger-or-equal values from the back
        while self.dq_len > 0 {
            let back = (self.dq_head + self.dq_len - 1) % cap;
            if self.dq_val[back] >= v {
                self.dq_len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.dq_head + self.dq_len) % cap;
        self.dq_idx[slot] = self.t;
        self.dq_val[slot] = v;
        self.dq_len += 1;
        // drop values that left the hold window (one longer than the average: the
        // detector's interval reaches a quarter sample into the previous sample)
        let w = self.window as u64 + 1;
        while self.dq_len > 0 && self.dq_idx[self.dq_head] + w <= self.t {
            self.dq_head = (self.dq_head + 1) % cap;
            self.dq_len -= 1;
        }
        self.t += 1;
        self.dq_val[self.dq_head]
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let delay = TruePeak4x::DELAY + self.window;
        for i in a..b {
            let g_in = self.in_gain.next();
            let l = input(io, 0, 0, i) * g_in;
            let r = input(io, 0, 1, i) * g_in;
            let peak = self.tp[0].process(l).max(self.tp[1].process(r));
            let need = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            let hold = self.push_min(need);
            self.r = if hold < self.r {
                hold
            } else {
                hold + (self.r - hold) * self.rel
            };
            let w = self.window;
            self.sum += self.r as f64 - self.avg[self.avg_pos];
            self.avg[self.avg_pos] = self.r as f64;
            self.avg_pos = if self.avg_pos + 1 == w {
                0
            } else {
                self.avg_pos + 1
            };
            let g = (self.sum / w as f64).min(1.0) as f32;
            self.delay[0].push(l);
            self.delay[1].push(r);
            let ol = self.delay[0].read(delay);
            let or = self.delay[1].read(delay);
            output(io, i, ol * g, or * g);
        }
    }
}

impl Processor for Limiter {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        self.alloc(info.sample_rate);
        self.window = 0;
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
        for t in &mut self.tp {
            t.reset();
        }
        for d in &mut self.delay {
            d.clear();
        }
        self.t = 0;
        self.r = 1.0;
        self.restart_window();
        self.silence.reset();
    }

    fn latency(&self) -> u32 {
        (TruePeak4x::DELAY + self.window) as u32
    }

    fn tail(&self) -> u32 {
        self.latency() + 1
    }
}
