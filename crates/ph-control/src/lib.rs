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

mod host;

pub use host::EngineHost;

use ph_commands::{Caller, Registry, Request, Session, SessionHandle, UiRequest};
use ph_core::Project;
use ph_engine::assets::FileResolver;
use ph_engine::{Engine, EngineConfig, SimulatedDevice};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
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
        let host = EngineHost::new(engine.clone(), assets, &config, ui_tx);
        let session = Session::new(Arc::new(Registry::with_builtins()), (*project).clone());
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
        })
    }

    /// The snapshot the engine is playing (or about to).
    pub fn project(&self) -> Arc<Project> {
        self.latest.lock().unwrap().clone()
    }

    /// Convenience: run one command as `caller`.
    pub fn run(
        &self,
        caller: &Caller,
        command: &str,
        params: serde_json::Value,
    ) -> ph_commands::CmdResult<ph_commands::Executed> {
        self.session.execute(caller, Request::new(command, params))
    }
}

#[cfg(test)]
mod tests;
