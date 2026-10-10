use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Musical time in ticks. One quarter note is [`PPQ`] ticks.
pub type Tick = u64;

/// Ticks per quarter note. 960 divides evenly by 2, 3, 4, 5, 6, 8, 10, 12, 16, 32 and 64.
pub const PPQ: Tick = 960;

/// One step of the step sequencer: a 16th note.
pub const STEP: Tick = PPQ / 4;

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    pub fn ticks_per_bar(&self) -> Tick {
        PPQ * 4 * self.numerator as Tick / self.denominator as Tick
    }
}

/// Converts ticks to seconds at a constant tempo. Tempo automation is handled by the engine.
pub fn ticks_to_seconds(ticks: Tick, bpm: f64) -> f64 {
    ticks as f64 * 60.0 / (bpm * PPQ as f64)
}

pub fn seconds_to_ticks(seconds: f64, bpm: f64) -> f64 {
    seconds * bpm * PPQ as f64 / 60.0
}
