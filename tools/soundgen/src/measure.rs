//! Measurements used by the quality tests and the `report` command. These are analysis
//! only (they never feed the generator), so they may use the platform's math.

use crate::dsp::SR;
use std::f64::consts::PI;

pub fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

pub fn db(lin: f64) -> f64 {
    20.0 * lin.max(1e-12).log10()
}

/// Mean level in dBFS (the DC offset).
pub fn dc_db(x: &[f64]) -> f64 {
    db((x.iter().sum::<f64>() / x.len() as f64).abs())
}

/// Seconds until the last sample louder than `threshold_db` below the peak.
pub fn active_len(x: &[f64], threshold_db: f64) -> f64 {
    let t = peak(x) * 10f64.powf(threshold_db / 20.0);
    let last = x.iter().rposition(|s| s.abs() > t).unwrap_or(0);
    (last + 1) as f64 / SR
}

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let tw: Vec<(f64, f64)> = (0..len / 2)
            .map(|k| ((ang * k as f64).cos(), (ang * k as f64).sin()))
            .collect();
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = tw[k];
                let (a, b) = (s + k, s + k + len / 2);
                let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        len <<= 1;
    }
}

const FFT_N: usize = 1 << 18;

/// Power spectrum of `x[a..b]` (seconds) after a Hann window, zero-padded to 2^18 points.
fn spectrum(x: &[f64], a: f64, b: f64) -> Vec<f64> {
    let i0 = (a * SR) as usize;
    let i1 = ((b * SR) as usize).min(x.len());
    let seg = &x[i0..i1];
    let mut re = vec![0.0; FFT_N];
    let mut im = vec![0.0; FFT_N];
    for (i, s) in seg.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / seg.len() as f64).cos();
        re[i] = s * w;
    }
    fft(&mut re, &mut im);
    (0..=FFT_N / 2)
        .map(|k| re[k] * re[k] + im[k] * im[k])
        .collect()
}

/// Strongest spectral peak in `x[a..b]` (seconds), parabolically interpolated.
pub fn dominant_hz(x: &[f64], a: f64, b: f64) -> f64 {
    let p = spectrum(x, a, b);
    let k = (2..p.len() - 1)
        .max_by(|&i, &j| p[i].total_cmp(&p[j]))
        .unwrap();
    let (l, c, r) = (p[k - 1].ln(), p[k].ln(), p[k + 1].ln());
    let d = 0.5 * (l - r) / (l - 2.0 * c + r);
    (k as f64 + d) * SR / FFT_N as f64
}

/// Spectral centroid of the whole sound in Hz.
pub fn centroid_hz(x: &[f64]) -> f64 {
    let p = spectrum(x, 0.0, x.len() as f64 / SR);
    let (mut num, mut den) = (0.0, 0.0);
    for (k, v) in p.iter().enumerate() {
        num += k as f64 * SR / FFT_N as f64 * v;
        den += v;
    }
    num / den
}

/// Frequency from interpolated rising zero crossings in `x[a..b]` (seconds).
pub fn zero_cross_hz(x: &[f64], a: f64, b: f64) -> f64 {
    let (i0, i1) = ((a * SR) as usize, ((b * SR) as usize).min(x.len() - 1));
    let mut crossings = Vec::new();
    for i in i0..i1 {
        if x[i] < 0.0 && x[i + 1] >= 0.0 {
            crossings.push(i as f64 + (-x[i]) / (x[i + 1] - x[i]));
        }
    }
    assert!(crossings.len() > 3, "too few zero crossings");
    (crossings.len() - 1) as f64 * SR / (crossings[crossings.len() - 1] - crossings[0])
}

/// Signed distance in cents from `hz` to the nearest C.
pub fn cents_from_c(hz: f64) -> f64 {
    let semis_from_c0 = 12.0 * (hz / 16.351_597_831_287_414).log2();
    (semis_from_c0 - (semis_from_c0 / 12.0).round() * 12.0) * 100.0
}
