//! The Haus Sampler instrument model (M3/T0).
//!
//! A [`HausInstrument`] is a multi-sample instrument: [`Zone`]s (one sample each, mapped to a
//! key and velocity range) collected in [`Group`]s (shared envelopes, filter, LFOs, voice
//! limits, round robin, output bus, effects), plus [`Articulation`]s (keyswitches), a
//! [modulation matrix](ModRoute) and global [`InstrumentSettings`].
//!
//! # Where it lives
//!
//! The model is project data, so it is undoable, patchable and editable by agents through
//! `ph-commands` like everything else. Instruments sit in [`Project::instruments`]; a channel
//! plays one through [`InstrumentSpec::MultiSampler`]`{ instrument: Some(id) }`. Like an
//! automation clip's points, an instrument's groups and zones are **their own objects**:
//!
//! | object | path | value |
//! |---|---|---|
//! | instrument | `instrument/<i>` | [`HausInstrument`] without groups and zones |
//! | group | `instrument/<i>/group/<g>` | [`Group`] |
//! | zone | `instrument/<i>/zone/<z>` | [`Zone`] |
//!
//! so moving one zone is one small patch, and two authors editing different zones never
//! conflict. Group and zone ids are unique across the project and never reused.
//!
//! # Units
//!
//! Keys are MIDI keys (60 = C4). Velocity ranges are MIDI velocities 0..=127, inclusive; the
//! engine's 0..1 velocity maps to `round(v × 127)`. Times are seconds, levels decibels
//! (`*_db`), pitch offsets semitones (`tune`) and cents (`fine`, modulation depths), pan
//! -1..=1, sample positions frames **of the sample file** at its own rate, controller values
//! normalised 0..=1 (MIDI value / 127).
//!
//! # Invariants (enforced by [`crate::set`])
//!
//! - a zone's group belongs to the same instrument; a zone's registry sample exists;
//! - a group's articulations and a modulation route's group exist in its instrument;
//! - an instrument is deleted only when it has no groups or zones and no channel plays it; a
//!   group only when no zone or route uses it; a registry sample only when no zone plays it;
//! - ranges are ordered (`lo <= hi`), loops lie inside the played range, levels are finite.

use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version of the instrument model. Saved in native instrument files (`instrument.toml`
/// `version`, see `docs/formats/haus-instrument.md`); projects carry the model under the
/// project's own `format_version`.
pub const INSTRUMENT_MODEL_VERSION: u32 = 1;

/// Most output buses an instrument may use: the main output (0) plus 15 extra stereo outs.
pub const MAX_OUTPUT_BUSES: u32 = 16;

/// A Haus Sampler instrument. See the module docs for the object layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HausInstrument {
    pub id: InstrumentId,
    pub name: String,
    /// The folder that [`SampleRef::File`] paths resolve against: a project-relative path
    /// (`instruments/<name>`, after "collect into project"), or an absolute path inside an
    /// allowed library folder (a linked library). `None` = the project folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default)]
    pub settings: InstrumentSettings,
    /// Keyswitched / CC-switched articulations. Empty = no articulations: every group plays.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub articulations: Vec<Articulation>,
    /// The modulation matrix, applied in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modulation: Vec<ModRoute>,
    /// Where the instrument came from. `None` = made in Patternhaus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<InstrumentSource>,
    /// What an importer approximated, skipped or kept without using.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_notes: Vec<ImportNote>,
    /// Opaque data importers keep for later milestones (M3b: scripts, skins), keyed
    /// `<format>.<what>` (e.g. `ksp.script`, `dspreset.ui`). Never interpreted in M3.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
    /// Groups by id. Separate objects (`instrument/<i>/group/<g>`): never inside the
    /// instrument's own patch value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<GroupId, Group>,
    /// Zones by id. Separate objects (`instrument/<i>/zone/<z>`): never inside the
    /// instrument's own patch value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub zones: BTreeMap<ZoneId, Zone>,
}

/// Whole-instrument settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct InstrumentSettings {
    /// Output level, dB (-144..=24).
    pub volume_db: f32,
    /// -1..=1.
    pub pan: f32,
    /// Whole-instrument transpose, semitones (-48..=48).
    pub transpose: i32,
    /// Whole-instrument fine tune, cents (-100..=100).
    pub tune_cents: f32,
    /// Most voices at once across all groups (1..=1024).
    pub voice_limit: u32,
    /// Pitch-bend range up and down, semitones (0..=48).
    pub bend_up: f32,
    pub bend_down: f32,
    /// The articulation active before any keyswitch is played. `None` = the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_articulation: Option<ArticulationId>,
    /// Frames of each sample held in memory for instant starts (the rest streams from
    /// disk). `None` = the engine default (`ph_sampler::DEFAULT_PRELOAD_FRAMES`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preload_frames: Option<u32>,
}

impl Default for InstrumentSettings {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            pan: 0.0,
            transpose: 0,
            tune_cents: 0.0,
            voice_limit: 256,
            bend_up: 2.0,
            bend_down: 2.0,
            default_articulation: None,
            preload_frames: None,
        }
    }
}

/// Provenance of an imported instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InstrumentSource {
    /// `native`, `sfz`, `sf2`, `sf3`, `dspreset`, `exs`, `nki`, `nkm`, ...
    pub format: String,
    /// The file it was read from, as given to the importer.
    pub file: String,
    /// Version of the importer that read it (bumped when an importer changes what it maps).
    pub importer_version: u32,
}

/// One thing an importer could not carry over exactly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportNote {
    pub kind: ImportNoteKind,
    /// What it concerns: an opcode, chunk or feature name (`opcode:amp_velcurve_64`).
    pub what: String,
    /// Plain words for a musician.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// How many times it occurred (1 = once).
    #[serde(default = "one_u32")]
    pub count: u32,
}

fn one_u32() -> u32 {
    1
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportNoteKind {
    /// Mapped to the nearest thing the sampler has.
    Approximated,
    /// Dropped.
    Skipped,
    /// Kept as data but not used yet (scripts and UI until M3b).
    Kept,
}

/// A keyswitched or controller-switched articulation (sustain, staccato, ...). Groups join
/// articulations through [`Group::articulations`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Articulation {
    pub id: ArticulationId,
    pub name: String,
    pub trigger: ArticulationTrigger,
    /// Colour of its key on the keyboard strip, 0xRRGGBB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

/// What selects an articulation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArticulationTrigger {
    /// A keyswitch: playing `key` selects it (latching). The key itself makes no sound.
    Key { key: u8 },
    /// A controller in `lo..=hi` (normalised 0..=1) selects it.
    Cc { controller: u8, lo: f32, hi: f32 },
}

/// One row of the modulation matrix: `target += depth × curve(source)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModRoute {
    pub source: ModSource,
    pub target: ModTarget,
    /// In the target's unit at full source (see [`ModTarget`]).
    pub depth: f32,
    #[serde(default)]
    pub curve: ModCurve,
    /// Only voices of this group. `None` = every group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
}

/// A modulation source. Unipolar sources run 0..=1, bipolar ones -1..=1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModSource {
    /// Note velocity, 0..=1.
    Velocity,
    /// Note key, (key - 60) / 64: -0.94..=1.05, bipolar around C4.
    Key,
    /// A MIDI controller, 0..=1.
    Cc { controller: u8 },
    /// Channel pitch bend, -1..=1.
    PitchBend,
    /// Channel pressure, 0..=1.
    Aftertouch,
    /// Polyphonic pressure of the voice's note, 0..=1.
    PolyPressure,
    /// The group's LFO number `index` (0-based), -1..=1.
    Lfo { index: u8 },
    /// The group's amp envelope (0) or filter envelope (1), 0..=1.
    Envelope { index: u8 },
    /// A random value drawn once per note, 0..=1 (seeded: see `ph_sampler`).
    Random,
}

/// A modulation target and the unit of [`ModRoute::depth`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModTarget {
    /// Cents.
    Pitch,
    /// Decibels.
    Volume,
    /// Pan units (-1..=1 is the full width).
    Pan,
    /// Filter cutoff, cents.
    Cutoff,
    /// Filter resonance, dB.
    Resonance,
    /// Sample start offset, frames of the file.
    SampleStart,
}

/// How a source value is shaped before scaling by depth.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModCurve {
    #[default]
    Linear,
    /// x² (slow start).
    Convex,
    /// √x (fast start).
    Concave,
    /// 0 below one half, 1 from one half.
    Switch,
}

/// Inclusive key range.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyRange {
    pub lo: u8,
    pub hi: u8,
}

/// Inclusive MIDI velocity range, 0..=127.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VelRange {
    pub lo: u8,
    pub hi: u8,
}

impl KeyRange {
    pub const ALL: KeyRange = KeyRange { lo: 0, hi: 127 };
    pub fn contains(&self, key: u8) -> bool {
        (self.lo..=self.hi).contains(&key)
    }
}

impl VelRange {
    pub const ALL: VelRange = VelRange { lo: 0, hi: 127 };
    pub fn contains(&self, vel: u8) -> bool {
        (self.lo..=self.hi).contains(&vel)
    }
}

impl Default for KeyRange {
    fn default() -> Self {
        Self::ALL
    }
}

impl Default for VelRange {
    fn default() -> Self {
        Self::ALL
    }
}

/// Where a zone's audio comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SampleRef {
    /// An entry of the project's sample registry (`sample/<id>`).
    Registry(SampleId),
    /// A path relative to the instrument's [`HausInstrument::folder`] (forward slashes),
    /// e.g. `samples/piano_C4_f.wav`.
    File(String),
}

/// One mapped sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Zone {
    pub id: ZoneId,
    /// The group it belongs to (same instrument).
    pub group: GroupId,
    pub sample: SampleRef,
    #[serde(default)]
    pub keys: KeyRange,
    #[serde(default)]
    pub velocity: VelRange,
    /// The key that plays the sample at its recorded pitch.
    pub root_key: u8,
    /// Semitones, -48..=48.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub tune: i32,
    /// Cents, -100..=100.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub fine: f32,
    /// dB, -144..=24.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub volume_db: f32,
    /// -1..=1.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub pan: f32,
    /// First frame played.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub start: u64,
    /// One past the last frame played. `None` = the end of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "loop")]
    pub looping: Option<ZoneLoop>,
    /// Plays the (start..end) range backwards. Loops still run forwards in the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reverse: bool,
    #[serde(default)]
    pub trigger: ZoneTrigger,
    /// Ignores note-off: the zone plays to its end (drums).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub one_shot: bool,
    /// Release trigger only: level falls by this many dB per second the note was held
    /// (0 = no decay).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub release_decay_db: f32,
    /// Position in its group's round-robin cycle, 0-based. `None` = plays every time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_robin: Option<u32>,
    /// Plays only while every condition holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc_conditions: Vec<CcCondition>,
    /// Velocity and controller crossfades (layer blending).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crossfades: Vec<Crossfade>,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// When a zone sounds.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZoneTrigger {
    /// On note-on.
    #[default]
    Attack,
    /// On note-off (release samples), with the note's velocity.
    Release,
}

/// A sample loop, in frames of the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ZoneLoop {
    #[serde(default)]
    pub mode: LoopMode,
    pub start: u64,
    /// One past the last looped frame.
    pub end: u64,
    /// Crossfade length before `end`, frames (0 = none).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub crossfade: u64,
    /// What happens at note-off: keep looping through the release (`continue`), or leave
    /// the loop and play on to the zone's end (`exit`, a sustain loop).
    #[serde(default)]
    pub on_release: LoopRelease,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Forward,
    PingPong,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopRelease {
    #[default]
    Continue,
    Exit,
}

/// A controller condition: the zone plays while `controller` is in `lo..=hi` (0..=1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CcCondition {
    pub controller: u8,
    pub lo: f32,
    pub hi: f32,
}

/// A crossfade over velocity or a controller: gain rises from 0 at `in_lo` to 1 at `in_hi`
/// and falls from 1 at `out_lo` to 0 at `out_hi` (all 0..=1, `in_lo <= in_hi <= out_lo <=
/// out_hi`; equal ends mean no fade on that side).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Crossfade {
    pub source: CrossfadeSource,
    pub in_lo: f32,
    pub in_hi: f32,
    pub out_lo: f32,
    pub out_hi: f32,
    #[serde(default)]
    pub curve: CrossfadeCurve,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrossfadeSource {
    Velocity,
    Cc { controller: u8 },
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrossfadeCurve {
    /// Equal power (constant loudness).
    #[default]
    Power,
    /// Linear gain.
    Gain,
}

/// Settings shared by a set of zones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    #[serde(default)]
    pub color: Color,
    #[serde(default)]
    pub amp_env: Envelope,
    /// Drives the filter cutoff by [`Filter::env_depth`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_env: Option<Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    /// LFOs; routed through the modulation matrix ([`ModSource::Lfo`]). At most 4.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lfos: Vec<Lfo>,
    /// Most voices this group plays at once. `None` = only the instrument's limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_limit: Option<u32>,
    #[serde(default)]
    pub stealing: StealMode,
    #[serde(default)]
    pub polyphony: Polyphony,
    /// Glide time between notes of a mono or legato group, seconds (0 = off).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub portamento: f32,
    #[serde(default)]
    pub round_robin: RoundRobinMode,
    /// Output bus: 0 = the main output, 1.. = extra outputs (multi-out), below
    /// [`MAX_OUTPUT_BUSES`].
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub output: u32,
    /// Insert chain of built-in effects, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<GroupEffect>,
    /// The articulations this group plays in. Empty = all of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub articulations: Vec<ArticulationId>,
    /// Group-level volume, dB (-144..=24).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub volume_db: f32,
    /// Opaque importer data for M3b (script text, UI controls), as on the instrument.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// A DAHDSR envelope. Times in seconds (0..=60), sustain level 0..=1 (linear).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Envelope {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            delay: 0.0,
            attack: 0.0,
            hold: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.001,
        }
    }
}

/// A group filter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Filter {
    pub kind: FilterKind,
    /// Hz, 10..=22000.
    pub cutoff: f32,
    /// dB, 0..=40.
    #[serde(default)]
    pub resonance: f32,
    /// Cutoff follows the key: cents per key above C4 (0..=1200; 100 = full tracking).
    #[serde(default)]
    pub key_track: f32,
    /// Filter-envelope depth, cents (-9600..=9600).
    #[serde(default)]
    pub env_depth: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    LowPass12,
    LowPass24,
    HighPass12,
    HighPass24,
    BandPass,
    Notch,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Lfo {
    #[serde(default)]
    pub shape: LfoShape,
    /// Hz, 0.01..=100.
    pub rate: f32,
    /// Seconds before it starts.
    #[serde(default)]
    pub delay: f32,
    /// Seconds to fade in after the delay.
    #[serde(default)]
    pub fade: f32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Square,
    SawUp,
    SawDown,
    /// Sample and hold (seeded).
    Random,
}

/// Which voice gives way when a limit is reached.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StealMode {
    /// The voice that started first.
    #[default]
    Oldest,
    /// The quietest voice.
    Quietest,
    /// The newest voice.
    Newest,
    /// None: the new note does not sound.
    None,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Polyphony {
    #[default]
    Poly,
    /// One note at a time; a new note retriggers.
    Mono,
    /// One note at a time; an overlapping note glides without retriggering.
    Legato,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoundRobinMode {
    /// Positions 0, 1, 2, ... in turn.
    #[default]
    Sequential,
    /// A seeded random position, never the same one twice in a row when there are two or
    /// more.
    Random,
}

/// A built-in effect in a group's insert chain (same ids and parameters as mixer effects:
/// `EffectSpec::Builtin`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroupEffect {
    /// Built-in effect id (`eq`, `compressor`, `reverb`, ...).
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    /// Dry/wet 0..=1.
    #[serde(default = "one_f32")]
    pub mix: f32,
}

fn one_f32() -> f32 {
    1.0
}

// ------------------------------------------------------------------------- constructors

impl HausInstrument {
    /// An empty instrument (add groups and zones with the `Edit` helpers).
    pub fn new(name: &str) -> Self {
        Self {
            id: InstrumentId(0),
            name: name.into(),
            folder: None,
            settings: InstrumentSettings::default(),
            articulations: Vec::new(),
            modulation: Vec::new(),
            source: None,
            import_notes: Vec::new(),
            extra: BTreeMap::new(),
            groups: BTreeMap::new(),
            zones: BTreeMap::new(),
        }
    }

    /// The instrument's own patch value: everything but its groups and zones.
    pub fn header(&self) -> HausInstrument {
        let mut h = self.clone();
        h.groups.clear();
        h.zones.clear();
        h
    }

    pub fn group(&self, id: GroupId) -> ModelResult<&Group> {
        self.groups.get(&id).ok_or(ModelError::NoGroup(id))
    }

    pub fn zone(&self, id: ZoneId) -> ModelResult<&Zone> {
        self.zones.get(&id).ok_or(ModelError::NoZone(id))
    }

    pub fn articulation(&self, id: ArticulationId) -> Option<&Articulation> {
        self.articulations.iter().find(|a| a.id == id)
    }

    /// The lowest articulation id not in use.
    pub fn next_articulation_id(&self) -> ArticulationId {
        let mut n = 0;
        while self.articulations.iter().any(|a| a.id.0 == n) {
            n += 1;
        }
        ArticulationId(n)
    }

    /// Zones in id order that belong to `group`.
    pub fn zones_of(&self, group: GroupId) -> impl Iterator<Item = &Zone> {
        self.zones.values().filter(move |z| z.group == group)
    }

    /// The highest output bus any group uses (0 = only the main output).
    pub fn max_output(&self) -> u32 {
        self.groups.values().map(|g| g.output).max().unwrap_or(0)
    }

    /// The asset reference a zone's sample resolves to (what `AssetResolver` takes): a
    /// registry entry's path, or the instrument folder joined with the zone's file path.
    /// `None` for a missing registry entry.
    pub fn sample_reference(&self, project: &Project, sample: &SampleRef) -> Option<String> {
        match sample {
            SampleRef::Registry(id) => project.samples.get(id).map(|e| e.path.clone()),
            SampleRef::File(path) => Some(match &self.folder {
                Some(dir) if !dir.is_empty() => {
                    format!("{}/{}", dir.trim_end_matches(['/', '\\']), path)
                }
                _ => path.clone(),
            }),
        }
    }
}

impl Group {
    /// A group with default settings (`id` is assigned by `Edit::add_group`).
    pub fn new(name: &str) -> Self {
        Self {
            id: GroupId(0),
            name: name.into(),
            color: 0,
            amp_env: Envelope::default(),
            filter_env: None,
            filter: None,
            lfos: Vec::new(),
            voice_limit: None,
            stealing: StealMode::default(),
            polyphony: Polyphony::default(),
            portamento: 0.0,
            round_robin: RoundRobinMode::default(),
            output: 0,
            effects: Vec::new(),
            articulations: Vec::new(),
            volume_db: 0.0,
            extra: BTreeMap::new(),
        }
    }
}

impl Zone {
    /// A zone over `keys` at full velocity range, with default settings (`id` is assigned by
    /// `Edit::add_zone`).
    pub fn new(group: GroupId, sample: SampleRef, keys: KeyRange, root_key: u8) -> Self {
        Self {
            id: ZoneId(0),
            group,
            sample,
            keys,
            velocity: VelRange::ALL,
            root_key,
            tune: 0,
            fine: 0.0,
            volume_db: 0.0,
            pan: 0.0,
            start: 0,
            end: None,
            looping: None,
            reverse: false,
            trigger: ZoneTrigger::Attack,
            one_shot: false,
            release_decay_db: 0.0,
            round_robin: None,
            cc_conditions: Vec::new(),
            crossfades: Vec::new(),
        }
    }

    /// Plays `key` at MIDI velocity `vel`, ignoring controller conditions.
    pub fn maps(&self, key: u8, vel: u8) -> bool {
        self.keys.contains(key) && self.velocity.contains(vel)
    }
}

/// MIDI velocity (0..=127) for an engine velocity (0..=1): `round(v × 127)`.
pub fn midi_velocity(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 127.0).round() as u8
}

// ------------------------------------------------------------------------- validation

fn bad<T>(msg: String) -> ModelResult<T> {
    Err(ModelError::Invalid(msg))
}

fn check(name: &str, v: f32, lo: f32, hi: f32) -> ModelResult<()> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        bad(format!("{name} {v} outside {lo}..{hi}"))
    }
}

fn check_key(name: &str, k: u8) -> ModelResult<()> {
    if k <= 127 {
        Ok(())
    } else {
        bad(format!("{name} {k} > 127"))
    }
}

fn check_range(name: &str, lo: u8, hi: u8) -> ModelResult<()> {
    check_key(name, lo)?;
    check_key(name, hi)?;
    if lo > hi {
        return bad(format!("{name} {lo}..{hi} is reversed"));
    }
    Ok(())
}

impl Envelope {
    pub fn validate(&self, name: &str) -> ModelResult<()> {
        for (n, v) in [
            ("delay", self.delay),
            ("attack", self.attack),
            ("hold", self.hold),
            ("decay", self.decay),
            ("release", self.release),
        ] {
            check(&format!("{name}.{n}"), v, 0.0, 60.0)?;
        }
        check(&format!("{name}.sustain"), self.sustain, 0.0, 1.0)
    }
}

/// Checks a zone on its own (the group and sample references are checked by the model).
pub fn validate_zone(z: &Zone) -> ModelResult<()> {
    check_range("keys", z.keys.lo, z.keys.hi)?;
    check_range("velocity", z.velocity.lo, z.velocity.hi)?;
    check_key("root_key", z.root_key)?;
    if !(-48..=48).contains(&z.tune) {
        return bad(format!("tune {} outside -48..48", z.tune));
    }
    check("fine", z.fine, -100.0, 100.0)?;
    check("volume_db", z.volume_db, -144.0, 24.0)?;
    check("pan", z.pan, -1.0, 1.0)?;
    check("release_decay_db", z.release_decay_db, 0.0, 200.0)?;
    if let SampleRef::File(p) = &z.sample
        && p.is_empty()
    {
        return bad("empty sample path".into());
    }
    if let Some(end) = z.end
        && end <= z.start
    {
        return bad(format!("end {end} is not after start {}", z.start));
    }
    if let Some(l) = &z.looping {
        if l.end <= l.start {
            return bad(format!(
                "loop end {} is not after loop start {}",
                l.end, l.start
            ));
        }
        if l.start < z.start || z.end.is_some_and(|e| l.end > e) {
            return bad("the loop lies outside the zone's start..end".into());
        }
        if l.crossfade > l.end - l.start {
            return bad("loop crossfade is longer than the loop".into());
        }
    }
    for c in &z.cc_conditions {
        check_key("cc controller", c.controller)?;
        check("cc lo", c.lo, 0.0, 1.0)?;
        check("cc hi", c.hi, 0.0, 1.0)?;
        if c.lo > c.hi {
            return bad("cc condition range is reversed".into());
        }
    }
    for x in &z.crossfades {
        if let CrossfadeSource::Cc { controller } = x.source {
            check_key("crossfade controller", controller)?;
        }
        for v in [x.in_lo, x.in_hi, x.out_lo, x.out_hi] {
            check("crossfade point", v, 0.0, 1.0)?;
        }
        if !(x.in_lo <= x.in_hi && x.in_hi <= x.out_lo && x.out_lo <= x.out_hi) {
            return bad("crossfade points must rise: in_lo <= in_hi <= out_lo <= out_hi".into());
        }
    }
    Ok(())
}

/// Checks a group on its own (articulation references are checked by the model).
pub fn validate_group(g: &Group) -> ModelResult<()> {
    g.amp_env.validate("amp_env")?;
    if let Some(e) = &g.filter_env {
        e.validate("filter_env")?;
    }
    if let Some(f) = &g.filter {
        check("filter.cutoff", f.cutoff, 10.0, 22_000.0)?;
        check("filter.resonance", f.resonance, 0.0, 40.0)?;
        check("filter.key_track", f.key_track, 0.0, 1200.0)?;
        check("filter.env_depth", f.env_depth, -9600.0, 9600.0)?;
    }
    if g.lfos.len() > 4 {
        return bad(format!("{} LFOs, at most 4", g.lfos.len()));
    }
    for l in &g.lfos {
        check("lfo.rate", l.rate, 0.01, 100.0)?;
        check("lfo.delay", l.delay, 0.0, 60.0)?;
        check("lfo.fade", l.fade, 0.0, 60.0)?;
    }
    if g.voice_limit == Some(0) {
        return bad("voice_limit must be at least 1".into());
    }
    check("portamento", g.portamento, 0.0, 10.0)?;
    check("volume_db", g.volume_db, -144.0, 24.0)?;
    if g.output >= MAX_OUTPUT_BUSES {
        return bad(format!("output {} >= {MAX_OUTPUT_BUSES}", g.output));
    }
    for e in &g.effects {
        if e.name.is_empty() {
            return bad("group effect without a name".into());
        }
        check("effect mix", e.mix, 0.0, 1.0)?;
    }
    let mut seen = std::collections::BTreeSet::new();
    if !g.articulations.iter().all(|a| seen.insert(*a)) {
        return bad("an articulation is listed twice".into());
    }
    Ok(())
}

/// Checks an instrument's own settings (not its groups and zones).
pub fn validate_instrument_header(i: &HausInstrument) -> ModelResult<()> {
    let s = &i.settings;
    check("volume_db", s.volume_db, -144.0, 24.0)?;
    check("pan", s.pan, -1.0, 1.0)?;
    if !(-48..=48).contains(&s.transpose) {
        return bad(format!("transpose {} outside -48..48", s.transpose));
    }
    check("tune_cents", s.tune_cents, -100.0, 100.0)?;
    if !(1..=1024).contains(&s.voice_limit) {
        return bad(format!("voice_limit {} outside 1..1024", s.voice_limit));
    }
    check("bend_up", s.bend_up, 0.0, 48.0)?;
    check("bend_down", s.bend_down, 0.0, 48.0)?;
    let mut ids = std::collections::BTreeSet::new();
    for a in &i.articulations {
        if !ids.insert(a.id) {
            return bad(format!("articulation {} twice", a.id));
        }
        match &a.trigger {
            ArticulationTrigger::Key { key } => check_key("keyswitch", *key)?,
            ArticulationTrigger::Cc { controller, lo, hi } => {
                check_key("articulation controller", *controller)?;
                check("articulation lo", *lo, 0.0, 1.0)?;
                check("articulation hi", *hi, 0.0, 1.0)?;
                if lo > hi {
                    return bad("articulation controller range is reversed".into());
                }
            }
        }
    }
    if let Some(d) = s.default_articulation
        && !ids.contains(&d)
    {
        return bad(format!("default articulation {d} does not exist"));
    }
    for r in &i.modulation {
        if !r.depth.is_finite() {
            return bad("modulation depth is not a number".into());
        }
        if let ModSource::Cc { controller } = r.source {
            check_key("modulation controller", controller)?;
        }
        if let ModSource::Lfo { index } = r.source
            && index >= 4
        {
            return bad(format!("lfo {index}: at most 4 per group"));
        }
        if let ModSource::Envelope { index } = r.source
            && index >= 2
        {
            return bad(format!("envelope {index}: 0 = amp, 1 = filter"));
        }
    }
    Ok(())
}

/// The modulation routes and groups that refer to group/articulation ids, for the model's
/// reference checks.
pub(crate) fn instrument_refs_ok(i: &HausInstrument) -> Result<(), String> {
    for r in &i.modulation {
        if let Some(g) = r.group
            && !i.groups.contains_key(&g)
        {
            return Err(format!("a modulation route names missing group {g}"));
        }
    }
    for g in i.groups.values() {
        for a in &g.articulations {
            if i.articulation(*a).is_none() {
                return Err(format!("group {} plays missing articulation {a}", g.id));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------- project access

impl Project {
    pub fn instrument(&self, id: InstrumentId) -> ModelResult<&HausInstrument> {
        self.instruments
            .get(&id)
            .ok_or(ModelError::NoInstrument(id))
    }

    /// The channels that play an instrument.
    pub fn channels_playing(&self, id: InstrumentId) -> impl Iterator<Item = &Channel> {
        self.channels.values().filter(move |c| {
            matches!(c.instrument, InstrumentSpec::MultiSampler { instrument: Some(i), .. } if i == id)
        })
    }
}
