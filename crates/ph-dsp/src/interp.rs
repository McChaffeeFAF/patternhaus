//! Interpolation for resampling: linear and 4-point cubic Hermite (live), and a
//! Kaiser-windowed sinc with anti-aliasing cutoff scaling (high quality).

/// Linear interpolation between `x0` and `x1`.
#[inline]
pub fn linear(x0: f32, x1: f32, t: f32) -> f32 {
    x0 + (x1 - x0) * t
}

/// 4-point, 3rd-order Hermite (Catmull-Rom) interpolation between `x0` and `x1`;
/// `xm1` and `x2` are the neighbours.
#[inline]
pub fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

/// Zeroth-order modified Bessel function of the first kind (power series).
pub fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser window value for `x` in −1..1.
pub fn kaiser(x: f64, beta: f64) -> f64 {
    if x.abs() > 1.0 {
        0.0
    } else {
        bessel_i0(beta * (1.0 - x * x).sqrt()) / bessel_i0(beta)
    }
}

/// Normalised sinc, `sin(πx)/(πx)`.
pub fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Windowed-sinc interpolator. The kernel `sinc(t)·kaiser(t/W)` is tabulated over
/// `t ∈ [0, W]` and read with linear interpolation; when reading *faster* than the source
/// rate (`ratio > 1`) the kernel is stretched (cutoff lowered to `1/ratio`) so pitched-up
/// material does not alias.
#[derive(Clone, Debug)]
pub struct SincInterpolator {
    half_width: usize,
    res: usize,
    table: Vec<f32>,
    max_stretch: f64,
}

impl SincInterpolator {
    /// `half_width` zero crossings per side (16 → 32 taps at unity ratio).
    pub fn new(half_width: usize, max_stretch: f64) -> Self {
        let res = 512;
        let beta = 9.0;
        let n = half_width * res + 2;
        let table = (0..n)
            .map(|i| {
                let t = i as f64 / res as f64;
                (sinc(t) * kaiser(t / half_width as f64, beta)) as f32
            })
            .collect();
        Self {
            half_width,
            res,
            table,
            max_stretch: max_stretch.max(1.0),
        }
    }

    #[inline]
    fn kernel(&self, t: f64) -> f32 {
        let p = t.abs() * self.res as f64;
        let i = p as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let f = (p - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * f
    }

    /// Interpolate at fractional position `pos`, reading samples through `fetch` (which
    /// handles bounds and loops). `ratio` = source samples advanced per output sample.
    #[inline]
    pub fn interpolate(&self, pos: f64, ratio: f64, fetch: impl Fn(i64) -> f32) -> f32 {
        let stretch = ratio.clamp(1.0, self.max_stretch);
        let c = 1.0 / stretch;
        let reach = self.half_width as f64 * stretch;
        let lo = (pos - reach).ceil() as i64;
        let hi = (pos + reach).floor() as i64;
        let mut acc = 0.0f32;
        let mut wsum = 0.0f32;
        for j in lo..=hi {
            let w = self.kernel((j as f64 - pos) * c);
            acc += w * fetch(j);
            wsum += w;
        }
        if wsum.abs() > 1e-6 { acc / wsum } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermite_passes_through_points_and_is_exact_for_lines() {
        assert_eq!(hermite(0.0, 1.0, 2.0, 3.0, 0.0), 1.0);
        assert!((hermite(0.0, 1.0, 2.0, 3.0, 0.5) - 1.5).abs() < 1e-6);
        assert!((hermite(0.0, 1.0, 2.0, 3.0, 1.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn bessel_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-12);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008).abs() < 1e-12);
    }

    /// Known-answer: sinc interpolation of a band-limited tone is accurate to −80 dB.
    #[test]
    fn sinc_reconstructs_tone() {
        let s = SincInterpolator::new(16, 4.0);
        let f = 0.1; // cycles per sample
        let x: Vec<f32> = (0..400)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64).sin() as f32)
            .collect();
        let fetch = |i: i64| x.get(i as usize).copied().unwrap_or(0.0);
        let mut max_err = 0.0f64;
        for k in 0..500 {
            let pos = 100.0 + k as f64 * 0.3717;
            let y = s.interpolate(pos, 1.0, fetch) as f64;
            let e = (y - (2.0 * std::f64::consts::PI * f * pos).sin()).abs();
            max_err = max_err.max(e);
        }
        assert!(max_err < 1e-4, "{max_err}");
    }
}
