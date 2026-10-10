//! Parameter smoothing.

/// One-pole exponential smoother. Reaches ~63 % of a step in `time`, snaps exactly to the
/// target once within 1e-6 or once rounding stalls it (so a settled smoother costs nothing and is bit-stable).
#[derive(Clone, Debug)]
pub struct OnePole {
    y: f32,
    target: f32,
    coef: f32,
}

impl OnePole {
    pub fn new(value: f32) -> Self {
        Self {
            y: value,
            target: value,
            coef: 0.0,
        }
    }

    /// Time constant in seconds; `0` makes the smoother pass the target straight through.
    pub fn set_time(&mut self, seconds: f32, sample_rate: f32) {
        self.coef = if seconds <= 0.0 {
            0.0
        } else {
            (-1.0 / (seconds * sample_rate)).exp()
        };
    }

    #[inline]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump to `value` with no smoothing.
    pub fn reset(&mut self, value: f32) {
        self.y = value;
        self.target = value;
    }

    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.y
    }

    #[inline]
    pub fn is_settled(&self) -> bool {
        self.y == self.target
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.y != self.target {
            let y = self.target + (self.y - self.target) * self.coef;
            // snap once close, or once f32 rounding stops it moving
            self.y = if y == self.y || (y - self.target).abs() <= 1e-6 * (1.0 + self.target.abs()) {
                self.target
            } else {
                y
            };
        }
        self.y
    }
}

/// Linear ramp to a target over a fixed number of samples.
#[derive(Clone, Debug)]
pub struct LinearRamp {
    value: f32,
    target: f32,
    step: f32,
    remaining: u32,
    len: u32,
}

impl LinearRamp {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
            len: 1,
        }
    }

    pub fn set_len(&mut self, samples: u32) {
        self.len = samples.max(1);
    }

    pub fn set_target(&mut self, target: f32) {
        if target != self.target {
            self.target = target;
            self.remaining = self.len;
            self.step = (target - self.value) / self.len as f32;
        }
    }

    pub fn reset(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.remaining = 0;
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }

    #[inline]
    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.value = if self.remaining == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pole_time_constant() {
        let mut s = OnePole::new(0.0);
        s.set_time(0.01, 48000.0);
        s.set_target(1.0);
        let mut y = 0.0;
        for _ in 0..480 {
            y = s.next();
        }
        assert!((y - (1.0 - (-1.0f32).exp())).abs() < 1e-3, "{y}");
        for _ in 0..48000 {
            s.next();
        }
        assert!(s.is_settled(), "{} {}", s.value(), s.target());
        assert_eq!(s.value(), 1.0);
    }

    #[test]
    fn linear_ramp_lands_exactly() {
        let mut r = LinearRamp::new(0.0);
        r.set_len(100);
        r.set_target(2.0);
        for i in 1..=100 {
            let v = r.next();
            if i == 50 {
                assert!((v - 1.0).abs() < 1e-5);
            }
        }
        assert_eq!(r.value(), 2.0);
        assert!(r.is_settled());
    }
}
