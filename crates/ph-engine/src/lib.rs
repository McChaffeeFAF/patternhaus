//! Patternhaus audio engine — public API contract (T0), implemented in T3.
//!
//! - The control thread hands the engine `Arc<Project>` snapshots. The engine compiles each
//!   into a render plan *off* the audio thread and swaps it in lock-free; old plans are freed
//!   on a non-real-time thread (DESIGN §4).
//! - Offline rendering runs the same plan and node code with a fake clock. It is the test
//!   oracle: real-time output through [`Engine::start_simulated`] must match it exactly.
//! - The engine never depends on `ph-commands`; the app adapts it to the `Host` port.

//!
//! Internals: [`tempo`] converts ticks to samples exactly; `plan` compiles snapshots into a
//! DAG with node reuse; `runtime` is the audio callback; `pool` is the real-time worker
//! pool; `device` drives `cpal`; `offline` renders with a fake clock; [`assets`] resolves
//! sample references.

// The loom build compiles only the models (`loom_models.rs`); test-only helpers go unused.
#![cfg_attr(loom, allow(dead_code))]

pub mod assets;
pub mod audio_clips;
mod audition;
pub mod automation;
pub mod catalog;
mod control;
mod device;
#[cfg(all(test, loom))]
mod loom_models;
mod offline;
mod plan;
mod player;
pub mod plugins;
mod pool;
pub mod record;
mod rt;
mod runtime;
mod sched;
mod stats;
mod stream;
mod sync;
pub mod tempo;
#[cfg(all(test, not(loom)))]
mod tests;

use control::Controller;
use ph_core::{MIXER_INSERTS, ObjectPath, PatternId, Project, Tick};
use ph_node::{AssetResolver, Quality};
use plan::Settings;
use rt::{Collector, Mailbox};
use runtime::{Command, Runtime};
use stats::{Shared, load_f32};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

/// True when the engine was built with the `asio` feature. Such a binary links Steinberg's
/// ASIO SDK and is distributed under GPLv3 (DESIGN §11); front ends say so in their About
/// box.
pub const ASIO_BUILD: bool = cfg!(feature = "asio");

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("audio device error: {0}")]
    Device(String),
    #[error("could not build render plan: {0}")]
    Plan(String),
    #[error("asset error: {0}")]
    Asset(String),
    /// A feature whose contract exists but whose implementation is a later M2 track.
    #[error("{feature} is not available until M2/{track}")]
    Unavailable {
        feature: &'static str,
        track: &'static str,
    },
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
    /// Things that compile to silence or a pass-through: a sample that cannot be loaded, an
    /// unsupported instrument or effect. Empty when the project is fully playable.
    pub warnings: Vec<String>,
    /// Audio-clip disk reads that were not ready in time (silence was played), since the
    /// engine started (M2/T1). Stays 0 while streaming keeps up.
    pub underruns: u64,
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
    inner: Mutex<Inner>,
    shared: Arc<Shared>,
}

struct Inner {
    config: EngineConfig,
    assets: Arc<dyn AssetResolver>,
    ctl: Controller,
    cmds: rtrb::Producer<Command>,
    mirror: TransportStatus,
    project: Arc<Project>,
    /// Real device (absent for the simulated device).
    device: Option<device::DeviceHandle>,
    _collector: Collector,
    sample_rate: u32,
    buffer_frames: u32,
    /// `project_warnings` of the project at this pointer (recomputed when it changes).
    warnings: (usize, Vec<String>),
    /// Preview voices to and from the audio thread.
    audition: Arc<audition::Port>,
    /// Plugin hosting (M2/T3), carried across reconfigures.
    plugins: Option<plugins::PluginEnv>,
    /// Plugin faults reported so far (M2/T3), oldest first.
    faults: Vec<plugins::PluginFaultReport>,
}

/// Command ring capacity (transport, previews) between control and audio threads.
const COMMAND_RING: usize = 1024;
/// Retired plans in flight to the collector.
const RETIRE_RING: usize = 64;

/// Graph worker threads for a `threads` setting (`0` = physical cores − 1, estimated as
/// half the logical CPUs on SMT machines). The audio thread is the remaining participant.
fn worker_count(threads: usize) -> usize {
    if threads > 0 {
        return threads - 1;
    }
    let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
    let physical = if logical >= 4 { logical / 2 } else { logical };
    physical.saturating_sub(1)
}

fn initial_status() -> TransportStatus {
    TransportStatus {
        playing: false,
        mode: PlayMode::Song,
        position: 0,
        loop_range: None,
        metronome: false,
    }
}

struct Parts {
    rt: Runtime,
    ctl: Controller,
    cmds: rtrb::Producer<Command>,
    collector: Collector,
    audition: Arc<audition::Port>,
}

/// Builds a runtime for `settings` with `project` loaded and the transport in `status`.
#[allow(clippy::too_many_arguments)]
fn build(
    project: &Project,
    settings: Settings,
    assets: &dyn AssetResolver,
    shared: Arc<Shared>,
    workers: usize,
    max_shed: u8,
    status: &TransportStatus,
    policy: audio_clips::StreamPolicy,
) -> EngineResult<Parts> {
    let mailbox = Arc::new(Mailbox::new());
    let mut ctl = Controller::new(settings, mailbox.clone(), policy);
    let plan = ctl.initial(project, assets)?;
    let (tx, rx) = rtrb::RingBuffer::new(COMMAND_RING);
    let (collector, retire) = Collector::spawn(RETIRE_RING);
    let port = Arc::new(audition::Port::new());
    let mut rt = Runtime::new(
        plan,
        mailbox,
        retire,
        rx,
        shared,
        workers,
        max_shed,
        port.clone(),
    );
    // carry the transport over (reconfigure)
    if status.mode != PlayMode::Song {
        rt.apply(Command::Transport(TransportCommand::SetMode(status.mode)));
    }
    if status.loop_range.is_some() {
        rt.apply(Command::Transport(TransportCommand::SetLoop(
            status.loop_range,
        )));
    }
    if status.metronome {
        rt.apply(Command::Transport(TransportCommand::SetMetronome(true)));
    }
    if status.position > 0 {
        rt.apply(Command::Transport(TransportCommand::Seek(status.position)));
    }
    if status.playing {
        rt.apply(Command::Transport(TransportCommand::Play));
    }
    Ok(Parts {
        rt,
        ctl,
        cmds: tx,
        collector,
        audition: port,
    })
}

impl Engine {
    /// Opens the audio device and starts the audio thread with an empty project.
    pub fn start(config: EngineConfig, assets: Arc<dyn AssetResolver>) -> EngineResult<Engine> {
        let shared = Arc::new(Shared::new(MIXER_INSERTS + 1));
        let project = Arc::new(Project::empty("Untitled"));
        let inner = Self::open_device(config, assets, shared.clone(), project, initial_status())?;
        Ok(Engine {
            inner: Mutex::new(inner),
            shared,
        })
    }

    fn open_device(
        config: EngineConfig,
        assets: Arc<dyn AssetResolver>,
        shared: Arc<Shared>,
        project: Arc<Project>,
        status: TransportStatus,
    ) -> EngineResult<Inner> {
        let (tx, rx) = std::sync::mpsc::channel();
        let workers = worker_count(config.threads);
        let (a, p, sh, st) = (
            assets.clone(),
            project.clone(),
            shared.clone(),
            status.clone(),
        );
        let handle = device::open(&config, move |sr, frames| {
            let settings = Settings {
                sample_rate: sr,
                max_frames: frames.max(16) as usize,
                quality: Quality::Live,
            };
            let parts = build(
                &p,
                settings,
                &*a,
                sh,
                workers,
                3,
                &st,
                audio_clips::StreamPolicy::Live,
            )?;
            let _ = tx.send((parts.ctl, parts.cmds, parts.collector, parts.audition));
            Ok(parts.rt)
        })?;
        let (ctl, cmds, collector, audition) = rx
            .recv()
            .map_err(|_| EngineError::Device("device thread ended unexpectedly".into()))?;
        Ok(Inner {
            plugins: None,
            faults: Vec::new(),
            sample_rate: handle.sample_rate,
            buffer_frames: handle.buffer_frames,
            warnings: (0, Vec::new()),
            config,
            assets,
            ctl,
            cmds,
            mirror: status,
            project,
            device: Some(handle),
            _collector: collector,
            audition,
        })
    }

    /// Starts the engine on a simulated device driven by [`SimulatedDevice::pull`], for the
    /// real-time vs offline oracle tests (DESIGN §12).
    pub fn start_simulated(
        config: EngineConfig,
        assets: Arc<dyn AssetResolver>,
    ) -> EngineResult<(Engine, SimulatedDevice)> {
        let shared = Arc::new(Shared::new(MIXER_INSERTS + 1));
        let project = Arc::new(Project::empty("Untitled"));
        let settings = Settings {
            sample_rate: config.sample_rate.max(1),
            max_frames: config.buffer_frames.max(1) as usize,
            quality: Quality::Live,
        };
        let status = initial_status();
        // shedding levels above 1 change the audio; the simulated device must match the
        // offline render exactly, so it stops at level 1 (meters)
        let parts = build(
            &project,
            settings,
            &*assets,
            shared.clone(),
            worker_count(config.threads),
            1,
            &status,
            // the simulated device waits for the prefetch thread, so it matches offline
            audio_clips::StreamPolicy::Wait,
        )?;
        let inner = Inner {
            plugins: None,
            faults: Vec::new(),
            sample_rate: settings.sample_rate,
            buffer_frames: settings.max_frames as u32,
            warnings: (0, Vec::new()),
            config,
            assets,
            ctl: parts.ctl,
            cmds: parts.cmds,
            mirror: status,
            project,
            device: None,
            _collector: parts.collector,
            audition: parts.audition,
        };
        Ok((
            Engine {
                inner: Mutex::new(inner),
                shared,
            },
            SimulatedDevice { rt: parts.rt },
        ))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hands the engine a new project snapshot. Compiles the plan on the calling thread (or a
    /// helper) and swaps it in at the next block boundary. Voices of unchanged channels keep
    /// sounding across the swap.
    pub fn set_project(&self, project: Arc<Project>) -> EngineResult<()> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.ctl.submit(&project, &*inner.assets)?;
        inner.project = project;
        Ok(())
    }

    /// Like [`Engine::set_project`], but for a *different* project replacing this one (file
    /// open): nothing is carried over from the running plan, so every instrument and effect is
    /// rebuilt and every sample re-resolved. Sounding voices stop at the swap.
    pub fn replace_project(&self, project: Arc<Project>) -> EngineResult<()> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.ctl.submit_with(&project, &*inner.assets, true)?;
        inner.project = project;
        Ok(())
    }

    pub fn transport(&self, cmd: TransportCommand) -> TransportStatus {
        let mut g = self.lock();
        let m = &mut g.mirror;
        match &cmd {
            TransportCommand::Play => m.playing = true,
            TransportCommand::Stop => {
                m.playing = false;
                m.position = match (m.mode, m.loop_range) {
                    (PlayMode::Song, Some((a, _))) => a,
                    _ => 0,
                };
            }
            TransportCommand::Seek(t) => m.position = *t,
            TransportCommand::SetMode(mode) => {
                if *mode != m.mode {
                    m.mode = *mode;
                    m.position = 0;
                }
            }
            TransportCommand::SetLoop(r) => m.loop_range = r.filter(|(a, b)| b > a),
            TransportCommand::SetMetronome(on) => m.metronome = *on,
        }
        let status = m.clone();
        let _ = g.cmds.push(Command::Transport(cmd));
        status
    }

    pub fn status(&self) -> TransportStatus {
        let g = self.lock();
        TransportStatus {
            playing: self.shared.playing.load(Relaxed),
            position: self.shared.position.load(Relaxed),
            ..g.mirror.clone()
        }
    }

    pub fn stats(&self) -> EngineStats {
        let mut g = self.lock();
        g.audition.collect();
        let ptr = Arc::as_ptr(&g.project) as usize;
        if g.warnings.0 != ptr {
            let w = project_warnings_with(&g.project, &*g.assets, g.plugins.as_ref());
            g.warnings = (ptr, w);
        }
        Self::check_faults(&mut g);
        let mut warnings = g.warnings.1.clone();
        warnings.extend(g.faults.iter().map(|f| f.message()));
        let sh = &self.shared;
        let budget_us = g.buffer_frames as f32 / g.sample_rate.max(1) as f32 * 1e6;
        let nodes = g
            .ctl
            .board
            .entries
            .iter()
            .map(|e| {
                let micros = load_f32(&e.micros);
                NodeStats {
                    owner: e.owner,
                    label: e.label.clone(),
                    micros,
                    load: micros / budget_us,
                }
            })
            .collect();
        let suggest = sh.suggest_frames.load(Relaxed);
        EngineStats {
            dsp_load: load_f32(&sh.dsp_load),
            overloads: sh.overloads.load(Relaxed),
            sample_rate: g.sample_rate,
            buffer_frames: g.buffer_frames,
            suggest_buffer_frames: (suggest > 0).then_some(suggest),
            nodes,
            shedding: sh.shedding.load(Relaxed),
            warnings,
            underruns: g.ctl.audio.underruns.load(Relaxed),
        }
    }

    /// Peak since the previous call and the latest RMS, per insert (index = insert number).
    pub fn meters(&self) -> Meters {
        let mut m = Meters::default();
        for slot in &self.shared.meters {
            m.peak.push([
                f32::from_bits(slot.peak[0].swap(0, Relaxed)),
                f32::from_bits(slot.peak[1].swap(0, Relaxed)),
            ]);
            m.rms.push([load_f32(&slot.rms[0]), load_f32(&slot.rms[1])]);
        }
        m
    }

    /// Plays a note on a channel immediately (typing keyboard, piano-roll preview).
    pub fn preview_note(&self, channel: ph_core::ChannelId, key: u8, velocity: f32, on: bool) {
        let _ = self.lock().cmds.push(Command::Preview {
            channel,
            key,
            velocity,
            on,
        });
    }

    /// Previews a sample (the browser's click-to-preview) on a dedicated voice outside the
    /// project: it is added to the output after the master insert at a fixed preview level
    /// (about -6 dB at velocity 1), so it never reaches the mixer, meters, renders or exports.
    /// A new audition replaces the previous one. `sample` is a reference the engine's asset
    /// resolver understands (`factory:drums/haus808/kick`, a project-relative or absolute
    /// path). The file is decoded and resampled here, on the calling thread.
    pub fn audition(&self, sample: &str, velocity: f32) -> EngineResult<()> {
        let assets = self.lock().assets.clone();
        let data = assets.sample(sample).map_err(EngineError::Asset)?;
        self.audition_samples(&data, velocity);
        Ok(())
    }

    /// Previews an instrument (a browser preset) on the audition voice: renders `key` for
    /// `seconds` (plus its release tail, up to 3 s) offline on the calling thread, from a
    /// throw-away one-channel project, then plays the result like [`Engine::audition`]. The
    /// running project is not touched.
    pub fn audition_instrument(
        &self,
        instrument: &ph_core::InstrumentSpec,
        key: u8,
        velocity: f32,
        seconds: f64,
    ) -> EngineResult<()> {
        let (assets, rate) = {
            let g = self.lock();
            (g.assets.clone(), g.sample_rate)
        };
        let audio = audition::render_instrument(instrument, key, seconds, rate, &*assets)?;
        let data = ph_node::SampleData {
            sample_rate: audio.sample_rate as f64,
            channels: audio.channels,
            loop_range: None,
            root_key: None,
        };
        self.audition_samples(&data, velocity);
        Ok(())
    }

    /// Previews decoded audio on the audition voice, like [`Engine::audition`] but from
    /// samples the caller already holds (the audio editor's selection). Longer than 30 s is
    /// cut with a short fade. The data is resampled here, on the calling thread.
    pub fn audition_data(&self, data: &ph_node::SampleData, velocity: f32) {
        self.audition_samples(data, velocity);
    }

    fn audition_samples(&self, data: &ph_node::SampleData, velocity: f32) {
        let (rate, port) = {
            let g = self.lock();
            (g.sample_rate, g.audition.clone())
        };
        let gain = audition::PREVIEW_LEVEL * velocity.clamp(0.0, 1.0);
        port.post(Box::new(audition::Voice::new(data, rate, gain)));
    }

    /// Stops the audition with a short fade.
    /// Audio inputs the device offers (M2/T8; empty until then).
    pub fn inputs(&self) -> Vec<record::InputInfo> {
        Vec::new()
    }

    /// Arms `input` to record into `target` (M2/T8).
    pub fn arm(&self, input: record::InputId, target: record::RecordTarget) -> EngineResult<()> {
        let _ = (input, target);
        Err(record_unavailable())
    }

    /// Disarms `input`. A no-op for inputs that are not armed.
    pub fn disarm(&self, input: record::InputId) {
        let _ = input;
    }

    /// Hear an armed input through its target (M2/T8).
    pub fn set_monitor(&self, input: record::InputId, on: bool) -> EngineResult<()> {
        let _ = (input, on);
        Err(record_unavailable())
    }

    /// Starts recording every armed input at the current position (M2/T8).
    pub fn record_start(&self) -> EngineResult<()> {
        Err(record_unavailable())
    }

    /// Stops recording and hands back one take per armed input (M2/T8). The caller writes
    /// them to `samples/` and adds them to the project through commands.
    pub fn record_stop(&self) -> EngineResult<Vec<record::RecordedTake>> {
        Err(record_unavailable())
    }

    pub fn stop_audition(&self) {
        let port = self.lock().audition.clone();
        port.post(Box::new(audition::Voice::silence()));
    }

    /// Changes device settings (buffer size, device, exclusive mode) with a brief dropout.
    pub fn reconfigure(&self, config: EngineConfig) -> EngineResult<()> {
        let mut g = self.lock();
        if g.device.is_none() {
            return Err(EngineError::Device(
                "the simulated device cannot be reconfigured; start a new one".into(),
            ));
        }
        let _ = &g.config;
        // stop the old stream first (its runtime is dropped on the device thread)
        g.device = None;
        let status = TransportStatus {
            playing: self.shared.playing.load(Relaxed),
            position: self.shared.position.load(Relaxed),
            ..g.mirror.clone()
        };
        let mut inner = Self::open_device(
            config,
            g.assets.clone(),
            self.shared.clone(),
            g.project.clone(),
            status,
        )?;
        inner.faults = std::mem::take(&mut g.faults);
        if let Some(env) = g.plugins.take() {
            inner.plugins = Some(env.clone());
            inner.ctl.opts.plugins = Some(env);
            let project = inner.project.clone();
            inner.ctl.submit_with(&project, &*inner.assets, true)?;
        }
        *g = inner;
        Ok(())
    }
}

impl Engine {
    /// Enables plugin hosting (M2/T3): plugin instruments and effects compile through `env`
    /// from now on (the running project is rebuilt so its plugins load). `None` turns it off.
    pub fn set_plugins(&self, env: Option<plugins::PluginEnv>) -> EngineResult<()> {
        let mut g = self.lock();
        let inner = &mut *g;
        inner.plugins = env.clone();
        inner.ctl.opts.plugins = env;
        inner.warnings = (0, Vec::new());
        let project = inner.project.clone();
        inner.ctl.submit_with(&project, &*inner.assets, true)
    }

    /// The plugin hosting the engine uses, if enabled.
    pub fn plugin_env(&self) -> Option<plugins::PluginEnv> {
        self.lock().plugins.clone()
    }

    /// Plugin nodes of the current plan, by where they sit.
    pub fn plugins(&self) -> Vec<plugins::LivePlugin> {
        self.lock().ctl.plugins.values().cloned().collect()
    }

    /// The plugin node at `slot`, if the current plan has one.
    pub fn plugin(&self, slot: plugins::PluginSlot) -> Option<plugins::LivePlugin> {
        self.lock().ctl.plugins.get(&slot).cloned()
    }

    /// Plugins that crashed so far, oldest first. A new fault is detected here (and by
    /// [`Engine::stats`]): it is recorded once, and the plugin is marked *isolate* in the scan
    /// cache so it is not loaded in-process again (DESIGN §6). The session keeps running; the
    /// node plays silence.
    pub fn plugin_faults(&self) -> Vec<plugins::PluginFaultReport> {
        let mut g = self.lock();
        Self::check_faults(&mut g);
        g.faults.clone()
    }

    fn check_faults(g: &mut Inner) {
        let mut new = Vec::new();
        for lp in g.ctl.plugins.values() {
            if let Some(fault) = lp.fault.fault()
                && !g
                    .faults
                    .iter()
                    .any(|f| f.slot == lp.slot && f.fault == fault && f.plugin.id == lp.info.id)
            {
                new.push(plugins::PluginFaultReport {
                    slot: lp.slot,
                    plugin: lp.info.clone(),
                    fault,
                });
            }
        }
        for f in new {
            if let Some(env) = &g.plugins {
                env.host.mark_fault(f.plugin.format, &f.fault);
            }
            g.warnings = (0, Vec::new());
            g.faults.push(f);
        }
    }

    /// The live state of every plugin node that names a state file, as `plugins/<file>`
    /// contents by reference. For saving the project and for offline renders of the live
    /// session. Plugins that cannot save (crashed, no control) are left out.
    pub fn plugin_states(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        let live = self.plugins();
        live.into_iter()
            .filter(|p| !p.fault.is_dead())
            .filter_map(|p| Some((p.state_ref.clone()?, p.state_file().ok()?)))
            .collect()
    }
}

fn record_unavailable() -> EngineError {
    EngineError::Unavailable {
        feature: "audio input recording",
        track: "T8",
    }
}

/// Test device: each `pull` runs the audio callback once, like a sound card would.
pub struct SimulatedDevice {
    rt: Runtime,
}

impl SimulatedDevice {
    /// Runs one device callback and returns its output frames.
    pub fn pull(&mut self, frames: usize) -> AudioBuffer {
        let mut l = vec![0.0f32; frames];
        let mut r = vec![0.0f32; frames];
        self.pull_into(&mut l, &mut r);
        AudioBuffer {
            sample_rate: self.rt.plan.settings.sample_rate,
            channels: vec![l, r],
        }
    }

    /// The callback itself, into caller buffers: allocation-free (checked under
    /// `assert_no_alloc` in debug builds), with FTZ/DAZ on like a device thread.
    pub(crate) fn pull_into(&mut self, l: &mut [f32], r: &mut [f32]) {
        let saved = rt::enable_ftz_daz();
        rt::no_alloc(|| self.rt.process(l, r));
        rt::restore_fp(saved);
    }

    /// Graph worker threads (besides the calling thread).
    #[cfg(test)]
    pub(crate) fn workers(&self) -> usize {
        self.rt.workers()
    }
}

/// Renders a project offline, faster than real time, deterministically. The oracle.
pub fn render_offline(
    project: &Project,
    settings: &RenderSettings,
    assets: &dyn AssetResolver,
) -> EngineResult<AudioBuffer> {
    offline::render(project, settings, assets, worker_count(0))
}

/// [`render_offline`] with plugin hosting (M2/T3): plugin specs instantiate fresh instances
/// through `plugins`, loading their states from the project folder, or from
/// [`plugins::PluginEnv::with_states`] (e.g. [`Engine::plugin_states`], to render the live
/// session's plugin settings).
pub fn render_offline_with(
    project: &Project,
    settings: &RenderSettings,
    assets: &dyn AssetResolver,
    plugins: Option<&plugins::PluginEnv>,
) -> EngineResult<AudioBuffer> {
    offline::render_with(project, settings, assets, worker_count(0), plugins)
}

/// Instruments and effects of `project` that cannot be built (an unloadable sample, an
/// unsupported or plugin-only instrument): the engine plays them as silence or a
/// pass-through, so front ends and agents are told here. Off the audio thread only.
pub fn project_warnings(project: &Project, assets: &dyn AssetResolver) -> Vec<String> {
    project_warnings_with(project, assets, None)
}

/// [`project_warnings`] knowing which plugins can be loaded (M2/T3): a plugin spec warns only
/// when its plugin was not scanned, is isolated or blocked, or hosting is off.
pub fn project_warnings_with(
    project: &Project,
    assets: &dyn AssetResolver,
    plugins: Option<&plugins::PluginEnv>,
) -> Vec<String> {
    let mut out = Vec::new();
    for ch in project.channels_in_order() {
        if let Some((format, id, _)) = plugins::instrument_plugin(&ch.instrument) {
            if let Some(why) = plugins::unavailable(plugins, format, id) {
                out.push(format!(
                    "channel {:?} (id {}): {why}; it will play silence",
                    ch.name, ch.id.0
                ));
            }
            continue;
        }
        if let Err(e) = ph_instruments::create(&ch.instrument, assets) {
            out.push(format!(
                "channel {:?} (id {}): {e}; it will play silence",
                ch.name, ch.id.0
            ));
        }
    }
    for (i, ins) in project.mixer.iter().enumerate() {
        for (s, slot) in ins.effects.iter().enumerate() {
            if let Some((format, id, _)) = plugins::effect_plugin(&slot.effect) {
                if let Some(why) = plugins::unavailable(plugins, format, id) {
                    out.push(format!(
                        "insert {i} {:?}, effect slot {s}: {why}; the effect is skipped",
                        ins.name
                    ));
                }
                continue;
            }
            if let Err(e) = ph_effects::create(&slot.effect) {
                out.push(format!(
                    "insert {i} {:?}, effect slot {s}: {e}; the effect is skipped",
                    ins.name
                ));
            }
        }
    }
    // audio clips: registry problems, unsynced clips, unreadable files
    let automation = automation::AutomationEvaluator::new(project);
    let tempo = automation.tempo_map(project.settings.bpm, 48_000, plan::TEMPO_RESOLUTION);
    let (plans, warnings) = audio_clips::plan_audio_clips(project, &tempo);
    out.extend(warnings);
    let mut checked = std::collections::BTreeSet::new();
    for p in &plans {
        if !checked.insert(p.path.as_str()) {
            continue;
        }
        let readable = match assets.stream(&p.path) {
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(e),
            None => assets.sample(&p.path).map(|_| ()),
        };
        if let Err(e) = readable {
            out.push(format!(
                "audio clip {}: {} cannot be read ({e}); it plays silence",
                p.clip, p.path
            ));
        }
    }
    // automation aimed at something that is not there plays nothing
    for t in automation.targets() {
        let gone = match t {
            ph_core::ParamPath::Tempo => None,
            ph_core::ParamPath::ChannelVolume(c)
            | ph_core::ParamPath::ChannelPan(c)
            | ph_core::ParamPath::ChannelParam(c, _) => (!project.channels.contains_key(c))
                .then(|| format!("channel {} does not exist", c.0)),
            ph_core::ParamPath::InsertVolume(i)
            | ph_core::ParamPath::InsertPan(i)
            | ph_core::ParamPath::InsertMute(i) => project
                .insert(*i)
                .is_err()
                .then(|| format!("insert {} does not exist", i.0)),
            ph_core::ParamPath::InsertSend { insert, to } => match project.insert(*insert) {
                Ok(x) if x.sends.iter().any(|s| s.to == *to) => None,
                Ok(_) => Some(format!(
                    "insert {} has no send to insert {}",
                    insert.0, to.0
                )),
                Err(_) => Some(format!("insert {} does not exist", insert.0)),
            },
            ph_core::ParamPath::EffectParam { insert, slot, .. } => match project.insert(*insert) {
                Ok(x) if (*slot as usize) < x.effects.len() => None,
                Ok(_) => Some(format!("insert {} has no effect slot {slot}", insert.0)),
                Err(_) => Some(format!("insert {} does not exist", insert.0)),
            },
        };
        if let Some(why) = gone {
            out.push(format!("automation of {t}: {why}; it is ignored"));
        }
    }
    out
}
