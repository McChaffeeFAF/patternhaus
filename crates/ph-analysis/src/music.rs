//! Musical analysis: note names, key and chord estimation.
//!
//! From **notes** the estimate is exact input (no signal processing): pitch-class weights by
//! duration and velocity, Krumhansl-Kessler key profiles, template-matched chords with bass
//! (inversion) detection. From **audio** the same estimators run on a chroma vector built
//! from spectral peaks.

use crate::{Timing, mono_mix, round};
use ph_core::{ChannelId, Note, PPQ, PatternId, Project, Tick};
use ph_dsp::analysis::fft;
use serde::Serialize;
use std::f64::consts::PI;

const PC_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn pc_name(pc: usize) -> &'static str {
    PC_NAMES[pc % 12]
}

/// `60` is `C4` (scientific pitch notation; FL labels the same key C5).
pub fn note_name(key: u8) -> String {
    format!("{}{}", PC_NAMES[key as usize % 12], key as i32 / 12 - 1)
}

/// Parses `C4`, `f#3`, `Bb2`, `c-1`. `None` when it is not a note name or out of range.
pub fn parse_note_name(s: &str) -> Option<u8> {
    let s = s.trim();
    let mut chars = s.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let oct: i32 = oct.parse().ok()?;
    let key = (oct + 1) * 12 + base + acc;
    (0..=127).contains(&key).then_some(key as u8)
}

/// One note on the song timeline (patterns flattened through the playlist).
#[derive(Clone, Debug, PartialEq)]
pub struct SongNote {
    pub channel: ChannelId,
    pub key: u8,
    pub start: Tick,
    pub length: Tick,
    pub velocity: f32,
}

fn song_note(channel: ChannelId, n: &Note, start: Tick, length: Tick) -> SongNote {
    SongNote {
        channel,
        key: n.key,
        start,
        length,
        velocity: n.velocity,
    }
}

/// Notes of one pattern, at pattern time.
pub fn pattern_notes(project: &Project, pattern: PatternId) -> Vec<SongNote> {
    let Some(p) = project.patterns.get(&pattern) else {
        return vec![];
    };
    let mut v: Vec<SongNote> = p
        .notes
        .iter()
        .flat_map(|(ch, m)| {
            m.values()
                .map(move |n| song_note(*ch, n, n.start, n.length))
        })
        .collect();
    v.sort_by_key(|n| (n.start, n.key));
    v
}

/// Notes of the whole song as the engine plays them: playlist clips place pattern notes
/// (muted playlist tracks and muted channels are skipped). With no clips, the first
/// pattern stands in, so a project that has only patterns can still be read.
pub fn song_notes(project: &Project) -> Vec<SongNote> {
    if project.clips.is_empty() {
        return project
            .pattern_order
            .first()
            .map(|p| {
                pattern_notes(project, *p)
                    .into_iter()
                    .filter(|n| project.channels.get(&n.channel).is_some_and(|c| !c.muted))
                    .collect()
            })
            .unwrap_or_default();
    }
    let mut v = Vec::new();
    for clip in project.clips.values() {
        if project
            .tracks
            .get(clip.track as usize)
            .is_some_and(|t| t.muted)
        {
            continue;
        }
        let Some(p) = project.patterns.get(&clip.pattern) else {
            continue;
        };
        let plen = project.pattern_length(clip.pattern).unwrap_or(0);
        let win_end = clip.start + clip.length;
        for (ch, m) in &p.notes {
            if project.channels.get(ch).is_none_or(|c| c.muted) {
                continue;
            }
            for n in m.values() {
                if n.start < clip.offset || n.start >= clip.offset + clip.length || n.start >= plen
                {
                    continue;
                }
                let st = clip.start + (n.start - clip.offset);
                let end = (st + n.length).min(win_end);
                if end > st {
                    v.push(song_note(*ch, n, st, end - st));
                }
            }
        }
    }
    v.sort_by_key(|n| (n.start, n.key, n.channel));
    v
}

// ---------------------------------------------------------------------------------------
// key

const KK_MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const KK_MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KeyEstimate {
    /// `"A minor"`, `"C major"`.
    pub key: String,
    pub tonic: String,
    pub mode: &'static str,
    /// Pearson correlation with the profile, -1..1 (above ~0.7 is a clear key).
    pub confidence: f64,
    /// The next best candidates, e.g. the relative major.
    pub alternatives: Vec<(String, f64)>,
}

fn pearson(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let (ma, mb) = (a.iter().sum::<f64>() / 12.0, b.iter().sum::<f64>() / 12.0);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da < 1e-12 || db < 1e-12 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

/// Key from a 12-bin pitch-class weight vector.
pub fn key_from_chroma(chroma: &[f64; 12]) -> Option<KeyEstimate> {
    key_from_chroma_hint(chroma, None)
}

/// As [`key_from_chroma`]; `tonic_hint` (the root of the opening chord) breaks ties between
/// near-equal candidates such as a major key and its relative minor.
pub fn key_from_chroma_hint(chroma: &[f64; 12], tonic_hint: Option<usize>) -> Option<KeyEstimate> {
    if chroma.iter().sum::<f64>() <= 1e-12 {
        return None;
    }
    let mut scores: Vec<(usize, bool, f64)> = Vec::with_capacity(24);
    for tonic in 0..12 {
        let mut rotated = [0.0; 12];
        for (i, r) in rotated.iter_mut().enumerate() {
            *r = chroma[(tonic + i) % 12];
        }
        scores.push((tonic, true, pearson(&rotated, &KK_MAJOR)));
        scores.push((tonic, false, pearson(&rotated, &KK_MINOR)));
    }
    scores.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    // The opening chord names the tonic when the top candidates are close (A minor vs C major).
    if let Some(h) = tonic_hint {
        let best = scores[0];
        let close = scores.iter().position(|s| s.0 == h && s.2 >= best.2 - 0.12);
        if let Some(i) = close.filter(|_| best.0 != h) {
            let pick = scores.remove(i);
            scores.insert(0, pick);
        }
    }
    let name =
        |t: usize, major: bool| format!("{} {}", pc_name(t), if major { "major" } else { "minor" });
    let (t, major, c) = scores[0];
    Some(KeyEstimate {
        key: name(t, major),
        tonic: pc_name(t).into(),
        mode: if major { "major" } else { "minor" },
        confidence: round(c, 3),
        alternatives: scores[1..4]
            .iter()
            .map(|(t, m, c)| (name(*t, *m), round(*c, 3)))
            .collect(),
    })
}

/// Duration- and velocity-weighted pitch-class profile, with the bass note of each bar
/// counted again (roots and tonics live in the bass).
pub fn chroma_of_notes(notes: &[SongNote], timing: &Timing) -> [f64; 12] {
    let mut c = [0.0; 12];
    for n in notes {
        c[n.key as usize % 12] += n.length as f64 * (0.5 + 0.5 * n.velocity as f64);
    }
    // Bass emphasis: per bar, the lowest key by sounding time.
    let bar = timing.ticks_per_bar();
    let last = notes.iter().map(|n| n.start + n.length).max().unwrap_or(0);
    let last_idx = last.saturating_sub(1) / bar;
    for b in 0..=last_idx {
        let (lo, hi) = (b * bar, (b + 1) * bar);
        let mut lowest: Option<(u8, f64)> = None;
        for n in notes {
            let ov = overlap(n.start, n.start + n.length, lo, hi) as f64;
            if ov >= (bar / 8) as f64 && lowest.is_none_or(|(k, _)| n.key < k) {
                lowest = Some((n.key, ov));
            }
        }
        // Count each bar's lowest note once more, per pitch class.
        let mut by_pc = [0.0f64; 12];
        for n in notes {
            if Some(n.key) == lowest.map(|l| l.0) {
                by_pc[n.key as usize % 12] += overlap(n.start, n.start + n.length, lo, hi) as f64
                    * (0.5 + 0.5 * n.velocity as f64);
            }
        }
        // Pieces tend to start and end on the tonic: weigh those bars' bass more.
        let boundary = if b == 0 || b == last_idx { 2.0 } else { 0.6 };
        for i in 0..12 {
            c[i] += boundary * by_pc[i];
        }
    }
    c
}

pub fn key_from_notes(notes: &[SongNote], timing: &Timing) -> Option<KeyEstimate> {
    let first = notes.iter().map(|n| n.start).min().unwrap_or(0) / timing.ticks_per_bar()
        * timing.ticks_per_bar();
    let hint = chord_in_window(notes, first, first + timing.ticks_per_bar())
        .and_then(|c| pc_index(&c.root));
    key_from_chroma_hint(&chroma_of_notes(notes, timing), hint)
}

fn pc_index(name: &str) -> Option<usize> {
    PC_NAMES.iter().position(|n| *n == name)
}

fn overlap(a0: Tick, a1: Tick, b0: Tick, b1: Tick) -> Tick {
    a1.min(b1).saturating_sub(a0.max(b0))
}

// ---------------------------------------------------------------------------------------
// chords

const TEMPLATES: &[(&str, &[usize])] = &[
    ("", &[0, 4, 7]),
    ("m", &[0, 3, 7]),
    ("dim", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("7", &[0, 4, 7, 10]),
    ("maj7", &[0, 4, 7, 11]),
    ("m7", &[0, 3, 7, 10]),
    ("m7b5", &[0, 3, 6, 10]),
    ("dim7", &[0, 3, 6, 9]),
    ("5", &[0, 7]),
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Chord {
    /// `Am`, `F`, `Cmaj7`, `G7/B`, `Dsus4`.
    pub symbol: String,
    pub root: String,
    pub quality: String,
    /// Pitch classes sounding (names), lowest first when known.
    pub bass: Option<String>,
    /// 0..1: how much of the sounding material the chord explains.
    pub fit: f64,
    /// Number of distinct pitch classes found.
    pub pitch_classes: usize,
}

/// Names the best chord for pitch-class weights `w` and an optional bass pitch class.
pub fn chord_from_weights(w: &[f64; 12], bass: Option<usize>) -> Option<Chord> {
    let total: f64 = w.iter().sum();
    let maxw = w.iter().copied().fold(0.0, f64::max);
    if total <= 1e-12 {
        return None;
    }
    // Ignore ornaments: pitch classes with under 8% of the strongest.
    let mut v = *w;
    for x in &mut v {
        if *x < 0.08 * maxw {
            *x = 0.0;
        }
    }
    let total: f64 = v.iter().sum();
    let pcs = v.iter().filter(|x| **x > 0.0).count();
    let mut best: Option<(f64, usize, &str, &[usize])> = None;
    for (suffix, ivs) in TEMPLATES {
        if *suffix == "5" && pcs > 2 {
            continue;
        }
        for root in 0..12 {
            let covered: f64 = ivs.iter().map(|i| v[(root + i) % 12]).sum();
            let extra = total - covered;
            let missing = ivs
                .iter()
                .filter(|i| v[(root + **i) % 12] < 0.12 * maxw)
                .count();
            let mut s = covered / total - 0.5 * extra / total - 0.2 * missing as f64;
            if ivs.len() > 3 {
                s -= 0.03;
            }
            if let Some(b) = bass {
                if b == root {
                    s += 0.08;
                } else if ivs.iter().any(|i| (root + i) % 12 == b) {
                    s += 0.03;
                }
            }
            if best.is_none_or(|(bs, ..)| s > bs) {
                best = Some((s, root, suffix, ivs));
            }
        }
    }
    let (_, root, suffix, ivs) = best?;
    let covered: f64 = ivs.iter().map(|i| v[(root + i) % 12]).sum();
    let slash = bass
        .filter(|b| *b != root && ivs.iter().any(|i| (root + i) % 12 == *b))
        .map(|b| format!("/{}", pc_name(b)))
        .unwrap_or_default();
    Some(Chord {
        symbol: format!("{}{suffix}{slash}", pc_name(root)),
        root: pc_name(root).into(),
        quality: if suffix.is_empty() { "maj" } else { suffix }.into(),
        bass: bass.map(|b| pc_name(b).to_string()),
        fit: round(covered / total, 2),
        pitch_classes: pcs,
    })
}

/// Chord sounding in `[from, to)` ticks.
pub fn chord_in_window(notes: &[SongNote], from: Tick, to: Tick) -> Option<Chord> {
    let mut w = [0.0; 12];
    let mut lowest: Option<(u8, f64)> = None;
    let win = (to - from) as f64;
    let mut sounding: Vec<(u8, f64)> = Vec::new();
    for n in notes {
        let ov = overlap(n.start, n.start + n.length, from, to) as f64;
        if ov <= 0.0 {
            continue;
        }
        let weight = ov * (0.5 + 0.5 * n.velocity as f64);
        w[n.key as usize % 12] += weight;
        sounding.push((n.key, ov));
    }
    for (k, ov) in sounding {
        // The bass is the lowest note that sounds for a meaningful part of the window.
        if ov >= 0.2 * win && lowest.is_none_or(|(lk, _)| k < lk) {
            lowest = Some((k, ov));
        }
    }
    chord_from_weights(&w, lowest.map(|(k, _)| k as usize % 12))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChordSpan {
    /// 1-based bar and beat where the chord starts.
    pub bar: u32,
    pub beat: u32,
    pub start_tick: Tick,
    pub length_ticks: Tick,
    pub symbol: String,
    pub fit: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChordGrain {
    /// One chord per bar.
    Bar,
    /// One chord per beat.
    Beat,
    /// One per bar, split at the half bar when both halves are clearly different chords.
    Auto,
}

/// Chord track for bars `first_bar..=last_bar` (1-based).
pub fn chords_from_notes(
    notes: &[SongNote],
    timing: &Timing,
    first_bar: u32,
    last_bar: u32,
    grain: ChordGrain,
) -> Vec<ChordSpan> {
    let bar_ticks = timing.ticks_per_bar();
    let mut out: Vec<ChordSpan> = Vec::new();
    let mut push = |start: Tick, len: Tick, c: Option<Chord>| {
        if let Some(c) = c {
            let (bar, beat, _) = timing.bar_beat_tick(start);
            out.push(ChordSpan {
                bar,
                beat,
                start_tick: start,
                length_ticks: len,
                symbol: c.symbol,
                fit: c.fit,
            });
        }
    };
    for bar in first_bar.max(1)..=last_bar {
        let start = (bar as u64 - 1) * bar_ticks;
        match grain {
            ChordGrain::Bar => push(
                start,
                bar_ticks,
                chord_in_window(notes, start, start + bar_ticks),
            ),
            ChordGrain::Beat => {
                let bt = timing.ticks_per_beat();
                let mut t = start;
                while t < start + bar_ticks {
                    push(t, bt, chord_in_window(notes, t, t + bt));
                    t += bt;
                }
            }
            ChordGrain::Auto => {
                let half = bar_ticks / 2;
                let whole = chord_in_window(notes, start, start + bar_ticks);
                let a = chord_in_window(notes, start, start + half);
                let b = chord_in_window(notes, start + half, start + bar_ticks);
                match (a, b) {
                    (Some(a), Some(b))
                        if a.symbol != b.symbol && a.pitch_classes >= 3 && b.pitch_classes >= 3 =>
                    {
                        push(start, half, Some(a));
                        push(start + half, bar_ticks - half, Some(b));
                    }
                    _ => push(start, bar_ticks, whole),
                }
            }
        }
    }
    out
}

/// `"Am - F - C - G"`: consecutive repeats are folded to `Am x2`.
pub fn progression(spans: &[ChordSpan]) -> String {
    let mut parts: Vec<(String, usize)> = Vec::new();
    for s in spans {
        match parts.last_mut() {
            Some((sym, n)) if *sym == s.symbol => *n += 1,
            _ => parts.push((s.symbol.clone(), 1)),
        }
    }
    parts
        .into_iter()
        .map(|(s, n)| if n > 1 { format!("{s} x{n}") } else { s })
        .collect::<Vec<_>>()
        .join(" - ")
}

// ---------------------------------------------------------------------------------------
// audio

const CHROMA_FFT: usize = 16384;

/// Pitch-class energy of the audio in `[from_s, to_s)`: windowed FFT, spectral peaks between
/// 55 Hz and 4.2 kHz with parabolic interpolation, magnitude folded onto 12 classes.
pub fn chroma_of_audio(mono: &[f32], sample_rate: u32, from_s: f64, to_s: f64) -> [f64; 12] {
    let sr = sample_rate as f64;
    let mut c = [0.0; 12];
    let (a, b) = (
        (from_s * sr) as usize,
        ((to_s * sr) as usize).min(mono.len()),
    );
    if b <= a + 256 {
        return c;
    }
    // Frames of CHROMA_FFT samples hopping half a frame, summed over the span.
    let mut start = a;
    loop {
        let end = (start + CHROMA_FFT).min(b);
        let seg = &mono[start..end];
        let mut re = vec![0.0; CHROMA_FFT];
        let mut im = vec![0.0; CHROMA_FFT];
        let w = |i: usize| 0.5 - 0.5 * (2.0 * PI * i as f64 / seg.len() as f64).cos();
        for (i, s) in seg.iter().enumerate() {
            re[i] = *s as f64 * w(i);
        }
        fft(&mut re, &mut im);
        let mag: Vec<f64> = (0..CHROMA_FFT / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .collect();
        let bin_hz = sr / CHROMA_FFT as f64;
        let (klo, khi) = (
            (55.0 / bin_hz) as usize,
            ((4200.0 / bin_hz) as usize).min(mag.len() - 2),
        );
        // (frequency, magnitude) of spectral peaks, lowest first.
        let mut peaks: Vec<(f64, f64)> = Vec::new();
        for k in klo.max(1)..khi {
            if mag[k] > mag[k - 1] && mag[k] >= mag[k + 1] && mag[k] > 1e-6 {
                let (l, m, r) = (mag[k - 1], mag[k], mag[k + 1]);
                let denom = l - 2.0 * m + r;
                let d = if denom.abs() > 1e-12 {
                    0.5 * (l - r) / denom
                } else {
                    0.0
                };
                peaks.push(((k as f64 + d) * bin_hz, m));
            }
        }
        // Harmonic suppression: assume partial h of a note has about 1/h of its amplitude,
        // so the 5th partial of a G is not mistaken for a B. Walk upward; each peak
        // that survives is a fundamental and subtracts its own partials from the peaks above.
        for i in 0..peaks.len() {
            let (f, m) = peaks[i];
            if m <= 0.0 {
                continue;
            }
            let midi = 69.0 + 12.0 * (f / 440.0).log2();
            if (midi - midi.round()).abs() < 0.35 {
                c[(midi.round() as i64).rem_euclid(12) as usize] += m.powf(0.6);
            }
            for h in 2..=8 {
                let target = f * h as f64;
                for p in peaks[i + 1..].iter_mut() {
                    if (12.0 * (p.0 / target).log2()).abs() < 0.4 {
                        p.1 = (p.1 - m / h as f64).max(0.0);
                    }
                }
            }
        }
        if end >= b || b - start <= CHROMA_FFT {
            break;
        }
        start += CHROMA_FFT / 2;
    }
    c
}

/// Key of rendered audio (whole range).
pub fn key_from_audio(
    channels: &[Vec<f32>],
    sample_rate: u32,
    timing: &Timing,
) -> Option<KeyEstimate> {
    let mono = mono_mix(channels);
    let secs = mono.len() as f64 / sample_rate as f64;
    let opening = chroma_of_audio(&mono, sample_rate, 0.0, timing.seconds_per_bar().min(secs));
    let hint = chord_from_weights(&opening, None).and_then(|c| pc_index(&c.root));
    key_from_chroma_hint(&chroma_of_audio(&mono, sample_rate, 0.0, secs), hint)
}

/// Chords of rendered audio, one per bar (or per beat).
pub fn chords_from_audio(
    channels: &[Vec<f32>],
    sample_rate: u32,
    timing: &Timing,
    first_bar: u32,
    grain: ChordGrain,
) -> Vec<ChordSpan> {
    let mono = mono_mix(channels);
    let secs = mono.len() as f64 / sample_rate as f64;
    let (step_ticks, bars) = match grain {
        ChordGrain::Beat => (
            timing.ticks_per_beat(),
            (secs / timing.seconds_per_bar()).ceil() as u32,
        ),
        _ => (
            timing.ticks_per_bar(),
            (secs / timing.seconds_per_bar()).ceil() as u32,
        ),
    };
    let mut out = Vec::new();
    let total_ticks = timing.ticks_per_bar() * bars as Tick;
    let mut t = 0;
    while t < total_ticks {
        let (s0, s1) = (
            timing.ticks_to_seconds(t),
            timing.ticks_to_seconds(t + step_ticks),
        );
        if s0 >= secs {
            break;
        }
        let chroma = chroma_of_audio(&mono, sample_rate, s0, s1);
        // Bass pitch class: the strongest pitch class below ~200 Hz is not separable from the
        // chroma alone, so audio chords are reported without inversions.
        if let Some(c) = chord_from_weights(&chroma, None) {
            let (bar, beat, _) = timing.bar_beat_tick(t);
            out.push(ChordSpan {
                bar: bar - 1 + first_bar,
                beat,
                start_tick: t,
                length_ticks: step_ticks,
                symbol: c.symbol,
                fit: c.fit,
            });
        }
        t += step_ticks;
    }
    out
}

/// Ticks per quarter, re-exported for callers building notes.
pub const TICKS_PER_QUARTER: Tick = PPQ;

#[cfg(test)]
mod tests;
