//! Tempo and beat grid of a render or an audio file: what `analyze what: ["tempo"]` reports.
//!
//! The tempo itself comes from [`crate::tempo`] (one estimator for samples and songs). On
//! top of it this module reads the *grid*:
//!
//! 1. **refined tempo:** the estimator's 0.05 BPM grid is too coarse over minutes (0.05 BPM
//!    at 115 BPM is 0.13 s after five minutes), so the tempo is refined by maximising the
//!    phase coherence of the onsets: with period `T`, the weighted mean of `exp(2πi·t/T)`
//!    over the onsets is large exactly when they sit on a grid of that period. The search
//!    covers ±1% around the estimate, coarse then fine (0.0005 BPM);
//! 2. **first beat:** the angle of that mean is the grid phase, so `first_beat_s` is the time
//!    of the first grid line (0 ≤ t < one beat) from the start of the audio. Onsets are
//!    weighted by the square of their strength (relative to the strongest), so the kick and
//!    snare outvote hi-hat subdivisions;
//! 3. **downbeat:** each onset near a grid line is assigned to one of `beats_per_bar` beat
//!    slots (grid beat index modulo the bar length). The slot whose beats are loudest (RMS of
//!    the 50 ms after the onset, averaged; silent beats count as zero) is the guess for beat 1; its
//!    confidence is the gap to the runner-up, and with no clear accent the first grid beat is
//!    assumed to be beat 1;
//! 4. **drift:** the same estimate on windows of about 30 s (the last one absorbs a
//!    remainder shorter than half a window). A steady click spreads less than 0.5%; a live
//!    performance drifts by a few percent; a tempo change shows as a step. Each window also
//!    reports how far its onsets sit from the *whole-song* grid, so slow drift shows as a
//!    growing `phase_vs_grid_ms` even when the tempo reads the same.

use crate::tempo::{self, Alternative};
use ph_dsp::onset;
use serde::Serialize;

/// Window of the drift measurement, in seconds.
pub const DRIFT_WINDOW_S: f64 = 30.0;

/// Spread of window tempos (relative to their median) under which the tempo is called steady.
const STEADY_PCT: f64 = 0.5;
/// Spread under which it is called drifting; beyond it the tempo changes.
const DRIFT_PCT: f64 = 3.0;
/// A beat of the bar must be at least this much stronger than the runner-up (relative to its
/// own strength) to be taken as the downbeat.
const MIN_ACCENT: f64 = 0.08;
/// Windows below this confidence are left out of the spread (but still listed).
const MIN_WINDOW_CONFIDENCE: f64 = 0.25;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BeatGrid {
    /// Beats per minute, refined against the onsets.
    pub bpm: f64,
    /// 0..1, from the estimator (above about 0.5 is reliable).
    pub confidence: f64,
    /// Half and double readings with their scores relative to `bpm` (1.0 = as good).
    pub alternatives: Vec<Alternative>,
    /// When the audio is a loop of a whole number of beats: that number (samples only).
    pub loop_beats: Option<u32>,
    pub beat_period_s: f64,
    /// Time of the first grid beat, seconds from the start of the audio (0 up to one beat).
    pub first_beat_s: f64,
    /// Time of the first detected onset, seconds.
    pub first_onset_s: f64,
    pub onsets: usize,
    pub downbeat: Downbeat,
    pub drift: Drift,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Downbeat {
    pub beats_per_bar: u32,
    /// Which beat of the bar the first grid beat is (1 = it is a downbeat).
    pub first_beat_is_beat: u32,
    /// Time of the first bar start on the grid, seconds.
    pub first_downbeat_s: f64,
    /// 0..1: how much stronger the chosen beat is than the runner-up.
    pub confidence: f64,
    /// `accent` (one beat of the bar is clearly stronger) or `assumed` (no clear accent: the
    /// first grid beat is taken as beat 1).
    pub basis: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Drift {
    pub window_s: f64,
    pub windows: Vec<WindowTempo>,
    /// Highest minus lowest window tempo (confident windows only).
    pub spread_bpm: f64,
    /// The spread as a percentage of the median window tempo.
    pub spread_pct: f64,
    /// `steady`, `drifting` (a few percent) or `changing` (tempo steps or a free performance).
    pub verdict: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WindowTempo {
    pub from_s: f64,
    pub to_s: f64,
    pub bpm: f64,
    pub confidence: f64,
    /// Mean offset of the window's onsets from the whole-song grid, ms: positive = late.
    /// `None` when the window has too few onsets.
    pub phase_vs_grid_ms: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectComparison {
    pub project_bpm: f64,
    pub measured_bpm: f64,
    /// Measured minus project, BPM (measured at the nearest octave when it reads half or double).
    pub difference_bpm: f64,
    /// `matches`, `half`, `double` or `differs`.
    pub relation: &'static str,
    /// Offset of the first grid beat from the project grid (the start of the range), ms,
    /// signed: positive = the audio's beats come late.
    pub grid_offset_ms: f64,
    /// One sentence for the agent.
    pub summary: String,
}

/// Onset times (seconds) with weights.
struct Pulse {
    t: Vec<f64>,
    /// Weight for the phase: the squared relative onset strength.
    w: Vec<f64>,
    /// Loudness (RMS of the 50 ms after the onset): what the downbeat is judged by, because
    /// the onset strength is log-compressed and hides accents.
    level: Vec<f64>,
}

/// Tempo and beat grid of audio (`[channel][frame]`). `beats_per_bar` is the numerator the
/// downbeat is searched over (4 when unknown); `snap_loops` lets a whole-number-of-beats
/// length snap the tempo (right for a sample, wrong for a song). `None` for audio shorter
/// than a second or without onsets.
pub fn analyze(
    channels: &[Vec<f32>],
    sample_rate: f64,
    beats_per_bar: u32,
    snap_loops: bool,
) -> Option<BeatGrid> {
    let sr = sample_rate.max(1.0);
    let x = onset::mono(channels);
    let seconds = x.len() as f64 / sr;
    let bpb = beats_per_bar.clamp(1, 16);
    let (est, strength) = tempo::estimate_full(channels, sr, snap_loops)?;
    let pulse = pulse_of(&x, &est.onsets, &strength, sr, 0.0);
    // a loop snapped to whole beats starts the search at its snapped tempo; the refinement
    // only moves away from it when the onsets line up better (a lead-in before the first
    // beat makes the file longer than its beats)
    let bpm = refine(&pulse, est.bpm).0;
    let period = 60.0 / bpm;
    let phase = phase_of(&pulse, period).0.rem_euclid(period);
    let downbeat = downbeat_of(&pulse, period, phase, bpb, seconds);
    let drift = drift_of(&x, sr, bpm, phase, seconds);
    Some(BeatGrid {
        bpm: round(bpm, 3),
        confidence: est.confidence,
        alternatives: est.alternatives,
        loop_beats: est.loop_beats,
        beat_period_s: round(period, 5),
        first_beat_s: round(phase, 4),
        first_onset_s: round(est.onsets.first().map_or(0.0, |o| *o as f64 / sr), 4),
        onsets: est.onsets.len(),
        downbeat,
        drift,
    })
}

impl BeatGrid {
    /// How the measured grid compares with the project's tempo and its grid (a render that
    /// starts at the project's bar line has its first beat at 0).
    pub fn compare_with_project(&self, project_bpm: f64) -> ProjectComparison {
        let measured = self.bpm;
        let tol = (0.005 * project_bpm).max(0.2);
        let (relation, at_octave) = if (measured - project_bpm).abs() <= tol {
            ("matches", measured)
        } else if (measured / 2.0 - project_bpm).abs() <= tol {
            ("double", measured / 2.0)
        } else if (measured * 2.0 - project_bpm).abs() <= 2.0 * tol {
            ("half", measured * 2.0)
        } else {
            ("differs", measured)
        };
        let diff = at_octave - project_bpm;
        // signed offset of the first grid beat from the nearest project beat line
        let period = self.beat_period_s.max(1e-6);
        let mut off = self.first_beat_s % period;
        if off > period / 2.0 {
            off -= period;
        }
        let grid_offset_ms = round(off * 1000.0, 1);
        let low = if self.confidence < 0.4 {
            " (low confidence)"
        } else {
            ""
        };
        let summary = match relation {
            "matches" => format!(
                "matches the project tempo ({project_bpm} BPM; measured {measured}{low}); first beat {grid_offset_ms} ms from the project grid"
            ),
            "double" => format!(
                "reads {measured} BPM, double the project tempo of {project_bpm}: the estimator locked onto the double-time pulse{low}"
            ),
            "half" => format!(
                "reads {measured} BPM, half the project tempo of {project_bpm}: the estimator locked onto the half-time pulse{low}"
            ),
            _ => format!(
                "differs from the project tempo by {:.2} BPM (measured {measured}, project {project_bpm}{low})",
                diff.abs()
            ),
        };
        ProjectComparison {
            project_bpm,
            measured_bpm: measured,
            difference_bpm: round(diff, 3),
            relation,
            grid_offset_ms,
            summary,
        }
    }

    /// Where bar 1 starts on the measured grid, seconds: bar-aligned analysis of a sample uses
    /// it as time zero.
    pub fn bar_start_s(&self) -> f64 {
        self.downbeat.first_downbeat_s
    }
}

fn round(x: f64, places: i32) -> f64 {
    crate::round(x, places)
}

/// Onsets with weights: the square of the strongest onset-strength value near the onset,
/// relative to the global maximum. `offset_s` shifts times (a window inside a longer file).
fn pulse_of(x: &[f32], onsets: &[u64], s: &onset::Strength, sr: f64, offset_s: f64) -> Pulse {
    let gmax = s.values.iter().copied().fold(0.0f32, f32::max).max(1e-9) as f64;
    let mut t = Vec::with_capacity(onsets.len());
    let mut w = Vec::with_capacity(onsets.len());
    let mut level = Vec::with_capacity(onsets.len());
    let span = (0.05 * sr) as usize;
    for &o in onsets {
        // frame k covers samples k·HOP .. k·HOP + FRAME, so the flux peak of an onset at
        // sample o lies a few frames before and one after o / HOP
        let k = o as usize / onset::HOP;
        let (lo, hi) = (
            k.saturating_sub(4),
            (k + 2).min(s.values.len().saturating_sub(1)),
        );
        let m = s.values[lo..=hi.max(lo)]
            .iter()
            .copied()
            .fold(0.0f32, f32::max) as f64
            / gmax;
        t.push(o as f64 / sr + offset_s);
        w.push(m * m);
        let seg = &x[(o as usize).min(x.len())..(o as usize + span).min(x.len())];
        level.push(
            (seg.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / seg.len().max(1) as f64).sqrt(),
        );
    }
    Pulse { t, w, level }
}

/// Weighted circular mean of the onset times at `period`: (phase in seconds, in
/// -period/2..period/2, and the coherence 0..1).
fn phase_of(p: &Pulse, period: f64) -> (f64, f64) {
    let (mut c, mut s, mut sum) = (0.0, 0.0, 0.0);
    for (t, w) in p.t.iter().zip(&p.w) {
        let a = std::f64::consts::TAU * t / period;
        c += w * a.cos();
        s += w * a.sin();
        sum += w;
    }
    if sum <= 0.0 {
        return (0.0, 0.0);
    }
    (
        s.atan2(c) / std::f64::consts::TAU * period,
        (c * c + s * s).sqrt() / sum,
    )
}

/// Refines `bpm` within ±1% by maximising the phase coherence; returns (bpm, coherence).
fn refine(p: &Pulse, bpm: f64) -> (f64, f64) {
    if p.t.len() < 4 {
        return (bpm, 0.0);
    }
    let coherence = |b: f64| phase_of(p, 60.0 / b).1;
    let mut best = (bpm, coherence(bpm));
    let (lo, hi) = (bpm * 0.99, bpm * 1.01);
    let mut b = lo;
    while b <= hi {
        let r = coherence(b);
        if r > best.1 {
            best = (b, r);
        }
        b += 0.01;
    }
    let (centre, mut best) = (best.0, best);
    let mut b = centre - 0.01;
    while b <= centre + 0.01 {
        let r = coherence(b);
        if r > best.1 {
            best = (b, r);
        }
        b += 0.0005;
    }
    best
}

fn downbeat_of(p: &Pulse, period: f64, phase: f64, bpb: u32, seconds: f64) -> Downbeat {
    let bpb_u = bpb as usize;
    let beats = (((seconds - phase) / period).floor().max(0.0) as usize) + 1;
    let mut sum = vec![0.0f64; bpb_u];
    // only the strongest onset on each grid beat counts
    let mut on_beat: std::collections::BTreeMap<i64, f64> = Default::default();
    for (t, w) in p.t.iter().zip(&p.level) {
        let n = ((t - phase) / period).round();
        if ((t - phase) - n * period).abs() <= 0.2 * period && n >= 0.0 {
            let e = on_beat.entry(n as i64).or_insert(0.0);
            *e = e.max(*w);
        }
    }
    for (n, w) in on_beat {
        sum[n as usize % bpb_u] += w;
    }
    // beats in each slot (the audio need not hold a whole number of bars)
    let count = |k: usize| (beats + bpb_u - 1 - k) / bpb_u;
    let mean: Vec<f64> = (0..bpb_u)
        .map(|k| sum[k] / count(k).max(1) as f64)
        .collect();
    let (best, &top) = mean
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
        .expect("at least one slot");
    let second = mean
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != best)
        .map(|(_, v)| *v)
        .fold(0.0, f64::max);
    let confidence = if bpb_u == 1 || top <= 0.0 {
        0.0
    } else {
        ((top - second) / top).clamp(0.0, 1.0)
    };
    // equal strengths (a kick on every beat) say nothing about the bar: keep the grid's start
    let best = if confidence < MIN_ACCENT { 0 } else { best };
    Downbeat {
        beats_per_bar: bpb,
        first_beat_is_beat: ((bpb_u - best) % bpb_u) as u32 + 1,
        first_downbeat_s: round(phase + best as f64 * period, 4),
        confidence: round(confidence, 3),
        basis: if confidence < MIN_ACCENT {
            "assumed"
        } else {
            "accent"
        },
    }
}

fn drift_of(x: &[f32], sr: f64, bpm: f64, phase: f64, seconds: f64) -> Drift {
    let w = DRIFT_WINDOW_S;
    // window edges: whole windows, the remainder joins the last one unless it is half a
    // window or more
    let mut edges = vec![0.0];
    if seconds >= 1.5 * w {
        let mut n = (seconds / w).floor() as usize;
        if seconds - n as f64 * w >= 0.5 * w {
            n += 1;
        }
        let step = seconds / n as f64;
        edges.extend((1..=n).map(|i| i as f64 * step));
    } else {
        edges.push(seconds);
    }
    let period = 60.0 / bpm;
    let mut windows = Vec::new();
    for pair in edges.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let seg = &x[((a * sr) as usize).min(x.len())..((b * sr) as usize).min(x.len())];
        let Some((est, strength)) = tempo::estimate_full(&[seg.to_vec()], sr, false) else {
            continue;
        };
        let pulse = pulse_of(seg, &est.onsets, &strength, sr, a);
        let wbpm = refine(&pulse, est.bpm).0;
        let phase_ms = (pulse.t.len() >= 4).then(|| {
            // the window's mean onset phase at the *global* period, against the global phase
            let (ph, _) = phase_of(&pulse, period);
            let mut d = (ph - phase).rem_euclid(period);
            if d > period / 2.0 {
                d -= period;
            }
            round(d * 1000.0, 1)
        });
        windows.push(WindowTempo {
            from_s: round(a, 2),
            to_s: round(b, 2),
            bpm: round(wbpm, 3),
            confidence: est.confidence,
            phase_vs_grid_ms: phase_ms,
        });
    }
    let mut firm: Vec<f64> = windows
        .iter()
        .filter(|w| w.confidence >= MIN_WINDOW_CONFIDENCE)
        .map(|w| w.bpm)
        .collect();
    if firm.is_empty() {
        firm = windows.iter().map(|w| w.bpm).collect();
    }
    firm.sort_by(f64::total_cmp);
    let (spread, pct) = match (firm.first(), firm.last()) {
        (Some(lo), Some(hi)) => {
            let median = firm[firm.len() / 2];
            (hi - lo, (hi - lo) / median * 100.0)
        }
        _ => (0.0, 0.0),
    };
    let verdict = if pct < STEADY_PCT {
        "steady"
    } else if pct < DRIFT_PCT {
        "drifting"
    } else {
        "changing"
    };
    Drift {
        window_s: w,
        windows,
        spread_bpm: round(spread, 3),
        spread_pct: round(pct, 2),
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// A click is a 3 ms decaying burst (200 Hz boom plus noise), so the detector has an
    /// attack to find. `times` in seconds, `accents` scale each click.
    fn clicks(times: &[(f64, f32)], seconds: f64) -> Vec<f32> {
        let n = (seconds * SR) as usize;
        let mut out = vec![0.0f32; n];
        let mut seed = 99u32;
        for &(t, amp) in times {
            let start = (t * SR) as usize;
            for i in 0..(0.06 * SR) as usize {
                if start + i >= n {
                    break;
                }
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
                let env = (-(i as f32) / (0.012 * SR as f32)).exp();
                let tone = (std::f32::consts::TAU * 180.0 * i as f32 / SR as f32).sin();
                out[start + i] += amp * env * (0.7 * tone + 0.3 * noise);
            }
        }
        out
    }

    /// Steady clicks at `bpm`, the first at `offset` s, every fourth one accented.
    fn track(bpm: f64, offset: f64, seconds: f64) -> Vec<Vec<f32>> {
        let period = 60.0 / bpm;
        let times: Vec<(f64, f32)> = (0..)
            .map(|i| (offset + i as f64 * period, i))
            .take_while(|(t, _)| *t < seconds - 0.1)
            .map(|(t, i)| (t, if i % 4 == 0 { 0.9 } else { 0.5 }))
            .collect();
        vec![clicks(&times, seconds)]
    }

    #[test]
    fn steady_click_track_tempo_offset_and_downbeat() {
        let a = track(115.0, 0.012, 60.0);
        let g = analyze(&a, SR, 4, false).unwrap();
        assert!((g.bpm - 115.0).abs() < 0.02, "bpm {}", g.bpm);
        assert!((g.first_beat_s - 0.012).abs() < 0.004, "{}", g.first_beat_s);
        assert_eq!(g.downbeat.first_beat_is_beat, 1);
        assert!(g.downbeat.confidence > 0.1, "{:?}", g.downbeat);
        assert_eq!(g.drift.verdict, "steady", "{:?}", g.drift);
        assert_eq!(g.drift.windows.len(), 2);
        for w in &g.drift.windows {
            assert!(w.phase_vs_grid_ms.unwrap().abs() < 5.0, "{w:?}");
        }
    }

    #[test]
    fn offset_inside_the_beat_and_a_late_downbeat() {
        // first click 0.2 s in; the accent falls on the third click (beat 3 of the first bar)
        let period = 60.0 / 100.0;
        let times: Vec<(f64, f32)> = (0..40)
            .map(|i| {
                let amp = if (i + 2) % 4 == 0 { 0.9 } else { 0.5 };
                (0.2 + i as f64 * period, amp)
            })
            .collect();
        let a = vec![clicks(&times, 25.0)];
        let g = analyze(&a, SR, 4, false).unwrap();
        assert!((g.bpm - 100.0).abs() < 0.03, "{}", g.bpm);
        assert!((g.first_beat_s - 0.2).abs() < 0.005, "{}", g.first_beat_s);
        // accents on grid beats 2, 6, 10 ..: the first grid beat is beat 3 of its bar
        assert_eq!(g.downbeat.first_beat_is_beat, 3, "{:?}", g.downbeat);
        assert!(
            (g.downbeat.first_downbeat_s - (0.2 + 2.0 * period)).abs() < 0.006,
            "{:?}",
            g.downbeat
        );
    }

    #[test]
    fn tempo_change_shows_in_the_windows() {
        // 60 s at 100 BPM, then 60 s at 120 BPM
        let mut times: Vec<(f64, f32)> = Vec::new();
        let mut t = 0.05;
        let mut i = 0;
        while t < 60.0 {
            times.push((t, if i % 4 == 0 { 0.9 } else { 0.5 }));
            t += 0.6;
            i += 1;
        }
        while t < 119.9 {
            times.push((t, if i % 4 == 0 { 0.9 } else { 0.5 }));
            t += 0.5;
            i += 1;
        }
        let a = vec![clicks(&times, 120.0)];
        let g = analyze(&a, SR, 4, false).unwrap();
        assert_eq!(g.drift.verdict, "changing", "{:?}", g.drift);
        assert_eq!(g.drift.windows.len(), 4);
        assert!(
            (g.drift.windows[0].bpm - 100.0).abs() < 0.3,
            "{:?}",
            g.drift
        );
        assert!(
            (g.drift.windows[3].bpm - 120.0).abs() < 0.3,
            "{:?}",
            g.drift
        );
        assert!(g.drift.spread_bpm > 15.0);
    }

    #[test]
    fn slow_drift_is_visible_in_the_window_phases() {
        // 99 BPM for 90 s, then the whole song is read against its own grid: a click track a
        // hair slower in the second half drifts against the global grid
        let mut times: Vec<(f64, f32)> = Vec::new();
        let mut t = 0.1;
        let mut i = 0;
        while t < 89.5 {
            let p = if t < 45.0 { 60.0 / 100.0 } else { 60.0 / 99.0 };
            times.push((t, if i % 4 == 0 { 0.9 } else { 0.5 }));
            t += p;
            i += 1;
        }
        let a = vec![clicks(&times, 90.0)];
        let g = analyze(&a, SR, 4, false).unwrap();
        let ph: Vec<f64> = g
            .drift
            .windows
            .iter()
            .map(|w| w.phase_vs_grid_ms.unwrap())
            .collect();
        assert_eq!(ph.len(), 3);
        // the whole-song grid cannot follow both tempos, so the windows sit at visibly
        // different phases against it
        let spread = ph.iter().cloned().fold(f64::MIN, f64::max)
            - ph.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread > 30.0, "{ph:?}");
    }

    #[test]
    fn project_comparison_names_match_double_half_and_difference() {
        let a = track(120.0, 0.0, 30.0);
        let g = analyze(&a, SR, 4, false).unwrap();
        let m = g.compare_with_project(120.0);
        assert_eq!(m.relation, "matches", "{m:?}");
        assert!(m.summary.starts_with("matches the project tempo"), "{m:?}");
        assert!(m.grid_offset_ms.abs() < 4.0, "{m:?}");
        let d = g.compare_with_project(125.0);
        assert_eq!(d.relation, "differs");
        assert!(d.summary.contains("differs"), "{d:?}");
        assert!((d.difference_bpm + 5.0).abs() < 0.1);
        let dbl = g.compare_with_project(60.0);
        assert_eq!(dbl.relation, "double", "{dbl:?}");
        let half = g.compare_with_project(240.0);
        assert_eq!(half.relation, "half", "{half:?}");
    }

    #[test]
    fn silence_and_short_audio_have_no_grid() {
        assert!(analyze(&[vec![0.0; 96_000]], SR, 4, false).is_none());
        assert!(analyze(&[vec![0.1; 1000]], SR, 4, false).is_none());
    }
}
