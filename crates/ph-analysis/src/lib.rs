//! Perception for agents (DESIGN §5): numbers and pictures describing what was written.
//!
//! Everything here is pure: audio comes in as `&[Vec<f32>]` channels plus a sample rate,
//! musical material as `ph_core` data. Nothing touches the engine, so analysis is
//! reproducible and unit-testable against known-answer signals.
//!
//! - [`loudness`]: ITU-R BS.1770-4 / EBU R128 LUFS, true peak, RMS, crest factor.
//! - [`spectrum`]: band energies and spectral centroid.
//! - [`health`]: clipping, DC offset, phase correlation, mono compatibility, energy per bar.
//! - [`onsets`]: onset detection with `bar.beat.tick` positions.
//! - [`tempo`]: BPM, loop length and onsets of audio material (sample registry, slicer).
//! - [`music`]: key and chord estimation from notes (exact) and from audio (chroma).
//! - [`masking`]: spectral masking between stems and note clashes.
//! - [`image`]: spectrogram, waveform, piano roll and playlist PNGs.

pub mod health;
pub mod image;
pub mod loudness;
pub mod masking;
pub mod music;
pub mod onsets;
pub mod spectrum;
pub mod tempo;
mod text;

use ph_core::{PPQ, Tick, TimeSignature};

/// A 32nd note in ticks: the shortest overlap that counts as "sounding together".
pub const PPQ_32: Tick = PPQ / 8;
use serde::Serialize;

/// Tempo and meter, for converting between seconds, ticks and `bar.beat.tick`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub struct Timing {
    pub bpm: f64,
    pub time_signature: TimeSignature,
}

impl Timing {
    pub fn new(bpm: f64, time_signature: TimeSignature) -> Self {
        Self {
            bpm: bpm.max(1.0),
            time_signature,
        }
    }

    pub fn ticks_per_bar(&self) -> Tick {
        self.time_signature.ticks_per_bar().max(1)
    }

    /// Ticks per beat (the denominator note value).
    pub fn ticks_per_beat(&self) -> Tick {
        (PPQ * 4 / self.time_signature.denominator.max(1) as Tick).max(1)
    }

    pub fn seconds_to_ticks(&self, seconds: f64) -> Tick {
        (seconds * self.bpm * PPQ as f64 / 60.0).round().max(0.0) as Tick
    }

    pub fn ticks_to_seconds(&self, ticks: Tick) -> f64 {
        ticks as f64 * 60.0 / (self.bpm * PPQ as f64)
    }

    pub fn seconds_per_bar(&self) -> f64 {
        self.ticks_to_seconds(self.ticks_per_bar())
    }

    /// 1-based `(bar, beat, tick)`.
    pub fn bar_beat_tick(&self, ticks: Tick) -> (u32, u32, u32) {
        let bar = ticks / self.ticks_per_bar();
        let in_bar = ticks % self.ticks_per_bar();
        let beat = in_bar / self.ticks_per_beat();
        let tick = in_bar % self.ticks_per_beat();
        (bar as u32 + 1, beat as u32 + 1, tick as u32)
    }

    /// `"3.2.480"`: bar 3, beat 2, 480 ticks in.
    pub fn position(&self, ticks: Tick) -> String {
        let (b, be, t) = self.bar_beat_tick(ticks);
        format!("{b}.{be}.{t}")
    }
}

/// Linear amplitude to dBFS; `None` for silence (so JSON never carries `-inf`).
pub fn db_opt(linear: f64) -> Option<f64> {
    (linear > 1e-12).then(|| 20.0 * linear.log10())
}

/// Power ratio to dB; `None` for zero.
pub fn pow_db_opt(power: f64) -> Option<f64> {
    (power > 1e-24).then(|| 10.0 * power.log10())
}

/// Rounds for compact reports.
pub fn round(x: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (x * m).round() / m
}

/// Mixes channels down to mono (average).
pub fn mono_mix(channels: &[Vec<f32>]) -> Vec<f32> {
    let n = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    if channels.len() == 1 {
        return channels[0].clone();
    }
    let k = channels.len().max(1) as f32;
    (0..n)
        .map(|i| {
            channels
                .iter()
                .map(|c| c.get(i).copied().unwrap_or(0.0))
                .sum::<f32>()
                / k
        })
        .collect()
}

/// The slice of every channel between two times, in seconds.
pub fn slice_seconds(channels: &[Vec<f32>], sample_rate: u32, from: f64, to: f64) -> Vec<Vec<f32>> {
    channels
        .iter()
        .map(|c| {
            let a = ((from.max(0.0) * sample_rate as f64) as usize).min(c.len());
            let b = ((to.max(0.0) * sample_rate as f64) as usize).clamp(a, c.len());
            c[a..b].to_vec()
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod testsig {
    use std::f64::consts::PI;

    pub fn sine(freq: f64, amp: f64, sr: u32, seconds: f64) -> Vec<f32> {
        let n = (seconds * sr as f64) as usize;
        (0..n)
            .map(|i| (amp * (2.0 * PI * freq * i as f64 / sr as f64).sin()) as f32)
            .collect()
    }

    pub fn dbfs(db: f64) -> f64 {
        10f64.powf(db / 20.0)
    }
}
