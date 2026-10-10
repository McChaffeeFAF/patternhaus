//! Small deterministic DSP toolkit for the generator. Everything is f64 and built on
//! [`crate::dmath`], so output does not depend on the platform's libm.

use crate::dmath::{cos, exp, sin, tanh};
use std::f64::consts::PI;

pub const SR: f64 = 48_000.0;
pub const TAU: f64 = 2.0 * PI;

/// Frames in `seconds`.
pub fn frames(seconds: f64) -> usize {
    (seconds * SR).round() as usize
}

/// SplitMix64: a tiny, fully specified generator (fixed seeds only, never the clock).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [-1, 1).
    pub fn bipolar(&mut self) -> f64 {
        self.uniform() * 2.0 - 1.0
    }
}

#[derive(Clone, Copy)]
pub struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn from(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b: [b0 / a0, b1 / a0, b2 / a0],
            a: [a1 / a0, a2 / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    // Coefficients from R. Bristow-Johnson, "Cookbook formulae for audio EQ biquad filters".
    pub fn lowpass(f: f64, q: f64) -> Self {
        let w = TAU * f / SR;
        let (c, al) = (cos(w), sin(w) / (2.0 * q));
        Self::from(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + al,
            -2.0 * c,
            1.0 - al,
        )
    }

    pub fn highpass(f: f64, q: f64) -> Self {
        let w = TAU * f / SR;
        let (c, al) = (cos(w), sin(w) / (2.0 * q));
        Self::from(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + al,
            -2.0 * c,
            1.0 - al,
        )
    }

    /// Band-pass with 0 dB peak gain.
    pub fn bandpass(f: f64, q: f64) -> Self {
        let w = TAU * f / SR;
        let (c, al) = (cos(w), sin(w) / (2.0 * q));
        Self::from(al, 0.0, -al, 1.0 + al, -2.0 * c, 1.0 - al)
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x[1] = self.x[0];
        self.x[0] = x;
        self.y[1] = self.y[0];
        self.y[0] = y;
        y
    }
}

/// A band-limited (PolyBLEP) square-wave oscillator.
pub struct Square {
    phase: f64,
}

impl Square {
    pub fn new(phase: f64) -> Self {
        Self { phase }
    }

    pub fn next(&mut self, hz: f64) -> f64 {
        let dt = hz / SR;
        let p = self.phase;
        let mut v = if p < 0.5 { 1.0 } else { -1.0 };
        v += blep(p, dt);
        v -= blep(if p + 0.5 >= 1.0 { p - 0.5 } else { p + 0.5 }, dt);
        self.phase += dt;
        self.phase -= self.phase.floor();
        v
    }
}

fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// `exp(-t / tau)` for sample `i`.
pub fn decay(i: usize, tau: f64) -> f64 {
    exp(-(i as f64 / SR) / tau)
}

pub fn soft_clip(x: f64, drive: f64) -> f64 {
    if drive <= 0.0 { x } else { tanh(drive * x) }
}

/// Shape a raw render into a finished one-shot: a 0.5 ms fade-in and a raised-cosine
/// fade-out of `release` seconds that ends on exact zero, DC removed so the *faded* signal
/// has zero mean, then the peak normalised to -1 dBFS.
pub fn finish(x: Vec<f64>, release: f64) -> Vec<f64> {
    let n = x.len();
    let mut w = vec![1.0f64; n];
    let fin = frames(0.0005).max(2);
    for (i, g) in w.iter_mut().enumerate().take(fin) {
        let s = sin(PI / 2.0 * i as f64 / fin as f64);
        *g = s * s;
    }
    let fout = frames(release).clamp(2, n);
    for k in 0..fout {
        // The last sample of the file gets gain 0.
        let c = cos(PI / 2.0 * (k + 1) as f64 / fout as f64);
        w[n - fout + k] *= c * c;
    }
    // mean((x - mu) * w) = 0  =>  mu = sum(x w) / sum(w)
    let xw: f64 = x.iter().zip(&w).map(|(a, b)| a * b).sum();
    let mu = xw / w.iter().sum::<f64>();
    let mut y: Vec<f64> = x.iter().zip(&w).map(|(a, b)| (a - mu) * b).collect();
    let peak = y.iter().fold(0.0f64, |m, s| m.max(s.abs()));
    assert!(peak > 0.0, "silent sound");
    // -1 dBFS = 10^(-1/20)
    let g = exp(-crate::dmath::ln(10.0) / 20.0) / peak;
    for s in &mut y {
        *s *= g;
    }
    y
}

/// Quantise to 24-bit signed integers (round half away from zero).
pub fn to_i24(x: &[f64]) -> Vec<i32> {
    x.iter()
        .map(|s| {
            let v = s * 8_388_607.0;
            (if v >= 0.0 {
                (v + 0.5).floor()
            } else {
                (v - 0.5).ceil()
            }) as i32
        })
        .collect()
}
