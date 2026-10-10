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
pub use host::EngineHost;
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
    pub engine: EngineConfig,
    pub audio: AudioMode,
}

/// The running application core.
pub struct Studio {
    pub session: SessionHandle,
    pub engine: Arc<Engine>,
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
        let assets: Arc<dyn ph_node::AssetResolver> = Arc::new(resolver);
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
        let project = Arc::new(project);
        engine
            .set_project(project.clone())
            .map_err(|e| e.to_string())?;
        let latest = Arc::new(Mutex::new(project.clone()));

        let (ui_tx, ui_requests) = channel();
        let host = EngineHost::new(engine.clone(), assets.clone(), &config, ui_tx);
        let mut registry = Registry::with_builtins();
        node_params::register(&mut registry);
        let registry = Arc::new(registry);
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
                    if let Err(e) = bridge_engine.set_project(change.project) {
                        // Plan errors (e.g. a routing loop) keep the previous plan running.
                        eprintln!("engine rejected project txn {}: {e}", change.txn);
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Studio {
            session,
            engine,
            ui_requests,
            latest,
            pump_stop,
            config: Arc::new(config),
            assets,
            registry,
            hub: Arc::new(OnceLock::new()),
        })
    }

    /// The snapshot the engine is playing (or about to).
    pub fn project(&self) -> Arc<Project> {
        self.latest.lock().unwrap().clone()
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
            latest: self.latest.clone(),
            config: self.config.clone(),
            assets: self.assets.clone(),
            registry: self.registry.clone(),
            hub: self.hub.clone(),
        }
    }
}

/// A cloneable view of a running [`Studio`] for front ends that run on their own threads
/// (MCP and JSON-RPC connections, the CLI). It carries no UI receiver.
#[derive(Clone)]
pub struct StudioHandle {
    pub session: SessionHandle,
    pub engine: Arc<Engine>,
    latest: Arc<Mutex<Arc<Project>>>,
    pub config: Arc<StudioConfig>,
    pub assets: Arc<dyn ph_node::AssetResolver>,
    pub registry: Arc<Registry>,
    hub: Arc<OnceLock<Arc<EventHub>>>,
}

impl StudioHandle {
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

#[cfg(test)]
mod tests;
