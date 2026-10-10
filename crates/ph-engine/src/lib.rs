//! Patternhaus audio engine — **public API contract (T0)**. Bodies are implemented in T3.
//!
//! - The control thread hands the engine `Arc<Project>` snapshots. The engine compiles each
//!   into a render plan *off* the audio thread and swaps it in lock-free; old plans are freed
//!   on a non-real-time thread (DESIGN §4).
//! - Offline rendering runs the same plan and node code with a fake clock. It is the test
//!   oracle: real-time output through [`Engine::start_simulated`] must match it exactly.
//! - The engine never depends on `ph-commands`; the app adapts it to the `Host` port.

use ph_core::{ObjectPath, PatternId, Project, Tick};
use ph_node::{AssetResolver, Quality};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("audio device error: {0}")]
    Device(String),
    #[error("could not build render plan: {0}")]
    Plan(String),
    #[error("asset error: {0}")]
    Asset(String),
}

pub type EngineResult<T> = Result<T, EngineError>;

#[derive(Clone, Debug, PartialEq)]
pub struct EngineConfig {
    /// Output device name; `None` = system default.
    pub device: Option<String>,
    pub sample_rate: u32,
    /// Device buffer size in frames (DESIGN §4: adaptive, default 256).
    pub buffer_frames: u32,
    /// Worker threads for the graph; `0` = physical cores − 1.
    pub threads: usize,
    /// Use WASAPI exclusive mode / ASIO when available.
    pub exclusive: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48_000,
            buffer_frames: 256,
            threads: 0,
            exclusive: false,
        }
    }
}

/// Non-interleaved audio.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    /// `[channel][frame]`
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, |c| c.len())
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PlayMode {
    /// Loop one pattern (FL "pattern mode").
    Pattern(PatternId),
    /// Play the playlist.
    Song,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    pub mode: PlayMode,
    /// `None` = whole song / one pass of the pattern.
    pub range: Option<(Tick, Tick)>,
    pub sample_rate: u32,
    pub quality: Quality,
    /// Keep rendering until reverb/delay tails decay below -96 dBFS (capped at 10 s).
    pub include_tail: bool,
    /// Block size used for the offline render; must not change the output.
    pub block_frames: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransportCommand {
    Play,
    Stop,
    Seek(Tick),
    SetMode(PlayMode),
    SetLoop(Option<(Tick, Tick)>),
    SetMetronome(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransportStatus {
    pub playing: bool,
    pub mode: PlayMode,
    /// Playhead in ticks, as last reported by the audio thread.
    pub position: Tick,
    pub loop_range: Option<(Tick, Tick)>,
    pub metronome: bool,
}

/// CPU and health figures (DESIGN §4 per-node meter, overloads).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EngineStats {
    /// Processing time ÷ buffer duration, smoothed. 1.0 = deadline.
    pub dsp_load: f32,
    pub overloads: u64,
    pub sample_rate: u32,
    pub buffer_frames: u32,
    /// Engine wants a bigger buffer (repeated overloads); the UI shows a notice.
    pub suggest_buffer_frames: Option<u32>,
    pub nodes: Vec<NodeStats>,
    /// Current overload-shedding level (0 = none; 1 meters, 2 oversampling, 3 voices).
    pub shedding: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeStats {
    /// Channel, insert or effect slot this node belongs to.
    pub owner: Option<ObjectPath>,
    pub label: String,
    /// Mean processing time per block, microseconds.
    pub micros: f32,
    /// Share of the buffer deadline.
    pub load: f32,
}

/// Peak/RMS per mixer insert for the UI meters, published by the audio thread.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Meters {
    /// Index = insert number. `[left, right]` peak and RMS, linear.
    pub peak: Vec<[f32; 2]>,
    pub rms: Vec<[f32; 2]>,
}

/// A running engine. Cheap handle; methods are called from the control/UI threads.
pub struct Engine {
    _private: (),
}

impl Engine {
    /// Opens the audio device and starts the audio thread with an empty project.
    pub fn start(_config: EngineConfig, _assets: Arc<dyn AssetResolver>) -> EngineResult<Engine> {
        todo!("T3")
    }

    /// Starts the engine on a simulated device driven by [`SimulatedDevice::pull`], for the
    /// real-time vs offline oracle tests (DESIGN §12).
    pub fn start_simulated(
        _config: EngineConfig,
        _assets: Arc<dyn AssetResolver>,
    ) -> EngineResult<(Engine, SimulatedDevice)> {
        todo!("T3")
    }

    /// Hands the engine a new project snapshot. Compiles the plan on the calling thread (or a
    /// helper) and swaps it in at the next block boundary. Voices of unchanged channels keep
    /// sounding across the swap.
    pub fn set_project(&self, _project: Arc<Project>) -> EngineResult<()> {
        todo!("T3")
    }

    pub fn transport(&self, _cmd: TransportCommand) -> TransportStatus {
        todo!("T3")
    }

    pub fn status(&self) -> TransportStatus {
        todo!("T3")
    }

    pub fn stats(&self) -> EngineStats {
        todo!("T3")
    }

    pub fn meters(&self) -> Meters {
        todo!("T3")
    }

    /// Plays a note on a channel immediately (typing keyboard, piano-roll preview).
    pub fn preview_note(&self, _channel: ph_core::ChannelId, _key: u8, _velocity: f32, _on: bool) {
        todo!("T3")
    }

    /// Changes device settings (buffer size, device, exclusive mode) with a brief dropout.
    pub fn reconfigure(&self, _config: EngineConfig) -> EngineResult<()> {
        todo!("T3")
    }
}

/// Test device: each `pull` runs the audio callback once, like a sound card would.
pub struct SimulatedDevice {
    _private: (),
}

impl SimulatedDevice {
    /// Runs one device callback and returns its output frames.
    pub fn pull(&mut self, _frames: usize) -> AudioBuffer {
        todo!("T3")
    }
}

/// Renders a project offline, faster than real time, deterministically. The oracle.
pub fn render_offline(
    _project: &Project,
    _settings: &RenderSettings,
    _assets: &dyn AssetResolver,
) -> EngineResult<AudioBuffer> {
    todo!("T3")
}
