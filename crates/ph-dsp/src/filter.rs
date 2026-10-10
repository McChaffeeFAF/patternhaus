//! Filters: TPT state-variable, TPT 4-pole ladder, RBJ biquads, one-poles, DC blocker.
//!
//! The SVF and ladder are zero-delay-feedback (topology-preserving transform) structures
//! after Zavalishin: trapezoidal integrators with the instantaneous feedback solved
//! analytically. They keep the analog prototype's response at the (pre-warped) cutoff,
//! stay stable at any cutoff below Nyquist, and tolerate per-sample modulation.

use crate::math::{fast_tanh, flush_denormal, flush_denormal64};
use std::f64::consts::PI;

/// Filter response shape shared by the SVF and the ladder.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FilterMode {
    LowPass,
    HighPass,
    BandPass,
    Notch,
}

impl FilterMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => FilterMode::LowPass,
            1 => FilterMode::HighPass,
            2 => FilterMode::BandPass,
            _ => FilterMode::Notch,
        }
    }
}

/// Pre-warped integrator gain `g = tan(π·fc/fs)`, cutoff clamped to 10 Hz..0.49·fs.
#[inline]
pub fn tpt_g(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(10.0, 0.49 * sample_rate);
    ((PI as f32) * fc / sample_rate).tan()
}

/// TPT state-variable filter (2-pole, 12 dB/oct), outputs LP/HP/BP/notch.
#[derive(Clone, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Svf {
    pub fn new() -> Self {
        let mut s = Self::default();
        s.set_coefs(0.1, std::f32::consts::SQRT_2);
        s
    }

    /// `g` from [`tpt_g`]; damping `k = 1/Q` (2 = no resonance, → 0 self-oscillates).
    #[inline]
    pub fn set_coefs(&mut self, g: f32, k: f32) {
        self.g = g;
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Cutoff in Hz and resonance 0..1 (mapped to Q 0.5..~50).
    #[inline]
    pub fn set(&mut self, cutoff_hz: f32, resonance: f32, sample_rate: f32) {
        self.set_coefs(tpt_g(cutoff_hz, sample_rate), res_to_k(resonance));
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// One sample; returns `(lp, bp, hp)`. `bp` is the raw band output (peak gain = Q).
    #[inline]
    pub fn tick(&mut self, v0: f32) -> (f32, f32, f32) {
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = flush_denormal(2.0 * v1 - self.ic1);
        self.ic2 = flush_denormal(2.0 * v2 - self.ic2);
        (v2, v1, v0 - self.k * v1 - v2)
    }

    /// One sample in `mode`. Band-pass is normalised to unity gain at the centre.
    #[inline]
    pub fn process(&mut self, x: f32, mode: FilterMode) -> f32 {
        let (lp, bp, hp) = self.tick(x);
        match mode {
            FilterMode::LowPass => lp,
            FilterMode::HighPass => hp,
            FilterMode::BandPass => self.k * bp,
            FilterMode::Notch => x - self.k * bp,
        }
    }
}

/// Resonance 0..1 to SVF damping `k = 1/Q`: 0 → Q 0.5, 0.29 → Butterworth, 1 → Q≈50.
#[inline]
pub fn res_to_k(resonance: f32) -> f32 {
    let r = resonance.clamp(0.0, 1.0);
    // exponential in Q: Q = 0.5 * 100^r
    2.0 * (-r * 4.605_170_2).exp()
}

/// TPT 4-pole transistor-ladder low-pass with drive, after Zavalishin ch. 5.
///
/// The global feedback is solved with the linear ZDF equation, then the cascade input is
/// passed through a tanh saturator (drive). Mode outputs mix the stage taps (LP4, HP4,
/// BP4, notch), the multimode trick used by classic "Xpander"-style ladders.
#[derive(Clone, Debug, Default)]
pub struct Ladder {
    s: [f32; 4],
    g: f32,
    big_g: f32,
    k: f32,
    drive: f32,
    inv_drive: f32,
}

impl Ladder {
    pub fn new() -> Self {
        let mut l = Self::default();
        l.set_coefs(0.1, 0.0);
        l.set_drive(1.0);
        l
    }

    /// `g` from [`tpt_g`]; feedback `k` in 0..4 (4 = self-oscillation).
    #[inline]
    pub fn set_coefs(&mut self, g: f32, k: f32) {
        self.g = g;
        self.big_g = g / (1.0 + g);
        self.k = k.clamp(0.0, 4.2);
    }

    /// Cutoff in Hz, resonance 0..1 (1 = self-oscillation onset).
    #[inline]
    pub fn set(&mut self, cutoff_hz: f32, resonance: f32, sample_rate: f32) {
        self.set_coefs(
            tpt_g(cutoff_hz, sample_rate),
            4.0 * resonance.clamp(0.0, 1.05),
        );
    }

    /// Input gain into the saturator (linear, ≥ 1). The output is scaled back by `1/drive`
    /// so low-level signals keep unity gain.
    #[inline]
    pub fn set_drive(&mut self, drive: f32) {
        self.drive = drive.max(0.01);
        self.inv_drive = 1.0 / self.drive;
    }

    pub fn reset(&mut self) {
        self.s = [0.0; 4];
    }

    /// One sample in `mode`.
    #[inline]
    pub fn process(&mut self, x: f32, mode: FilterMode) -> f32 {
        let gg = self.big_g;
        let inv1g = 1.0 / (1.0 + self.g);
        let s = self.s;
        // linear estimate of y4: y4 = G^4 u + Σ G^(3-i) S_i
        let sigma = (gg * gg * gg * s[0] + gg * gg * s[1] + gg * s[2] + s[3]) * inv1g;
        let g4 = gg * gg * gg * gg;
        // partial passband compensation for the resonance's DC loss
        let xin = x * self.drive * (1.0 + 0.5 * self.k);
        let y4_est = (g4 * xin + sigma) / (1.0 + self.k * g4);
        let u = fast_tanh(xin - self.k * y4_est);
        let mut inp = u;
        let mut y = [0.0f32; 4];
        for i in 0..4 {
            let v = (inp - self.s[i]) * gg;
            let lp = v + self.s[i];
            self.s[i] = flush_denormal(lp + v);
            y[i] = lp;
            inp = lp;
        }
        let out = match mode {
            FilterMode::LowPass => y[3],
            FilterMode::HighPass => u - 4.0 * y[0] + 6.0 * y[1] - 4.0 * y[2] + y[3],
            FilterMode::BandPass => 4.0 * (y[1] - 2.0 * y[2] + y[3]),
            FilterMode::Notch => u - 2.0 * y[0] + 2.0 * y[1],
        };
        out * self.inv_drive
    }
}

/// RBJ cookbook filter shapes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BiquadShape {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peak { gain_db: f64 },
    LowShelf { gain_db: f64 },
    HighShelf { gain_db: f64 },
}

/// Normalised biquad coefficients (`a0 = 1`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BiquadCoefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoefs {
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// RBJ Audio EQ Cookbook design. `q` is the filter Q (shelves use it as the "S"-free
    /// Q form of the cookbook: `alpha = sin(w0)/(2Q)`).
    pub fn design(shape: BiquadShape, freq: f64, q: f64, sample_rate: f64) -> Self {
        let f = freq.clamp(1.0, 0.4999 * sample_rate);
        let w0 = 2.0 * PI * f / sample_rate;
        let (sn, cs) = w0.sin_cos();
        let q = q.max(0.01);
        let alpha = sn / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match shape {
            BiquadShape::LowPass => (
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            BiquadShape::HighPass => (
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            BiquadShape::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BiquadShape::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BiquadShape::Peak { gain_db } => {
                let a = 10f64.powf(gain_db / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cs,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cs,
                    1.0 - alpha / a,
                )
            }
            BiquadShape::LowShelf { gain_db } => {
                let a = 10f64.powf(gain_db / 40.0);
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + sa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - sa),
                    (a + 1.0) + (a - 1.0) * cs + sa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - sa,
                )
            }
            BiquadShape::HighShelf { gain_db } => {
                let a = 10f64.powf(gain_db / 40.0);
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + sa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - sa),
                    (a + 1.0) - (a - 1.0) * cs + sa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - sa,
                )
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Analytic magnitude response in dB at `freq`.
    pub fn magnitude_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
    }
}

/// Transposed direct form II biquad with `f64` state (clean at low frequencies).
#[derive(Clone, Debug)]
pub struct Biquad {
    pub c: BiquadCoefs,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            c: BiquadCoefs::IDENTITY,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    pub fn new(c: BiquadCoefs) -> Self {
        Self {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = flush_denormal64(c.b1 * x - c.a1 * y + self.z2);
        self.z2 = flush_denormal64(c.b2 * x - c.a2 * y);
        y
    }
}

/// TPT one-pole (6 dB/oct) with low- and high-pass outputs.
#[derive(Clone, Debug, Default)]
pub struct OnePoleTpt {
    s: f32,
    big_g: f32,
}

impl OnePoleTpt {
    pub fn set(&mut self, cutoff_hz: f32, sample_rate: f32) {
        let g = tpt_g(cutoff_hz, sample_rate);
        self.big_g = g / (1.0 + g);
    }

    pub fn reset(&mut self) {
        self.s = 0.0;
    }

    /// Returns the low-pass output; high-pass is `x - lp`.
    #[inline]
    pub fn lp(&mut self, x: f32) -> f32 {
        let v = (x - self.s) * self.big_g;
        let y = v + self.s;
        self.s = flush_denormal(y + v);
        y
    }

    #[inline]
    pub fn hp(&mut self, x: f32) -> f32 {
        x - self.lp(x)
    }
}

/// First-order DC blocker (high-pass at ~`cutoff_hz`).
#[derive(Clone, Debug, Default)]
pub struct DcBlocker {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlocker {
    pub fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            x1: 0.0,
            y1: 0.0,
            r: (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp(),
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = flush_denormal(y);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::magnitude_response_db;

    const SR: f32 = 48000.0;

    fn ir(mut f: impl FnMut(f32) -> f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| f(if i == 0 { 1.0 } else { 0.0 })).collect()
    }

    fn at(db: &[f64], hz: f64, n: usize) -> f64 {
        db[(hz * n as f64 / SR as f64).round() as usize]
    }

    /// Known-answer: TPT SVF matches the analog 2-pole prototype at the warped cutoff.
    #[test]
    fn svf_frequency_response() {
        let n = 1 << 16;
        for (mode, fc) in [
            (FilterMode::LowPass, 1000.0),
            (FilterMode::HighPass, 1000.0),
            (FilterMode::BandPass, 3000.0),
        ] {
            let mut f = Svf::new();
            f.set_coefs(tpt_g(fc, SR), std::f32::consts::SQRT_2); // Butterworth
            let db = magnitude_response_db(&ir(|x| f.process(x, mode), n), n);
            let at_fc = at(&db, fc as f64, n);
            match mode {
                FilterMode::LowPass => {
                    assert!((at_fc + 3.01).abs() < 0.05, "LP @fc {at_fc}");
                    assert!(at(&db, 100.0, n).abs() < 0.01);
                    // -12 dB/oct well above fc
                    let slope = at(&db, 8000.0, n) - at(&db, 4000.0, n);
                    assert!((slope + 12.0).abs() < 1.5, "LP slope {slope}");
                }
                FilterMode::HighPass => {
                    assert!((at_fc + 3.01).abs() < 0.05, "HP @fc {at_fc}");
                    assert!(at(&db, 20000.0, n).abs() < 0.05);
                    let slope = at(&db, 250.0, n) - at(&db, 125.0, n);
                    assert!((slope - 12.0).abs() < 0.5, "HP slope {slope}");
                }
                _ => {
                    assert!(at_fc.abs() < 0.05, "BP @fc {at_fc}");
                    assert!(at(&db, 300.0, n) < -16.0);
                }
            }
        }
        // notch is deep at fc
        let mut f = Svf::new();
        f.set_coefs(tpt_g(2000.0, SR), 1.0);
        let db = magnitude_response_db(&ir(|x| f.process(x, FilterMode::Notch), n), n);
        assert!(at(&db, 2000.0, n) < -40.0);
    }

    /// Known-answer: resonance peak gain at fc equals Q for the SVF low-pass.
    #[test]
    fn svf_resonance_peak_equals_q() {
        let n = 1 << 16;
        let q = 8.0f32;
        let mut f = Svf::new();
        f.set_coefs(tpt_g(1000.0, SR), 1.0 / q);
        let db = magnitude_response_db(&ir(|x| f.process(x, FilterMode::LowPass), n), n);
        let expected = 20.0 * (q as f64).log10();
        assert!((at(&db, 1000.0, n) - expected).abs() < 0.1);
    }

    /// Known-answer: ladder LP4 with no resonance is four matched one-poles: -12 dB at fc
    /// and -24 dB/oct above.
    #[test]
    fn ladder_frequency_response() {
        let n = 1 << 16;
        let mut l = Ladder::new();
        l.set(1000.0, 0.0, SR);
        // small signal keeps the tanh linear
        let small = 1e-3;
        let db: Vec<f64> =
            magnitude_response_db(&ir(|x| l.process(x * small, FilterMode::LowPass), n), n)
                .into_iter()
                .map(|d| d - 20.0 * (small as f64).log10())
                .collect();
        assert!(at(&db, 50.0, n).abs() < 0.05, "{}", at(&db, 50.0, n));
        assert!(
            (at(&db, 1000.0, n) + 12.04).abs() < 0.1,
            "{}",
            at(&db, 1000.0, n)
        );
        let slope = at(&db, 6000.0, n) - at(&db, 3000.0, n);
        assert!((slope + 24.0).abs() < 2.0, "slope {slope}");

        // HP4 mirrors it
        let mut l = Ladder::new();
        l.set(1000.0, 0.0, SR);
        let db: Vec<f64> =
            magnitude_response_db(&ir(|x| l.process(x * small, FilterMode::HighPass), n), n)
                .into_iter()
                .map(|d| d - 20.0 * (small as f64).log10())
                .collect();
        assert!((at(&db, 1000.0, n) + 12.04).abs() < 0.1);
        assert!(at(&db, 15000.0, n).abs() < 0.3);
    }

    /// Resonance builds a peak near fc and full resonance self-oscillates with bounded
    /// amplitude (no blow-up), even under audio-rate cutoff modulation.
    #[test]
    fn ladder_resonance_and_stability() {
        let n = 1 << 15;
        let mut l = Ladder::new();
        l.set(2000.0, 0.9, SR);
        let db = magnitude_response_db(&ir(|x| l.process(x * 1e-3, FilterMode::LowPass), n), n);
        let peak = at(&db, 2000.0, n) + 60.0;
        assert!(peak > 6.0, "resonant peak {peak}");

        let mut l = Ladder::new();
        let mut maxv = 0.0f32;
        let mut ph = 0.0f32;
        for i in 0..96000 {
            ph += 0.013;
            let fc = 1000.0 * (4.0 * ph.sin()).exp2();
            l.set(fc, 1.0, SR);
            let x = if i < 10 { 1.0 } else { 0.0 };
            let y = l.process(x, FilterMode::LowPass);
            assert!(y.is_finite());
            maxv = maxv.max(y.abs());
        }
        assert!(maxv < 4.0, "max {maxv}");
        assert!(maxv > 0.05, "should self-oscillate, max {maxv}");
    }

    /// Known-answer: biquads match their analytic response (cookbook formulas).
    #[test]
    fn biquad_matches_analytic_response() {
        let n = 1 << 16;
        let sr = SR as f64;
        for shape in [
            BiquadShape::Peak { gain_db: 6.0 },
            BiquadShape::LowShelf { gain_db: -9.0 },
            BiquadShape::HighShelf { gain_db: 4.5 },
            BiquadShape::LowPass,
            BiquadShape::HighPass,
        ] {
            let c = BiquadCoefs::design(shape, 1500.0, std::f64::consts::FRAC_1_SQRT_2, sr);
            let mut b = Biquad::new(c);
            let db = magnitude_response_db(&ir(|x| b.process(x as f64) as f32, n), n);
            for hz in [30.0, 400.0, 1500.0, 6000.0, 18000.0] {
                let m = at(&db, hz, n);
                let a = c.magnitude_db((hz * n as f64 / sr).round() * sr / n as f64, sr);
                assert!((m - a).abs() < 0.01, "{shape:?} @{hz}: {m} vs {a}");
            }
            match shape {
                BiquadShape::Peak { gain_db } => {
                    assert!((c.magnitude_db(1500.0, sr) - gain_db).abs() < 1e-9)
                }
                BiquadShape::LowShelf { gain_db } => {
                    assert!((c.magnitude_db(10.0, sr) - gain_db).abs() < 0.05)
                }
                BiquadShape::HighShelf { gain_db } => {
                    assert!((c.magnitude_db(23000.0, sr) - gain_db).abs() < 0.2)
                }
                _ => assert!((c.magnitude_db(1500.0, sr) + 3.01).abs() < 0.01),
            }
        }
    }

    #[test]
    fn dc_blocker_removes_dc() {
        let mut d = DcBlocker::new(10.0, SR);
        let mut y = 1.0;
        for _ in 0..48000 {
            y = d.process(1.0);
        }
        assert!(y.abs() < 1e-3);
    }
}
