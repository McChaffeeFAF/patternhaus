//! ADSR envelope with adjustable curvature.
//!
//! Each segment is a one-pole approach towards a target placed slightly *beyond* the
//! segment's end point; the overshoot ratio sets the curvature (large ratio ≈ linear,
//! small ratio ≈ RC-exponential) and the coefficient is chosen so a full-scale segment
//! (0 → 1 attack, 1 → 0 decay or release) takes exactly its nominal time. Retriggering and
//! release start from the current level (no clicks).

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
    /// Fast linear fade used when a voice is stolen or choked.
    Kill,
}

#[derive(Clone, Debug)]
pub struct Adsr {
    stage: Stage,
    level: f32,
    sample_rate: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    curve: f32,
    att_coef: f32,
    att_base: f32,
    dec_coef: f32,
    dec_base: f32,
    rel_coef: f32,
    rel_base: f32,
    ratio_att: f32,
    ratio_dr: f32,
    kill_step: f32,
}

/// Segment coefficient for a one-pole that covers its span in `samples` given `ratio`.
fn coef(samples: f32, ratio: f32) -> f32 {
    if samples <= 1.0 {
        0.0
    } else {
        (-((1.0 + ratio) / ratio).ln() / samples).exp()
    }
}

impl Adsr {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            stage: Stage::Idle,
            level: 0.0,
            sample_rate,
            attack: 0.005,
            decay: 0.2,
            sustain: 1.0,
            release: 0.2,
            curve: 0.5,
            att_coef: 0.0,
            att_base: 0.0,
            dec_coef: 0.0,
            dec_base: 0.0,
            rel_coef: 0.0,
            rel_base: 0.0,
            ratio_att: 0.3,
            ratio_dr: 0.001,
            kill_step: 0.0,
        };
        e.recompute();
        e
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.recompute();
    }

    /// Times in seconds, sustain 0..1, curve 0 (linear) ..1 (strongly exponential).
    pub fn set(&mut self, attack: f32, decay: f32, sustain: f32, release: f32, curve: f32) {
        if (attack, decay, sustain, release, curve)
            != (
                self.attack,
                self.decay,
                self.sustain,
                self.release,
                self.curve,
            )
        {
            self.attack = attack.max(0.0);
            self.decay = decay.max(0.0);
            self.sustain = sustain.clamp(0.0, 1.0);
            self.release = release.max(0.0);
            self.curve = curve.clamp(0.0, 1.0);
            self.recompute();
        }
    }

    fn recompute(&mut self) {
        let c = self.curve;
        self.ratio_att = 10f32.powf(2.0 - 2.5 * c); // 100 .. 0.3
        self.ratio_dr = 10f32.powf(2.0 - 6.0 * c); // 100 .. 1e-4
        let sr = self.sample_rate;
        self.att_coef = coef(self.attack * sr, self.ratio_att);
        self.att_base = (1.0 + self.ratio_att) * (1.0 - self.att_coef);
        self.dec_coef = coef(self.decay * sr, self.ratio_dr);
        self.dec_base = (self.sustain - self.ratio_dr) * (1.0 - self.dec_coef);
        self.rel_coef = coef(self.release * sr, self.ratio_dr);
        self.rel_base = -self.ratio_dr * (1.0 - self.rel_coef);
        if self.stage == Stage::Sustain {
            self.level = self.sustain;
        }
    }

    pub fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn gate_off(&mut self) {
        if !matches!(self.stage, Stage::Idle | Stage::Kill) {
            self.stage = Stage::Release;
        }
    }

    /// Fade to zero linearly over `seconds` (voice steal / choke).
    pub fn kill(&mut self, seconds: f32) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Kill;
            self.kill_step = self.level.max(1e-6) / (seconds * self.sample_rate).max(1.0);
        }
    }

    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    #[inline]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    #[inline]
    pub fn level(&self) -> f32 {
        self.level
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level = self.att_base + self.level * self.att_coef;
                if self.level >= 1.0 || self.att_coef == 0.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.dec_base + self.level * self.dec_coef;
                if self.level <= self.sustain || self.dec_coef == 0.0 {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = self.sustain,
            Stage::Release => {
                self.level = self.rel_base + self.level * self.rel_coef;
                if self.level <= 0.0 || self.rel_coef == 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Kill => {
                self.level -= self.kill_step;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_times_are_exact() {
        let sr = 48000.0;
        for curve in [0.0, 0.5, 1.0] {
            let mut e = Adsr::new(sr);
            e.set(0.01, 0.1, 0.0, 0.2, curve);
            e.gate_on();
            let mut n = 0;
            while e.stage() == Stage::Attack {
                e.next();
                n += 1;
            }
            assert!((n as f32 - 480.0).abs() <= 2.0, "attack {n} curve {curve}");
            let mut n = 0;
            while e.stage() == Stage::Decay {
                e.next();
                n += 1;
            }
            assert!((n as f32 - 4800.0).abs() <= 48.0, "decay {n} curve {curve}");
            assert_eq!(e.next(), 0.0);
            // a sustain change while sustaining moves the level
            e.set(0.01, 0.1, 0.5, 0.2, curve);
            assert_eq!(e.next(), 0.5);
            e.gate_off();
            let mut n = 0;
            while e.is_active() {
                e.next();
                n += 1;
            }
            // release covers 1 → 0 in 0.2 s, so from 0.5 it takes less
            assert!(n <= 9600 && n > 100, "release {n}");
        }
    }

    #[test]
    fn curve_changes_shape() {
        let sr = 48000.0;
        let mid = |curve| {
            let mut e = Adsr::new(sr);
            e.set(0.0, 0.1, 0.0, 0.1, curve);
            e.gate_on();
            for _ in 0..2400 {
                e.next();
            }
            e.level()
        };
        // halfway through the decay: linear ≈ 0.5, exponential much lower
        assert!((mid(0.0) - 0.5).abs() < 0.02, "{}", mid(0.0));
        assert!(mid(1.0) < 0.1, "{}", mid(1.0));
    }

    #[test]
    fn kill_fades_quickly() {
        let mut e = Adsr::new(48000.0);
        e.set(0.0, 0.0, 1.0, 5.0, 0.5);
        e.gate_on();
        e.next();
        e.kill(0.002);
        let mut n = 0;
        while e.is_active() {
            e.next();
            n += 1;
        }
        assert!((95..=97).contains(&n), "{n}");
    }
}
