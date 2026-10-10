//! Offline measurement helpers (FFT, windows, levels). These allocate and are **not**
//! real-time safe; they exist for tests, known-answer checks and future analyzers.

use std::f64::consts::PI;

/// In-place iterative radix-2 complex FFT. `re.len()` must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// 4-term Blackman-Harris window (−92 dB sidelobes).
pub fn blackman_harris(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / n as f64;
            0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
        })
        .collect()
}

/// Power spectrum (|X|², bins `0..=n/2`) of `signal` (length a power of two) after a
/// Blackman-Harris window.
pub fn power_spectrum(signal: &[f32]) -> Vec<f64> {
    let n = signal.len();
    let w = blackman_harris(n);
    let mut re: Vec<f64> = signal.iter().zip(&w).map(|(s, w)| *s as f64 * w).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    (0..=n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
}

/// Complex frequency response of an impulse response, sampled at FFT bins `0..=n/2`
/// (`n` = next power of two ≥ `ir.len()`, at least `min_n`). Returns magnitudes in dB.
pub fn magnitude_response_db(ir: &[f32], min_n: usize) -> Vec<f64> {
    let n = ir.len().max(min_n).next_power_of_two();
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for (r, x) in re.iter_mut().zip(ir) {
        *r = *x as f64;
    }
    fft(&mut re, &mut im);
    (0..=n / 2)
        .map(|k| 10.0 * (re[k] * re[k] + im[k] * im[k]).max(1e-300).log10())
        .collect()
}

/// Ratio (dB) of energy *not* within `guard` bins of a harmonic of `f0`, to the total.
/// Measures aliasing (and any other inharmonic content) of a periodic signal.
pub fn inharmonic_ratio_db(signal: &[f32], sample_rate: f64, f0: f64, guard: usize) -> f64 {
    inharmonic_ratio_db_below(signal, sample_rate, f0, guard, sample_rate / 2.0)
}

/// [`inharmonic_ratio_db`] counting only inharmonic energy below `max_hz` (e.g. 20 kHz,
/// the audible band; oversamplers deliberately let aliases land in 20–24 kHz).
pub fn inharmonic_ratio_db_below(
    signal: &[f32],
    sample_rate: f64,
    f0: f64,
    guard: usize,
    max_hz: f64,
) -> f64 {
    let p = power_spectrum(signal);
    let n = signal.len();
    let bin_hz = sample_rate / n as f64;
    let mut harmonic = vec![false; p.len()];
    let mut h = f0;
    while h < sample_rate / 2.0 {
        let b = (h / bin_hz).round() as isize;
        for k in (b - guard as isize)..=(b + guard as isize) {
            if k >= 0 && (k as usize) < p.len() {
                harmonic[k as usize] = true;
            }
        }
        h += f0;
    }
    let total: f64 = p[guard + 1..].iter().sum();
    let inh: f64 = p
        .iter()
        .enumerate()
        .skip(guard + 1) // ignore DC
        .filter(|(k, _)| !harmonic[*k] && (*k as f64) * bin_hz < max_hz)
        .map(|(_, v)| v)
        .sum();
    10.0 * (inh / total).max(1e-300).log10()
}

/// RMS level of a signal.
pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// Peak absolute sample value.
pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Amplitude of the `freq` component of `x` (single-bin DFT, rectangular window, use an
/// integer number of periods).
pub fn tone_amplitude(x: &[f32], sample_rate: f64, freq: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let ph = 2.0 * PI * freq * i as f64 / sample_rate;
        re += *v as f64 * ph.cos();
        im -= *v as f64 * ph.sin();
    }
    2.0 * (re * re + im * im).sqrt() / x.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_tone() {
        let n = 1024;
        let x: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * 64.0 * i as f64 / n as f64).sin() as f32)
            .collect();
        let p = power_spectrum(&x);
        let max = p
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(max, 64);
        assert!((tone_amplitude(&x, n as f64, 64.0) - 1.0).abs() < 1e-6);
    }
}
