//! Audio in the model (M2): the project's sample registry, recording takes and audio clips.
//!
//! # Units and invariants (contract, M2/T0)
//!
//! - **Source positions are frames** of the source file at its own sample rate
//!   ([`AudioClip::source_offset`]). Frames, not ticks, because the audio itself does not move
//!   when the tempo changes: a slip edit of an unsynced clip must stay on the same sound, and
//!   frames are exact integers where tick ↔ frame conversions would round. Tempo-synced clips
//!   map playlist ticks to source frames through [`AudioClip::time_ratio`].
//! - **Playlist positions are ticks**, like every other clip ([`crate::Clip`] `start` and
//!   `length`, and the fade lengths in [`Fade`]). An audio clip's `Clip::offset` is always 0.
//! - **Registry paths are unique**: one [`SampleEntry`] per asset reference.
//! - A registry entry cannot be deleted while an audio clip uses it.

use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One audio file the project knows about: the sample registry entry.
///
/// Audio clips refer to entries by [`SampleId`]; the engine loads [`SampleEntry::path`]
/// through its `AssetResolver`. Sampler channels keep their plain string reference
/// (`InstrumentSpec::Sampler::sample`), which may or may not have an entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleEntry {
    pub id: SampleId,
    /// Asset reference: `samples/<file>` (project-relative, the usual case), `factory:<path>`
    /// or an absolute path inside an allowed library folder. Unique in the registry.
    pub path: String,
    /// The file's original name (before an import renamed a clash to `name (2).wav`).
    pub name: String,
    /// Length in frames (samples per channel).
    pub frames: u64,
    /// The file's own sample rate, in Hz.
    pub sample_rate: u32,
    /// 1 = mono, 2 = stereo.
    pub channels: u16,
    /// Tempo of the material, if known: detected by analysis or set by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<SampleBpm>,
    /// MIDI key the material is in or pitched at (60 = C4), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<u8>,
    /// Cache keys of analysis results kept outside the project file.
    #[serde(default, skip_serializing_if = "SampleAnalysis::is_empty")]
    pub analysis: SampleAnalysis,
    /// Where the audio came from.
    #[serde(default)]
    pub provenance: Provenance,
}

impl SampleEntry {
    /// Length in seconds.
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }

    /// The tempo, if one is known.
    pub fn bpm(&self) -> Option<f64> {
        self.bpm.map(|b| b.value)
    }
}

/// A sample's tempo and who said so.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleBpm {
    /// Beats per minute, 10..=999.
    pub value: f64,
    pub source: BpmSource,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BpmSource {
    /// Found by onset/tempo analysis (`ph-analysis`). Analysis may replace it.
    Detected,
    /// Set by a person or agent. Analysis never overwrites it.
    User,
}

/// Cache keys for analysis results stored outside `project.json` (onset lists, key
/// estimates). A key is opaque to the model; `ph-analysis` defines its form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleAnalysis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onsets: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl SampleAnalysis {
    pub fn is_empty(&self) -> bool {
        self.onsets.is_none() && self.key.is_none()
    }
}

/// Where a sample came from, and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    #[serde(default)]
    pub origin: Origin,
    /// When it entered the project, in seconds since the Unix epoch (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// Not recorded (e.g. registered by hand).
    #[default]
    Unknown,
    /// Copied in from a file: `from` is the path it was imported from, as given.
    Imported { from: String },
    /// Recorded in this project: a recording take.
    Recorded(Take),
    /// Written by the audio editor from another entry (the original is never overwritten).
    Edited {
        from: SampleId,
        /// What was done, e.g. `normalize`, `trim`, `reverse`.
        operation: String,
    },
}

/// A recording take: the registry entry holding it is the audio; this is the context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Take {
    /// The input it was recorded from, as the host names it (e.g. `Focusrite USB: In 1-2`).
    pub input: String,
    /// Song position where recording started, in ticks.
    pub start: Tick,
    /// Length of the recorded range, in ticks.
    pub length: Tick,
}

/// An audio clip's settings (the playlist position is on [`crate::Clip`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioClip {
    /// Registry entry that is played.
    pub sample: SampleId,
    /// Source position at the clip start, in **frames of the source file**. For a reversed
    /// clip this is still the start of the played range; it is played from its end.
    #[serde(default)]
    pub source_offset: u64,
    /// Clip gain in dB, -60..=24.
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub fade_in: Fade,
    #[serde(default)]
    pub fade_out: Fade,
    /// Transposition in semitones, -48..=48.
    #[serde(default)]
    pub pitch_semitones: i32,
    /// Fine tuning in cents, -100..=100.
    #[serde(default)]
    pub fine_cents: f32,
    /// Play the range backwards.
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub stretch: StretchMode,
    /// Follow the project tempo: the source is played at `project bpm / sample bpm` speed
    /// (needs the registry entry's `bpm`; without it the clip plays unsynced and the engine
    /// warns).
    #[serde(default)]
    pub tempo_sync: bool,
}

impl AudioClip {
    pub fn new(sample: SampleId) -> Self {
        Self {
            sample,
            source_offset: 0,
            gain_db: 0.0,
            fade_in: Fade::default(),
            fade_out: Fade::default(),
            pitch_semitones: 0,
            fine_cents: 0.0,
            reverse: false,
            stretch: StretchMode::default(),
            tempo_sync: false,
        }
    }

    /// Total transposition in semitones.
    pub fn pitch(&self) -> f64 {
        self.pitch_semitones as f64 + self.fine_cents as f64 / 100.0
    }

    /// Source seconds played per second of output at project tempo `bpm`.
    ///
    /// - tempo factor: `bpm / sample bpm` when `tempo_sync` and the entry has a tempo, else 1;
    /// - [`StretchMode::Resample`] multiplies by the pitch factor `2^(pitch/12)` (a turntable:
    ///   pitch and speed move together, so a transposed synced clip drifts off the grid);
    /// - [`StretchMode::Stretch`] and [`StretchMode::Slice`] keep the duration; pitch is
    ///   shifted independently.
    ///
    /// Sample-rate conversion (file rate vs engine rate) is on top of this and not included.
    pub fn time_ratio(&self, entry: &SampleEntry, bpm: f64) -> f64 {
        let tempo = match (self.tempo_sync, entry.bpm()) {
            (true, Some(src)) if src > 0.0 => bpm / src,
            _ => 1.0,
        };
        match self.stretch {
            StretchMode::Resample => tempo * 2f64.powf(self.pitch() / 12.0),
            StretchMode::Stretch | StretchMode::Slice => tempo,
        }
    }

    pub fn validate(&self) -> ModelResult<()> {
        let bad = |m: String| Err(ModelError::Invalid(m));
        if !self.gain_db.is_finite() || !(-60.0..=24.0).contains(&self.gain_db) {
            return bad(format!("gain_db {} outside -60..24", self.gain_db));
        }
        if !(-48..=48).contains(&self.pitch_semitones) {
            return bad(format!(
                "pitch_semitones {} outside -48..48",
                self.pitch_semitones
            ));
        }
        if !self.fine_cents.is_finite() || !(-100.0..=100.0).contains(&self.fine_cents) {
            return bad(format!("fine_cents {} outside -100..100", self.fine_cents));
        }
        Ok(())
    }
}

/// How an audio clip is fitted to time.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StretchMode {
    /// Turntable: speed and pitch change together.
    #[default]
    Resample,
    /// Time-stretch: duration follows the tempo, pitch is kept (Signalsmith Stretch, M2/T1).
    Stretch,
    /// Re-time at transients: each slice starts on its own beat position, unstretched.
    Slice,
}

/// A fade at one end of an audio clip.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Fade {
    /// Length in ticks; 0 = no fade. Fade-in plus fade-out never exceed the clip length.
    #[serde(default)]
    pub length: Tick,
    #[serde(default)]
    pub curve: FadeCurve,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    #[default]
    Linear,
    /// Slow start, fast finish (`t²`).
    Exp,
    /// Fast start, slow finish (`1 − (1 − t)²`).
    Log,
    /// Slow at both ends (`(1 − cos πt) / 2`).
    SCurve,
}

impl FadeCurve {
    /// Fade-in gain at position `t` in 0..=1 of the fade (0 → 0, 1 → 1). A fade-out uses
    /// `gain(1 − t)`. Exact at the ends, monotonic in between.
    pub fn gain(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            FadeCurve::Linear => t,
            FadeCurve::Exp => t * t,
            FadeCurve::Log => 1.0 - (1.0 - t) * (1.0 - t),
            FadeCurve::SCurve => {
                if t >= 1.0 {
                    1.0
                } else {
                    (1.0 - (std::f64::consts::PI * t).cos()) / 2.0
                }
            }
        }
    }
}

pub(crate) fn validate_sample(e: &SampleEntry) -> ModelResult<()> {
    let bad = |m: String| Err(ModelError::Invalid(m));
    if e.path.trim().is_empty() {
        return bad("sample path is empty".into());
    }
    if e.sample_rate == 0 || e.channels == 0 {
        return bad("sample rate and channels must be > 0".into());
    }
    if let Some(b) = e.bpm
        && (!b.value.is_finite() || !(10.0..=999.0).contains(&b.value))
    {
        return bad(format!("sample bpm {} outside 10..999", b.value));
    }
    if e.root_key.is_some_and(|k| k > 127) {
        return bad("root_key > 127".into());
    }
    Ok(())
}
