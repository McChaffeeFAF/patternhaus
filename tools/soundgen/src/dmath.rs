//! Platform-independent math.
//!
//! `f64::sin`/`exp`/`ln` defer to the system libm, whose last-bit results differ between
//! operating systems. Factory sounds must be byte-identical everywhere, so soundgen uses
//! these routines instead. They are built only from `+ - * /`, `floor` and `sqrt`, which
//! IEEE 754 requires to be exactly rounded.

use std::f64::consts::{LN_2, PI};

const TAU: f64 = 2.0 * PI;

/// Sine, accurate to about 1e-15 for |x| up to a few thousand radians.
pub fn sin(x: f64) -> f64 {
    let mut r = x - TAU * (x / TAU + 0.5).floor(); // [-pi, pi]
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    let r2 = r * r;
    // Taylor series to r^21; at |r| <= pi/2 the truncation error is below 1e-16.
    let mut term = r;
    let mut sum = r;
    let mut n = 1.0;
    for _ in 0..10 {
        term *= -r2 / ((n + 1.0) * (n + 2.0));
        sum += term;
        n += 2.0;
    }
    sum
}

pub fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

pub fn tan(x: f64) -> f64 {
    sin(x) / cos(x)
}

/// e^x for |x| < 700.
pub fn exp(x: f64) -> f64 {
    let x = x.clamp(-700.0, 700.0);
    let n = (x / LN_2 + 0.5).floor();
    let r = x - n * LN_2; // |r| <= ln2 / 2
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..18 {
        term *= r / k as f64;
        sum += term;
    }
    // 2^n built from the exponent bits.
    let scale = f64::from_bits(((n as i64 + 1023) as u64) << 52);
    sum * scale
}

/// Natural logarithm for x > 0.
pub fn ln(x: f64) -> f64 {
    assert!(x > 0.0 && x.is_finite());
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | (1023u64 << 52)); // [1, 2)
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut term = s;
    let mut sum = 0.0;
    for k in 0..16 {
        sum += term / (2 * k + 1) as f64;
        term *= s2;
    }
    2.0 * sum + e as f64 * LN_2
}

pub fn pow(a: f64, b: f64) -> f64 {
    exp(b * ln(a))
}

pub fn tanh(x: f64) -> f64 {
    let x = x.clamp(-20.0, 20.0);
    let e = exp(2.0 * x);
    (e - 1.0) / (e + 1.0)
}

/// 2^x.
pub fn exp2(x: f64) -> f64 {
    exp(x * LN_2)
}

pub fn log10(x: f64) -> f64 {
    ln(x) / std::f64::consts::LN_10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_std_closely() {
        for i in -2000..2000 {
            let x = i as f64 * 0.0137;
            assert!((sin(x) - x.sin()).abs() < 1e-12, "sin {x}");
            assert!((cos(x) - x.cos()).abs() < 1e-12, "cos {x}");
            assert!((exp(x) / x.exp() - 1.0).abs() < 1e-12, "exp {x}");
            assert!(
                (tanh(x / 10.0) - (x / 10.0).tanh()).abs() < 1e-12,
                "tanh {x}"
            );
        }
        for i in 1..4000 {
            let x = i as f64 * 0.37;
            assert!((ln(x) - x.ln()).abs() < 1e-12, "ln {x}");
        }
        assert!((pow(2.0, 0.5) - 2f64.sqrt()).abs() < 1e-12);
    }
}
