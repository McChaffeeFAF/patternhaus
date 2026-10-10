//! Onset detection (spectral flux with an adaptive threshold) and `bar.beat.tick` stamps.

use crate::{Timing, mono_mix, round};
use ph_dsp::analysis::fft;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Onset {
    pub time_s: f64,
    /// `bar.beat.tick`, 1-based bar and beat, relative to the start of the analysed range.
    pub position: String,
    /// 0..1, relative to the strongest onset found.
    pub strength: f64,
}

const N: usize = 1024;
const HOP: usize = 256;

/// `first_bar` offsets the bar number so a range starting at bar 5 reports bar 5.
pub fn detect(
    channels: &[Vec<f32>],
    sample_rate: u32,
    timing: &Timing,
    first_bar: u32,
) -> Vec<Onset> {
    let x = mono_mix(channels);
    if x.len() < N * 2 {
        return vec![];
    }
    let w: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N as f64).cos())
        .collect();
    let max_bin = ((16000.0 * N as f64 / sample_rate as f64) as usize).min(N / 2);
    let frames = (x.len() - N) / HOP + 1;
    let mut flux = vec![0.0f64; frames];
    let mut prev = vec![0.0f64; max_bin + 1];
    for f in 0..frames {
        let mut re: Vec<f64> = (0..N).map(|i| x[f * HOP + i] as f64 * w[i]).collect();
        let mut im = vec![0.0; N];
        fft(&mut re, &mut im);
        let mut s = 0.0;
        for k in 1..=max_bin {
            let m = ((re[k] * re[k] + im[k] * im[k]).sqrt() * 8.0 / N as f64).ln_1p() * 10.0;
            if f > 0 {
                s += (m - prev[k]).max(0.0);
            }
            prev[k] = m;
        }
        flux[f] = s;
    }
    let gmax = flux.iter().copied().fold(0.0, f64::max);
    if gmax < 1e-6 {
        return vec![];
    }
    let min_gap = ((0.05 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let mut picks: Vec<usize> = Vec::new();
    for f in 1..frames.saturating_sub(1) {
        let lo = f.saturating_sub(10);
        let hi = (f + 11).min(frames);
        let mean = flux[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
        let thr = mean * 1.6 + 0.08 * gmax;
        let is_peak = flux[lo.max(f.saturating_sub(3))..(f + 4).min(frames)]
            .iter()
            .all(|&v| v <= flux[f]);
        if flux[f] > thr && is_peak {
            match picks.last() {
                Some(&p) if f - p < min_gap => {
                    if flux[f] > flux[p] {
                        *picks.last_mut().unwrap() = f;
                    }
                }
                _ => picks.push(f),
            }
        }
    }
    // Refine to the sample where the envelope rises fastest inside the frame.
    let env_win = 24usize;
    picks
        .into_iter()
        .map(|f| {
            let start = f * HOP;
            let end = (start + N).min(x.len());
            let mut best = (0.0f64, start);
            let mut i = start;
            while i + 2 * env_win < end {
                let a: f64 = x[i..i + env_win].iter().map(|v| v.abs() as f64).sum();
                let b: f64 = x[i + env_win..i + 2 * env_win]
                    .iter()
                    .map(|v| v.abs() as f64)
                    .sum();
                if b - a > best.0 {
                    best = (b - a, i + env_win);
                }
                i += 8;
            }
            let t = best.1 as f64 / sample_rate as f64;
            let ticks = timing.seconds_to_ticks(t);
            let (bar, beat, tick) = timing.bar_beat_tick(ticks);
            Onset {
                time_s: round(t, 4),
                position: format!("{}.{beat}.{tick}", bar - 1 + first_bar),
                strength: round(flux[f] / gmax, 2),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::TimeSignature;

    /// A click train at known times (decaying noise bursts) is found within 12 ms.
    #[test]
    fn finds_clicks_at_known_times() {
        let sr = 48_000u32;
        let mut x = vec![0.0f32; sr as usize * 4];
        let mut seed = 12345u32;
        let times = [0.25, 0.75, 1.0, 1.5, 2.25, 3.0];
        for &t in &times {
            let s = (t * sr as f64) as usize;
            for i in 0..4000 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
                x[s + i] += n * (-(i as f32) / 600.0).exp() * 0.8;
            }
        }
        let timing = Timing::new(120.0, TimeSignature::default());
        let found = detect(&[x], sr, &timing, 1);
        assert_eq!(found.len(), times.len(), "{found:?}");
        for (o, t) in found.iter().zip(times) {
            assert!((o.time_s - t).abs() < 0.012, "{o:?} vs {t}");
        }
        // 120 bpm: 0.75 s is beat 2 + half a beat... = bar 1, beat 2, tick 480
        assert_eq!(found[1].position, "1.2.480");
        assert_eq!(found[3].position, "1.4.0");
        assert_eq!(found[4].position, "2.1.480");
    }

    #[test]
    fn silence_has_no_onsets() {
        let timing = Timing::new(120.0, TimeSignature::default());
        assert!(detect(&[vec![0.0; 48_000]], 48_000, &timing, 1).is_empty());
    }
}
