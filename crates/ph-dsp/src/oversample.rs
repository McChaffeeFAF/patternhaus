//! Oversampling (2×/4×/8× cascades of polyphase halfband FIR stages) and a 4× true-peak
//! detector.
//!
//! Halfband FIRs are Kaiser-windowed sincs: every second tap is zero, so the polyphase
//! form costs `K` multiply-adds per output (symmetric folding). They are linear phase, so
//! an oversampled path can be latency-matched to a dry path exactly; the cascade pads its
//! delay to a whole number of base-rate samples ([`Oversampler::latency`]).

use crate::interp::{kaiser, sinc};

/// Halfband prototype taps `h[n]`, `n = -M..=M`, `M = 2K-1`.
fn halfband(k: usize, beta: f64) -> Vec<f64> {
    let m = (2 * k - 1) as i64;
    let mut h: Vec<f64> = (-m..=m)
        .map(|n| 0.5 * sinc(n as f64 / 2.0) * kaiser(n as f64 / (m + 1) as f64, beta))
        .collect();
    // exact zeros at even offsets, DC gain 1
    let odd_sum: f64 = (-m..=m)
        .filter(|n| n % 2 != 0)
        .map(|n| h[(n + m) as usize])
        .sum();
    for n in -m..=m {
        let v = &mut h[(n + m) as usize];
        if n == 0 {
            *v = 0.5;
        } else if n % 2 == 0 {
            *v = 0.0;
        } else {
            *v *= 0.5 / odd_sum;
        }
    }
    h
}

/// Polyphase coefficients `c_j = h[2j - M]` for `j = 0..K` (the other half is symmetric).
fn polyphase(k: usize, beta: f64) -> Vec<f32> {
    let h = halfband(k, beta);
    (0..k).map(|j| h[2 * j] as f32).collect()
}

/// Ring buffer that keeps its last `n` values contiguous (newest first) by writing twice.
#[derive(Clone, Debug)]
struct History {
    buf: Vec<f32>,
    n: usize,
    pos: usize,
}

impl History {
    fn new(n: usize) -> Self {
        Self {
            buf: vec![0.0; 2 * n],
            n,
            pos: 0,
        }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = if self.pos == 0 {
            self.n - 1
        } else {
            self.pos - 1
        };
        self.buf[self.pos] = x;
        self.buf[self.pos + self.n] = x;
    }

    /// `[x[m], x[m-1], …, x[m-n+1]]`
    #[inline]
    fn slice(&self) -> &[f32] {
        &self.buf[self.pos..self.pos + self.n]
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

/// Folded symmetric FIR over a history slice of length `2K` with half-coefficients `c`.
#[inline]
fn folded(c: &[f32], s: &[f32]) -> f32 {
    let k = c.len();
    let mut acc = 0.0f32;
    for j in 0..k {
        acc += c[j] * (s[j] + s[2 * k - 1 - j]);
    }
    acc
}

/// 2× interpolator: one input → two outputs.
#[derive(Clone, Debug)]
pub struct Upsampler2x {
    c: Vec<f32>,
    hist: History,
}

impl Upsampler2x {
    pub fn new(k: usize, beta: f64) -> Self {
        let c = polyphase(k, beta).iter().map(|v| 2.0 * v).collect();
        Self {
            c,
            hist: History::new(2 * k),
        }
    }

    /// Delay in output-rate samples.
    pub fn delay(&self) -> usize {
        2 * self.c.len() - 1
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> [f32; 2] {
        self.hist.push(x);
        let s = self.hist.slice();
        [folded(&self.c, s), s[self.c.len() - 1]]
    }

    pub fn reset(&mut self) {
        self.hist.clear();
    }
}

/// 2× decimator: two inputs → one output.
#[derive(Clone, Debug)]
pub struct Decimator2x {
    c: Vec<f32>,
    even: History,
    odd: History,
}

impl Decimator2x {
    pub fn new(k: usize, beta: f64) -> Self {
        Self {
            c: polyphase(k, beta),
            even: History::new(2 * k),
            odd: History::new(k + 1),
        }
    }

    /// Delay in input-rate samples.
    pub fn delay(&self) -> usize {
        2 * self.c.len() - 1
    }

    #[inline]
    pub fn process(&mut self, x0: f32, x1: f32) -> f32 {
        self.even.push(x0);
        self.odd.push(x1);
        let k = self.c.len();
        folded(&self.c, self.even.slice()) + 0.5 * self.odd.slice()[k]
    }

    pub fn reset(&mut self) {
        self.even.clear();
        self.odd.clear();
    }
}

/// Halfband sizes per cascade stage: the first stage (next to the base rate) needs a
/// narrow transition band (20 kHz pass, 28 kHz stop at 48 kHz base → ~100 dB stopband),
/// later stages only need to reject images far above the audio band.
const STAGE1_K: usize = 20;
const STAGE1_BETA: f64 = 10.0;
const STAGEN_K: usize = 6;
const STAGEN_BETA: f64 = 8.0;

/// Mono up/down sampler for factor 1, 2, 4 or 8.
#[derive(Clone, Debug)]
pub struct Oversampler {
    factor: usize,
    ups: Vec<Upsampler2x>,
    downs: Vec<Decimator2x>,
    pad: Vec<f32>,
    pad_pos: usize,
    latency: u32,
    a: Vec<f32>,
    b: Vec<f32>,
}

impl Oversampler {
    /// `factor` is rounded to 1, 2, 4 or 8. Buffers hold `max_frames` base-rate samples.
    pub fn new(factor: usize, max_frames: usize) -> Self {
        let factor: usize = match factor {
            0 | 1 => 1,
            2 | 3 => 2,
            4..=7 => 4,
            _ => 8,
        };
        let stages = factor.trailing_zeros() as usize;
        let mk = |s: usize| {
            if s == 0 {
                (STAGE1_K, STAGE1_BETA)
            } else {
                (STAGEN_K, STAGEN_BETA)
            }
        };
        let ups: Vec<_> = (0..stages)
            .map(|s| Upsampler2x::new(mk(s).0, mk(s).1))
            .collect();
        let downs: Vec<_> = (0..stages)
            .map(|s| Decimator2x::new(mk(s).0, mk(s).1))
            .collect();
        // delay of stage s (rate 2^(s+1)) counted at the top rate
        let total_hi: usize = (0..stages)
            .map(|s| (ups[s].delay() + downs[s].delay()) * (factor >> (s + 1)))
            .sum();
        let pad = (factor - total_hi % factor) % factor;
        Self {
            factor,
            ups,
            downs,
            pad: vec![0.0; pad],
            pad_pos: 0,
            latency: ((total_hi + pad) / factor) as u32,
            a: vec![0.0; max_frames * factor],
            b: vec![0.0; max_frames * factor],
        }
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    /// Round-trip (up + down) latency in base-rate samples. Always whole.
    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// Delay of [`Self::down`] alone, in base-rate samples (rounded down).
    pub fn decimation_latency(&self) -> u32 {
        let hi: usize = self
            .downs
            .iter()
            .enumerate()
            .map(|(s, d)| d.delay() * (self.factor >> (s + 1)))
            .sum();
        (hi / self.factor) as u32
    }

    pub fn reset(&mut self) {
        self.ups.iter_mut().for_each(|u| u.reset());
        self.downs.iter_mut().for_each(|d| d.reset());
        self.pad.fill(0.0);
        self.a.fill(0.0);
        self.b.fill(0.0);
    }

    /// The top-rate buffer for `frames` base-rate samples.
    pub fn hi_buffer(&mut self, frames: usize) -> &mut [f32] {
        &mut self.a[..frames * self.factor]
    }

    /// Upsample `input` into the top-rate buffer and return it.
    pub fn up(&mut self, input: &[f32]) -> &mut [f32] {
        let n = input.len();
        self.a[..n].copy_from_slice(input);
        let mut len = n;
        for s in 0..self.ups.len() {
            for i in 0..len {
                let [y0, y1] = self.ups[s].process(self.a[i]);
                self.b[2 * i] = y0;
                self.b[2 * i + 1] = y1;
            }
            std::mem::swap(&mut self.a, &mut self.b);
            len *= 2;
        }
        &mut self.a[..len]
    }

    /// Decimate the top-rate buffer (as left by [`Self::up`] / [`Self::hi_buffer`]) into
    /// `output` (`output.len()` base-rate samples).
    pub fn down(&mut self, output: &mut [f32]) {
        let n = output.len();
        let mut len = n * self.factor;
        if !self.pad.is_empty() {
            let pl = self.pad.len();
            for i in 0..len {
                std::mem::swap(&mut self.a[i], &mut self.pad[self.pad_pos]);
                self.pad_pos = (self.pad_pos + 1) % pl;
            }
        }
        for s in (0..self.downs.len()).rev() {
            len /= 2;
            for i in 0..len {
                self.b[i] = self.downs[s].process(self.a[2 * i], self.a[2 * i + 1]);
            }
            std::mem::swap(&mut self.a, &mut self.b);
        }
        output.copy_from_slice(&self.a[..n]);
    }
}

/// 4× oversampled true-peak detector: returns the largest absolute value among the input
/// sample and three interpolated points before it, delayed by [`TruePeak4x::DELAY`]
/// samples. Kaiser-windowed sinc, 24 taps per phase (twice the ITU-R BS.1770-4 example).
#[derive(Clone, Debug)]
pub struct TruePeak4x {
    coefs: [[f32; 24]; 4],
    hist: History,
}

impl Default for TruePeak4x {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak4x {
    /// Half-width of the interpolation kernel; also the detector's delay in samples.
    pub const DELAY: usize = 12;

    pub fn new() -> Self {
        let l = Self::DELAY as i64;
        let mut coefs = [[0.0f32; 24]; 4];
        for (p, row) in coefs.iter_mut().enumerate() {
            let frac = p as f64 / 4.0;
            // slice index i holds x[m - i]; offset j = (L - i) relative to the centre
            let mut sum = 0.0;
            let mut tmp = [0.0f64; 24];
            for i in 0..24 {
                let j = (l - i as i64) as f64;
                let t = j - frac;
                let w = sinc(t) * kaiser(t / (l as f64 + 1.0), 8.0);
                tmp[i] = w;
                sum += w;
            }
            for i in 0..24 {
                row[i] = (tmp[i] / sum) as f32;
            }
        }
        Self {
            coefs,
            hist: History::new(24),
        }
    }

    pub fn reset(&mut self) {
        self.hist.clear();
    }

    /// Push one sample; return the 4× peak around the sample `DELAY` samples ago.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.hist.push(x);
        let s = self.hist.slice();
        let mut m = 0.0f32;
        for row in &self.coefs {
            let mut acc = 0.0f32;
            for i in 0..24 {
                acc += row[i] * s[i];
            }
            m = m.max(acc.abs());
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::tone_amplitude;

    fn tone(f: f64, sr: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin() as f32)
            .collect()
    }

    /// Known-answer: round trip passes the audio band flat and has whole-sample latency.
    #[test]
    fn round_trip_is_transparent() {
        for factor in [2, 4, 8] {
            let sr = 48000.0;
            let n = 8192;
            let mut os = Oversampler::new(factor, n);
            for f in [100.0, 1000.0, 10000.0, 19000.0] {
                os.reset();
                let x = tone(f, sr, n);
                let mut y = vec![0.0; n];
                os.up(&x);
                os.down(&mut y);
                let lat = os.latency() as usize;
                // compare delayed output with input
                let mut err = 0.0f32;
                for i in 1000..n {
                    err = err.max((y[i] - x[i - lat]).abs());
                }
                assert!(err < 2e-3, "factor {factor} f {f} err {err} lat {lat}");
            }
        }
    }

    /// Known-answer: the first decimation stage rejects 28–48 kHz content by ≥ 90 dB
    /// (it would otherwise alias into 0–20 kHz).
    #[test]
    fn decimator_stopband() {
        let hi_sr = 96000.0;
        let n = 1 << 15;
        for f in [28500.0, 33000.0, 40000.0, 47000.0] {
            let mut d = Decimator2x::new(STAGE1_K, STAGE1_BETA);
            let x = tone(f, hi_sr, 2 * n);
            let y: Vec<f32> = (0..n).map(|i| d.process(x[2 * i], x[2 * i + 1])).collect();
            // content at f (24..48 kHz) folds to 48 kHz - f
            let a = tone_amplitude(&y[n / 2..], 48000.0, 48000.0 - f);
            let db = 20.0 * a.max(1e-12).log10();
            assert!(db < -90.0, "f {f}: {db:.1} dB");
        }
    }

    /// Known-answer: an intersample peak between two samples of a quarter-sample-rate tone
    /// phase-shifted by 45° is found (sample peak 0.707, true peak 1.0).
    #[test]
    fn true_peak_finds_intersample_peak() {
        let mut tp = TruePeak4x::new();
        let mut m = 0.0f32;
        let mut sample_peak = 0.0f32;
        for i in 0..1000 {
            let x =
                (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin() as f32;
            sample_peak = sample_peak.max(x.abs());
            let p = tp.process(x);
            if i > 100 {
                m = m.max(p);
            }
        }
        assert!((sample_peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!(m > 0.98 && m < 1.02, "true peak {m}");
    }
}
