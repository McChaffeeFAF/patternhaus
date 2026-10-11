//! Per-user settings: keymap file, theme choice and saved layouts.

use crate::keymap::Keymap;
use crate::theme::Theme;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A saved window layout: every window's rectangle, visibility, maximized state, stacking and
/// detach flag, plus the browser dock.
#[derive(Serialize, Deserialize)]
pub struct Layout {
    pub workspace: Workspace,
    /// Toolbar items and their order; layouts saved before the toolbar existed lack it.
    #[serde(default)]
    pub toolbar: crate::toolbar::ToolbarConfig,
}

pub fn keymap_path(config: &Path) -> PathBuf {
    config.join("keymap.toml")
}
pub fn layout_path(config: &Path) -> PathBuf {
    config.join("layout.ron")
}
pub fn project_layout_path(project_dir: &Path) -> PathBuf {
    project_dir.join("layout.ron")
}
pub fn settings_path(config: &Path) -> PathBuf {
    config.join("settings.toml")
}

/// The default config folder, e.g. `%APPDATA%/patternhaus`.
pub fn default_config_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    };
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    base.map(|d| d.join("patternhaus"))
}

/// The factory `assets/` folder: next to the executable (installed), or found by walking up from
/// the executable / working directory (development inside the repo).
pub fn find_assets_dir(exe_dir: Option<&Path>, cwd: Option<&Path>) -> Option<PathBuf> {
    let is_assets = |p: &Path| p.join("PROVENANCE.toml").exists();
    if let Some(e) = exe_dir {
        let next_to = e.join("assets");
        if is_assets(&next_to) {
            return Some(next_to);
        }
    }
    for start in [exe_dir, cwd].into_iter().flatten() {
        let mut dir = Some(start);
        while let Some(d) = dir {
            let candidate = d.join("assets");
            if is_assets(&candidate) {
                return Some(candidate);
            }
            dir = d.parent();
        }
    }
    None
}

/// Defaults with the user's keymap file layered on top. A broken file is reported and ignored.
pub fn load_keymap(config: Option<&Path>) -> (Keymap, Option<String>) {
    let defaults = Keymap::defaults();
    let Some(path) = config.map(keymap_path) else {
        return (defaults, None);
    };
    match std::fs::read_to_string(&path) {
        Err(_) => (defaults, None),
        Ok(text) => match Keymap::parse(&text) {
            Ok(user) => (defaults.with_user(user), None),
            Err(e) => (defaults, Some(format!("{}: {e}", path.display()))),
        },
    }
}

/// Small per-user preferences (`settings.toml`).
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Prefs {
    /// Theme id (built-in, or a file in `<config>/themes`).
    pub theme: Option<String>,
    /// Skip decorative animation (flashes, glow fades, easing, slides).
    pub reduce_motion: bool,
    /// Library folders added by dropping a folder on the browser; shown on the next launch too.
    pub library_folders: Vec<PathBuf>,
}

pub fn load_prefs(config: Option<&Path>) -> Prefs {
    config
        .map(settings_path)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_prefs(config: &Path, prefs: &Prefs) -> std::io::Result<()> {
    std::fs::create_dir_all(config)?;
    std::fs::write(
        settings_path(config),
        toml::to_string(prefs).unwrap_or_default(),
    )
}

pub fn load_theme(config: Option<&Path>) -> (Theme, Option<String>) {
    match load_prefs(config).theme.as_deref() {
        None => (Theme::default_theme(), None),
        Some(name) => match crate::theme::load_from(name, config) {
            Ok(t) => (t, None),
            Err(e) => (Theme::default_theme(), Some(format!("theme {name}: {e}"))),
        },
    }
}

pub fn save_theme(config: &Path, name: &str) -> std::io::Result<()> {
    let mut prefs = load_prefs(Some(config));
    prefs.theme = Some(name.to_string());
    save_prefs(config, &prefs)
}

pub fn save_keymap(config: &Path, km: &Keymap) -> std::io::Result<()> {
    std::fs::create_dir_all(config)?;
    std::fs::write(keymap_path(config), km.to_toml())
}

pub fn save_layout(path: &Path, layout: &Layout) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = ron::ser::to_string_pretty(layout, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn load_layout(path: &Path) -> Option<Layout> {
    let text = std::fs::read_to_string(path).ok()?;
    match ron::from_str(&text) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("ignoring layout {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_round_trip_through_a_file() {
        use crate::workspace::{Rect, WindowId};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.ron");
        let mut workspace = Workspace::new(Rect::new(0.0, 0.0, 1200.0, 800.0));
        workspace.show(WindowId::Mixer);
        workspace.drag_move(WindowId::Mixer, 200.0, 300.0);
        let layout = Layout {
            workspace,
            toolbar: Default::default(),
        };
        save_layout(&path, &layout).unwrap();
        let back = load_layout(&path).unwrap();
        assert_eq!(back.workspace, layout.workspace);
        assert_eq!(back.toolbar, layout.toolbar);
        assert!(load_layout(&dir.path().join("missing.ron")).is_none());
        // a layout from the old docked shell is ignored, not fatal
        std::fs::write(&path, "(dock: (surfaces: []), detached: [])").unwrap();
        assert!(load_layout(&path).is_none());
    }

    #[test]
    fn assets_are_found_next_to_the_exe_or_up_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("target/debug");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_assets_dir(Some(&deep), None), None);
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/PROVENANCE.toml"), "").unwrap();
        assert_eq!(
            find_assets_dir(Some(&deep), None),
            Some(dir.path().join("assets"))
        );
        assert_eq!(
            find_assets_dir(None, Some(&deep)),
            Some(dir.path().join("assets"))
        );
        std::fs::create_dir_all(deep.join("assets")).unwrap();
        std::fs::write(deep.join("assets/PROVENANCE.toml"), "").unwrap();
        assert_eq!(
            find_assets_dir(Some(&deep), None),
            Some(deep.join("assets"))
        );
    }

    #[test]
    fn keymap_and_theme_files_layer_on_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            keymap_path(dir.path()),
            "[[bind]]\nkey = \"F5\"\ncommand = \"app.toggle_panel\"\nparams = { panel = \"mixer\" }\n",
        )
        .unwrap();
        let (km, err) = load_keymap(Some(dir.path()));
        assert!(err.is_none());
        let none = egui::Modifiers::NONE;
        assert_eq!(
            km.lookup(egui::Key::F5, &none, crate::keymap::Context::Global)
                .unwrap()
                .params["panel"],
            "mixer"
        );
        std::fs::write(keymap_path(dir.path()), "garbage [").unwrap();
        let (km, err) = load_keymap(Some(dir.path()));
        assert!(err.is_some());
        assert!(
            km.lookup(egui::Key::F9, &none, crate::keymap::Context::Global)
                .is_some()
        );

        save_theme(dir.path(), "red-line").unwrap();
        let (t, err) = load_theme(Some(dir.path()));
        assert!(err.is_none() && t.id == "red-line");
        // saving the theme keeps the other preferences
        let mut p = load_prefs(Some(dir.path()));
        p.reduce_motion = true;
        save_prefs(dir.path(), &p).unwrap();
        save_theme(dir.path(), "neon-slate").unwrap();
        assert!(load_prefs(Some(dir.path())).reduce_motion);
        // an old settings file naming the retired themes falls back to the default
        save_theme(dir.path(), "light").unwrap();
        assert_eq!(load_theme(Some(dir.path())).0.id, "neon-slate");
    }
}
