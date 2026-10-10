//! Level detection for dynamics processors.

/// Detector law.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Detect {
    Peak,
    Rms,
}

/// One-pole attack/release coefficient for a time constant in seconds.
#[inline]
pub fn time_coef(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 {
        0.0
    } else {
        (-1.0 / (seconds * sample_rate)).exp()
    }
}

/// Envelope follower: rectifies (peak) or squares (RMS) its input and smooths it with
/// separate attack and release time constants ("branching" detector). Output is a linear
/// amplitude.
#[derive(Clone, Debug)]
pub struct EnvelopeFollower {
    mode: Detect,
    att: f32,
    rel: f32,
    env: f32,
}

impl EnvelopeFollower {
    pub fn new(mode: Detect) -> Self {
        Self {
            mode,
            att: 0.0,
            rel: 0.0,
            env: 0.0,
        }
    }

    pub fn set_mode(&mut self, mode: Detect) {
        self.mode = mode;
    }

    pub fn set_times(&mut self, attack_s: f32, release_s: f32, sample_rate: f32) {
        self.att = time_coef(attack_s, sample_rate);
        self.rel = time_coef(release_s, sample_rate);
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let v = match self.mode {
            Detect::Peak => x.abs(),
            Detect::Rms => x * x,
        };
        let c = if v > self.env { self.att } else { self.rel };
        self.env = crate::flush_denormal(v + (self.env - v) * c);
        match self.mode {
            Detect::Peak => self.env,
            Detect::Rms => self.env.sqrt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_sine_is_0707() {
        let mut f = EnvelopeFollower::new(Detect::Rms);
        f.set_times(0.05, 0.05, 48000.0);
        let mut y = 0.0;
        for i in 0..48000 {
            y = f.process((i as f32 * 0.1).sin());
        }
        assert!((y - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02, "{y}");
    }

    #[test]
    fn peak_attack_is_fast_release_slow() {
        let mut f = EnvelopeFollower::new(Detect::Peak);
        f.set_times(0.0001, 0.1, 48000.0);
        for _ in 0..100 {
            f.process(1.0);
        }
        assert!(f.process(1.0) > 0.99);
        for _ in 0..480 {
            f.process(0.0);
        }
        let y = f.process(0.0);
        assert!(y > 0.85 && y < 0.95, "{y}");
    }
}
