//! Per-voice DSP (M3/T2): reading (preload, stream window, loops and their crossfades,
//! Hermite or windowed-sinc resampling), the group filter, envelopes, LFOs, the modulation
//! matrix and crossfades.
//!
//! # Control rate
//!
//! Modulation, pitch, the filter coefficients and the output gain are worked out every
//! [`CTRL`] frames **of the voice** (counted from its note-on, never from block edges), and
//! the gain ramps linearly between updates. So the result does not depend on how the host
//! cuts blocks: live output is bit-identical to an offline render. Envelopes run per frame.
//!
//! Everything here is real-time: no allocation, locks or I/O (stream reads go through
//! [`StreamSource`], which is real-time by contract).

use crate::env::Stage;
use crate::plan::{MAX_LFOS, Plan, PlanLoop, PlanRoute};
use crate::samples::{StreamSource, ZoneSample};
use crate::voice::{Mods, Voice};
use ph_core::{
    Crossfade, CrossfadeCurve, CrossfadeSource, FilterKind, LfoShape, ModCurve, ModSource,
    ModTarget,
};
use ph_dsp::filter::{FilterMode, tpt_g};
use ph_dsp::interp::{SincInterpolator, hermite};

/// Frames between control updates of a voice.
pub const CTRL: u32 = 32;
/// Frames of a voice's stream window (per channel).
pub const WINDOW: usize = 2048;
/// Frames the window keeps behind the read position (interpolation reaches back).
const MARGIN: i64 = 160;
/// Half width of the High-quality sinc kernel, and its longest stretch (as `ph.sampler`).
pub const SINC_HALF_WIDTH: usize = 16;
pub const SINC_MAX_STRETCH: f64 = 4.0;
/// Most taps a sinc read gathers (`2 × half width × max stretch`, plus the edges).
const MAX_TAPS: usize = 2 * SINC_HALF_WIDTH * SINC_MAX_STRETCH as usize + 4;

/// Channel-wide state the voices read.
#[derive(Clone, Debug)]
pub struct Controls {
    pub cc: [f32; 128],
    /// -1..=1.
    pub bend: f32,
    /// Channel pressure, 0..=1.
    pub aftertouch: f32,
    /// The `volume` parameter as a linear gain, and the `pan` parameter.
    pub volume: f32,
    pub pan: f32,
    pub sample_rate: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            cc: [0.0; 128],
            bend: 0.0,
            aftertouch: 0.0,
            volume: 1.0,
            pan: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

/// Shapes a source value before it is scaled by the route's depth. Bipolar values keep their
/// sign.
pub fn curve(x: f32, c: ModCurve) -> f32 {
    match c {
        ModCurve::Linear => x,
        ModCurve::Convex => x * x.abs(),
        ModCurve::Concave => x.signum() * x.abs().sqrt(),
        ModCurve::Switch => {
            if x >= 0.5 {
                1.0
            } else if x <= -0.5 {
                -1.0
            } else {
                0.0
            }
        }
    }
}

/// Gain of one crossfade at source value `x` (0..=1): rises from 0 at `in_lo` to 1 at
/// `in_hi`, falls from 1 at `out_lo` to 0 at `out_hi`. Equal ends make a hard edge. `Power`
/// is equal power (sin/cos quarter waves: the two layers of a crossfade sum to constant
/// power), `Gain` linear.
pub fn crossfade(c: &Crossfade, x: f32) -> f32 {
    let rise = if x < c.in_lo {
        0.0
    } else if x >= c.in_hi {
        1.0
    } else {
        (x - c.in_lo) / (c.in_hi - c.in_lo)
    };
    let fall = if x > c.out_hi {
        0.0
    } else if x <= c.out_lo {
        1.0
    } else {
        (c.out_hi - x) / (c.out_hi - c.out_lo)
    };
    match c.curve {
        CrossfadeCurve::Gain => rise * fall,
        CrossfadeCurve::Power => {
            let q = std::f32::consts::FRAC_PI_2;
            let r = if rise >= 1.0 { 1.0 } else { (rise * q).sin() };
            let f = if fall >= 1.0 { 1.0 } else { (fall * q).sin() };
            r * f
        }
    }
}

/// The product of a zone's crossfades over velocity (`velocity`) or over controllers
/// (`cc`). `which` = `Some(true)`: velocity ones only; `Some(false)`: controller ones only.
pub fn crossfade_gain(
    xf: &[Crossfade],
    velocity: f32,
    cc: &[f32; 128],
    which: Option<bool>,
) -> f32 {
    let mut g = 1.0;
    for c in xf {
        let (x, is_vel) = match c.source {
            CrossfadeSource::Velocity => (velocity, true),
            CrossfadeSource::Cc { controller } => (cc[controller as usize & 127], false),
        };
        if which.is_none_or(|w| w == is_vel) {
            g *= crossfade(c, x);
        }
    }
    g
}

/// A source's value for voice `v` (LFO values as computed this control update).
fn source(s: &ModSource, v: &Voice, ctl: &Controls, lfo: &[f32; MAX_LFOS]) -> f32 {
    match *s {
        ModSource::Velocity => v.velocity,
        ModSource::Key => (v.key as f32 - 60.0) / 64.0,
        ModSource::Cc { controller } => ctl.cc[controller as usize & 127],
        ModSource::PitchBend => ctl.bend,
        ModSource::Aftertouch => ctl.aftertouch,
        ModSource::PolyPressure => v.pressure,
        ModSource::Lfo { index } => lfo.get(index as usize).copied().unwrap_or(0.0),
        ModSource::Envelope { index: 0 } => v.amp.level(),
        ModSource::Envelope { .. } => v.fenv.level(),
        ModSource::Random => v.random,
    }
}

fn add(m: &mut Mods, t: ModTarget, x: f32) {
    match t {
        ModTarget::Pitch => m.pitch += x,
        ModTarget::Volume => m.volume += x,
        ModTarget::Pan => m.pan += x,
        ModTarget::Cutoff => m.cutoff += x,
        ModTarget::Resonance => m.resonance += x,
        ModTarget::SampleStart => m.start += x,
    }
}

/// Sums routes: the fixed ones (`fixed = true`: velocity, key, random) or the moving ones.
pub fn modulate(
    routes: &[PlanRoute],
    fixed: bool,
    v: &Voice,
    ctl: &Controls,
    lfo: &[f32; MAX_LFOS],
) -> Mods {
    let mut m = Mods::default();
    for r in routes.iter().filter(|r| r.is_static() == fixed) {
        add(
            &mut m,
            r.target,
            r.depth * curve(source(&r.source, v, ctl, lfo), r.curve),
        );
    }
    m
}

/// The sample-start offset of a new voice, frames (every route to `sample_start`, sources
/// as they are at note-on; LFOs and envelopes are 0 then).
pub fn start_offset(routes: &[PlanRoute], v: &Voice, ctl: &Controls) -> f64 {
    let zero = [0.0; MAX_LFOS];
    let mut x = 0.0f32;
    for r in routes.iter().filter(|r| r.target == ModTarget::SampleStart) {
        let s = match r.source {
            ModSource::Lfo { .. } | ModSource::Envelope { .. } => 0.0,
            ref s => source(s, v, ctl, &zero),
        };
        x += r.depth * curve(s, r.curve);
    }
    x.max(0.0) as f64
}

/// SplitMix64 step (per-voice random LFOs).
pub fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit_bipolar(x: u64) -> f32 {
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// An LFO's waveform at `phase` (0..1), -1..=1.
pub fn lfo_shape(shape: LfoShape, phase: f64, hold: f32) -> f32 {
    let p = phase as f32;
    match shape {
        LfoShape::Sine => ph_dsp::sin_turns(phase),
        LfoShape::Triangle => {
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        }
        LfoShape::Square => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::SawUp => 2.0 * p - 1.0,
        LfoShape::SawDown => 1.0 - 2.0 * p,
        LfoShape::Random => hold,
    }
}

/// Resonance in dB (0..=40) to SVF damping: 0 dB is Butterworth (Q = 1/√2), +20 dB is ten
/// times the Q.
pub fn res_db_to_k(db: f32) -> f32 {
    let q = std::f32::consts::FRAC_1_SQRT_2 * 10f32.powf(db.clamp(0.0, 40.0) / 20.0);
    1.0 / q
}

fn filter_mode(kind: FilterKind) -> (FilterMode, bool) {
    match kind {
        FilterKind::LowPass12 => (FilterMode::LowPass, false),
        FilterKind::LowPass24 => (FilterMode::LowPass, true),
        FilterKind::HighPass12 => (FilterMode::HighPass, false),
        FilterKind::HighPass24 => (FilterMode::HighPass, true),
        FilterKind::BandPass => (FilterMode::BandPass, false),
        FilterKind::Notch => (FilterMode::Notch, false),
    }
}

/// The filter cutoff of voice `v` now, Hz.
pub fn cutoff_hz(f: &ph_core::Filter, key: u8, fenv: f32, mod_cents: f32) -> f32 {
    let cents = f.key_track * (key as f32 - 60.0) + f.env_depth * fenv + mod_cents;
    f.cutoff.clamp(10.0, 22_000.0) * (cents / 1200.0).exp2()
}

/// The control update of a voice (every [`CTRL`] frames): LFOs, modulation, gain target,
/// pitch, glide, filter coefficients.
pub fn control(v: &mut Voice, plan: &Plan, ctl: &Controls) {
    let z = &plan.zones[v.zone];
    let g = &plan.groups[v.group];
    let sr = ctl.sample_rate;
    // LFOs: delay, then fade in; phases run from the note-on
    let mut lfo = [0.0f32; MAX_LFOS];
    let age = v.age as f64 / sr;
    for (i, l) in g.lfos.iter().enumerate() {
        let depth = if age < l.delay as f64 {
            0.0
        } else if l.fade > 0.0 {
            ((age - l.delay as f64) / l.fade as f64).min(1.0) as f32
        } else {
            1.0
        };
        lfo[i] = depth * lfo_shape(l.shape, v.lfo_phase[i], v.lfo_hold[i]);
        let mut p = v.lfo_phase[i] + l.rate.clamp(0.01, 100.0) as f64 * CTRL as f64 / sr;
        if p >= 1.0 {
            p -= p.floor();
            if l.shape == LfoShape::Random {
                v.lfo_hold[i] = unit_bipolar(splitmix(&mut v.lfo_rng));
            }
        }
        v.lfo_phase[i] = p;
    }
    let mut m = v.fixed;
    if !g.routes.is_empty() {
        let d = modulate(&g.routes, false, v, ctl, &lfo);
        m.pitch += d.pitch;
        m.volume += d.volume;
        m.pan += d.pan;
        m.cutoff += d.cutoff;
        m.resonance += d.resonance;
    }
    // gain: levels, the volume parameter, crossfades, modulation; then pan
    let mut vol = v.level * ctl.volume * v.vel_xfade;
    if z.cc_crossfade {
        vol *= crossfade_gain(&z.crossfades, v.velocity, &ctl.cc, Some(false));
    }
    if m.volume != 0.0 {
        vol *= 10f32.powf(m.volume / 20.0);
    }
    let p = (v.note_pan + ctl.pan + m.pan).clamp(-1.0, 1.0);
    let target = [vol * (1.0 - p).min(1.0), vol * (1.0 + p).min(1.0)];
    if v.age == 0 {
        v.g_now = target;
        v.g_inc = [0.0; 2];
    } else {
        for c in 0..2 {
            v.g_inc[c] = (target[c] - v.g_now[c]) / CTRL as f32;
        }
    }
    v.gain = target;
    v.loudness = (target[0] + target[1]) * v.amp.level().max(1e-6);
    // pitch: glide, bend, modulation
    let bend = if ctl.bend >= 0.0 {
        ctl.bend * plan.bend_up
    } else {
        ctl.bend * plan.bend_down
    };
    let semis = v.glide + bend as f64 + m.pitch as f64 / 100.0;
    v.rate = if semis == 0.0 {
        v.step
    } else {
        v.step * (semis / 12.0).exp2()
    };
    if v.glide != 0.0 {
        let d = v.glide_step * CTRL as f64;
        if v.glide.abs() <= d.abs() {
            v.glide = 0.0;
        } else {
            v.glide -= d;
        }
    }
    // filter
    if let Some(f) = &g.filter {
        let fc = cutoff_hz(f, v.key, v.fenv.level(), m.cutoff);
        let k = res_db_to_k(f.resonance + m.resonance);
        let gg = tpt_g(fc, sr as f32);
        for s in v.filt.iter_mut() {
            s.set_coefs(gg, k);
        }
    }
}

/// A voice's view of its sample: the preload, and the stream past it.
struct Reader<'a> {
    l: &'a [f32],
    r: &'a [f32],
    /// Frames held in memory, and in the file.
    head: i64,
    frames: i64,
    stream: Option<(&'a dyn StreamSource, usize)>,
    slot: usize,
}

impl Reader<'_> {
    /// Frame `j` of the file.
    #[inline]
    fn raw(&self, win: &mut [f32], win_start: &mut i64, reverse: bool, j: i64) -> (f32, f32) {
        if j < 0 || j >= self.frames {
            return (0.0, 0.0);
        }
        if j < self.head {
            return (self.l[j as usize], self.r[j as usize]);
        }
        let Some((s, id)) = self.stream else {
            return (0.0, 0.0);
        };
        if *win_start == i64::MIN || j < *win_start || j >= *win_start + WINDOW as i64 {
            let start = if reverse {
                j + MARGIN + 1 - WINDOW as i64
            } else {
                j - MARGIN
            }
            .max(0);
            s.want(self.slot, id, start as u64, reverse);
            let (wl, wr) = win.split_at_mut(WINDOW);
            // an underrun leaves silence in the window (counted by the source)
            s.read(id, start, [wl, wr]);
            *win_start = start;
        }
        let k = (j - *win_start) as usize;
        (win[k], win[WINDOW + k])
    }
}

/// The loop in effect for a read at `pos`, if any.
#[inline]
fn loop_at<'a>(v: &Voice, l: Option<&'a PlanLoop>, pos: f64) -> Option<&'a PlanLoop> {
    let l = l?;
    (v.looping && pos >= l.start as f64 && pos < l.end as f64).then_some(l)
}

/// Frame `j` as the loop plays it: wrapped (forward) or mirrored (ping-pong), with the
/// forward loop's crossfade over the `crossfade` frames before its end (the loop's tail
/// fades into the material just before its start, so the seam is continuous).
#[inline]
fn tap(
    rd: &Reader,
    win: &mut [f32],
    win_start: &mut i64,
    reverse: bool,
    lp: Option<&PlanLoop>,
    j: i64,
) -> (f32, f32) {
    let Some(l) = lp else {
        return rd.raw(win, win_start, reverse, j);
    };
    let (ls, le) = (l.start as i64, l.end as i64);
    let len = le - ls;
    let mut j = j;
    if l.ping_pong {
        let last = le - 1;
        let span = (last - ls).max(1);
        // mirror into ls..=last
        let mut k = (j - ls).rem_euclid(2 * span);
        if k > span {
            k = 2 * span - k;
        }
        j = ls + k;
        return rd.raw(win, win_start, reverse, j);
    }
    if j < ls || j >= le {
        j = ls + (j - ls).rem_euclid(len);
    }
    let xf = l.crossfade as i64;
    if xf > 0 && j >= le - xf {
        let t = (j - (le - xf)) as f32 / xf as f32;
        let a = rd.raw(win, win_start, reverse, j);
        let b = rd.raw(win, win_start, reverse, j - len);
        return (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    }
    rd.raw(win, win_start, reverse, j)
}

#[inline]
fn at(buf: &[f32; MAX_TAPS], k: i64, n: usize) -> f32 {
    if k >= 0 && (k as usize) < n {
        buf[k as usize]
    } else {
        0.0
    }
}

/// Renders voice `v` (pool slot `slot`) into `out_l`/`out_r` over `from..to`, adding. Ends
/// the voice (and releases its stream) when it finishes. Real-time.
#[allow(clippy::too_many_arguments)]
pub fn render_voice(
    v: &mut Voice,
    slot: usize,
    plan: &Plan,
    sample: &ZoneSample,
    streams: Option<&dyn StreamSource>,
    win: &mut [f32],
    sinc: Option<&SincInterpolator>,
    ctl: &Controls,
    out_l: &mut [f32],
    out_r: &mut [f32],
    from: usize,
    to: usize,
) {
    let z = &plan.zones[v.zone];
    let g = &plan.groups[v.group];
    let head = &sample.head.channels;
    let Some(hl) = head.first() else {
        v.active = false;
        return;
    };
    let rd = Reader {
        l: hl,
        r: head.get(1).unwrap_or(hl),
        head: hl.len() as i64,
        frames: sample.frames as i64,
        stream: match (sample.stream, streams) {
            (Some(id), Some(s)) if !sample.complete => Some((s, id)),
            _ => None,
        },
        slot,
    };
    let (lo, hi) = (z.start as f64, z.end as f64);
    let filter = g.filter.as_ref().map(|f| filter_mode(f.kind));
    let has_fenv = g.filter_env.is_some();
    let mut done = false;
    for i in from..to {
        if v.amp.is_idle() {
            done = true;
            break;
        }
        if v.ctrl == 0 {
            control(v, plan, ctl);
            v.ctrl = CTRL;
        }
        v.ctrl -= 1;
        let a = v.amp.next();
        if has_fenv {
            v.fenv.next();
        }
        let mut fm = 1.0;
        let mut last = false;
        if v.fade > 0 {
            fm = v.fade as f32 / v.fade_len as f32;
            v.fade -= 1;
            last = v.fade == 0;
        }
        // read
        let pos = v.pos;
        let reverse = v.dir < 0.0;
        let lp = loop_at(v, z.looping.as_ref(), pos);
        let ip = pos.floor();
        let t = (pos - ip) as f32;
        let ip = ip as i64;
        let (mut sl, mut sr) = match sinc {
            None => {
                if lp.is_none() && ip >= 1 && ip + 2 < rd.head {
                    let k = ip as usize;
                    (
                        hermite(rd.l[k - 1], rd.l[k], rd.l[k + 1], rd.l[k + 2], t),
                        hermite(rd.r[k - 1], rd.r[k], rd.r[k + 1], rd.r[k + 2], t),
                    )
                } else {
                    let mut x = [(0.0, 0.0); 4];
                    for (k, xk) in x.iter_mut().enumerate() {
                        *xk = tap(&rd, win, &mut v.win_start, reverse, lp, ip - 1 + k as i64);
                    }
                    (
                        hermite(x[0].0, x[1].0, x[2].0, x[3].0, t),
                        hermite(x[0].1, x[1].1, x[2].1, x[3].1, t),
                    )
                }
            }
            Some(si) => {
                let stretch = v.rate.clamp(1.0, SINC_MAX_STRETCH);
                let reach = SINC_HALF_WIDTH as f64 * stretch;
                let first = (pos - reach).ceil() as i64 - 1;
                let n = (((pos + reach).floor() as i64 + 1 - first + 1) as usize).min(MAX_TAPS);
                let mut bl = [0.0f32; MAX_TAPS];
                let mut br = [0.0f32; MAX_TAPS];
                for k in 0..n {
                    let (a, b) = tap(&rd, win, &mut v.win_start, reverse, lp, first + k as i64);
                    bl[k] = a;
                    br[k] = b;
                }
                let fl = |j: i64| at(&bl, j - first, n);
                let fr = |j: i64| at(&br, j - first, n);
                (
                    si.interpolate(pos, v.rate, fl),
                    si.interpolate(pos, v.rate, fr),
                )
            }
        };
        if let Some((mode, two)) = filter {
            sl = v.filt[0].process(sl, mode);
            sr = v.filt[1].process(sr, mode);
            if two {
                sl = v.filt[2].process(sl, mode);
                sr = v.filt[3].process(sr, mode);
            }
        }
        let e = a * fm;
        out_l[i] += sl * (v.g_now[0] * e);
        out_r[i] += sr * (v.g_now[1] * e);
        v.g_now[0] += v.g_inc[0];
        v.g_now[1] += v.g_inc[1];
        v.age += 1;
        if last {
            done = true;
            break;
        }
        // advance, wrapping or turning in the loop
        let prev = v.pos;
        v.pos += v.rate * v.dir;
        match z.looping.as_ref().filter(|_| v.looping) {
            Some(l) if l.ping_pong => {
                let (ls, last) = (l.start as f64, (l.end - 1) as f64);
                if v.dir > 0.0 && prev <= last && v.pos > last {
                    v.pos = (2.0 * last - v.pos).max(ls);
                    v.dir = -1.0;
                } else if v.dir < 0.0 && prev >= ls && v.pos < ls {
                    v.pos = (2.0 * ls - v.pos).min(last);
                    v.dir = 1.0;
                }
            }
            Some(l) => {
                let (ls, le) = (l.start as f64, l.end as f64);
                let len = le - ls;
                if v.dir > 0.0 && prev < le && v.pos >= le {
                    v.pos = ls + (v.pos - le) % len;
                } else if v.dir < 0.0 && prev >= ls && v.pos < ls {
                    v.pos = le - (ls - v.pos) % len;
                }
            }
            None => {}
        }
        if v.pos >= hi || v.pos < lo {
            done = true;
            break;
        }
        // a release that has run out
        if v.amp.stage() == Stage::Idle {
            done = true;
            break;
        }
    }
    if done {
        finish(v, slot, streams);
    }
}

/// Ends a voice and stops its stream reads.
pub fn finish(v: &mut Voice, slot: usize, streams: Option<&dyn StreamSource>) {
    v.active = false;
    v.fade = 0;
    if v.win_start != i64::MIN {
        if let Some(s) = streams {
            s.release(slot);
        }
        v.win_start = i64::MIN;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xf(source: CrossfadeSource, i: (f32, f32), o: (f32, f32), c: CrossfadeCurve) -> Crossfade {
        Crossfade {
            source,
            in_lo: i.0,
            in_hi: i.1,
            out_lo: o.0,
            out_hi: o.1,
            curve: c,
        }
    }

    /// Known answer: an equal-power velocity crossfade between two layers keeps the summed
    /// power at 1; a linear one keeps the summed gain at 1.
    #[test]
    fn crossfade_gains_known_answer() {
        let soft = xf(
            CrossfadeSource::Velocity,
            (0.0, 0.0),
            (0.4, 0.6),
            CrossfadeCurve::Power,
        );
        let loud = xf(
            CrossfadeSource::Velocity,
            (0.4, 0.6),
            (1.0, 1.0),
            CrossfadeCurve::Power,
        );
        let cc = [0.0; 128];
        for k in 0..=20 {
            let v = k as f32 / 20.0;
            let a = crossfade_gain(std::slice::from_ref(&soft), v, &cc, None);
            let b = crossfade_gain(std::slice::from_ref(&loud), v, &cc, None);
            assert!((a * a + b * b - 1.0).abs() < 1e-5, "v {v}: {a} {b}");
        }
        assert_eq!(crossfade(&soft, 0.3), 1.0);
        assert_eq!(crossfade(&soft, 0.7), 0.0);
        assert!((crossfade(&soft, 0.5) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        let lin = xf(
            CrossfadeSource::Cc { controller: 1 },
            (0.2, 0.4),
            (1.0, 1.0),
            CrossfadeCurve::Gain,
        );
        let mut cc = [0.0; 128];
        cc[1] = 0.3;
        assert!((crossfade_gain(std::slice::from_ref(&lin), 1.0, &cc, None) - 0.5).abs() < 1e-6);
        assert_eq!(
            crossfade_gain(std::slice::from_ref(&lin), 1.0, &cc, Some(true)),
            1.0,
            "velocity-only ignores controller crossfades"
        );
    }

    #[test]
    fn curves_and_lfo_shapes() {
        assert_eq!(curve(0.5, ModCurve::Convex), 0.25);
        assert_eq!(curve(-0.5, ModCurve::Convex), -0.25);
        assert_eq!(curve(0.25, ModCurve::Concave), 0.5);
        assert_eq!(curve(0.49, ModCurve::Switch), 0.0);
        assert_eq!(curve(0.5, ModCurve::Switch), 1.0);
        assert_eq!(lfo_shape(LfoShape::Triangle, 0.25, 0.0), 1.0);
        assert_eq!(lfo_shape(LfoShape::Triangle, 0.75, 0.0), -1.0);
        assert_eq!(lfo_shape(LfoShape::SawUp, 0.0, 0.0), -1.0);
        assert_eq!(lfo_shape(LfoShape::Square, 0.6, 0.0), -1.0);
        assert!((lfo_shape(LfoShape::Sine, 0.25, 0.0) - 1.0).abs() < 1e-6);
        assert!((res_db_to_k(0.0) - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!((res_db_to_k(20.0) - std::f32::consts::SQRT_2 / 10.0).abs() < 1e-6);
        let f = ph_core::Filter {
            kind: FilterKind::LowPass12,
            cutoff: 1000.0,
            resonance: 0.0,
            key_track: 100.0,
            env_depth: 1200.0,
        };
        assert!((cutoff_hz(&f, 72, 0.0, 0.0) - 2000.0).abs() < 1e-2);
        assert!((cutoff_hz(&f, 60, 1.0, -1200.0) - 1000.0).abs() < 1e-2);
    }
}
