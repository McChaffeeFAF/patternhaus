//! `ph-app`: the Patternhaus desktop application.
//!
//! ```text
//! ph-app [project_dir] [--silent] [--demo-agent] [--assets DIR] [--config DIR] [--library DIR]... [--buffer N]
//! ```
//!
//! Opens the project folder (or starts a new project with `project.new`), starts the engine on
//! the default audio device (falling back to a silent engine with a visible notice), and shows
//! the window.

mod agents;

use ph_commands::{Caller, Request};
use ph_control::{AudioMode, Studio, StudioConfig};
use ph_core::Project;
use ph_engine::EngineConfig;
use ph_ui::{AppPaths, Hooks, PhApp, settings};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

struct Args {
    project_dir: PathBuf,
    silent: bool,
    demo_agent: bool,
    assets: Option<PathBuf>,
    config: Option<PathBuf>,
    libraries: Vec<PathBuf>,
    buffer: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        project_dir: PathBuf::from("."),
        silent: false,
        demo_agent: false,
        assets: None,
        config: settings::default_config_dir(),
        libraries: Vec::new(),
        buffer: 256,
    };
    let mut it = std::env::args().skip(1);
    let mut dir = None;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--silent" => a.silent = true,
            "--demo-agent" => a.demo_agent = true,
            "--assets" => a.assets = Some(it.next().ok_or("--assets needs a folder")?.into()),
            "--config" => a.config = Some(it.next().ok_or("--config needs a folder")?.into()),
            "--library" => a
                .libraries
                .push(it.next().ok_or("--library needs a folder")?.into()),
            "--buffer" => {
                a.buffer = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--buffer needs a number of frames")?
            }
            "-h" | "--help" => {
                println!(
                    "usage: ph-app [project_dir] [--silent] [--demo-agent] [--assets DIR] [--config DIR] [--library DIR]... [--buffer N]"
                );
                std::process::exit(0);
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => dir = Some(PathBuf::from(other)),
        }
    }
    if let Some(d) = dir {
        a.project_dir = d;
    }
    Ok(a)
}

/// Opens the local MCP/JSON-RPC endpoint (`patternhaus-cli mcp` attaches to it) and connects
/// its grant prompts and client list to the Agents panel. Without it the app still runs.
fn start_local_agent_endpoint(studio: &Studio) -> Hooks {
    match agents::AppAgents::start(studio) {
        Ok(a) => Hooks {
            agents: Some(a as Arc<dyn ph_ui::AgentDirectory>),
        },
        Err(e) => {
            eprintln!("agent endpoint unavailable: {e}");
            Hooks::default()
        }
    }
}

fn load_or_new(dir: &std::path::Path) -> (Project, bool) {
    if dir.join("project.json").exists() {
        match ph_formats::load_project(dir) {
            Ok(p) => return (p, false),
            Err(e) => eprintln!(
                "could not open {}: {e}; starting a new project",
                dir.display()
            ),
        }
    }
    (Project::empty("Untitled"), true)
}

fn main() -> eframe::Result {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ph-app: {e}");
            std::process::exit(2);
        }
    };
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from));
    let cwd = std::env::current_dir().ok();
    let assets_dir = args
        .assets
        .clone()
        .or_else(|| settings::find_assets_dir(exe_dir.as_deref(), cwd.as_deref()))
        .unwrap_or_else(|| PathBuf::from("assets"));
    let project_dir = args.project_dir.clone();
    let (project, is_new) = load_or_new(&project_dir);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Patternhaus")
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Patternhaus",
        options,
        Box::new(move |cc| {
            let engine_cfg = EngineConfig {
                buffer_frames: args.buffer,
                ..EngineConfig::default()
            };
            let studio_cfg = |audio| StudioConfig {
                project_dir: project_dir.clone(),
                assets_dir: assets_dir.clone(),
                library_roots: args.libraries.clone(),
                exports_dir: None,
                engine: engine_cfg.clone(),
                audio,
            };
            let mut notice = None;
            let studio = if args.silent {
                notice = Some("Silent mode: no audio device (--silent)".to_string());
                Studio::start(studio_cfg(AudioMode::Silent), project.clone())
            } else {
                match Studio::start(studio_cfg(AudioMode::Device), project.clone()) {
                    Ok(s) => Ok(s),
                    Err(e) => {
                        notice = Some(format!("No audio device ({e}); running silently"));
                        Studio::start(studio_cfg(AudioMode::Silent), project.clone())
                    }
                }
            }
            .map_err(|e| format!("could not start the engine: {e}"))?;

            if is_new {
                let _ = studio.session.execute(
                    &Caller::human(),
                    Request {
                        confirm: true,
                        ..Request::new("project.new", serde_json::json!({}))
                    },
                );
            }
            let hooks = start_local_agent_endpoint(&studio);
            if args.demo_agent {
                let session = studio.session.clone();
                let stop = Arc::new(AtomicBool::new(false));
                std::thread::spawn(move || {
                    ph_ui::demo_agent::run(session, stop, Duration::from_secs(4))
                });
            }
            let paths = AppPaths {
                project_dir: project_dir.clone(),
                assets_dir: assets_dir.clone(),
                library_roots: args.libraries.clone(),
                config_dir: args.config.clone(),
            };
            let app = PhApp::new(&cc.egui_ctx, studio, paths, hooks, engine_cfg, notice)?;
            Ok(Box::new(app))
        }),
    )
}
