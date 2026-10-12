//! Starting a Studio without a UI: for `patternhaus-cli`, CI and tests.

use crate::{AudioMode, Studio, StudioConfig};
use ph_commands::Grant;
use ph_core::Project;
use ph_engine::EngineConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Agent, Grants};

#[derive(Clone, Debug)]
pub struct HeadlessOptions {
    /// The project folder. Opened when it holds a `project.json`, otherwise a new empty
    /// project is started there (nothing is written until `file.save`).
    pub project_dir: PathBuf,
    /// Factory assets; found next to the executable or in the source tree when `None`.
    pub assets_dir: Option<PathBuf>,
    pub audio: AudioMode,
    /// The Patternhaus projects folder (see [`StudioConfig::projects_dir`]); front ends pass
    /// `default_projects_dir()`, tests a temp folder or `None`.
    pub projects_dir: Option<PathBuf>,
    /// Scopes every client gets (headless sessions are started on purpose, so the default
    /// the CLI passes is `full`).
    pub grant: Grant,
}

/// Looks for the `assets/` folder: `$PATTERNHAUS_ASSETS`, then next to the executable and
/// up to three folders above it (a cargo `target/debug` build), then the source tree.
pub fn find_assets_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PATTERNHAUS_ASSETS") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent();
    for _ in 0..4 {
        let d = dir?;
        if d.join("assets").join("PROVENANCE.toml").exists() {
            return Some(d.join("assets"));
        }
        dir = d.parent();
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    src.join("PROVENANCE.toml").exists().then_some(src)
}

/// Starts a headless Studio and an [`Agent`] for it.
pub fn start_headless(opts: HeadlessOptions) -> Result<(Studio, Arc<Agent>), String> {
    let assets =
        opts.assets_dir.clone().or_else(find_assets_dir).ok_or(
            "cannot find the factory assets folder; pass --assets or set PATTERNHAUS_ASSETS",
        )?;
    std::fs::create_dir_all(&opts.project_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.project_dir.display()))?;
    let project = if opts.project_dir.join("project.json").exists() {
        ph_formats::load_project(&opts.project_dir).map_err(|e| e.to_string())?
    } else {
        let title = opts
            .project_dir
            .file_name()
            .map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
        Project::empty(&title)
    };
    let config = StudioConfig {
        project_dir: opts.project_dir.clone(),
        assets_dir: assets,
        library_roots: Vec::new(),
        exports_dir: None,
        projects_dir: opts.projects_dir.clone(),
        engine: EngineConfig::default(),
        audio: opts.audio,
    };
    let studio = Studio::start(config, project)?;
    let agent = Agent::new(studio.handle(), Grants::fixed(opts.grant));
    Ok((studio, agent))
}
