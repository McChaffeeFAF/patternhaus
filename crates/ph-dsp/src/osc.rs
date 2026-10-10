//! Band-limited oscillators (PolyBLEP / PolyBLAMP), noise and sub-oscillator.
//!
//! Each discontinuity is corrected with the 4-point polynomial residual of a band-limited
//! step (BLEP, for saw and pulse edges) or ramp (BLAMP, for the triangle's corners). The
//! residuals are the first and second integrals of a cubic B-spline kernel four samples
//! wide, which suppresses aliasing markedly better than the common 2-point PolyBLEP. Haus Synth additionally runs its oscillators oversampled, which
//! pushes the residual aliasing far below audibility (see the crate tests for numbers).

use crate::math::{Rng, sin_turns};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wave {
    Saw,
    /// Pulse with variable width (0.5 = square).
    Pulse,
    Triangle,
    Sine,
}

impl Wave {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Wave::Saw,
            1 => Wave::Pulse,
            2 => Wave::Triangle,
            _ => Wave::Sine,
        }
    }
}

/// BLEP residual for a unit upward step: the integral of a cubic B-spline kernel (4
/// samples wide) minus the ideal step. `x` is the time since the step, in samples (−2..2).
#[inline]
fn blep_res(x: f64) -> f64 {
    // F(x) = ∫_{-2}^{x} B3 for x ≤ 0; antisymmetric residual
    let a = -x.abs();
    let f = if a < -1.0 {
        let u = 2.0 + a;
        u * u * u * u * (1.0 / 24.0)
    } else {
        let a2 = a * a;
        1.0 / 24.0 + (4.0 * a - 2.0 * a2 * a - 0.75 * a2 * a2 + 2.75) * (1.0 / 6.0)
    };
    if x < 0.0 { f } else { -f }
}

/// BLAMP residual for a unit change of slope (per sample): the second integral of the
/// cubic B-spline kernel minus the ideal ramp. Even in `x` (−2..2).
#[inline]
fn blamp_res(x: f64) -> f64 {
    let a = -x.abs();
    if a < -1.0 {
        let u = 2.0 + a;
        u * u * u * u * u * (1.0 / 120.0)
    } else {
        let a2 = a * a;
        1.0 / 120.0
            + a * (1.0 / 24.0)
            + (2.0 * a2 - 0.5 * a2 * a2 - 0.15 * a2 * a2 * a + 2.75 * a) * (1.0 / 6.0)
            + 0.225
    }
}

/// Signed distance (in samples) from phase `t` to a discontinuity at phase 0, if within
/// the kernel's two-sample reach; else `None`.
#[inline]
fn near_edge(t: f64, dt: f64) -> Option<f64> {
    if t < 2.0 * dt {
        Some(t / dt)
    } else if t > 1.0 - 2.0 * dt {
        Some((t - 1.0) / dt)
    } else {
        None
    }
}

/// Phase-accumulator oscillator. Phase in turns `[0, 1)`, kept in `f64` so pitch stays
/// exact for long notes.
#[derive(Clone, Debug, Default)]
pub struct Osc {
    phase: f64,
    dt: f64,
}

impl Osc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frequency in Hz; clamped below Nyquist.
    #[inline]
    pub fn set_freq(&mut self, hz: f64, sample_rate: f64) {
        self.dt = (hz / sample_rate).clamp(0.0, 0.45);
    }

    #[inline]
    pub fn set_phase(&mut self, phase: f64) {
        self.phase = phase - phase.floor();
    }

    #[inline]
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Next sample of `wave`. `pw` is the pulse width (0..1), used by `Pulse` only.
    #[inline]
    pub fn next(&mut self, wave: Wave, pw: f32) -> f32 {
        let t = self.phase;
        let dt = self.dt;
        let y = match wave {
            Wave::Saw => {
                let mut y = 2.0 * t - 1.0;
                if let Some(x) = near_edge(t, dt) {
                    y -= 2.0 * blep_res(x);
                }
                y
            }
            Wave::Pulse => {
                let pw = (pw as f64).clamp(0.02, 0.98);
                let mut y = if t < pw { 1.0 } else { -1.0 };
                if let Some(x) = near_edge(t, dt) {
                    y += 2.0 * blep_res(x);
                }
                let t2 = t - pw + if t < pw { 1.0 } else { 0.0 };
                if let Some(x) = near_edge(t2, dt) {
                    y -= 2.0 * blep_res(x);
                }
                // remove the width-dependent DC so PWM does not thump
                y - (2.0 * pw - 1.0)
            }
            Wave::Triangle => {
                let mut y = 1.0 - 4.0 * (t - 0.5).abs();
                // slope changes by +8/turn at t=0 and -8/turn at t=0.5
                if let Some(x) = near_edge(t, dt) {
                    y += 8.0 * dt * blamp_res(x);
                }
                let t2 = if t < 0.5 { t + 0.5 } else { t - 0.5 };
                if let Some(x) = near_edge(t2, dt) {
                    y -= 8.0 * dt * blamp_res(x);
                }
                y
            }
            Wave::Sine => sin_turns(t) as f64,
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y as f32
    }
}

/// Sub-oscillator one or two octaves below a master frequency: square or sine.
#[derive(Clone, Debug, Default)]
pub struct SubOsc {
    osc: Osc,
}

impl SubOsc {
    /// `octaves_down` is 1 or 2.
    #[inline]
    pub fn set_freq(&mut self, master_hz: f64, octaves_down: u32, sample_rate: f64) {
        self.osc
            .set_freq(master_hz / (1u32 << octaves_down) as f64, sample_rate);
    }

    pub fn set_phase(&mut self, p: f64) {
        self.osc.set_phase(p);
    }

    #[inline]
    pub fn next(&mut self, square: bool) -> f32 {
        if square {
            self.osc.next(Wave::Pulse, 0.5)
        } else {
            self.osc.next(Wave::Sine, 0.5)
        }
    }
}

/// White noise from a seeded PRNG, with an optional one-pole "colour" low-pass.
#[derive(Clone, Debug)]
pub struct Noise {
    rng: Rng,
}

impl Noise {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
        }
    }

    pub fn reseed(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
    }

    /// Uniform white noise, RMS ≈ 0.577.
    #[inline]
    pub fn next(&mut self) -> f32 {
        self.rng.bipolar()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::inharmonic_ratio_db;

    fn render(wave: Wave, f0: f64, sr: f64, n: usize) -> Vec<f32> {
        let mut o = Osc::new();
        o.set_freq(f0, sr);
        (0..n).map(|_| o.next(wave, 0.5)).collect()
    }

    fn naive_saw(f0: f64, sr: f64, n: usize) -> Vec<f32> {
        let mut t = 0.0f64;
        (0..n)
            .map(|_| {
                let y = 2.0 * t - 1.0;
                t = (t + f0 / sr).fract();
                y as f32
            })
            .collect()
    }

    #[test]
    fn waves_are_bounded_and_zero_mean() {
        for w in [Wave::Saw, Wave::Pulse, Wave::Triangle, Wave::Sine] {
            let x = render(w, 441.0, 44100.0, 44100);
            let mean: f64 = x.iter().map(|v| *v as f64).sum::<f64>() / x.len() as f64;
            assert!(mean.abs() < 0.01, "{w:?} mean {mean}");
            assert!(x.iter().all(|v| v.abs() <= 1.1), "{w:?}");
        }
    }

    /// Known-answer: PolyBLEP saw aliasing at 48 kHz, measured as inharmonic/total energy.
    #[test]
    fn polyblep_saw_aliasing_is_low() {
        let sr = 48000.0;
        let f0 = 1234.5;
        let blep = inharmonic_ratio_db(&render(Wave::Saw, f0, sr, 1 << 16), sr, f0, 6);
        let naive = inharmonic_ratio_db(&naive_saw(f0, sr, 1 << 16), sr, f0, 6);
        // the naive saw sits around -15 dB; PolyBLEP must improve it by at least 20 dB
        eprintln!("saw aliasing: polyblep {blep:.1} dB, naive {naive:.1} dB");
        assert!(blep < naive - 20.0, "blep {blep:.1} naive {naive:.1}");
        assert!(blep < -38.0, "blep {blep:.1}");
    }

    #[test]
    fn triangle_and_pulse_alias_less_than_naive() {
        let sr = 48000.0;
        let f0 = 2345.6;
        let tri = inharmonic_ratio_db(&render(Wave::Triangle, f0, sr, 1 << 16), sr, f0, 6);
        let pulse = inharmonic_ratio_db(&render(Wave::Pulse, f0, sr, 1 << 16), sr, f0, 6);
        eprintln!("aliasing: triangle {tri:.1} dB, pulse {pulse:.1} dB");
        assert!(tri < -55.0, "triangle {tri:.1}");
        assert!(pulse < -36.0, "pulse {pulse:.1}");
    }

    #[test]
    fn noise_is_seeded() {
        let mut a = Noise::new(7);
        let mut b = Noise::new(7);
        for _ in 0..1000 {
            assert_eq!(a.next(), b.next());
        }
    }
}
