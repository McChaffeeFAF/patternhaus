//! Delay line with integer and fractional reads.

use crate::interp::hermite;

/// Power-of-two ring buffer. `push` then read `d` samples back (`d = 0` is the sample just
/// pushed).
#[derive(Clone, Debug)]
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// Holds at least `max_delay` samples of history (plus interpolation headroom).
    pub fn new(max_delay: usize) -> Self {
        let n = (max_delay + 4).next_power_of_two();
        Self {
            buf: vec![0.0; n],
            mask: n - 1,
            write: 0,
        }
    }

    /// Largest delay that can be read with [`Self::read_cubic`].
    pub fn max_delay(&self) -> usize {
        self.buf.len() - 4
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }

    #[inline]
    pub fn push(&mut self, x: f32) {
        self.write = (self.write + 1) & self.mask;
        self.buf[self.write] = x;
    }

    #[inline]
    pub fn read(&self, d: usize) -> f32 {
        self.buf[(self.write.wrapping_sub(d)) & self.mask]
    }

    /// Linear-interpolated read at fractional delay `d ≥ 0`.
    #[inline]
    pub fn read_linear(&self, d: f32) -> f32 {
        let i = d as usize;
        let f = d - i as f32;
        let a = self.read(i);
        a + (self.read(i + 1) - a) * f
    }

    /// Cubic Hermite read at fractional delay `d ≥ 1`.
    #[inline]
    pub fn read_cubic(&self, d: f32) -> f32 {
        let d = d.max(1.0);
        let i = d as usize;
        let f = d - i as f32;
        hermite(
            self.read(i - 1),
            self.read(i),
            self.read(i + 1),
            self.read(i + 2),
            f,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_and_fractional_reads() {
        let mut d = DelayLine::new(100);
        for i in 0..50 {
            d.push(i as f32);
        }
        assert_eq!(d.read(0), 49.0);
        assert_eq!(d.read(10), 39.0);
        assert!((d.read_linear(10.25) - 38.75).abs() < 1e-5);
        assert!((d.read_cubic(10.25) - 38.75).abs() < 1e-4);
    }
}
