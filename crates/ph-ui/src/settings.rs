//! Per-user settings: keymap file, theme choice and saved layouts.

use crate::core::PanelId;
use crate::keymap::Keymap;
use crate::theme::Theme;
use egui_dock::DockState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A saved window layout: the dock tree plus panels living in their own OS windows.
#[derive(Serialize, Deserialize)]
pub struct Layout {
    pub dock: DockState<PanelId>,
    #[serde(default)]
    pub detached: BTreeSet<PanelId>,
}

pub fn keymap_path(config: &Path) -> PathBuf {
    config.join("keymap.toml")
}
pub fn layout_path(config: &Path) -> PathBuf {
    config.join("layout.json")
}
pub fn project_layout_path(project_dir: &Path) -> PathBuf {
    project_dir.join("layout.json")
}
pub fn settings_path(config: &Path) -> PathBuf {
    config.join("settings.toml")
}

/// The default config folder, e.g. `%APPDATA%/patternhaus`.
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("patternhaus"))
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

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    #[serde(default)]
    theme: Option<String>,
}

pub fn load_theme(config: Option<&Path>) -> (Theme, Option<String>) {
    let Some(path) = config.map(settings_path) else {
        return (Theme::dark(), None);
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (Theme::dark(), None);
    };
    let s: Settings = toml::from_str(&text).unwrap_or_default();
    match s.theme.as_deref() {
        None => (Theme::dark(), None),
        Some(name) => match crate::theme::load(name) {
            Ok(t) => (t, None),
            Err(e) => (Theme::dark(), Some(format!("theme {name}: {e}"))),
        },
    }
}

pub fn save_theme(config: &Path, name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(config)?;
    let s = Settings {
        theme: Some(name.to_string()),
    };
    std::fs::write(
        settings_path(config),
        toml::to_string(&s).unwrap_or_default(),
    )
}

pub fn save_keymap(config: &Path, km: &Keymap) -> std::io::Result<()> {
    std::fs::create_dir_all(config)?;
    std::fs::write(keymap_path(config), km.to_toml())
}

pub fn save_layout(path: &Path, layout: &Layout) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(layout).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn load_layout(path: &Path) -> Option<Layout> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The layout a fresh install opens with.
pub fn default_dock() -> DockState<PanelId> {
    use egui_dock::NodeIndex;
    let mut dock = DockState::new(vec![PanelId::ChannelRack]);
    let tree = dock.main_surface_mut();
    let [_, below] = tree.split_below(
        NodeIndex::root(),
        0.27,
        vec![PanelId::Playlist, PanelId::PianoRoll],
    );
    tree.split_below(
        below,
        0.58,
        vec![
            PanelId::Mixer,
            PanelId::Inspector,
            PanelId::History,
            PanelId::Agents,
        ],
    );
    tree.split_left(NodeIndex::root(), 0.17, vec![PanelId::Browser]);
    dock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_every_panel_once() {
        let dock = default_dock();
        let mut seen: Vec<PanelId> = dock.iter_all_tabs().map(|(_, t)| *t).collect();
        seen.sort();
        let mut all = PanelId::ALL.to_vec();
        all.sort();
        assert_eq!(seen, all);
    }

    #[test]
    fn layouts_round_trip_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.json");
        let layout = Layout {
            dock: default_dock(),
            detached: [PanelId::Mixer].into(),
        };
        save_layout(&path, &layout).unwrap();
        let back = load_layout(&path).unwrap();
        assert_eq!(back.detached, layout.detached);
        assert_eq!(back.dock.iter_all_tabs().count(), 8);
        assert!(load_layout(&dir.path().join("missing.json")).is_none());
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

        save_theme(dir.path(), "light").unwrap();
        let (t, err) = load_theme(Some(dir.path()));
        assert!(err.is_none() && !t.dark);
    }
}
