//! Onset detection for audio material: an onset-strength envelope (log-magnitude spectral
//! flux) and transient picking with a sensitivity control. Offline, allocating, and
//! deterministic (no randomness, fixed frame grid), so the engine's Slice mode and the
//! slicer commands find the same slice points every time.
//!
//! It lives in `ph-dsp` because both the engine (Slice playback) and `ph-analysis` (tempo
//! estimation, `sample.detect_bpm`, `slicer.detect`) need it, and the engine may not depend
//! on `ph-analysis`.
//!
//! Method (public literature: Bello et al., "A Tutorial on Onset Detection in Music
//! Signals", IEEE TSAP 2005; Dixon, "Onset Detection Revisited", DAFx 2006):
//! 1. mono mix, Hann-windowed frames of [`FRAME`] samples every [`HOP`] samples;
//! 2. per bin `log(1 + k·|X|)`, positive differences to the previous frame summed over bins
//!    up to 16 kHz: the onset strength of the frame;
//! 3. peaks above an adaptive threshold (local mean × `a` + global max × `b`, both from the
//!    sensitivity), at least [`MIN_GAP_S`] apart;
//! 4. each peak refined to the sample where the short-time amplitude rises fastest.

use crate::analysis::fft;

/// Analysis frame, in samples.
pub const FRAME: usize = 1024;
/// Hop between frames, in samples.
pub const HOP: usize = 256;
/// Two onsets are at least this far apart, in seconds.
pub const MIN_GAP_S: f64 = 0.04;

/// The onset-strength envelope of a signal: one value per hop.
#[derive(Clone, Debug, PartialEq)]
pub struct Strength {
    pub sample_rate: f64,
    /// Value `k` describes the frame starting at sample `k × HOP` (centred at
    /// `k × HOP + FRAME / 2`).
    pub values: Vec<f32>,
}

impl Strength {
    /// Frames per second of the envelope.
    pub fn rate(&self) -> f64 {
        self.sample_rate / HOP as f64
    }

    /// Sample at the centre of frame `k`.
    pub fn centre(k: usize) -> usize {
        k * HOP + FRAME / 2
    }
}

/// Averages channels to mono.
pub fn mono(channels: &[Vec<f32>]) -> Vec<f32> {
    match channels {
        [] => Vec::new(),
        [one] => one.clone(),
        many => {
            let n = many.iter().map(|c| c.len()).max().unwrap_or(0);
            let k = 1.0 / many.len() as f32;
            (0..n)
                .map(|i| {
                    many.iter()
                        .map(|c| c.get(i).copied().unwrap_or(0.0))
                        .sum::<f32>()
                        * k
                })
                .collect()
        }
    }
}

/// The onset-strength envelope of a mono signal (bins up to 16 kHz). Frames run while they
/// start inside the signal; the tail is zero-padded.
///
/// Two bands are combined, each scaled to its own maximum: the full band (snares, hats,
/// anything bright) and the band below [`LOW_BAND_HZ`] (kicks and bass, whose few bins
/// would otherwise be outweighed by broadband noise). The low band counts fully only when it
/// carries real energy (its peak at least [`LOW_BAND_FLOOR`] of the full band's).
pub fn strength(x: &[f32], sample_rate: f64) -> Strength {
    let mut full = flux(x, sample_rate, 16_000.0, TRANSIENT_GAMMA);
    let low = flux(x, sample_rate, LOW_BAND_HZ, TRANSIENT_GAMMA);
    let gf = full.values.iter().copied().fold(0.0f32, f32::max);
    let gl = low.values.iter().copied().fold(0.0f32, f32::max);
    if gf <= 0.0 {
        return full;
    }
    let w_low = if gl > 0.0 {
        (gl / (LOW_BAND_FLOOR * gf)).min(1.0) / gl
    } else {
        0.0
    };
    for (v, l) in full.values.iter_mut().zip(&low.values) {
        *v = *v / gf + l * w_low;
    }
    full
}

/// Upper edge of the low band of [`strength`].
pub const LOW_BAND_HZ: f64 = 200.0;
/// The low band's peak, relative to the full band's, from which it counts fully.
pub const LOW_BAND_FLOOR: f32 = 0.05;

/// [`strength`] over the bins below `max_hz` only (e.g. kick and snare bodies, without
/// hi-hats, for tempo estimation).
pub fn strength_below(x: &[f32], sample_rate: f64, max_hz: f64) -> Strength {
    flux(x, sample_rate, max_hz, 1.0)
}

/// Log compression `ln(1 + γ·|X|)` for transient picking: strong enough that a kick (a few
/// low bins) counts as much as a snare (noise across all bins).
pub const TRANSIENT_GAMMA: f64 = 100.0;

/// Spectral flux of `x` over bins below `max_hz`, magnitudes compressed by `ln(1 + γ|X|)`.
pub fn flux(x: &[f32], sample_rate: f64, max_hz: f64, gamma: f64) -> Strength {
    let sr = sample_rate.max(1.0);
    let frames = if x.is_empty() {
        0
    } else {
        x.len().div_ceil(HOP)
    };
    let w: Vec<f64> = (0..FRAME)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FRAME as f64).cos())
        .collect();
    let max_bin = ((max_hz * FRAME as f64 / sr) as usize).clamp(2, FRAME / 2);
    // log magnitudes of the last two frames (SuperFlux-style reference: the frame two hops
    // back, max-filtered over neighbouring bins, so noise and vibrato add little flux)
    let mut hist = [vec![0.0f64; max_bin + 2], vec![0.0f64; max_bin + 2]];
    let mut cur = vec![0.0f64; max_bin + 2];
    let mut values = Vec::with_capacity(frames);
    let (mut re, mut im) = (vec![0.0f64; FRAME], vec![0.0f64; FRAME]);
    for f in 0..frames {
        for i in 0..FRAME {
            re[i] = x.get(f * HOP + i).copied().unwrap_or(0.0) as f64 * w[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for k in 1..=max_bin {
            cur[k] = (1.0 + gamma * (re[k] * re[k] + im[k] * im[k]).sqrt()).ln();
        }
        let mut s = 0.0;
        if f > 0 {
            let back = &hist[if f >= 2 { 0 } else { 1 }];
            for k in 1..=max_bin {
                let r = back[k - 1].max(back[k]).max(back[k + 1]);
                s += (cur[k] - r).max(0.0);
            }
        }
        // hist[0] = frame f-1 becomes f-2, hist[1] = f becomes f-1
        hist.swap(0, 1);
        std::mem::swap(&mut hist[1], &mut cur);
        values.push(s as f32);
    }
    Strength {
        sample_rate: sr,
        values,
    }
}

/// Peak frames of an onset-strength envelope. `sensitivity` 0..=1: higher finds more
/// (softer) onsets; 0.5 is the default.
pub fn pick(s: &Strength, sensitivity: f64) -> Vec<usize> {
    let v = &s.values;
    let n = v.len();
    let gmax = v.iter().copied().fold(0.0f32, f32::max) as f64;
    if n < 3 || gmax <= 1e-6 {
        return Vec::new();
    }
    let sens = if sensitivity.is_finite() {
        sensitivity.clamp(0.0, 1.0)
    } else {
        0.5
    };
    // sensitivity 0 → mean × 2.0 + 50% of max; 1 → mean × 1.1 + 3% of max
    let a = 2.0 - 0.9 * sens;
    let b = 0.50 - 0.47 * sens;
    let half = ((0.1 * s.rate()).round() as usize).max(2);
    let min_gap = ((MIN_GAP_S * s.rate()).ceil() as usize).max(1);
    let mut picks: Vec<usize> = Vec::new();
    for f in 1..n - 1 {
        let lo = f.saturating_sub(half);
        let hi = (f + half + 1).min(n);
        let mean = v[lo..hi].iter().map(|x| *x as f64).sum::<f64>() / (hi - lo) as f64;
        let x = v[f] as f64;
        let local_max = v[f.saturating_sub(2)..(f + 3).min(n)]
            .iter()
            .all(|&y| y as f64 <= x);
        if x > mean * a + gmax * b && local_max {
            match picks.last() {
                Some(&p) if f - p < min_gap => {
                    if x > v[p] as f64 {
                        *picks.last_mut().expect("non-empty") = f;
                    }
                }
                _ => picks.push(f),
            }
        }
    }
    picks
}

/// Onset positions in samples of a mono signal: picked frames refined to the sample where
/// the amplitude envelope rises fastest (the start of the attack), sorted and distinct.
pub fn detect(x: &[f32], sample_rate: f64, sensitivity: f64) -> Vec<u64> {
    let s = strength(x, sample_rate);
    let mut out: Vec<u64> = pick(&s, sensitivity)
        .into_iter()
        .map(|f| refine(x, f) as u64)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The attack inside frame `f`: where the energy of the next `W` samples exceeds the
/// previous `W` by the most, searched over the frame and the hop before it.
fn refine(x: &[f32], f: usize) -> usize {
    const W: usize = 32;
    let start = (f * HOP).saturating_sub(HOP);
    let end = (f * HOP + FRAME).min(x.len());
    let energy = |a: usize, b: usize| -> f64 { x[a..b].iter().map(|v| (*v as f64).powi(2)).sum() };
    let mut best = (f64::MIN, Strength::centre(f).min(x.len().saturating_sub(1)));
    let mut i = start + W;
    while i + W <= end {
        let rise = energy(i, i + W) - energy(i - W, i);
        if rise > best.0 {
            best = (rise, i);
        }
        i += 4;
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decaying noise bursts at known samples.
    fn clicks(sr: f64, at: &[f64], len_s: f64) -> Vec<f32> {
        let mut x = vec![0.0f32; (len_s * sr) as usize];
        let mut seed = 1u32;
        for &t in at {
            let s = (t * sr) as usize;
            for i in 0..3000.min(x.len() - s) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let n = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
                x[s + i] += n * (-(i as f32) / 500.0).exp() * 0.8;
            }
        }
        x
    }

    #[test]
    fn finds_bursts_to_within_two_milliseconds() {
        let sr = 48_000.0;
        let times = [0.1, 0.5, 0.75, 1.2, 1.25, 2.0];
        let x = clicks(sr, &times, 2.5);
        let found = detect(&x, sr, 0.5);
        assert_eq!(found.len(), times.len(), "{found:?}");
        for (f, t) in found.iter().zip(times) {
            let err_ms = (*f as f64 / sr - t).abs() * 1000.0;
            assert!(err_ms < 2.0, "{f} vs {t}: {err_ms} ms");
        }
    }

    #[test]
    fn sensitivity_controls_how_many_soft_onsets_count() {
        let sr = 44_100.0;
        let mut x = clicks(sr, &[0.2, 1.0], 2.0);
        // a soft burst at 0.6 s
        let soft = clicks(sr, &[0.6], 2.0);
        for (a, b) in x.iter_mut().zip(soft) {
            *a += b * 0.08;
        }
        let low = detect(&x, sr, 0.0);
        let high = detect(&x, sr, 1.0);
        assert_eq!(low.len(), 2, "{low:?}");
        assert_eq!(high.len(), 3, "{high:?}");
        assert_eq!(detect(&x, sr, 0.5), detect(&x, sr, 0.5), "deterministic");
    }

    #[test]
    fn silence_and_dc_have_no_onsets() {
        assert!(detect(&vec![0.0; 48_000], 48_000.0, 1.0).is_empty());
        assert!(detect(&vec![0.3; 48_000], 48_000.0, 1.0).len() <= 1);
        assert!(detect(&[], 48_000.0, 0.5).is_empty());
        assert_eq!(mono(&[vec![1.0, 0.0], vec![0.0]]), vec![0.5, 0.0]);
    }
}
