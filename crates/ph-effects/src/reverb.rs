//! `ph.fx.reverb`: algorithmic plate reverb after J. Dattorro, "Effect Design Part 1:
//! Reverberator and Other Filters" (JAES 45(9), 1997).
//!
//! Mono input → pre-delay → band-limiting (low and high cut) → four input diffusers →
//! a figure-eight tank of two modulated-allpass/delay/damping/allpass/delay halves; stereo
//! output is the paper's 7-tap sum per side. The paper's delay lengths (at 29 761 Hz) are
//! scaled by sample rate and `size`. `decay` is a real RT60: the tank gain is derived from
//! the loop length so the broadband decay matches it, and `tail()` reports the time to
//! −100 dB.

use crate::common::{Params, SilenceTracker, input, output, stereo_desc};
use ph_dsp::delay::DelayLine;
use ph_dsp::filter::OnePoleTpt;
use ph_dsp::flush_denormal;
use ph_dsp::math::sin_turns;
use ph_dsp::smooth::OnePole;
use ph_node::*;

const SIZE: usize = 0;
const DECAY: usize = 1;
const DAMPING: usize = 2;
const PREDELAY: usize = 3;
const WIDTH: usize = 4;
const MIX: usize = 5;
const LOW_CUT: usize = 6;
const HIGH_CUT: usize = 7;
const MODULATION: usize = 8;

const DESIGN_SR: f64 = 29761.0;
const MAX_SCALE: f64 = 1.6;
const MAX_PREDELAY_MS: f64 = 250.0;

// Dattorro's lengths at 29 761 Hz.
const IN_DIFF: [(f64, f32); 4] = [(142.0, 0.75), (107.0, 0.75), (379.0, 0.625), (277.0, 0.625)];
const MOD_AP: [f64; 2] = [672.0, 908.0];
const DELAY1: [f64; 2] = [4453.0, 4217.0];
const AP2: [f64; 2] = [1800.0, 2656.0];
const DELAY2: [f64; 2] = [3720.0, 3163.0];
const EXCURSION: f64 = 16.0;
// Output taps: (half, element, position, sign); element 0 = delay1, 1 = ap2, 2 = delay2.
const TAPS_L: [(usize, usize, f64, f32); 7] = [
    (1, 0, 266.0, 1.0),
    (1, 0, 2974.0, 1.0),
    (1, 1, 1913.0, -1.0),
    (1, 2, 1996.0, 1.0),
    (0, 0, 1990.0, -1.0),
    (0, 1, 187.0, -1.0),
    (0, 2, 1066.0, -1.0),
];
const TAPS_R: [(usize, usize, f64, f32); 7] = [
    (0, 0, 353.0, 1.0),
    (0, 0, 3627.0, 1.0),
    (0, 1, 1228.0, -1.0),
    (0, 2, 2673.0, 1.0),
    (1, 0, 2111.0, -1.0),
    (1, 1, 335.0, -1.0),
    (1, 2, 121.0, -1.0),
];

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new("size", "Size", 0.0, 1.0, 0.6, ParamUnit::Percent),
        ParamInfo::new("decay", "Decay", 0.2, 20.0, 2.5, ParamUnit::Seconds),
        ParamInfo::new("damping", "Damping", 0.0, 1.0, 0.4, ParamUnit::Percent),
        ParamInfo::new(
            "predelay",
            "Pre-delay",
            0.0,
            MAX_PREDELAY_MS,
            10.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new("width", "Width", 0.0, 1.0, 1.0, ParamUnit::Percent),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.25, ParamUnit::Percent),
        ParamInfo::new("low_cut", "Low Cut", 20.0, 1000.0, 80.0, ParamUnit::Hertz),
        ParamInfo::new(
            "high_cut",
            "High Cut",
            1000.0,
            20000.0,
            12000.0,
            ParamUnit::Hertz,
        ),
        ParamInfo::new(
            "modulation",
            "Modulation",
            0.0,
            1.0,
            0.5,
            ParamUnit::Percent,
        ),
    ];
    stereo_desc("ph.fx.reverb", "Reverb", params, false)
}

/// Allpass `v = x − g·d; y = d + g·v` over a delay line (tappable).
struct Allpass {
    line: DelayLine,
    len: usize,
}

impl Allpass {
    fn new(max: usize) -> Self {
        Self {
            line: DelayLine::new(max + 2),
            len: 1,
        }
    }

    #[inline]
    fn process(&mut self, x: f32, g: f32) -> f32 {
        // read before push: `len` samples back after the push means `len - 1` now
        let d = self.line.read(self.len - 1);
        let v = flush_denormal(x - g * d);
        self.line.push(v);
        d + g * v
    }

    /// Modulated: fractional length `len_f` (≥ 2).
    #[inline]
    fn process_mod(&mut self, x: f32, g: f32, len_f: f32) -> f32 {
        let d = self.line.read_cubic(len_f - 1.0);
        let v = flush_denormal(x - g * d);
        self.line.push(v);
        d + g * v
    }
}

struct Half {
    mod_ap: Allpass,
    delay1: DelayLine,
    damp: f32,
    ap2: Allpass,
    delay2: DelayLine,
    len1: usize,
    len2: usize,
    mod_len: f64,
}

pub struct Reverb {
    p: Params,
    sr: f64,
    scale: f64,
    predelay: DelayLine,
    predelay_s: OnePole,
    low_cut: OnePoleTpt,
    high_cut: OnePoleTpt,
    diff: Vec<Allpass>,
    diff_len: [usize; 4],
    half: Vec<Half>,
    taps_l: [(usize, usize, usize, f32); 7],
    taps_r: [(usize, usize, usize, f32); 7],
    decay_g: f32,
    diff2_g: f32,
    damp_coef: f32,
    lfo_phase: f64,
    lfo_inc: f64,
    mix: OnePole,
    width: OnePole,
    silence: SilenceTracker,
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            scale: 1.0,
            predelay: DelayLine::new(1),
            predelay_s: OnePole::new(0.0),
            low_cut: OnePoleTpt::default(),
            high_cut: OnePoleTpt::default(),
            diff: Vec::new(),
            diff_len: [1; 4],
            half: Vec::new(),
            taps_l: [(0, 0, 0, 0.0); 7],
            taps_r: [(0, 0, 0, 0.0); 7],
            decay_g: 0.5,
            diff2_g: 0.5,
            damp_coef: 0.0,
            lfo_phase: 0.0,
            lfo_inc: 0.0,
            mix: OnePole::new(0.0),
            width: OnePole::new(1.0),
            silence: SilenceTracker::default(),
        };
        s.alloc(48000.0);
        s.update_all();
        s
    }

    fn alloc(&mut self, sr: f64) {
        self.sr = sr;
        let k = sr / DESIGN_SR * MAX_SCALE;
        let n = |l: f64| (l * k).ceil() as usize + 4;
        self.predelay = DelayLine::new((MAX_PREDELAY_MS * 1e-3 * sr) as usize + 4);
        self.diff = IN_DIFF.iter().map(|(l, _)| Allpass::new(n(*l))).collect();
        self.half = (0..2)
            .map(|h| Half {
                mod_ap: Allpass::new(n(MOD_AP[h] + EXCURSION * 2.0)),
                delay1: DelayLine::new(n(DELAY1[h])),
                damp: 0.0,
                ap2: Allpass::new(n(AP2[h])),
                delay2: DelayLine::new(n(DELAY2[h])),
                len1: 1,
                len2: 1,
                mod_len: 2.0,
            })
            .collect();
    }

    fn update_all(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let t = self.mix.target();
        self.mix.reset(t);
        let t = self.width.target();
        self.width.reset(t);
        let t = self.predelay_s.target();
        self.predelay_s.reset(t);
    }

    fn rt60(&self) -> f64 {
        self.p.v[DECAY]
    }

    fn update_lengths(&mut self) {
        self.scale = (0.4 + 1.2 * self.p.v[SIZE]) * self.sr / DESIGN_SR;
        let sc = self.scale;
        let len = |l: f64| ((l * sc).round() as usize).max(2);
        for (i, (l, _)) in IN_DIFF.iter().enumerate() {
            self.diff_len[i] = len(*l);
            self.diff[i].len = self.diff_len[i];
        }
        for h in 0..2 {
            let half = &mut self.half[h];
            half.mod_len = MOD_AP[h] * sc;
            half.mod_ap.len = len(MOD_AP[h]);
            half.len1 = len(DELAY1[h]);
            half.ap2.len = len(AP2[h]);
            half.len2 = len(DELAY2[h]);
        }
        let tap =
            |t: &(usize, usize, f64, f32)| (t.0, t.1, ((t.2 * sc).round() as usize).max(1), t.3);
        for i in 0..7 {
            self.taps_l[i] = tap(&TAPS_L[i]);
            self.taps_r[i] = tap(&TAPS_R[i]);
        }
        self.update_decay();
    }

    fn update_decay(&mut self) {
        // loop = all eight tank elements; four decay multipliers per trip
        let loop_len: f64 = (0..2)
            .map(|h| (MOD_AP[h] + DELAY1[h] + AP2[h] + DELAY2[h]) * self.scale)
            .sum();
        let g = 10f64.powf(-3.0 * loop_len / (4.0 * self.rt60() * self.sr));
        self.decay_g = g as f32;
        self.diff2_g = (g as f32 + 0.15).clamp(0.25, 0.5);
    }

    fn apply(&mut self, i: usize) {
        let sr = self.sr as f32;
        match i {
            SIZE => self.update_lengths(),
            DECAY => self.update_decay(),
            DAMPING => {
                // one-pole damping in the tank: 0 → open (20 kHz), 1 → ~1 kHz
                let fc = 20000.0 * (0.05f32).powf(self.p.f(DAMPING));
                let w = 2.0 * std::f32::consts::PI * fc.min(0.45 * sr) / sr;
                self.damp_coef = (-w).exp();
                if self.p.f(DAMPING) == 0.0 {
                    self.damp_coef = 0.0;
                }
            }
            PREDELAY => {
                self.predelay_s.set_time(0.05, sr);
                self.predelay_s
                    .set_target((self.p.v[PREDELAY] * 1e-3 * self.sr) as f32);
            }
            WIDTH => {
                self.width.set_time(0.02, sr);
                self.width.set_target(self.p.f(WIDTH));
            }
            MIX => {
                self.mix.set_time(0.02, sr);
                self.mix.set_target(self.p.f(MIX));
            }
            LOW_CUT => self.low_cut.set(self.p.f(LOW_CUT), sr),
            HIGH_CUT => self.high_cut.set(self.p.f(HIGH_CUT), sr),
            MODULATION => self.lfo_inc = 0.8 / self.sr,
            _ => {}
        }
    }

    #[inline]
    fn tap(&self, t: &(usize, usize, usize, f32)) -> f32 {
        let h = &self.half[t.0];
        let v = match t.1 {
            0 => h.delay1.read(t.2.min(h.len1)),
            1 => h.ap2.line.read(t.2.min(h.ap2.len)),
            _ => h.delay2.read(t.2.min(h.len2)),
        };
        v * t.3
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let depth = self.p.f(MODULATION) * (EXCURSION * self.scale) as f32;
        let decay = self.decay_g;
        let d2 = self.diff2_g;
        let damp = self.damp_coef;
        for i in a..b {
            let l = input(io, 0, 0, i);
            let r = input(io, 0, 1, i);
            self.predelay.push(0.5 * (l + r));
            let pd = self.predelay_s.next();
            let mut x = if pd < 1.0 {
                self.predelay.read_linear(pd)
            } else {
                self.predelay.read_cubic(pd)
            };
            x = self.high_cut.lp(x);
            x = self.low_cut.hp(x);
            for (k, ap) in self.diff.iter_mut().enumerate() {
                x = ap.process(x, IN_DIFF[k].1);
            }
            // tank: each half is fed by the other half's output
            let lfo = [sin_turns(self.lfo_phase), sin_turns(self.lfo_phase + 0.25)];
            self.lfo_phase += self.lfo_inc;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }
            let fb = [
                self.half[1].delay2.read(self.half[1].len2 - 1),
                self.half[0].delay2.read(self.half[0].len2 - 1),
            ];
            for h in 0..2 {
                let half = &mut self.half[h];
                let mut v = x + decay * fb[h];
                let ml = (half.mod_len as f32 + depth * lfo[h]).max(2.0);
                v = half.mod_ap.process_mod(v, -0.7, ml);
                half.delay1.push(v);
                let mut w = half.delay1.read(half.len1);
                half.damp = flush_denormal(w + (half.damp - w) * damp);
                w = half.damp * decay;
                w = half.ap2.process(w, d2);
                half.delay2.push(w);
            }
            let mut yl = 0.0;
            let mut yr = 0.0;
            for t in &self.taps_l {
                yl += self.tap(t);
            }
            for t in &self.taps_r {
                yr += self.tap(t);
            }
            yl *= 0.6;
            yr *= 0.6;
            let wdt = self.width.next();
            let mid = 0.5 * (yl + yr);
            let side = 0.5 * (yl - yr) * wdt;
            let m = self.mix.next();
            output(
                io,
                i,
                l * (1.0 - m) + (mid + side) * m,
                r * (1.0 - m) + (mid - side) * m,
            );
        }
    }
}

impl Processor for Reverb {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.alloc(info.sample_rate);
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
        self.predelay.clear();
        self.low_cut.reset();
        self.high_cut.reset();
        for d in &mut self.diff {
            d.line.clear();
        }
        for h in &mut self.half {
            h.mod_ap.line.clear();
            h.delay1.clear();
            h.ap2.line.clear();
            h.delay2.clear();
            h.damp = 0.0;
        }
        self.lfo_phase = 0.0;
        self.silence.reset();
    }

    /// Pre-delay plus the time to decay by 100 dB.
    fn tail(&self) -> u32 {
        let secs = self.p.v[PREDELAY] * 1e-3 + self.rt60() * 100.0 / 60.0;
        (secs * self.sr) as u32
    }
}
