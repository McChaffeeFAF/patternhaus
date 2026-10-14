use crate::*;
use ph_core::{Project, Tick};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The side-effect port: everything a command can do outside the project data.
///
/// The app implements it on top of the engine, filesystem and UI; tests use [`NullHost`].
/// Keeping these behind a trait is what lets `ph-commands` stay below `ph-engine` and
/// `ph-ui` in the layering (DESIGN §2).
pub trait Host {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState>;

    fn transport_state(&self) -> TransportState;

    /// Render `project` offline (the oracle path) to a file.
    fn render(&mut self, project: &Project, request: &RenderRequest) -> CmdResult<RenderReport>;

    /// Resolves a user- or agent-supplied path, enforcing the file-access rules of DESIGN §5
    /// (project folder, sample/library folders, exports). Relative paths are relative to the
    /// project folder.
    fn resolve_path(&self, path: &str, access: FileAccess) -> CmdResult<PathBuf>;

    /// Copies `source` into the project (DESIGN §10) and returns the project-relative
    /// reference to use in the model (e.g. `samples/kick.wav`).
    ///
    /// The human may import from any path (the drop is the intent); an agent only from
    /// inside the allowed roots, checked like [`Host::resolve_path`]. The file is validated
    /// first, and a file already there with identical bytes is reused. Additive default:
    /// hosts without a filesystem refuse.
    fn import_file(
        &mut self,
        source: &std::path::Path,
        into: ImportKind,
        author: &Author,
    ) -> CmdResult<String> {
        let _ = (source, into, author);
        Err(CmdError::Host("this host cannot import files".into()))
    }

    /// Reads an audio file's format (M2: for the sample registry). `reference` is an asset
    /// reference as the engine resolves it (`samples/<file>`, `factory:<path>`, an allowed
    /// absolute path). Additive default: hosts that cannot read audio refuse.
    fn sample_info(&self, reference: &str) -> CmdResult<SampleInfo> {
        let _ = reference;
        Err(CmdError::Host("this host cannot read audio files".into()))
    }

    /// Analyses an audio file's tempo and transients (M2/T1: `sample.detect_bpm`,
    /// `audio.fit_to_tempo`, `slicer.detect`). `sensitivity` (0..=1, higher finds more) sets
    /// the onset list; the tempo always uses the default. Additive default: hosts that cannot
    /// read audio refuse.
    fn analyze_audio(&self, reference: &str, sensitivity: f32) -> CmdResult<AudioAnalysis> {
        let _ = (reference, sensitivity);
        Err(CmdError::Host("this host cannot analyse audio".into()))
    }

    /// Ask the UI to show something (`ui.*` commands). Headless hosts may ignore it.
    fn ui(&mut self, request: UiRequest) -> CmdResult<()>;

    /// A different project was opened from `dir`: from now on the project folder is `dir`, so
    /// saves, imports, exports and project-relative samples all go there, never to the folder
    /// the session started in. Called by `file.open` *before* the session switches projects.
    /// Additive default: hosts without a filesystem ignore it.
    fn set_project_dir(&mut self, dir: &std::path::Path) {
        let _ = dir;
    }

    /// The Patternhaus projects folder (`Documents/Patternhaus Projects`): where
    /// `file.import_flp` puts new projects by default, and a folder agents may write to.
    /// Additive default: hosts without one return `None`.
    fn projects_dir(&self) -> Option<PathBuf> {
        None
    }

    // ---- plugins (M2/T3). Additive defaults: hosts without plugin hosting refuse.

    /// Scans `paths` (folders or plugin files; empty = the standard plugin folders) for CLAP
    /// and VST3 plugins, each file in a helper process, and updates the plugin list.
    fn plugin_scan(&mut self, paths: &[PathBuf]) -> CmdResult<PluginScanResult> {
        let _ = paths;
        Err(no_plugins())
    }

    /// Every scanned plugin with its status.
    fn plugin_list(&self) -> CmdResult<Vec<PluginEntry>> {
        Err(no_plugins())
    }

    /// The parameters of the plugin at `target` in `project`: live values when it is running,
    /// otherwise its declared defaults with the spec's own values.
    fn plugin_params(&self, project: &Project, target: PluginTarget) -> CmdResult<PluginParams> {
        let _ = (project, target);
        Err(no_plugins())
    }

    /// The parameters a scanned plugin declares (its defaults), without adding it.
    fn plugin_describe(&self, format: ph_core::PluginFormat, id: &str) -> CmdResult<PluginParams> {
        let _ = (format, id);
        Err(no_plugins())
    }

    /// Writes the state of every plugin of `project` that names a `plugins/<file>` into `dir`
    /// (called by `file.save` after `project.json`). Returns the files written. Additive
    /// default: nothing to write.
    fn save_plugin_states(
        &mut self,
        project: &Project,
        dir: &std::path::Path,
    ) -> CmdResult<Vec<String>> {
        let _ = (project, dir);
        Ok(Vec::new())
    }

    /// Opens (or closes) the plugin's own editor window. Returns whether a UI took the request
    /// (headless hosts return false).
    fn plugin_editor(&mut self, target: PluginTarget, open: bool) -> CmdResult<bool> {
        let _ = (target, open);
        Err(no_plugins())
    }
}

fn no_plugins() -> CmdError {
    CmdError::Host("this host cannot load plugins".into())
}

/// Where a plugin sits in the project.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginTarget {
    /// A channel's instrument.
    Channel(ph_core::ChannelId),
    /// Effect `slot` (position in the chain) of mixer insert `insert`.
    Effect { insert: u32, slot: u32 },
}

impl std::fmt::Display for PluginTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginTarget::Channel(c) => write!(f, "channel {}", c.0),
            PluginTarget::Effect { insert, slot } => write!(f, "insert {insert} slot {slot}"),
        }
    }
}

/// A scanned plugin, as `plugin.list` shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginEntry {
    pub format: ph_core::PluginFormat,
    /// CLAP id or VST3 class id: what `plugin.add_*` takes.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// `instrument`, `effect` or `note_effect`.
    pub kind: String,
    pub has_editor: bool,
    /// `ok`, `isolate` (crashed; not loaded until M4 can run it out of process) or `blocked`.
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    pub path: String,
}

/// What `plugin.scan` found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginScanResult {
    pub plugins: Vec<PluginEntry>,
    /// Files that could not be scanned (blocked until they change), with why.
    pub failed: Vec<PluginScanFailure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginScanFailure {
    pub path: String,
    pub error: String,
}

/// One plugin parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginParam {
    /// The plugin's own parameter id (as text): what `plugin.set_param` takes.
    pub id: String,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// Current value (live from the running plugin when it runs).
    pub value: f64,
    /// The plugin's own text for the value, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub stepped: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    pub automatable: bool,
}

/// A plugin's parameters at a target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginParams {
    /// Where it sits; `None` when describing a scanned plugin that is not in the project.
    pub target: Option<PluginTarget>,
    pub format: ph_core::PluginFormat,
    pub plugin_id: String,
    pub name: String,
    /// Read from the running instance (false: declared defaults plus the project's values).
    pub live: bool,
    /// Latency in samples the plugin reports.
    pub latency: u32,
    pub has_editor: bool,
    pub params: Vec<PluginParam>,
}

/// An audio file's format, from [`Host::sample_info`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleInfo {
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
    /// Root key stored in the file (WAV `smpl`), if any.
    pub root_key: Option<u8>,
}

/// Tempo and transients of an audio file, from [`Host::analyze_audio`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioAnalysis {
    /// Estimated tempo, 60..=200 BPM; `None` when the material has no clear beat.
    pub bpm: Option<f64>,
    /// 0..1: how sure the estimate is (above about 0.5 is reliable).
    pub confidence: f64,
    /// The sample is a loop of this many beats at `bpm`.
    pub loop_beats: Option<u32>,
    /// Half and double tempo readings, with scores relative to `bpm` (1.0 = as good).
    pub alternatives: Vec<(f64, f64)>,
    /// Onsets (transients) in frames of the file.
    pub onsets: Vec<u64>,
    /// The file's sample rate (to turn frames into seconds).
    pub sample_rate: u32,
    /// Version key of the analysis (stored in `SampleEntry::analysis.onsets`).
    pub key: String,
}

/// What [`Host::import_file`] brings into the project folder.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    /// An audio file, validated by decoding and copied into `samples/`.
    Sample,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileAccess {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TransportCmd {
    Play,
    Stop,
    /// Toggle play/stop (space bar).
    Toggle,
    Record,
    /// Jump to a song position (ticks).
    Seek {
        position: Tick,
    },
    /// Pattern mode loops `pattern` (resolved by the command from the project, so the host
    /// never needs a possibly stale snapshot); song mode plays the playlist.
    SetMode {
        mode: TransportMode,
        #[serde(default)]
        pattern: Option<ph_core::PatternId>,
    },
    /// Loop region in song mode; `None` clears it.
    SetLoop {
        range: Option<(Tick, Tick)>,
    },
    SetMetronome {
        on: bool,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    Pattern,
    Song,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransportState {
    pub playing: bool,
    pub recording: bool,
    pub mode: TransportMode,
    /// Pattern played in pattern mode.
    pub pattern: Option<ph_core::PatternId>,
    pub position: Tick,
    pub loop_range: Option<(Tick, Tick)>,
    pub metronome: bool,
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            playing: false,
            recording: false,
            mode: TransportMode::Pattern,
            pattern: None,
            position: 0,
            loop_range: None,
            metronome: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderRequest {
    /// Output file; extension picks the format (`.wav`, `.flac`).
    pub path: String,
    /// `None` = whole song (or the current pattern in pattern mode).
    pub range: Option<(Tick, Tick)>,
    #[serde(default = "default_sr")]
    pub sample_rate: u32,
    #[serde(default = "default_bits")]
    pub bit_depth: u16,
    /// HQ export mode (DESIGN §4).
    #[serde(default = "yes")]
    pub high_quality: bool,
    /// Render reverb/delay tails past the end.
    #[serde(default = "yes")]
    pub include_tail: bool,
}

fn default_sr() -> u32 {
    48_000
}
fn default_bits() -> u16 {
    24
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderReport {
    pub path: PathBuf,
    pub seconds: f64,
    pub peak_dbfs: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UiRequest {
    /// Open/focus a panel: `browser`, `channel_rack`, `piano_roll`, `playlist`, `mixer`, `history`.
    Open {
        panel: String,
    },
    Close {
        panel: String,
    },
    /// Select objects so the human sees what an agent refers to.
    Select {
        paths: Vec<ph_core::ObjectPath>,
    },
    /// Show the piano roll for a pattern and channel.
    PianoRoll {
        pattern: ph_core::PatternId,
        channel: ph_core::ChannelId,
    },
    Notify {
        message: String,
    },
    /// Open or close a plugin's own editor window (M2/T3, `plugin.open_editor`).
    PluginEditor {
        target: PluginTarget,
        open: bool,
    },
}

/// A host that does nothing; for tests and pure model scripting.
#[derive(Default)]
pub struct NullHost {
    pub transport: TransportState,
}

impl Host for NullHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        match cmd {
            TransportCmd::Play => self.transport.playing = true,
            TransportCmd::Stop => self.transport.playing = false,
            TransportCmd::Toggle => self.transport.playing = !self.transport.playing,
            TransportCmd::Record => self.transport.recording = !self.transport.recording,
            TransportCmd::Seek { position } => self.transport.position = position,
            TransportCmd::SetMode { mode, pattern } => {
                self.transport.mode = mode;
                self.transport.pattern = pattern;
            }
            TransportCmd::SetLoop { range } => self.transport.loop_range = range,
            TransportCmd::SetMetronome { on } => self.transport.metronome = on,
        }
        Ok(self.transport.clone())
    }

    fn transport_state(&self) -> TransportState {
        self.transport.clone()
    }

    fn render(&mut self, _: &Project, _: &RenderRequest) -> CmdResult<RenderReport> {
        Err(CmdError::Host("this host cannot render".into()))
    }

    fn resolve_path(&self, _: &str, _: FileAccess) -> CmdResult<PathBuf> {
        Err(CmdError::Host("this host has no file access".into()))
    }

    fn ui(&mut self, _: UiRequest) -> CmdResult<()> {
        Ok(())
    }
}
