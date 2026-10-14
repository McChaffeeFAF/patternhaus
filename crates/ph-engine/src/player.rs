//! The audio-track node (M2/T1): plays one playlist track's audio clips.
//!
//! Compiled off the audio thread ([`build_track`]) into an [`AudioTrackState`] holding the
//! clips ([`ClipPlay`]: the [`AudioClipPlan`] plus its resolved source and, in Slice mode,
//! its slices) and a fixed pool of voices. On the audio thread [`AudioTrackState::render`]
//! renders one chunk without allocating.
//!
//! **Determinism.** Every output frame is a function of the song position only:
//! - *Resample* and *Slice* voices compute each frame's source position from its song
//!   sample (through the tempo map) and interpolate (windowed sinc, `ph_dsp::interp`);
//! - *Stretch* voices run the [`Stretcher`] in fixed quanta of [`QUANTUM`] output frames on
//!   a grid anchored where the voice started, keeping surplus output in a small FIFO, so how
//!   the host splits blocks never changes what the stretcher sees.
//!
//! A voice starts when its clip starts, or wherever playback lands inside it (a seek, a
//! plan swap that rebuilt the track): the node notices any jump in the song position and
//! restarts its voices there. A Stretch voice starts `output latency` frames *before* its
//! clip during continuous playback, so the stretcher is warmed up exactly at the clip start
//! without a burst of work; after a seek into a clip it warms up at once.
//!
//! **Loop wrap.** While the transport loops, the Stretch clips playing at the loop start
//! are **pre-warmed** during the last [`PREWARM_LEADS`] output latencies before the loop
//! end: extra voices run them on a virtual timeline shifted back by the loop length, at no
//! more than about twice real time each, so the warm-up is spread over many blocks. At the
//! wrap they take over seamlessly. A pre-warmed voice always starts at the same virtual
//! position (`loop start − window`), so what it computes does not depend on when, or in how
//! many blocks, the work was done: live and offline stay bit-identical.

use crate::audio_clips::{AudioClipPlan, SignalsmithStretcher, Stretcher};
use crate::stream::{Cache, Prefetcher, STREAM_MIN_SECONDS, Source, StreamPolicy};
use crate::sync::AtomicU64;
use crate::tempo::TempoMap;
use ph_core::StretchMode;
use ph_dsp::interp::SincInterpolator;
use ph_node::{AssetResolver, Quality, SampleData};
use std::collections::HashMap;
use std::sync::Arc;

/// Output frames per stretch quantum.
pub(crate) const QUANTUM: usize = 256;
/// Most input frames one quantum may consume (an 8× ratio).
const MAX_IN: usize = QUANTUM * 8;
/// Source frames per channel a sub-block may read.
const SCRATCH: usize = 16_384;
/// Output frames interpolated per sub-block.
const SUB: usize = 256;
/// Fade at a slice that is cut short or ends early, in seconds.
const SLICE_DECLICK_S: f64 = 0.002;
/// A slice starts this long before its onset (source seconds), so the attack is never cut;
/// the onset itself still lands on its grid position.
const SLICE_PRE_ONSET_S: f64 = 0.002;
/// Most voices one track node holds.
const MAX_VOICES: usize = 16;
/// The sinc kernel stretches (anti-aliases) up to this read speed.
const MAX_KERNEL_STRETCH: f64 = 4.0;
#[cfg(test)]
thread_local! {
    /// Stretch quanta computed on this thread (tests of how warm-up work is spread).
    pub(crate) static QUANTA: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Pre-warming starts this many output latencies before the loop end.
pub(crate) const PREWARM_LEADS: i64 = 3;

/// Where a clip's audio comes from.
pub(crate) enum Src {
    Memory(Arc<SampleData>),
    /// Index into the track cache's sources.
    Stream(usize),
}

/// One slice of a Slice-mode clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SlicePlay {
    /// Song sample where the slice starts.
    pub start: i64,
    /// Play-order source offset of its first frame.
    pub q: f64,
    /// Song sample where it stops (the next slice, the end of its audio, or the clip end).
    pub cut: i64,
    /// Fade out over the last [`SLICE_DECLICK_S`] (cut short, or a gap follows).
    pub fade: bool,
}

/// A clip, resolved for playback.
pub(crate) struct ClipPlay {
    pub plan: AudioClipPlan,
    src: Src,
    /// Play-order frames that exist (the span clipped to the file).
    span: i64,
    /// Last file frame of the played range.
    last: i64,
    /// Frames before the clip start at which the voice starts (stretch warm-up).
    lead: i64,
    pub slices: Vec<SlicePlay>,
    /// Slice mode: play-order frames per output frame inside a slice.
    slice_rate: f64,
    /// Stretch mode: total transposition (clip pitch and sample-rate compensation).
    transpose: f64,
}

impl ClipPlay {
    fn file_frame(&self, p: f64) -> f64 {
        if self.plan.reverse {
            self.last as f64 - p
        } else {
            self.plan.source_start as f64 + p
        }
    }
}

/// Everything a track node plays, shared between plans while it is unchanged.
pub(crate) struct TrackData {
    pub clips: Vec<ClipPlay>,
    tempo: TempoMap,
    voices: usize,
    cache: Option<Arc<Cache>>,
    half_width: usize,
    sample_rate: f64,
    max_lead: i64,
}

impl TrackData {
    /// File frames `start..start + l.len()` of a clip's source into `l`/`r` (mono fills
    /// both; outside the file is zero). Streamed sources go through the cache.
    fn read_file(&self, c: &ClipPlay, start: i64, l: &mut [f32], r: &mut [f32]) {
        match &c.src {
            Src::Memory(d) => {
                let n = l.len().min(r.len());
                let frames = d.frames() as i64;
                let a = d.channels.first();
                let b = d.channels.get(1).or(a);
                for i in 0..n {
                    let f = start + i as i64;
                    if f >= 0 && f < frames {
                        l[i] = a.map_or(0.0, |c| c[f as usize]);
                        r[i] = b.map_or(0.0, |c| c[f as usize]);
                    } else {
                        l[i] = 0.0;
                        r[i] = 0.0;
                    }
                }
            }
            Src::Stream(i) => match &self.cache {
                Some(cache) => {
                    cache.read(*i, start, [l, r]);
                }
                None => {
                    l.fill(0.0);
                    r.fill(0.0);
                }
            },
        }
    }

    /// Play-order frames `from..from + l.len()` (zero outside the played range).
    fn read_play(&self, c: &ClipPlay, from: i64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len()) as i64;
        l.fill(0.0);
        r.fill(0.0);
        let j0 = from.max(0);
        let j1 = (from + n).min(c.span);
        if j0 >= j1 {
            return;
        }
        let (a, b) = ((j0 - from) as usize, (j1 - from) as usize);
        let (dl, dr) = (&mut l[a..b], &mut r[a..b]);
        if c.plan.reverse {
            self.read_file(c, c.last - (j1 - 1), dl, dr);
            dl.reverse();
            dr.reverse();
        } else {
            self.read_file(c, c.plan.source_start as i64 + j0, dl, dr);
        }
    }

    /// Tells the prefetch thread where voice `v` reads (streamed sources only).
    fn want(&self, v: usize, c: &ClipPlay, file_frame: f64) {
        if let (Src::Stream(i), Some(cache)) = (&c.src, &self.cache) {
            cache.want(v, *i, file_frame.max(0.0) as u64, c.plan.reverse);
        }
    }

    fn release(&self, v: usize) {
        if let Some(cache) = &self.cache {
            cache.release(v);
        }
    }
}

/// A Stretch voice's stretcher and buffers.
struct StretchVoice {
    st: Box<dyn Stretcher>,
    in_lat: i64,
    out_lat: i64,
    pre: usize,
    channels: usize,
    /// Clip frame of stretcher output 0.
    anchor: i64,
    /// Quanta produced.
    k: i64,
    /// First clip frame that is output (earlier frames are warm-up).
    keep: i64,
    /// Play-order frame of the next input.
    fed: i64,
    fifo: [Vec<f32>; 2],
    fifo_head: usize,
    fifo_len: usize,
    /// Clip frame of the FIFO's first frame.
    fifo_frame: i64,
    inb: [Vec<f32>; 2],
    qout: [Vec<f32>; 2],
}

struct Voice {
    clip: Option<usize>,
    stretch: Option<StretchVoice>,
    /// Pre-warming for the next loop pass (output discarded, work budgeted).
    prewarm: bool,
    /// Song samples this voice's timeline runs behind the playhead (the loop length while
    /// pre-warming, else 0).
    shift: i64,
}

/// Audio-thread state of a track node; moves between plans while the track is unchanged.
pub(crate) struct AudioTrackState {
    data: Arc<TrackData>,
    voices: Vec<Voice>,
    pub out: [Vec<f32>; 2],
    /// Song sample the next chunk starts at during continuous playback.
    expected: Option<i64>,
    pub micros: f32,
    sinc: SincInterpolator,
    pos: Vec<f64>,
    scr: [Vec<f32>; 2],
    /// The loop region the current pre-warm voices were started for.
    prewarmed: Option<(i64, i64)>,
}

impl AudioTrackState {
    /// Real-time. Renders `n` frames from song sample `a` into `self.out` (overwritten).
    /// `playing` is false when the transport is stopped or not in song mode; frames at or
    /// after `limit` are silent (the end of an offline range). `looping` is the song loop
    /// region in samples while the transport loops.
    pub fn render(
        &mut self,
        a: i64,
        n: usize,
        playing: bool,
        limit: i64,
        looping: Option<(i64, i64)>,
    ) {
        let Self {
            data,
            voices,
            out,
            expected,
            sinc,
            pos,
            scr,
            prewarmed,
            ..
        } = self;
        let data: &TrackData = data;
        let [ol, or] = out;
        let (ol, or) = (&mut ol[..n], &mut or[..n]);
        ol.fill(0.0);
        or.fill(0.0);
        if !playing {
            stop_all(data, voices);
            *expected = None;
            *prewarmed = None;
            return;
        }
        let wrap = matches!(looping, Some((l0, l1)) if *expected == Some(l1) && a == l0);
        if wrap {
            // the pre-warmed voices take over; the others stop
            for (vi, v) in voices.iter_mut().enumerate() {
                if v.prewarm {
                    v.prewarm = false;
                    v.shift = 0;
                } else if v.clip.take().is_some() {
                    data.release(vi);
                }
            }
            *prewarmed = None;
        } else if *expected != Some(a) {
            stop_all(data, voices);
            *prewarmed = None;
        } else if prewarmed.is_some() && *prewarmed != looping {
            // the loop region changed: what was pre-warmed is for another loop start
            stop_prewarm(data, voices);
            *prewarmed = None;
        }
        let b = a + n as i64;
        *expected = Some(b);
        // free voices whose clip is over
        for (vi, v) in voices.iter_mut().enumerate() {
            if let Some(ci) = v.clip
                && data.clips[ci].plan.end <= a - v.shift
            {
                v.clip = None;
                v.prewarm = false;
                v.shift = 0;
                data.release(vi);
            }
        }
        // start voices for clips that begin (or are already playing) in this chunk
        for (ci, c) in data.clips.iter().enumerate() {
            if c.plan.start - data.max_lead >= b {
                break;
            }
            let (w0, w1) = (c.plan.start - c.lead, c.plan.end);
            if w1 <= a || w0 >= b || voices.iter().any(|v| v.clip == Some(ci) && !v.prewarm) {
                continue;
            }
            let Some(vi) = voices.iter().position(|v| v.clip.is_none()) else {
                continue;
            };
            let fs = (a - c.plan.start).max(0);
            start_voice(data, vi, &mut voices[vi], ci, fs);
        }
        // pre-warm the Stretch clips that play at the loop start
        if let Some((l0, l1)) = looping
            && prewarmed.is_none()
            && data.max_lead > 0
            && l1 > l0
            && a >= l1 - PREWARM_LEADS * data.max_lead
        {
            let v0 = l0 - PREWARM_LEADS * data.max_lead;
            for (ci, c) in data.clips.iter().enumerate() {
                if c.plan.stretch != StretchMode::Stretch
                    || c.plan.start - c.lead >= l0
                    || c.plan.end <= l0
                {
                    continue;
                }
                let Some(vi) = voices
                    .iter()
                    .position(|v| v.clip.is_none() && v.stretch.is_some())
                else {
                    continue;
                };
                let fs = (v0 - c.plan.start).max(0);
                start_voice(data, vi, &mut voices[vi], ci, fs);
                voices[vi].prewarm = true;
                voices[vi].shift = l1 - l0;
            }
            *prewarmed = looping;
        }
        // render
        let q = QUANTUM as i64;
        for (vi, v) in voices.iter_mut().enumerate() {
            let Some(ci) = v.clip else { continue };
            let c = &data.clips[ci];
            if c.plan.stretch == StretchMode::Stretch {
                // a pre-warming voice works at most about twice real time per chunk
                let budget = v.prewarm.then(|| 2 * (n as i64 + q - 1) / q + 1);
                let (shift, audible) = (v.shift, !v.prewarm);
                if let Some(sv) = v.stretch.as_mut() {
                    let span = Span {
                        a: a - shift,
                        b: b - shift,
                        audible,
                        budget,
                    };
                    render_stretch(data, vi, c, sv, span, ol, or);
                }
            } else {
                let [s0, s1] = &mut *scr;
                render_interp(data, vi, c, sinc, pos, [s0, s1], a, b, ol, or);
            }
        }
        // free voices whose clip ended in this chunk
        for (vi, v) in voices.iter_mut().enumerate() {
            if let Some(ci) = v.clip
                && data.clips[ci].plan.end <= b - v.shift
            {
                v.clip = None;
                v.prewarm = false;
                v.shift = 0;
                data.release(vi);
            }
        }
        if limit < b {
            let from = (limit - a).clamp(0, n as i64) as usize;
            ol[from..].fill(0.0);
            or[from..].fill(0.0);
        }
    }
}

fn stop_all(data: &TrackData, voices: &mut [Voice]) {
    for (vi, v) in voices.iter_mut().enumerate() {
        v.prewarm = false;
        v.shift = 0;
        if v.clip.take().is_some() {
            data.release(vi);
        }
    }
}

fn stop_prewarm(data: &TrackData, voices: &mut [Voice]) {
    for (vi, v) in voices.iter_mut().enumerate() {
        if v.prewarm {
            v.prewarm = false;
            v.shift = 0;
            v.clip = None;
            data.release(vi);
        }
    }
}

/// What a Stretch voice renders in one chunk, on its own timeline.
struct Span {
    /// Song frames `a..b` (already shifted for a pre-warming voice).
    a: i64,
    b: i64,
    /// False: compute but discard (pre-warming).
    audible: bool,
    /// Most quanta to compute in this chunk (`None` = all that are due).
    budget: Option<i64>,
}

/// Starts voice `vi` on clip `ci` at clip frame `fs` (0 = the clip start).
fn start_voice(data: &TrackData, vi: usize, v: &mut Voice, ci: usize, fs: i64) {
    let c = &data.clips[ci];
    v.clip = Some(ci);
    if c.plan.stretch != StretchMode::Stretch {
        return;
    }
    let Some(sv) = v.stretch.as_mut() else {
        return;
    };
    let s = c.plan.start;
    let p = |f: i64| c.plan.pos(&data.tempo, s + f);
    sv.st.reset();
    sv.st.set_transpose(c.transpose);
    sv.anchor = fs - sv.out_lat;
    sv.k = 0;
    sv.keep = fs.max(0);
    sv.fifo_head = 0;
    sv.fifo_len = 0;
    sv.fifo_frame = sv.keep;
    let e0 = p(fs).round() as i64 + sv.in_lat;
    let rate = ((p(fs + QUANTUM as i64) - p(fs)) / QUANTUM as f64).max(1e-6);
    let pre = sv.pre.min(sv.inb[0].len());
    if pre > 0 {
        data.want(vi, c, c.file_frame((e0 - pre as i64) as f64));
        let [il, ir] = &mut sv.inb;
        data.read_play(c, e0 - pre as i64, &mut il[..pre], &mut ir[..pre]);
        let ch: [&[f32]; 2] = [&il[..pre], &ir[..pre]];
        sv.st.seek(&ch[..sv.channels], rate);
    }
    sv.fed = e0;
}

/// Adds a Stretch voice's output for song frames `span.a..span.b` (only those inside the
/// clip). A budgeted (pre-warming) voice may stay behind; it catches up in later chunks,
/// computing exactly the same quanta.
fn render_stretch(
    data: &TrackData,
    vi: usize,
    c: &ClipPlay,
    sv: &mut StretchVoice,
    span: Span,
    ol: &mut [f32],
    or: &mut [f32],
) {
    let Span {
        a,
        b,
        audible,
        budget,
    } = span;
    let s = c.plan.start;
    let len = c.plan.end - s;
    let first = a - s; // clip frame of out[0]
    let target = (b - s).min(len); // produce through this clip frame (exclusive)
    let q = QUANTUM as i64;
    let emit = |f: i64, l: f32, r: f32, ol: &mut [f32], or: &mut [f32]| {
        if !audible {
            return;
        }
        let i = (f - first) as usize;
        let g = c.plan.gain * c.plan.fade_gain(f as u64) as f32;
        ol[i] += l * g;
        or[i] += r * g;
    };
    // 1. surplus from the previous chunk
    while sv.fifo_len > 0 && sv.fifo_frame < target {
        let h = sv.fifo_head;
        if sv.fifo_frame >= first {
            emit(sv.fifo_frame, sv.fifo[0][h], sv.fifo[1][h], ol, or);
        }
        sv.fifo_head = (h + 1) % QUANTUM;
        sv.fifo_len -= 1;
        sv.fifo_frame += 1;
    }
    // 2. new quanta
    let p = |f: i64| c.plan.pos(&data.tempo, s + f);
    let mut done = 0i64;
    while sv.anchor + sv.k * q < target && budget.is_none_or(|m| done < m) {
        done += 1;
        #[cfg(test)]
        QUANTA.with(|n| n.set(n.get() + 1));
        let g0 = sv.anchor + sv.k * q;
        let e = p(g0 + q + sv.out_lat).round() as i64 + sv.in_lat;
        let mut n_in = (e - sv.fed).max(0);
        if n_in > MAX_IN as i64 {
            sv.fed = e - MAX_IN as i64;
            n_in = MAX_IN as i64;
        }
        let n_in = n_in as usize;
        data.want(vi, c, c.file_frame(sv.fed as f64));
        {
            let [il, ir] = &mut sv.inb;
            data.read_play(c, sv.fed, &mut il[..n_in], &mut ir[..n_in]);
            let [ql, qr] = &mut sv.qout;
            let ins: [&[f32]; 2] = [&il[..n_in], &ir[..n_in]];
            if sv.channels == 1 {
                sv.st.process(&ins[..1], &mut [&mut ql[..]]);
                qr.copy_from_slice(ql);
            } else {
                sv.st.process(&ins, &mut [&mut ql[..], &mut qr[..]]);
            }
        }
        sv.fed = e;
        sv.k += 1;
        for i in 0..QUANTUM {
            let f = g0 + i as i64;
            if f < sv.keep || f >= len {
                continue;
            }
            let (l, r) = (sv.qout[0][i], sv.qout[1][i]);
            if f < target {
                if f >= first {
                    emit(f, l, r, ol, or);
                }
            } else if sv.fifo_len < QUANTUM {
                if sv.fifo_len == 0 {
                    sv.fifo_frame = f;
                }
                let t = (sv.fifo_head + sv.fifo_len) % QUANTUM;
                sv.fifo[0][t] = l;
                sv.fifo[1][t] = r;
                sv.fifo_len += 1;
            }
        }
    }
}

/// Adds a Resample or Slice voice's output for song frames `a..b` (inside the clip).
#[allow(clippy::too_many_arguments)]
fn render_interp(
    data: &TrackData,
    vi: usize,
    c: &ClipPlay,
    sinc: &SincInterpolator,
    pos: &mut [f64],
    scr: [&mut Vec<f32>; 2],
    a: i64,
    b: i64,
    ol: &mut [f32],
    or: &mut [f32],
) {
    let [sl, sr] = scr;
    let s0 = a.max(c.plan.start);
    let s1 = b.min(c.plan.end);
    let slice_mode = c.plan.stretch == StretchMode::Slice;
    let declick = (SLICE_DECLICK_S * data.sample_rate).max(1.0);
    let hw = data.half_width as f64;
    let mut from = s0;
    while from < s1 {
        let mut m = (s1 - from).min(SUB as i64) as usize;
        // positions (play order) of frames from..from + m (+1 for the read speed), and the
        // file range their kernels touch; halve the sub-block until it fits the scratch
        let (lo, hi, speed_max) = loop {
            for (i, slot) in pos.iter_mut().enumerate().take(m + 1) {
                let s = from + i as i64;
                *slot = if slice_mode {
                    slice_pos(c, s)
                } else {
                    c.plan.pos(&data.tempo, s)
                };
            }
            let (mut lo, mut hi, mut speed_max) = (f64::MAX, f64::MIN, 1.0f64);
            for i in 0..m {
                let p = pos[i];
                if p.is_finite() && p >= 0.0 && p < c.span as f64 {
                    let fp = c.file_frame(p);
                    lo = lo.min(fp);
                    hi = hi.max(fp);
                    speed_max = speed_max.max(frame_speed(c, pos, i, slice_mode));
                }
            }
            let reach = hw * speed_max.clamp(1.0, MAX_KERNEL_STRETCH) + 2.0;
            let need = (hi + reach).ceil() - (lo - reach).floor() + 1.0;
            if lo > hi || need <= SCRATCH as f64 || m == 1 {
                break (lo, hi, speed_max);
            }
            m = (m / 2).max(1);
        };
        if lo <= hi {
            let reach = hw * speed_max.clamp(1.0, MAX_KERNEL_STRETCH) + 2.0;
            let start = (lo - reach).floor() as i64;
            let count = (((hi + reach).ceil() as i64 - start + 1).max(0) as usize).min(SCRATCH);
            data.want(vi, c, start as f64);
            data.read_file(c, start, &mut sl[..count], &mut sr[..count]);
            let (sl, sr) = (&sl[..count], &sr[..count]);
            for i in 0..m {
                let p = pos[i];
                if !p.is_finite() || p < 0.0 || p >= c.span as f64 {
                    continue;
                }
                let s = from + i as i64;
                let x = c.file_frame(p) - start as f64;
                let ratio = frame_speed(c, pos, i, slice_mode);
                let fl = |j: i64| -> f32 {
                    if j >= 0 && (j as usize) < count {
                        sl[j as usize]
                    } else {
                        0.0
                    }
                };
                let fr = |j: i64| -> f32 {
                    if j >= 0 && (j as usize) < count {
                        sr[j as usize]
                    } else {
                        0.0
                    }
                };
                let vl = sinc.interpolate(x, ratio, fl);
                let vr = sinc.interpolate(x, ratio, fr);
                let mut g = c.plan.gain as f64 * c.plan.fade_gain((s - c.plan.start) as u64);
                if slice_mode {
                    g *= slice_gain(c, s, declick);
                }
                let k = (s - a) as usize;
                ol[k] += vl * g as f32;
                or[k] += vr * g as f32;
            }
        }
        from += m as i64;
    }
}

/// Source frames per output frame at frame `i` of a sub-block.
fn frame_speed(c: &ClipPlay, pos: &[f64], i: usize, slice_mode: bool) -> f64 {
    if slice_mode {
        c.slice_rate
    } else {
        let d = (pos[i + 1] - pos[i]).abs();
        if d.is_finite() { d } else { 1.0 }
    }
}

/// The slice playing at song sample `s`, if any.
fn slice_at(c: &ClipPlay, s: i64) -> Option<&SlicePlay> {
    let i = c.slices.partition_point(|x| x.start <= s);
    let sl = c.slices.get(i.checked_sub(1)?)?;
    (s < sl.cut).then_some(sl)
}

/// Play-order position in Slice mode (NaN between slices).
fn slice_pos(c: &ClipPlay, s: i64) -> f64 {
    match slice_at(c, s) {
        Some(sl) => sl.q + (s - sl.start) as f64 * c.slice_rate,
        None => f64::NAN,
    }
}

fn slice_gain(c: &ClipPlay, s: i64, declick: f64) -> f64 {
    match slice_at(c, s) {
        Some(sl) if sl.fade => (((sl.cut - s) as f64) / declick).min(1.0),
        Some(_) => 1.0,
        None => 0.0,
    }
}

// ---------------------------------------------------------------------------- compiling

/// A source as the compiler resolved it.
#[derive(Clone)]
enum Resolved {
    Memory(Arc<SampleData>),
    Stream(Arc<Source>),
}

/// Audio-clip compile state kept by the controller across plans: resolved sources (so
/// unchanged clips keep their stream handles), onset lists, the prefetch thread.
pub(crate) struct AudioCtx {
    pub policy: StreamPolicy,
    prefetcher: Option<Prefetcher>,
    pub underruns: Arc<AtomicU64>,
    sources: HashMap<String, Resolved>,
    onsets: HashMap<String, Arc<Vec<u64>>>,
}

impl AudioCtx {
    pub fn new(policy: StreamPolicy) -> Self {
        Self {
            policy,
            prefetcher: None,
            underruns: Arc::new(AtomicU64::new(0)),
            sources: HashMap::new(),
            onsets: HashMap::new(),
        }
    }

    /// Forgets resolved sources (a different project, or project folder, took over).
    pub fn clear(&mut self) {
        self.sources.clear();
        self.onsets.clear();
    }

    fn resolve(&mut self, path: &str, assets: &dyn AssetResolver) -> Result<Resolved, String> {
        if let Some(r) = self.sources.get(path) {
            return Ok(r.clone());
        }
        let r = match assets.stream(path) {
            Some(Ok(s)) if s.frames() as f64 >= STREAM_MIN_SECONDS * s.sample_rate() => {
                Resolved::Stream(Arc::new(Source::new(s)))
            }
            Some(Err(e)) => return Err(e),
            _ => Resolved::Memory(assets.sample(path)?),
        };
        self.sources.insert(path.to_string(), r.clone());
        Ok(r)
    }

    fn onsets(&mut self, path: &str, assets: &dyn AssetResolver, r: &Resolved) -> Arc<Vec<u64>> {
        if let Some(o) = self.onsets.get(path) {
            return o.clone();
        }
        let data = match r {
            Resolved::Memory(d) => Some(d.clone()),
            Resolved::Stream(_) => assets.sample(path).ok(),
        };
        let o = Arc::new(data.map_or_else(Vec::new, |d| {
            let mono = ph_dsp::onset::mono(&d.channels);
            ph_dsp::onset::detect(&mono, d.sample_rate, 0.5)
        }));
        self.onsets.insert(path.to_string(), o.clone());
        o
    }

    fn register(&mut self, cache: Cache) -> Arc<Cache> {
        match self.policy {
            StreamPolicy::Sync => Arc::new(cache),
            StreamPolicy::Live | StreamPolicy::Wait => self
                .prefetcher
                .get_or_insert_with(Prefetcher::new)
                .register(cache),
        }
    }
}

/// What makes two compilations of a track the same (then its state moves across plans).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrackKey {
    plans: Vec<AudioClipPlan>,
    tempo: TempoMap,
}

impl TrackKey {
    pub fn new(plans: Vec<AudioClipPlan>, tempo: TempoMap) -> Self {
        Self { plans, tempo }
    }
}

/// Deterministic seed for a track's voice.
fn voice_seed(track: u32, voice: usize) -> u32 {
    (track.wrapping_mul(0x9e37_79b9) ^ (voice as u32).wrapping_mul(0x85eb_ca6b)) | 1
}

/// Compiles one track's clips into a node state. Off the audio thread.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_track(
    track: u32,
    plans: &[AudioClipPlan],
    tempo: &TempoMap,
    sample_rate: u32,
    max_frames: usize,
    quality: Quality,
    audio: &mut AudioCtx,
    assets: &dyn AssetResolver,
    warnings: &mut Vec<String>,
) -> AudioTrackState {
    let sr = sample_rate.max(1) as f64;
    let n = max_frames.max(1);
    // resolve sources
    let mut resolved: Vec<(AudioClipPlan, Resolved)> = Vec::new();
    for p in plans {
        match audio.resolve(&p.path, assets) {
            Ok(r) => resolved.push((p.clone(), r)),
            Err(e) => warnings.push(format!(
                "audio clip {}: {} cannot be read ({e}); it plays silence",
                p.clip, p.path
            )),
        }
    }
    let mut stream_sources: Vec<Arc<Source>> = Vec::new();
    let mut stretch_channels = 0usize;
    for (p, r) in &resolved {
        let ch = match r {
            Resolved::Memory(d) => d.channels.len(),
            Resolved::Stream(s) => s.channels,
        };
        if p.stretch == StretchMode::Stretch {
            stretch_channels = stretch_channels.max(ch.clamp(1, 2));
        }
        if let Resolved::Stream(s) = r
            && !stream_sources.iter().any(|x| Arc::ptr_eq(x, s))
        {
            stream_sources.push(s.clone());
        }
    }
    // a configured stretcher tells the warm-up lead. Live playback uses Signalsmith's
    // cheaper preset (about half the CPU); renders at high quality the default one.
    let preset = match quality {
        Quality::Live => ph_stretch::Preset::Cheaper,
        Quality::High => ph_stretch::Preset::Default,
    };
    let make_stretcher = |seed: u32| -> Box<dyn Stretcher> {
        let mut st: Box<dyn Stretcher> = Box::new(SignalsmithStretcher::new(preset, seed));
        st.configure(stretch_channels.max(1), sr, QUANTUM);
        st
    };
    let probe = (stretch_channels > 0).then(|| make_stretcher(1));
    let lead = probe.as_ref().map_or(0, |s| s.output_latency() as i64);
    let in_lat = probe.as_ref().map_or(0, |s| s.input_latency() as i64);
    let pre = probe.as_ref().map_or(0, |s| s.preroll());
    // clips
    let mut clips = Vec::with_capacity(resolved.len());
    for (p, r) in resolved {
        let (frames, file_rate) = match &r {
            Resolved::Memory(d) => (d.frames() as u64, d.sample_rate),
            Resolved::Stream(s) => (s.frames, s.sample_rate),
        };
        let span_want = p.span(tempo).max(0.0).ceil() as u64;
        let span = span_want.min(frames.saturating_sub(p.source_start)) as i64;
        let last = p.source_start as i64 + span - 1;
        let src = match &r {
            Resolved::Memory(d) => Src::Memory(d.clone()),
            Resolved::Stream(s) => Src::Stream(
                stream_sources
                    .iter()
                    .position(|x| Arc::ptr_eq(x, s))
                    .unwrap_or(0),
            ),
        };
        let pitch_factor = 2f64.powf(p.transpose / 12.0);
        let slice_rate = file_rate / sr * pitch_factor;
        let slices = if p.stretch == StretchMode::Slice {
            let onsets = audio.onsets(&p.path, assets, &r);
            slices_of(&p, tempo, &onsets, span, slice_rate)
        } else {
            Vec::new()
        };
        clips.push(ClipPlay {
            transpose: p.transpose + 12.0 * (file_rate / sr).log2(),
            lead: if p.stretch == StretchMode::Stretch {
                lead
            } else {
                0
            },
            plan: p,
            src,
            span,
            last,
            slices,
            slice_rate,
        });
    }
    // voices: the most clips whose active windows (lead and one block of slack included)
    // overlap
    let mut edges: Vec<(i64, i32)> = Vec::new();
    for c in &clips {
        edges.push((c.plan.start - c.lead, 1));
        edges.push((c.plan.end + n as i64, -1));
    }
    edges.sort_unstable();
    let (mut cur, mut most) = (0i32, 0i32);
    for (_, d) in edges {
        cur += d;
        most = most.max(cur);
    }
    // Stretch tracks get as many again for pre-warming at a loop wrap
    let voices = (most.max(1) as usize).min(MAX_VOICES) * if stretch_channels > 0 { 2 } else { 1 };
    let cache = if stream_sources.is_empty() {
        None
    } else {
        Some(audio.register(Cache::new(
            stream_sources,
            voices,
            audio.policy,
            audio.underruns.clone(),
        )))
    };
    let max_lead = clips.iter().map(|c| c.lead).max().unwrap_or(0);
    let half_width = match quality {
        Quality::Live => 8,
        Quality::High => 16,
    };
    let data = Arc::new(TrackData {
        clips,
        tempo: tempo.clone(),
        voices,
        cache,
        half_width,
        sample_rate: sr,
        max_lead,
    });
    let voices = (0..data.voices)
        .map(|v| Voice {
            clip: None,
            prewarm: false,
            shift: 0,
            stretch: (stretch_channels > 0).then(|| {
                let st = make_stretcher(voice_seed(track, v));
                StretchVoice {
                    st,
                    in_lat,
                    out_lat: lead,
                    pre,
                    channels: stretch_channels,
                    anchor: 0,
                    k: 0,
                    keep: 0,
                    fed: 0,
                    fifo: [vec![0.0; QUANTUM], vec![0.0; QUANTUM]],
                    fifo_head: 0,
                    fifo_len: 0,
                    fifo_frame: 0,
                    inb: [vec![0.0; MAX_IN.max(pre)], vec![0.0; MAX_IN.max(pre)]],
                    qout: [vec![0.0; QUANTUM], vec![0.0; QUANTUM]],
                }
            }),
        })
        .collect();
    AudioTrackState {
        data,
        voices,
        out: [vec![0.0; n], vec![0.0; n]],
        expected: None,
        micros: 0.0,
        sinc: SincInterpolator::new(half_width, MAX_KERNEL_STRETCH),
        pos: vec![0.0; SUB + 1],
        scr: [vec![0.0; SCRATCH], vec![0.0; SCRATCH]],
        prewarmed: None,
    }
}

/// The slices of a Slice-mode clip: one per onset inside the played range (plus one at its
/// start), each placed where the clip's time mapping puts its first frame and played
/// unstretched at `rate` until the next slice starts or its audio runs out.
fn slices_of(
    p: &AudioClipPlan,
    tempo: &TempoMap,
    onsets: &[u64],
    span: i64,
    rate: f64,
) -> Vec<SlicePlay> {
    if span <= 0 {
        return Vec::new();
    }
    let a = p.source_start as i64;
    // onsets in play order: (onset, where the slice starts) with a short lead-in
    let pre = (SLICE_PRE_ONSET_S * p.file_rate).round();
    let mut qs: Vec<(f64, f64)> = vec![(0.0, 0.0)];
    for &o in onsets {
        let o = o as i64;
        if o > a && o < a + span {
            // a boundary between file frames o-1 and o, in play order
            let q = if p.reverse {
                (a + span - o) as f64
            } else {
                (o - a) as f64
            };
            qs.push((q, (q - pre).max(0.0)));
        }
    }
    qs.sort_by(|x, y| x.0.total_cmp(&y.0));
    qs.dedup_by(|x, y| x.0 == y.0);
    // song sample where play-order offset q falls
    let place = |q: f64| -> i64 {
        if p.synced && p.source_per_tick > 0.0 {
            let tick = p.start_tick as f64 + q / p.source_per_tick;
            tempo.sample_at_frac((tick * 1024.0).round().max(0.0) as u128, 1024)
        } else {
            p.start + (q / p.speed.max(1e-9)).round() as i64
        }
    };
    // the onset lands on its place; the lead-in plays just before it
    let rate = rate.max(1e-9);
    let mut starts: Vec<i64> = qs
        .iter()
        .map(|(q, from)| place(*q) - ((q - from) / rate).round() as i64)
        .collect();
    for i in 1..starts.len() {
        starts[i] = starts[i].max(starts[i - 1] + 1);
    }
    let mut out = Vec::with_capacity(qs.len());
    for i in 0..qs.len() {
        let next_start = starts.get(i + 1).copied().unwrap_or(p.end).min(p.end);
        let q_next = qs.get(i + 1).map_or(span as f64, |n| n.1);
        let from = qs[i].1;
        let natural_end = starts[i] + ((q_next - from) / rate).ceil() as i64;
        let cut = next_start.min(natural_end).min(p.end);
        if cut <= starts[i] {
            continue;
        }
        out.push(SlicePlay {
            start: starts[i],
            q: from,
            cut,
            fade: (next_start - natural_end).abs() > 1 && cut < p.end,
        });
    }
    out
}
