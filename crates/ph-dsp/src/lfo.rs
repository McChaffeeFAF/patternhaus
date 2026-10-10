//! Low-frequency oscillators with tempo sync.
//!
//! Phase advances one sample at a time (so output never depends on block size). A synced
//! LFO is re-aligned to the song position only when the transport jumps; see
//! [`Lfo::sync_to_position`].

use crate::math::{Rng, sin_turns};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LfoShape {
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Square,
    SampleHold,
}

impl LfoShape {
    pub const NAMES: [&'static str; 6] =
        ["Sine", "Triangle", "Saw Up", "Saw Down", "Square", "S&H"];

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => LfoShape::Sine,
            1 => LfoShape::Triangle,
            2 => LfoShape::SawUp,
            3 => LfoShape::SawDown,
            4 => LfoShape::Square,
            _ => LfoShape::SampleHold,
        }
    }
}

/// Tempo-sync divisions: (label, length in quarter-note beats).
pub const SYNC_DIVISIONS: [(&str, f64); 18] = [
    ("1/64", 1.0 / 16.0),
    ("1/32T", 1.0 / 12.0),
    ("1/32", 1.0 / 8.0),
    ("1/16T", 1.0 / 6.0),
    ("1/16", 0.25),
    ("1/16.", 0.375),
    ("1/8T", 1.0 / 3.0),
    ("1/8", 0.5),
    ("1/8.", 0.75),
    ("1/4T", 2.0 / 3.0),
    ("1/4", 1.0),
    ("1/4.", 1.5),
    ("1/2T", 4.0 / 3.0),
    ("1/2", 2.0),
    ("1/2.", 3.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
];

/// Labels of [`SYNC_DIVISIONS`] for choice parameters.
pub fn sync_division_names() -> Vec<&'static str> {
    SYNC_DIVISIONS.iter().map(|d| d.0).collect()
}

#[derive(Clone, Debug)]
pub struct Lfo {
    phase: f64,
    inc: f64,
    rng: Rng,
    held: f32,
}

impl Lfo {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let held = rng.bipolar();
        Self {
            phase: 0.0,
            inc: 0.0,
            rng,
            held,
        }
    }

    pub fn set_rate_hz(&mut self, hz: f64, sample_rate: f64) {
        self.inc = hz / sample_rate;
    }

    /// Cycle length `beats` quarter notes at `bpm`.
    pub fn set_rate_synced(&mut self, beats: f64, bpm: f64, sample_rate: f64) {
        self.inc = bpm / 60.0 / beats.max(1e-6) / sample_rate;
    }

    pub fn set_phase(&mut self, phase: f64) {
        self.phase = phase - phase.floor();
    }

    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Align phase to a song position in beats for a cycle of `beats`.
    pub fn sync_to_position(&mut self, position_beats: f64, beats: f64) {
        self.set_phase(position_beats / beats.max(1e-6));
    }

    /// Value of the current phase, bipolar −1..1, then advance one sample.
    #[inline]
    pub fn next(&mut self, shape: LfoShape) -> f32 {
        let t = self.phase;
        let y = match shape {
            LfoShape::Sine => sin_turns(t),
            LfoShape::Triangle => {
                // starts at 0 rising, like the sine
                let t2 = (t + 0.25).fract();
                (1.0 - 4.0 * (t2 - 0.5).abs()) as f32
            }
            LfoShape::SawUp => (2.0 * t - 1.0) as f32,
            LfoShape::SawDown => (1.0 - 2.0 * t) as f32,
            LfoShape::Square => {
                if t < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::SampleHold => self.held,
        };
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.held = self.rng.bipolar();
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_rate_matches_tempo() {
        let mut l = Lfo::new(1);
        // 1/4 at 120 bpm = 2 Hz
        l.set_rate_synced(1.0, 120.0, 48000.0);
        let mut wraps = 0;
        let mut last = l.phase();
        for _ in 0..48000 {
            l.next(LfoShape::Sine);
            if l.phase() < last {
                wraps += 1;
            }
            last = l.phase();
        }
        assert_eq!(wraps, 2);
    }

    #[test]
    fn shapes_are_bipolar() {
        for i in 0..6 {
            let mut l = Lfo::new(3);
            l.set_rate_hz(5.0, 1000.0);
            let s = LfoShape::from_index(i);
            let (mut lo, mut hi) = (1.0f32, -1.0f32);
            for _ in 0..2000 {
                let y = l.next(s);
                lo = lo.min(y);
                hi = hi.max(y);
            }
            assert!(lo >= -1.0 && hi <= 1.0 && hi - lo > 0.5, "{s:?}");
        }
    }
}
