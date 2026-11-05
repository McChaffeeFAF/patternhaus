//! The group envelopes: DAHDSR with linear segments, counted in frames.
//!
//! Linear segments make every stage a known answer (the tests check them frame by frame):
//! - **delay** `d` frames at 0;
//! - **attack** `a` frames: frame `k` is `k / a` (0 at the first frame);
//! - **hold** `h` frames at 1;
//! - **decay** `dc` frames: frame `k` is `1 - (1 - sustain) × k / dc`;
//! - **sustain** at `sustain` until the gate closes;
//! - **release** `r` frames from the level the gate closed at, `L × (1 - k / r)`; then idle.
//!
//! A zero-length stage is skipped. Closing the gate in any stage releases from the current
//! level, so nothing clicks. Real-time: no allocation.

use ph_core::Envelope;

/// An envelope's stage lengths in frames, compiled from a model [`Envelope`] at a sample rate.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct EnvShape {
    pub delay: u32,
    pub attack: u32,
    pub hold: u32,
    pub decay: u32,
    pub sustain: f32,
    pub release: u32,
}

fn frames(seconds: f32, sr: f64) -> u32 {
    (seconds.clamp(0.0, 60.0) as f64 * sr).round() as u32
}

impl EnvShape {
    pub fn new(e: &Envelope, sample_rate: f64) -> Self {
        Self {
            delay: frames(e.delay, sample_rate),
            attack: frames(e.attack, sample_rate),
            hold: frames(e.hold, sample_rate),
            decay: frames(e.decay, sample_rate),
            sustain: e.sustain.clamp(0.0, 1.0),
            release: frames(e.release, sample_rate),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Idle,
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

/// One running envelope.
#[derive(Copy, Clone, Debug, Default)]
pub struct Env {
    shape: EnvShape,
    stage: Stage,
    /// Frames spent in the current stage.
    t: u32,
    level: f32,
    /// Level the release started from.
    from: f32,
}

impl Env {
    /// Opens the gate from the start (delay first).
    pub fn start(&mut self, shape: EnvShape) {
        self.shape = shape;
        self.level = 0.0;
        self.t = 0;
        self.stage = Stage::Delay;
        self.settle();
    }

    /// Moves past zero-length stages.
    fn settle(&mut self) {
        loop {
            let len = match self.stage {
                Stage::Delay => self.shape.delay,
                Stage::Attack => self.shape.attack,
                Stage::Hold => self.shape.hold,
                Stage::Decay => self.shape.decay,
                Stage::Release => self.shape.release,
                Stage::Sustain | Stage::Idle => return,
            };
            if self.t < len {
                return;
            }
            self.t = 0;
            self.stage = match self.stage {
                Stage::Delay => Stage::Attack,
                Stage::Attack => {
                    self.level = 1.0;
                    Stage::Hold
                }
                Stage::Hold => Stage::Decay,
                Stage::Decay => {
                    self.level = self.shape.sustain;
                    Stage::Sustain
                }
                _ => {
                    self.level = 0.0;
                    Stage::Idle
                }
            };
        }
    }

    /// Closes the gate: release from the current level.
    pub fn release(&mut self) {
        if matches!(self.stage, Stage::Idle | Stage::Release) {
            return;
        }
        self.from = self.level;
        self.stage = Stage::Release;
        self.t = 0;
        self.settle();
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// The level of the frame about to play.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// The level for this frame, then advances one frame.
    #[inline]
    pub fn next(&mut self) -> f32 {
        let s = &self.shape;
        let k = self.t as f32;
        self.level = match self.stage {
            Stage::Idle | Stage::Delay => 0.0,
            Stage::Attack => k / s.attack as f32,
            Stage::Hold => 1.0,
            Stage::Decay => 1.0 - (1.0 - s.sustain) * k / s.decay as f32,
            Stage::Sustain => s.sustain,
            Stage::Release => self.from * (1.0 - k / s.release as f32),
        };
        let out = self.level;
        if !matches!(self.stage, Stage::Sustain | Stage::Idle) {
            self.t += 1;
            self.settle();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(d: u32, a: u32, h: u32, dc: u32, s: f32, r: u32) -> EnvShape {
        EnvShape {
            delay: d,
            attack: a,
            hold: h,
            decay: dc,
            sustain: s,
            release: r,
        }
    }

    /// Known answer: every stage, frame by frame.
    #[test]
    fn envelope_stages_known_answer() {
        let mut e = Env::default();
        e.start(shape(2, 4, 2, 4, 0.5, 4));
        let got: Vec<f32> = (0..16).map(|_| e.next()).collect();
        let want = [
            0.0, 0.0, // delay
            0.0, 0.25, 0.5, 0.75, // attack
            1.0, 1.0, // hold
            1.0, 0.875, 0.75, 0.625, // decay
            0.5, 0.5, 0.5, 0.5, // sustain
        ];
        assert_eq!(got, want);
        e.release();
        let rel: Vec<f32> = (0..5).map(|_| e.next()).collect();
        assert_eq!(rel, [0.5, 0.375, 0.25, 0.125, 0.0]);
        assert!(e.is_idle());
    }

    #[test]
    fn zero_stages_are_skipped_and_release_starts_from_the_current_level() {
        let mut e = Env::default();
        e.start(shape(0, 0, 0, 0, 1.0, 0));
        assert_eq!(e.next(), 1.0, "no attack: full level at the first frame");
        e.release();
        assert!(e.is_idle(), "zero release: done at once");
        // release during the attack
        e.start(shape(0, 10, 0, 0, 1.0, 2));
        for _ in 0..5 {
            e.next();
        }
        e.release();
        // the last frame played was 4/10
        assert_eq!([e.next(), e.next()], [0.4, 0.2]);
        assert_eq!(e.next(), 0.0);
        assert!(e.is_idle());
        // seconds to frames
        let s = EnvShape::new(
            &Envelope {
                delay: 0.001,
                attack: 0.01,
                hold: 0.0,
                decay: 0.5,
                sustain: 0.3,
                release: 0.25,
            },
            48_000.0,
        );
        assert_eq!(s, shape(48, 480, 0, 24_000, 0.3, 12_000));
    }
}
