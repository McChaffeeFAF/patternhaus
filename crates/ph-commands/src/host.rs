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

    /// Ask the UI to show something (`ui.*` commands). Headless hosts may ignore it.
    fn ui(&mut self, request: UiRequest) -> CmdResult<()>;
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
