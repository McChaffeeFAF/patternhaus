//! Tempo (BPM) estimation of audio material, for the sample registry (`sample.detect_bpm`,
//! `audio.fit_to_tempo`) and the slicer (`slicer.detect`).
//!
//! Method (public literature: Ellis, "Beat Tracking by Dynamic Programming", JNMR 2007,
//! for the autocorrelation tempo with a log-Gaussian prior; Davies & Plumbley, "Context-
//! Dependent Beat Tracking", IEEE TASLP 2007, for comb-weighted periodicity):
//! 1. the onset-strength envelope of `ph_dsp::onset` (spectral flux, 256-sample hop),
//!    minus its local mean, half-wave rectified;
//! 2. its autocorrelation;
//! 3. every tempo from 60 to 200 BPM in 0.05 BPM steps is scored by a comb over the beat
//!    period and its multiples (2, 3, 4 beats) and half the period (eighth notes), times a
//!    log-Gaussian preference for tempos near 120 BPM; the best is refined by parabolic
//!    interpolation;
//! 4. **half/double:** the scores of half and double the winner are reported as
//!    alternatives; the winner must beat them by the prior-weighted comb score, which is
//!    what resolves the octave;
//! 5. **loops:** a sample whose length is within 2% of a whole number of beats (1, 2, 3, 4,
//!    6, 8, 12, 16, 24, 32, 48 or 64) at the estimate is taken to be a loop of that many
//!    beats and the tempo snaps to exactly `60 × beats / seconds`.
//!
//! The confidence (0..1) combines how far the winning score stands above the median score
//! and how clearly it beats its half and double.

use ph_dsp::onset;
use serde::Serialize;

/// Lowest and highest tempo reported.
pub const MIN_BPM: f64 = 60.0;
pub const MAX_BPM: f64 = 200.0;
/// Version tag of this analysis, stored in `SampleEntry::analysis.onsets` so results can be
/// recognised (and recomputed) later; the onset list itself is derived from the file.
pub const ONSETS_KEY: &str = "ph-analysis/onsets/v1";

/// Upper edge of the band the beat is taken from.
const BEAT_BAND_HZ: f64 = 1500.0;
/// Width of the tempo preference, in octaves (one standard deviation).
const PRIOR_OCTAVES: f64 = 1.0;

/// A faster octave is chosen when its beat pulse is at least this share of the strongest.
const OCTAVE_PULSE: f64 = 0.75;

/// Whole-beat lengths a loop may have.
const LOOP_BEATS: [u32; 12] = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TempoEstimate {
    /// Beats per minute, 60..=200.
    pub bpm: f64,
    /// 0..1: how sure the estimate is (above about 0.5 is reliable).
    pub confidence: f64,
    /// When the sample is a loop of a whole number of beats at this tempo: that number.
    pub loop_beats: Option<u32>,
    /// The half- and double-tempo readings with their scores relative to the winner (1.0 =
    /// as good), when they lie in 30..=400 BPM.
    pub alternatives: Vec<Alternative>,
    /// Onsets found (default sensitivity), in samples of the file.
    pub onsets: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Alternative {
    pub bpm: f64,
    pub score: f64,
}

/// Estimates the tempo of audio (`[channel][frame]`). `None` for material shorter than a
/// second or without onsets.
pub fn estimate(channels: &[Vec<f32>], sample_rate: f64) -> Option<TempoEstimate> {
    let x = onset::mono(channels);
    let sr = sample_rate.max(1.0);
    if (x.len() as f64) < sr {
        return None;
    }
    let s = onset::strength(&x, sr);
    let onsets = {
        let mut o: Vec<u64> = onset::pick(&s, 0.5)
            .into_iter()
            .map(|f| (f * onset::HOP) as u64)
            .collect();
        // sample-accurate positions, as the slicer reports them
        let fine = onset::detect(&x, sr, 0.5);
        if fine.len() == o.len() {
            o = fine;
        }
        o
    };
    if onsets.is_empty() {
        return None;
    }
    // the beat comes from the low and mid bands (kick, snare, bass, chords); hi-hats mark
    // subdivisions and pull estimates up an octave
    let low = onset::strength_below(&x, sr, BEAT_BAND_HZ);
    let env = novelty(&low.values, low.rate());
    let ac = autocorrelation(&env, (4.0 * lag_of(MIN_BPM, s.rate())).ceil() as usize + 3);
    let fr = s.rate();
    let score = |bpm: f64| -> f64 { comb(&ac, lag_of(bpm, fr)) * prior(bpm) };
    // grid search, then parabolic refinement
    let step = 0.05;
    let n = ((MAX_BPM - MIN_BPM) / step).round() as usize;
    let grid: Vec<f64> = (0..=n).map(|i| score(MIN_BPM + i as f64 * step)).collect();
    let (best_i, best) = grid
        .iter()
        .copied()
        .enumerate()
        .fold((0, f64::MIN), |a, (i, v)| if v > a.1 { (i, v) } else { a });
    if best <= 0.0 {
        return None;
    }
    let mut bpm = MIN_BPM + best_i as f64 * step;
    if best_i > 0 && best_i < n {
        let (a, b, c) = (grid[best_i - 1], grid[best_i], grid[best_i + 1]);
        let d = a - 2.0 * b + c;
        if d < 0.0 {
            bpm += step * (0.5 * (a - c) / d).clamp(-0.5, 0.5);
        }
    }
    // octave: the comb favours slower readings (their multiples are periodic too), so pick
    // the fastest of half, same and double whose own beat period is still clearly marked in
    // the low band (pulse at least OCTAVE_PULSE of the strongest)
    let pulse = |b: f64| peak_ac(&ac, lag_of(b, fr));
    let family: Vec<f64> = [bpm / 2.0, bpm, bpm * 2.0]
        .into_iter()
        .filter(|b| (MIN_BPM - 0.5..=MAX_BPM + 0.5).contains(b))
        .collect();
    let strongest = family.iter().map(|b| pulse(*b)).fold(0.0, f64::max);
    if let Some(&fast) = family
        .iter()
        .rev()
        .find(|b| pulse(**b) >= OCTAVE_PULSE * strongest)
    {
        bpm = fast.clamp(MIN_BPM, MAX_BPM);
    }
    // loops: snap to a whole number of beats
    let seconds = x.len() as f64 / sr;
    let mut loop_beats = None;
    let beats = seconds * bpm / 60.0;
    if let Some(&b) = LOOP_BEATS
        .iter()
        .filter(|&&b| ((beats - b as f64) / b as f64).abs() < 0.02)
        .min_by(|a, b| {
            let da = (beats - **a as f64).abs();
            let db = (beats - **b as f64).abs();
            da.total_cmp(&db)
        })
    {
        let snapped = 60.0 * b as f64 / seconds;
        if (MIN_BPM..=MAX_BPM).contains(&snapped) {
            bpm = snapped;
            loop_beats = Some(b);
        }
    }
    // confidence: prominence over the median, and clarity against half/double
    let mut sorted = grid.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let prominence = ((best - median) / best).clamp(0.0, 1.0);
    let raw = |b: f64| comb(&ac, lag_of(b, fr)) * prior(b);
    let win = raw(bpm).max(1e-12);
    let alternatives: Vec<Alternative> = [bpm / 2.0, bpm * 2.0]
        .into_iter()
        .filter(|b| (30.0..=400.0).contains(b))
        .map(|b| Alternative {
            bpm: round3(b),
            score: round3((raw(b) / win).max(0.0)),
        })
        .collect();
    let rival = alternatives.iter().map(|a| a.score).fold(0.0, f64::max);
    let clarity = (1.0 - rival).clamp(0.0, 1.0).sqrt();
    let confidence =
        (prominence * (0.5 + 0.5 * clarity) * if loop_beats.is_some() { 1.0 } else { 0.85 })
            .clamp(0.0, 1.0);
    Some(TempoEstimate {
        bpm: round3(bpm),
        confidence: round3(confidence),
        loop_beats,
        alternatives,
        onsets,
    })
}

/// Transients of audio (`[channel][frame]`), in frames: the slicer's points. `sensitivity`
/// 0..=1, higher finds more (0.5 is the default). The engine's Slice mode uses the same
/// detector at the default.
pub fn onsets(channels: &[Vec<f32>], sample_rate: f64, sensitivity: f64) -> Vec<u64> {
    onset::detect(&onset::mono(channels), sample_rate, sensitivity)
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Envelope frames per beat at `bpm`.
fn lag_of(bpm: f64, frame_rate: f64) -> f64 {
    60.0 * frame_rate / bpm
}

/// Onset strength minus its local mean (about 0.3 s), half-wave rectified.
fn novelty(v: &[f32], frame_rate: f64) -> Vec<f64> {
    let half = ((0.15 * frame_rate).round() as usize).max(1);
    let n = v.len();
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + v[i] as f64;
    }
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(n);
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (v[i] as f64 - mean).max(0.0)
        })
        .collect()
}

/// Unbiased autocorrelation for lags `0..max_lag`, normalised so lag 0 is 1.
fn autocorrelation(x: &[f64], max_lag: usize) -> Vec<f64> {
    let n = x.len();
    let max_lag = max_lag.min(n.saturating_sub(1));
    let mut out = Vec::with_capacity(max_lag);
    for l in 0..max_lag {
        let s: f64 = (0..n - l).map(|i| x[i] * x[i + l]).sum();
        out.push(s / (n - l) as f64);
    }
    let z = out.first().copied().unwrap_or(0.0);
    if z > 0.0 {
        for v in &mut out {
            *v /= z;
        }
    }
    out
}

/// Autocorrelation at a fractional lag (linear interpolation; 0 outside).
fn ac_at(ac: &[f64], lag: f64) -> f64 {
    let i = lag.floor() as usize;
    if lag < 0.0 || i + 1 >= ac.len() {
        return 0.0;
    }
    let f = lag - i as f64;
    ac[i] * (1.0 - f) + ac[i + 1] * f
}

/// Autocorrelation peak near a lag (onsets are quantised to frames).
fn peak_ac(ac: &[f64], lag: f64) -> f64 {
    ac_at(ac, lag)
        .max(ac_at(ac, lag - 1.0))
        .max(ac_at(ac, lag + 1.0))
}

/// Comb score of a beat period: the beat and its multiples, plus the half beat, each read
/// at the best of three neighbouring lags (onsets are quantised to frames).
fn comb(ac: &[f64], lag: f64) -> f64 {
    let peak = |l: f64| peak_ac(ac, l);
    let mut s = 0.0;
    let mut w = 0.0;
    for (m, weight) in [(1.0, 1.0), (2.0, 0.6), (3.0, 0.3), (4.0, 0.5)] {
        let l = lag * m;
        if l + 2.0 < ac.len() as f64 {
            s += weight * peak(l);
            w += weight;
        }
    }
    if w == 0.0 {
        return 0.0;
    }
    s / w + 0.25 * peak(lag / 2.0)
}

/// Log-Gaussian preference for tempos near 120 BPM ([`PRIOR_OCTAVES`] wide).
fn prior(bpm: f64) -> f64 {
    let z = (bpm / 120.0).log2() / PRIOR_OCTAVES;
    (-0.5 * z * z).exp()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// A generated drum part: `(step, voice)` hits on a 16th grid, `kick` 0, `snare` 1,
    /// `hat` 2. Deterministic noise.
    pub fn drums(bpm: f64, sr: f64, steps: usize, hits: &[(usize, u8)], swing: f64) -> Vec<f32> {
        let step_s = 60.0 / bpm / 4.0;
        let n = (steps as f64 * step_s * sr).round() as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 77u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / 8_388_608.0 - 1.0
        };
        // hats are high-passed noise (second difference), like real cymbals
        let (mut h1, mut h2) = (0.0f32, 0.0f32);
        for &(st, voice) in hits {
            let mut t = st as f64 * step_s;
            if st % 2 == 1 {
                t += swing * step_s * 0.5;
            }
            let s0 = (t * sr).round() as usize;
            let len = match voice {
                0 => 0.35,
                1 => 0.2,
                _ => 0.06,
            };
            let mut ph = 0.0f64;
            for i in 0..((len * sr) as usize) {
                let k = s0 + i;
                let tt = i as f64 / sr;
                // wrap around: the part is a loop
                let k = k % n;
                let v = match voice {
                    0 => {
                        ph += (50.0 + 120.0 * (-tt * 35.0).exp()) / sr;
                        ((ph * TAU).sin() * (-tt * 9.0).exp() * 0.9) as f32
                    }
                    1 => {
                        (0.35 * (tt * 190.0 * TAU).sin() * (-tt * 22.0).exp()) as f32
                            + 0.45 * noise() * (-tt * 16.0).exp() as f32
                    }
                    _ => {
                        let n = noise();
                        let hp = n - 2.0 * h1 + h2;
                        h2 = h1;
                        h1 = n;
                        0.12 * hp * (-tt * 60.0).exp() as f32
                    }
                };
                x[k] += v;
            }
        }
        x
    }

    /// Patterns over one bar of 16 steps, repeated `bars` times.
    pub fn pattern(kind: usize, bars: usize) -> Vec<(usize, u8)> {
        let one: Vec<(usize, u8)> = match kind {
            // four on the floor, off-beat hats
            0 => vec![
                (0, 0),
                (4, 0),
                (8, 0),
                (12, 0),
                (2, 2),
                (6, 2),
                (10, 2),
                (14, 2),
                (4, 1),
                (12, 1),
            ],
            // backbeat with 8th hats
            1 => {
                let mut v = vec![(0, 0), (6, 0), (8, 0), (4, 1), (12, 1)];
                v.extend((0..16).step_by(2).map(|s| (s, 2)));
                v
            }
            // breakbeat with 16th hats
            2 => {
                let mut v = vec![(0, 0), (3, 0), (10, 0), (4, 1), (7, 1), (12, 1), (15, 1)];
                v.extend((0..16).map(|s| (s, 2)));
                v
            }
            // half-time: kick on 1, snare on 3, 8th hats
            _ => {
                let mut v = vec![(0, 0), (10, 0), (8, 1)];
                v.extend((0..16).step_by(2).map(|s| (s, 2)));
                v
            }
        };
        (0..bars)
            .flat_map(|b| one.iter().map(move |(s, v)| (s + 16 * b, *v)))
            .collect()
    }

    /// The generated test set: (bpm, pattern kind, bars, swing, trim the end off-grid).
    pub fn test_set() -> Vec<(f64, usize, usize, f64, bool)> {
        let mut v = Vec::new();
        let tempos = [
            62.0, 70.0, 75.0, 85.0, 90.0, 96.0, 100.0, 110.0, 118.0, 120.0, 125.0, 128.0, 135.0,
            140.0, 150.0, 160.0, 165.0, 174.0, 180.0, 195.0,
        ];
        for (i, &bpm) in tempos.iter().enumerate() {
            let kind = i % 4;
            let bars = if bpm > 150.0 { 4 } else { 2 };
            let swing = if i % 5 == 3 { 0.4 } else { 0.0 };
            v.push((bpm, kind, bars, swing, false));
            // the same groove, not a whole loop: 1.37 bars more and cut off-grid
            v.push((bpm, (kind + 1) % 4, bars + 1, swing, true));
        }
        v
    }

    pub fn render(bpm: f64, kind: usize, bars: usize, swing: f64, trim: bool, sr: f64) -> Vec<f32> {
        let mut x = drums(bpm, sr, bars * 16, &pattern(kind, bars), swing);
        if trim {
            let keep = (x.len() as f64 * 0.83) as usize;
            x.truncate(keep);
        }
        x
    }

    /// The accuracy report for `docs/milestones/M2.md` (run with `--nocapture`): every
    /// reading is the right tempo or an octave of it; straight grooves (four on the floor,
    /// backbeat) are read at the right octave; syncopated breakbeats and half-time grooves
    /// may read at half tempo (they are genuinely ambiguous; `alternatives` offers double).
    #[test]
    fn detects_generated_drum_loops() {
        let sr = 44_100.0;
        let set = test_set();
        // per pattern kind: (exact, octave, total)
        let mut by_kind = [(0usize, 0usize, 0usize); 4];
        let mut lines = Vec::new();
        let mut max_err_pct = 0.0f64;
        for &(bpm, kind, bars, swing, trim) in &set {
            let x = render(bpm, kind, bars, swing, trim, sr);
            let e = estimate(&[x], sr).expect("estimate");
            let err = (e.bpm / bpm - 1.0).abs();
            let oct = [
                (e.bpm * 2.0 / bpm - 1.0).abs(),
                (e.bpm / 2.0 / bpm - 1.0).abs(),
            ]
            .into_iter()
            .fold(f64::MAX, f64::min);
            let k = &mut by_kind[kind];
            k.2 += 1;
            if err < 0.01 {
                k.0 += 1;
                max_err_pct = max_err_pct.max(err * 100.0);
            } else if oct < 0.01 {
                k.1 += 1;
                max_err_pct = max_err_pct.max(oct * 100.0);
            }
            lines.push(format!(
                "{bpm:>6} kind {kind} bars {bars} swing {swing} trim {trim:<5} -> {:>8.3} conf {:.2} loop {:?}",
                e.bpm, e.confidence, e.loop_beats
            ));
        }
        let names = ["four on the floor", "backbeat", "breakbeat", "half-time"];
        println!(
            "{}",
            lines.join(
                "
"
            )
        );
        for (k, (ex, oc, n)) in by_kind.iter().enumerate() {
            println!("{:<18} exact {ex}/{n}, octave {oc}/{n}", names[k]);
        }
        let exact: usize = by_kind.iter().map(|k| k.0).sum();
        let octave: usize = by_kind.iter().map(|k| k.1).sum();
        println!(
            "all: exact (within 1%) {exact}/{}, octave errors {octave}, worst error {max_err_pct:.3}%",
            set.len()
        );
        let report = lines.join(
            "
",
        );
        assert_eq!(
            exact + octave,
            set.len(),
            "a reading off by more than an octave
{report}"
        );
        for k in [0, 1] {
            assert_eq!(
                by_kind[k].0, by_kind[k].2,
                "{}: wrong octave
{report}",
                names[k]
            );
        }
        assert!(
            exact * 100 >= set.len() * 65,
            "{exact}/{} exact
{report}",
            set.len()
        );
    }

    #[test]
    fn exact_loops_snap_to_the_loop_length() {
        let sr = 48_000.0;
        let x = render(127.0, 0, 2, 0.0, false, sr);
        let e = estimate(&[x.clone(), x], sr).unwrap();
        assert_eq!(e.bpm, 127.0);
        assert_eq!(e.loop_beats, Some(8));
        assert!(e.confidence > 0.5, "{e:?}");
        assert!(!e.onsets.is_empty());
        assert_eq!(e.alternatives.len(), 2);
        assert!(e.alternatives.iter().all(|a| a.score < 1.0), "{e:?}");
    }

    #[test]
    fn silence_and_short_material_have_no_tempo() {
        assert!(estimate(&[vec![0.0; 96_000]], 48_000.0).is_none());
        assert!(estimate(&[vec![0.1; 1000]], 48_000.0).is_none());
    }
}
