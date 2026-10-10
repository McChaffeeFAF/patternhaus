//! Band energies and spectral centroid from a Welch-averaged FFT.

use crate::{mono_mix, pow_db_opt, round};
use ph_dsp::analysis::fft;
use serde::Serialize;
use std::f64::consts::PI;

/// The seven reporting bands: name, low edge, high edge (Hz).
pub const BANDS: [(&str, f64, f64); 7] = [
    ("sub", 20.0, 60.0),
    ("bass", 60.0, 250.0),
    ("low_mid", 250.0, 500.0),
    ("mid", 500.0, 2000.0),
    ("high_mid", 2000.0, 4000.0),
    ("presence", 4000.0, 6000.0),
    ("air", 6000.0, 20000.0),
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BandLevel {
    pub band: &'static str,
    pub low_hz: f64,
    pub high_hz: f64,
    /// RMS level of the signal inside the band, dBFS.
    pub level_db: Option<f64>,
    /// Share of the total (20 Hz to 20 kHz) energy, percent.
    pub share_pct: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spectrum {
    pub bands: Vec<BandLevel>,
    /// Energy-weighted mean frequency (magnitude-weighted), Hz.
    pub centroid_hz: Option<f64>,
}

pub const FFT_SIZE: usize = 4096;

pub(crate) fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos())
        .collect()
}

/// Welch-averaged one-sided power per bin, scaled so the sum over a band is the band's
/// mean-square (a sine of amplitude A contributes A^2/2).
pub fn mean_power_bins(mono: &[f32]) -> Vec<f64> {
    let n = FFT_SIZE;
    let w = hann(n);
    let wsum2: f64 = w.iter().map(|x| x * x).sum();
    let mut acc = vec![0.0; n / 2 + 1];
    let hop = n / 2;
    let mut frames = 0usize;
    let mut start = 0usize;
    // At least one (zero-padded) frame, even for short signals.
    while start == 0 || start + n <= mono.len() {
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        for i in 0..n {
            re[i] = mono.get(start + i).copied().unwrap_or(0.0) as f64 * w[i];
        }
        fft(&mut re, &mut im);
        for k in 0..=n / 2 {
            let p = re[k] * re[k] + im[k] * im[k];
            let scale = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            acc[k] += scale * p / (n as f64 * wsum2);
        }
        frames += 1;
        start += hop;
        if start >= mono.len() {
            break;
        }
    }
    for a in &mut acc {
        *a /= frames as f64;
    }
    acc
}

/// Per-band mean-square from [`mean_power_bins`].
pub fn band_powers(bins: &[f64], sample_rate: u32) -> [f64; 7] {
    let hz = |k: usize| k as f64 * sample_rate as f64 / FFT_SIZE as f64;
    let mut out = [0.0; 7];
    for (k, p) in bins.iter().enumerate() {
        let f = hz(k);
        for (i, (_, lo, hi)) in BANDS.iter().enumerate() {
            if f >= *lo && f < *hi {
                out[i] += p;
            }
        }
    }
    out
}

pub fn analyze(channels: &[Vec<f32>], sample_rate: u32) -> Spectrum {
    let mono = mono_mix(channels);
    let bins = mean_power_bins(&mono);
    let powers = band_powers(&bins, sample_rate);
    let total: f64 = powers.iter().sum();
    let bands = BANDS
        .iter()
        .zip(powers)
        .map(|((name, lo, hi), p)| BandLevel {
            band: name,
            low_hz: *lo,
            high_hz: *hi,
            level_db: pow_db_opt(p).map(|d| round(d, 1)),
            share_pct: if total > 0.0 {
                round(100.0 * p / total, 1)
            } else {
                0.0
            },
        })
        .collect();
    // Centroid over 20 Hz..20 kHz using magnitudes.
    let (mut num, mut den) = (0.0, 0.0);
    for (k, p) in bins.iter().enumerate() {
        let f = k as f64 * sample_rate as f64 / FFT_SIZE as f64;
        if (20.0..20000.0).contains(&f) {
            let m = p.sqrt();
            num += f * m;
            den += m;
        }
    }
    Spectrum {
        bands,
        centroid_hz: (den > 1e-9).then(|| round(num / den, 0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::*;

    #[test]
    fn single_tones_land_in_their_bands_at_the_right_level() {
        for (f, band) in [
            (40.0, "sub"),
            (100.0, "bass"),
            (400.0, "low_mid"),
            (1000.0, "mid"),
            (3000.0, "high_mid"),
            (5000.0, "presence"),
            (10000.0, "air"),
        ] {
            let x = vec![sine(f, dbfs(-12.0), 48_000, 2.0)];
            let s = analyze(&x, 48_000);
            let b = s
                .bands
                .iter()
                .max_by_key(|b| (b.share_pct * 10.0) as i64)
                .unwrap();
            assert_eq!(b.band, band, "{f} Hz");
            assert!(b.share_pct > 99.0);
            // A -12 dBFS sine has -15.0 dBFS RMS.
            assert!(
                (b.level_db.unwrap() - -15.0).abs() < 0.3,
                "{f}: {:?}",
                b.level_db
            );
            assert!((s.centroid_hz.unwrap() - f).abs() < f * 0.03 + 5.0, "{f}");
        }
    }

    #[test]
    fn silence_has_no_levels() {
        let s = analyze(&[vec![0.0; 10_000]], 48_000);
        assert!(s.centroid_hz.is_none());
        assert!(s.bands.iter().all(|b| b.level_db.is_none()));
    }
}
