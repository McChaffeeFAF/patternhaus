use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// RGB colour, 0xRRGGBB.
pub type Color = u32;

/// A whole song.
///
/// Read freely; **mutate only through [`crate::Edit`]**, which records every change as a
/// [`crate::Patch`] so it can be undone per author, replayed, and audited. Collections sit
/// behind `Arc` and are copied on write, so snapshots are cheap and share unchanged parts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    /// Schema version of the saved form; see `ph-formats` migrations.
    pub format_version: u32,
    pub settings: ProjectSettings,
    pub channels: Arc<BTreeMap<ChannelId, Channel>>,
    /// Channel rack display order.
    pub channel_order: Arc<Vec<ChannelId>>,
    pub patterns: Arc<BTreeMap<PatternId, Arc<Pattern>>>,
    /// Pattern list display order.
    pub pattern_order: Arc<Vec<PatternId>>,
    /// Positional, like FL's mixer. Index 0 is the master. Fixed length ([`MIXER_INSERTS`] + 1).
    pub mixer: Arc<Vec<MixerInsert>>,
    /// Positional playlist track headers. Fixed length ([`PLAYLIST_TRACKS`]).
    pub tracks: Arc<Vec<PlaylistTrack>>,
    pub clips: Arc<BTreeMap<ClipId, Clip>>,
    /// Next free ids. Ids are never reused, so references held by agents stay unambiguous.
    /// Not part of undo: undoing a creation does not free its id.
    pub next_ids: NextIds,
}

pub const FORMAT_VERSION: u32 = 1;
pub const MIXER_INSERTS: usize = 32;
pub const PLAYLIST_TRACKS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSettings {
    pub title: String,
    pub bpm: f64,
    pub time_signature: TimeSignature,
    /// Swing amount 0.0..=1.0 applied to off-beat 16ths (FL's main swing).
    #[serde(default)]
    pub swing: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NextIds {
    pub channel: u32,
    pub pattern: u32,
    pub clip: u32,
    pub note: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub color: Color,
    pub instrument: InstrumentSpec,
    /// 0.0..=1.0, FL-style default 0.78.
    pub volume: f32,
    /// -1.0 (left) ..= 1.0 (right).
    pub pan: f32,
    pub muted: bool,
    /// Mixer insert this channel feeds. 0 = master.
    pub insert: InsertId,
}

/// What makes the sound for a channel. Parameters are open maps keyed by the instrument's
/// stable parameter ids (see `ph-node` descriptors), so instruments can grow without breaking
/// saved projects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstrumentSpec {
    /// One-shot / loop sampler. `sample` is a project-relative path, an absolute path inside
    /// an allowed library folder, or `factory:<name>`.
    Sampler {
        sample: Option<String>,
        /// MIDI key that plays the sample at original pitch.
        root_key: u8,
        #[serde(default)]
        params: BTreeMap<String, f64>,
    },
    /// Haus Synth. `preset` names a factory or user preset; `params` override it.
    Synth {
        preset: Option<String>,
        #[serde(default)]
        params: BTreeMap<String, f64>,
    },
    /// Haus Sampler (M3). `library` points at an instrument file.
    MultiSampler {
        library: Option<String>,
        #[serde(default)]
        params: BTreeMap<String, f64>,
    },
    /// External plugin (M2). `state` is the plugin's opaque state, base64-encoded.
    Plugin {
        format: PluginFormat,
        /// CLAP id or VST3 class id.
        plugin_id: String,
        state: Option<String>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    Clap,
    Vst3,
}

/// A pattern holds notes for any number of channels, like FL Studio. The step sequencer is
/// a view of the same notes: a lit step is a note one [`STEP`] long starting on the step grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Pattern {
    pub id: PatternId,
    pub name: String,
    pub color: Color,
    /// Explicit length; `None` = last note end rounded up to a whole bar (min one bar).
    pub length: Option<Tick>,
    /// channel -> note id -> note. Ordered by id for stable saves; the engine sorts by time.
    pub notes: BTreeMap<ChannelId, BTreeMap<NoteId, Note>>,
}

/// The editable header of a pattern, i.e. everything except its notes. Patches on
/// [`ObjectPath::Pattern`] carry this.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PatternHeader {
    pub id: PatternId,
    pub name: String,
    pub color: Color,
    pub length: Option<Tick>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    pub id: NoteId,
    /// MIDI key. 60 = middle C (FL labels it C5).
    pub key: u8,
    pub start: Tick,
    pub length: Tick,
    /// 0.0..=1.0
    pub velocity: f32,
    /// -1.0..=1.0
    #[serde(default)]
    pub pan: f32,
    /// Fine pitch in cents.
    #[serde(default)]
    pub fine_pitch: f32,
    /// A muted note stays in the pattern but is not played or analysed. Added after format
    /// version 1; absent in older projects (so `false`), and omitted when `false` so projects
    /// without muted notes serialise exactly as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MixerInsert {
    pub name: String,
    pub color: Option<Color>,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub effects: Vec<EffectSlot>,
    /// Sends to other inserts. Every non-master insert also routes to master unless
    /// `route_to_master` is false.
    pub sends: Vec<Send>,
    pub route_to_master: bool,
    /// Insert whose output feeds this insert's sidechain input, if any.
    pub sidechain_from: Option<InsertId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Send {
    pub to: InsertId,
    pub level: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EffectSlot {
    pub effect: EffectSpec,
    #[serde(default)]
    pub bypass: bool,
    /// Dry/wet 0.0..=1.0.
    #[serde(default = "one")]
    pub mix: f32,
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectSpec {
    /// Built-in effect by id: `eq`, `compressor`, `limiter`, `reverb`, `delay`, `filter`,
    /// `saturator` in M1.
    Builtin {
        name: String,
        #[serde(default)]
        params: BTreeMap<String, f64>,
    },
    Plugin {
        format: PluginFormat,
        plugin_id: String,
        state: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlaylistTrack {
    pub name: String,
    pub color: Option<Color>,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Clip {
    pub id: ClipId,
    /// Playlist track index.
    pub track: u32,
    pub pattern: PatternId,
    pub start: Tick,
    pub length: Tick,
    /// Offset into the pattern where the clip begins playing.
    pub offset: Tick,
}

const DEFAULT_COLORS: [Color; 8] = [
    0x5f9ea0, 0xd2691e, 0x9acd32, 0xba55d3, 0xdaa520, 0x4682b4, 0xcd5c5c, 0x3cb371,
];

pub fn default_color(n: u32) -> Color {
    DEFAULT_COLORS[n as usize % DEFAULT_COLORS.len()]
}

impl Default for Project {
    fn default() -> Self {
        Self::empty("Untitled")
    }
}

impl Project {
    /// An empty project: master + inserts, playlist tracks, no channels, no patterns.
    /// New-project defaults (a first pattern, a starter kit) are the job of the
    /// `project.new` command, so they are recorded and undoable like any other edit.
    pub fn empty(title: &str) -> Self {
        let mixer = (0..=MIXER_INSERTS)
            .map(|i| MixerInsert {
                name: if i == 0 {
                    "Master".into()
                } else {
                    format!("Insert {i}")
                },
                color: None,
                volume: if i == 0 { 1.0 } else { 0.8 },
                pan: 0.0,
                muted: false,
                effects: Vec::new(),
                sends: Vec::new(),
                route_to_master: i != 0,
                sidechain_from: None,
            })
            .collect();
        let tracks = (1..=PLAYLIST_TRACKS)
            .map(|i| PlaylistTrack {
                name: format!("Track {i}"),
                color: None,
                muted: false,
            })
            .collect();
        Project {
            format_version: FORMAT_VERSION,
            settings: ProjectSettings {
                title: title.into(),
                bpm: 130.0,
                time_signature: TimeSignature::default(),
                swing: 0.0,
            },
            channels: Default::default(),
            channel_order: Default::default(),
            patterns: Default::default(),
            pattern_order: Default::default(),
            mixer: Arc::new(mixer),
            tracks: Arc::new(tracks),
            clips: Default::default(),
            next_ids: NextIds::default(),
        }
    }

    pub fn channel(&self, id: ChannelId) -> ModelResult<&Channel> {
        self.channels.get(&id).ok_or(ModelError::NoChannel(id))
    }

    pub fn pattern(&self, id: PatternId) -> ModelResult<&Pattern> {
        self.patterns
            .get(&id)
            .map(|p| &**p)
            .ok_or(ModelError::NoPattern(id))
    }

    pub fn insert(&self, id: InsertId) -> ModelResult<&MixerInsert> {
        self.mixer
            .get(id.0 as usize)
            .ok_or(ModelError::NoInsert(id))
    }

    pub fn clip(&self, id: ClipId) -> ModelResult<&Clip> {
        self.clips.get(&id).ok_or(ModelError::NoClip(id))
    }

    /// Channels in rack order.
    pub fn channels_in_order(&self) -> impl Iterator<Item = &Channel> {
        self.channel_order
            .iter()
            .filter_map(|id| self.channels.get(id))
    }

    /// Patterns in list order.
    pub fn patterns_in_order(&self) -> impl Iterator<Item = &Pattern> {
        self.pattern_order
            .iter()
            .filter_map(|id| self.patterns.get(id).map(|p| &**p))
    }

    /// Length of a pattern in ticks: explicit length, else last note end rounded up to a bar
    /// (minimum one bar).
    pub fn pattern_length(&self, id: PatternId) -> ModelResult<Tick> {
        let pat = self.pattern(id)?;
        if let Some(len) = pat.length {
            return Ok(len);
        }
        let bar = self.settings.time_signature.ticks_per_bar();
        let end = pat
            .notes
            .values()
            .flat_map(|m| m.values())
            .map(|n| n.start + n.length)
            .max()
            .unwrap_or(0);
        Ok(end.div_ceil(bar).max(1) * bar)
    }

    /// End of the last playlist clip, in ticks.
    pub fn song_length(&self) -> Tick {
        self.clips
            .values()
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(0)
    }
}

impl Pattern {
    pub fn header(&self) -> PatternHeader {
        PatternHeader {
            id: self.id,
            name: self.name.clone(),
            color: self.color,
            length: self.length,
        }
    }

    pub fn note_count(&self) -> usize {
        self.notes.values().map(|m| m.len()).sum()
    }
}

pub fn validate_note(n: &Note) -> ModelResult<()> {
    if n.key > 127 {
        return Err(ModelError::Invalid(format!("key {} > 127", n.key)));
    }
    if n.length == 0 {
        return Err(ModelError::Invalid("note length must be > 0".into()));
    }
    if !(0.0..=1.0).contains(&n.velocity) {
        return Err(ModelError::Invalid(format!(
            "velocity {} outside 0..1",
            n.velocity
        )));
    }
    if !(-1.0..=1.0).contains(&n.pan) {
        return Err(ModelError::Invalid(format!("pan {} outside -1..1", n.pan)));
    }
    Ok(())
}
