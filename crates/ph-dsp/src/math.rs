//! Conversions, fast approximations and a deterministic PRNG.

/// Decibels to linear gain.
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    (db * (std::f32::consts::LN_10 / 20.0)).exp()
}

/// Linear gain to decibels. Floors at -600 dB for zero input.
#[inline]
pub fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.abs().max(1e-30).log10()
}

/// MIDI key (fractional) to frequency, A4 = key 69 = 440 Hz.
#[inline]
pub fn key_to_hz(key: f32) -> f32 {
    440.0 * ((key - 69.0) * (1.0 / 12.0)).exp2()
}

/// Frequency to MIDI key (fractional).
#[inline]
pub fn hz_to_key(hz: f32) -> f32 {
    69.0 + 12.0 * (hz.max(1e-6) / 440.0).log2()
}

/// Replace tiny values with zero. Recursive filters and feedback loops call this on their
/// state so silence after a loud signal never decays into subnormal numbers (which are
/// 10–100× slower on x86) even when the thread has no flush-to-zero set.
#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-18 { 0.0 } else { x }
}

/// [`flush_denormal`] for `f64` state.
#[inline]
pub fn flush_denormal64(x: f64) -> f64 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// Rational (Padé 7/6) tanh approximation, clamped to ±1. Max error ≈ 2e-4 over all inputs,
/// monotonic, about 4× cheaper than `f32::tanh`.
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-4.97, 4.97);
    let x2 = x * x;
    let num = x * (135135.0 + x2 * (17325.0 + x2 * (378.0 + x2)));
    let den = 135135.0 + x2 * (62370.0 + x2 * (3150.0 + 28.0 * x2));
    (num / den).clamp(-1.0, 1.0)
}

/// `sin(2π·t)` for any `t` (in turns). Odd Taylor polynomial on a quarter wave;
/// max error ≈ 4e-8, no table, no branches beyond the reflection.
#[inline]
pub fn sin_turns(t: f64) -> f32 {
    // reduce to [-0.5, 0.5)
    let mut x = t - (t + 0.5).floor();
    // reflect to [-0.25, 0.25]: sin(2π(0.5 - x)) = sin(2πx)
    if x > 0.25 {
        x = 0.5 - x;
    } else if x < -0.25 {
        x = -0.5 - x;
    }
    let z = x * std::f64::consts::TAU;
    let z2 = z * z;
    let p = z
        * (1.0
            + z2 * (-1.0 / 6.0
                + z2 * (1.0 / 120.0
                    + z2 * (-1.0 / 5040.0 + z2 * (1.0 / 362880.0 + z2 * (-1.0 / 39916800.0))))));
    p as f32
}

/// Equal-power pan law. `pan` in -1..=1, returns (left, right) gains; centre = (√½, √½).
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25; // turns: 0..0.5 quarter wave
    (sin_turns(a as f64 + 0.25), sin_turns(a as f64))
}

/// Deterministic, seedable PRNG (xorshift64*, seeded through SplitMix64). Real-time safe.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // SplitMix64 so that nearby seeds give unrelated streams, and 0 is valid.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self { state: z | 1 }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
    }

    /// Uniform in `[0, 1)`, 53-bit.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// Uniform integer in `0..n` (`n > 0`).
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        for db in [-60.0f32, -6.0, 0.0, 6.0, 24.0] {
            assert!((lin_to_db(db_to_lin(db)) - db).abs() < 1e-4);
        }
        assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn key_hz_roundtrip() {
        assert!((key_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((key_to_hz(60.0) - 261.6256).abs() < 1e-3);
        assert!((hz_to_key(key_to_hz(37.3)) - 37.3).abs() < 1e-4);
    }

    #[test]
    fn fast_tanh_is_accurate_and_bounded() {
        let mut x = -10.0f32;
        while x < 10.0 {
            assert!((fast_tanh(x) - x.tanh()).abs() < 3e-4, "x={x}");
            x += 0.01;
        }
    }

    #[test]
    fn sin_turns_is_accurate() {
        for i in -2000..2000 {
            let t = i as f64 * 0.00123;
            let e = (sin_turns(t) as f64 - (t * std::f64::consts::TAU).sin()).abs();
            assert!(e < 1e-6, "t={t} e={e}");
        }
    }

    #[test]
    fn pan_law_is_equal_power() {
        for p in [-1.0f32, -0.3, 0.0, 0.5, 1.0] {
            let (l, r) = pan_gains(p);
            assert!((l * l + r * r - 1.0).abs() < 1e-5);
        }
        let (l, r) = pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    }

    #[test]
    fn rng_is_deterministic_and_uniform() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut sum = 0.0f64;
        for _ in 0..100_000 {
            let x = a.next_f32();
            assert_eq!(x, b.next_f32());
            assert!((0.0..1.0).contains(&x));
            sum += x as f64;
        }
        assert!((sum / 100_000.0 - 0.5).abs() < 0.01);
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }
}
