//! The control layer.
//!
//! [`Studio`] is the running application core shared by every front end (UI, MCP, JSON-RPC,
//! CLI): the co-editing [`SessionHandle`] on its control thread, the audio [`Engine`], and the
//! bridge that hands every new project snapshot to the engine. Front ends never talk to the
//! engine for edits; they submit commands, and the bridge keeps the engine in step.
//!
//! [`EngineHost`] is the `ph_commands::Host` implementation over the engine and the
//! filesystem, enforcing the file-access rules of DESIGN §5.
//!
//! **Contract (T0-level, orchestrator-owned):** `Studio`, `StudioConfig`, `AudioMode` and
//! `EngineHost`. T4 (agent interface) and T5 (UI) build on these.

mod agent;
mod connections;
mod diff;
mod endpoint;
mod events;
mod grants;
mod headless;
mod host;
mod jsonrpc;
mod mcp;
mod node_params;
mod views;

pub use agent::{Agent, ToolOutput, ToolSpec};
pub use connections::{ConnGuard, ConnectionInfo, Connections, Protocol};
pub use diff::{Diff, diff};
pub use endpoint::{
    LocalServer, connect, default_endpoint, default_grants_file, proxy_stdio, serve_local,
    serve_local_with, serve_stdio,
};
pub use events::{Event, EventHub};
pub use grants::{GrantPrompt, Grants, grant_text, parse_grant};
pub use headless::{HeadlessOptions, find_assets_dir, start_headless};
pub use host::{EngineHost, default_projects_dir};
pub use jsonrpc::serve_jsonrpc;
pub use mcp::PatternhausServer;

use ph_commands::{Caller, Registry, Request, Session, SessionHandle, UiRequest};
use ph_core::Project;
use ph_engine::assets::FileResolver;
use ph_engine::{Engine, EngineConfig, SimulatedDevice};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the pointer is over in the UI, as the hint bar shows it: the control's name, its
/// current value and its gestures. The UI publishes it (`Studio::set_ui_hint`) and agents read
/// it with `get_state { view: "ui" }`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UiHint {
    pub name: String,
    pub value: String,
    pub gestures: String,
}

/// How the engine produces sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMode {
    /// The real output device.
    Device,
    /// No device: the engine runs on a simulated device that a pump thread drives in real
    /// time and discards. Transport, position, meters and stats behave exactly as with a
    /// device. For headless agent sessions, CI and tests.
    Silent,
}

#[derive(Clone, Debug)]
pub struct StudioConfig {
    /// The project folder: relative paths resolve here; `file.save` writes here.
    pub project_dir: PathBuf,
    /// Factory content (`assets/` of the repo or install): `factory:` samples resolve here.
    pub assets_dir: PathBuf,
    /// Sample/library folders agents and the UI may read from (DESIGN §5).
    pub library_roots: Vec<PathBuf>,
    /// Where exports may be written besides the project folder.
    pub exports_dir: Option<PathBuf>,
    /// The Patternhaus projects folder: the default home of FLP imports, and an allowed
    /// read/write root for agents (see [`default_projects_dir`]). `None` for none.
    pub projects_dir: Option<PathBuf>,
    pub engine: EngineConfig,
    pub audio: AudioMode,
    /// Plugin hosting (M2/T3): the scan cache file, extra plugin folders and the scan helper.
    /// `HostConfig::default()` keeps the cache in memory (tests); front ends pass
    /// [`default_plugin_config`].
    pub plugins: ph_engine::plugins::HostConfig,
}

/// The plugin settings front ends use: the scan cache in the user's cache folder
/// (`%LOCALAPPDATA%\Patternhaus\plugin-cache.json`, `~/Library/Caches/Patternhaus/...`,
/// `$XDG_CACHE_HOME/patternhaus/...`), the helper next to the executable.
pub fn default_plugin_config() -> ph_engine::plugins::HostConfig {
    let env = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    };
    let dir = if cfg!(windows) {
        env("LOCALAPPDATA").map(|d| d.join("Patternhaus"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches/Patternhaus"))
    } else {
        env("XDG_CACHE_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".cache")))
            .map(|d| d.join("patternhaus"))
    };
    ph_engine::plugins::HostConfig {
        cache_file: dir.map(|d| d.join("plugin-cache.json")),
        ..Default::default()
    }
}

/// The running application core.
pub struct Studio {
    pub session: SessionHandle,
    pub engine: Arc<Engine>,
    /// MIDI input, learn and note recording (M2/T8). Starts without devices; a front end
    /// with a keyboard calls [`Studio::use_midi_devices`].
    pub midi: Arc<ph_engine::midi::MidiService>,
    /// `ui.*` command requests for the UI to act on. Headless front ends may ignore it.
    pub ui_requests: Receiver<UiRequest>,
    /// Latest snapshot the engine was given.
    latest: Arc<Mutex<Arc<Project>>>,
    /// Stops the silent-mode pump thread on drop.
    pump_stop: Arc<AtomicBool>,
    config: Arc<StudioConfig>,
    assets: Arc<dyn ph_node::AssetResolver>,
    registry: Arc<Registry>,
    hub: Arc<OnceLock<Arc<EventHub>>>,
    ui_hint: Arc<Mutex<Option<UiHint>>>,
    ui_attached: Arc<AtomicBool>,
}

impl Drop for Studio {
    fn drop(&mut self) {
        self.pump_stop.store(true, Ordering::Relaxed);
    }
}

/// Drives a simulated device at real-time pace, discarding the audio.
fn spawn_pump(
    mut device: SimulatedDevice,
    config: &EngineConfig,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let frames = config.buffer_frames.max(32) as usize;
    let period = Duration::from_secs_f64(frames as f64 / config.sample_rate as f64);
    std::thread::Builder::new()
        .name("ph-silent-pump".into())
        .spawn(move || {
            let mut next = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let _ = device.pull(frames);
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now; // fell behind (debugger, overload): don't try to catch up
                }
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

impl Studio {
    /// Starts the engine and the session on `project`, and connects them.
    pub fn start(config: StudioConfig, project: Project) -> Result<Studio, String> {
        let mut resolver = FileResolver::new()
            .with_project_dir(&config.project_dir)
            .with_assets_dir(&config.assets_dir);
        for root in &config.library_roots {
            resolver = resolver.allow_root(root);
        }
        let resolver = Arc::new(resolver);
        let assets: Arc<dyn ph_node::AssetResolver> = resolver.clone();
        let pump_stop = Arc::new(AtomicBool::new(false));
        let engine = match config.audio {
            AudioMode::Device => {
                Engine::start(config.engine.clone(), assets.clone()).map_err(|e| e.to_string())?
            }
            AudioMode::Silent => {
                let (e, d) = Engine::start_simulated(config.engine.clone(), assets.clone())
                    .map_err(|e| e.to_string())?;
                spawn_pump(d, &config.engine, pump_stop.clone())?;
                e
            }
        };
        let engine = Arc::new(engine);
        // plugin hosting (M2/T3): instruments and effects from scanned CLAP/VST3 plugins
        let plugin_host = Arc::new(ph_engine::plugins::PluginHost::new(config.plugins.clone()));
        engine
            .set_plugins(Some(ph_engine::plugins::PluginEnv::new(
                plugin_host,
                &config.project_dir,
            )))
            .map_err(|e| e.to_string())?;
        let project = Arc::new(project);
        engine
            .set_project(project.clone())
            .map_err(|e| e.to_string())?;
        let latest = Arc::new(Mutex::new(project.clone()));

        let (ui_tx, ui_requests) = channel();
        let ui_attached = Arc::new(AtomicBool::new(false));
        let midi = Arc::new(ph_engine::midi::MidiService::start(
            engine.clone(),
            Box::new(ph_engine::midi::NoMidi),
            ph_engine::midi::MappingStore::memory(),
        ));
        let host = EngineHost::new(engine.clone(), assets.clone(), &config, ui_tx)
            .with_resolver(resolver)
            .with_midi(midi.clone())
            .with_ui_flag(ui_attached.clone());
        let registry = Arc::new(studio_registry());
        let session = Session::new(registry.clone(), (*project).clone());
        let session = SessionHandle::spawn(session, host);

        // Bridge: every committed change goes to the engine (compiled off the audio thread).
        let changes = session.subscribe().map_err(|e| e.to_string())?;
        let (bridge_engine, bridge_latest) = (engine.clone(), latest.clone());
        std::thread::Builder::new()
            .name("ph-bridge".into())
            .spawn(move || {
                for change in changes {
                    *bridge_latest.lock().unwrap() = change.project.clone();
                    // txn 0 marks a whole-project replacement (file.open, project reset): rebuild
                    // every node so nothing from the previous project's folder is reused.
                    let applied = if change.txn == 0 {
                        bridge_engine.replace_project(change.project)
                    } else {
                        bridge_engine.set_project(change.project)
                    };
                    if let Err(e) = applied {
                        // Plan errors (e.g. a routing loop) keep the previous plan running.
                        eprintln!("engine rejected project txn {}: {e}", change.txn);
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Studio {
            session,
            engine,
            midi,
            ui_requests,
            latest,
            pump_stop,
            config: Arc::new(config),
            assets,
            registry,
            hub: Arc::new(OnceLock::new()),
            ui_hint: Arc::new(Mutex::new(None)),
            ui_attached,
        })
    }

    /// Points MIDI at the system's devices (hot-plug included) and keeps the user's controller
    /// mappings in `config_dir` (`midi-mappings.json`; in memory when `None`). Mappings are
    /// per user, never part of a project.
    pub fn use_midi_devices(&self, config_dir: Option<&std::path::Path>) {
        self.midi.set_store(match config_dir {
            Some(d) => ph_engine::midi::MappingStore::open(d),
            None => ph_engine::midi::MappingStore::memory(),
        });
        self.midi
            .set_backend(Box::new(ph_engine::midi::MidirBackend));
    }

    /// The UI says it is there (it takes `ui_requests`, e.g. opens plugin editors). Headless
    /// front ends never call it, so `plugin.open_editor` reports that no UI took the request.
    pub fn set_ui_attached(&self, attached: bool) {
        self.ui_attached.store(attached, Ordering::Relaxed);
    }

    /// The snapshot the engine is playing (or about to).
    pub fn project(&self) -> Arc<Project> {
        self.latest.lock().unwrap().clone()
    }

    /// Publishes the UI's current hint (what the pointer is over); `None` when nothing is.
    /// Called by the UI once per change.
    pub fn set_ui_hint(&self, hint: Option<UiHint>) {
        *self.ui_hint.lock().unwrap() = hint;
    }

    /// The hint the UI is showing, if a UI is running and the pointer is over a control.
    pub fn ui_hint(&self) -> Option<UiHint> {
        self.ui_hint.lock().unwrap().clone()
    }

    /// Convenience: run one command as `caller`. A successful `file.save` / `file.save_as`
    /// also persists the per-author history log to `history/log.jsonl` next to the project.
    pub fn run(
        &self,
        caller: &Caller,
        command: &str,
        params: serde_json::Value,
    ) -> ph_commands::CmdResult<ph_commands::Executed> {
        self.handle().execute(caller, Request::new(command, params))
    }

    /// A cloneable, `Send + Sync` handle to everything the agent interface needs.
    pub fn handle(&self) -> StudioHandle {
        StudioHandle {
            session: self.session.clone(),
            engine: self.engine.clone(),
            midi: self.midi.clone(),
            latest: self.latest.clone(),
            config: self.config.clone(),
            assets: self.assets.clone(),
            registry: self.registry.clone(),
            hub: self.hub.clone(),
            ui_hint: self.ui_hint.clone(),
        }
    }
}

/// A cloneable view of a running [`Studio`] for front ends that run on their own threads
/// (MCP and JSON-RPC connections, the CLI). It carries no UI receiver.
#[derive(Clone)]
pub struct StudioHandle {
    pub session: SessionHandle,
    pub engine: Arc<Engine>,
    pub midi: Arc<ph_engine::midi::MidiService>,
    latest: Arc<Mutex<Arc<Project>>>,
    pub config: Arc<StudioConfig>,
    pub assets: Arc<dyn ph_node::AssetResolver>,
    pub registry: Arc<Registry>,
    hub: Arc<OnceLock<Arc<EventHub>>>,
    ui_hint: Arc<Mutex<Option<UiHint>>>,
}

impl StudioHandle {
    /// The hint the UI is showing (see [`Studio::ui_hint`]).
    pub fn ui_hint(&self) -> Option<UiHint> {
        self.ui_hint.lock().unwrap().clone()
    }

    /// The snapshot the engine is playing (or about to).
    pub fn project(&self) -> Arc<Project> {
        self.latest.lock().unwrap().clone()
    }

    /// The live event hub (started on first use).
    pub fn events(&self) -> Result<Arc<EventHub>, String> {
        if let Some(h) = self.hub.get() {
            return Ok(h.clone());
        }
        let h = EventHub::start(&self.session, self.engine.clone())?;
        Ok(self.hub.get_or_init(|| h).clone())
    }

    /// Runs one command as `caller` (history is persisted on save).
    pub fn execute(
        &self,
        caller: &Caller,
        req: Request,
    ) -> ph_commands::CmdResult<ph_commands::Executed> {
        let done = self.session.execute(caller, req)?;
        self.after_save(std::slice::from_ref(&done));
        Ok(done)
    }

    /// Runs commands as one transaction.
    pub fn batch(
        &self,
        caller: &Caller,
        reqs: Vec<Request>,
    ) -> ph_commands::CmdResult<Vec<ph_commands::Executed>> {
        let done = self.session.batch(caller, reqs)?;
        self.after_save(&done);
        Ok(done)
    }

    /// `file.save` writes `project.json`; the history log lives in the session, which the
    /// command cannot see, so it is written here, through the session handle.
    pub(crate) fn after_save(&self, done: &[ph_commands::Executed]) {
        for d in done {
            if matches!(d.command.as_str(), "file.save" | "file.save_as")
                && let Some(dir) = d.result["path"].as_str()
                && let Err(e) = self.save_history(std::path::Path::new(dir))
            {
                eprintln!("could not write the history log: {e}");
            }
        }
    }

    /// Persists the history log (`history/log.jsonl`) into a project folder.
    pub fn save_history(&self, dir: &std::path::Path) -> Result<(), String> {
        let entries = self.session.history().map_err(|e| e.to_string())?;
        let values: Vec<serde_json::Value> = entries
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        ph_formats::write_history_log(dir, &values).map_err(|e| e.to_string())
    }
}

/// The command registry every front end serves: the `ph-commands` built-ins plus the
/// commands `ph-control` adds (`query.node_params`).
pub fn studio_registry() -> Registry {
    let mut registry = Registry::with_builtins();
    node_params::register(&mut registry);
    registry
}

#[cfg(test)]
mod coverage;
#[cfg(test)]
mod plugin_tests;
#[cfg(test)]
mod record_tests;
#[cfg(test)]
mod tests;
