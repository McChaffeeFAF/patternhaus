//! Dropping files from the operating system onto the window (Explorer, Finder, ...).
//!
//! egui reports `hovered_files` while a drag is over the window and `dropped_files` on the
//! frame it lands. Both are turned into a [`Plan`] (what each file is) and then:
//!
//! - **hovering** highlights the target panel or row with the accent edge and glow, and the
//!   hint bar says what will happen;
//! - **dropping** sends commands as the human, never touching the project or the filesystem
//!   itself: `sample.import`, `file.import_midi`, or `file.open` (or `file.import_flp` for an FL Studio project) (through the Save / Don't
//!   save / Cancel question). A folder becomes a library folder, remembered in settings.
//!
//! The panels record where they were drawn this frame ([`DropZone`]); the zone under the pointer
//! picks a channel row (replace its sample) over the rack (new channels). Some platforms do not
//! report the pointer during a file drag; then the default panel for the kind of file is used.

use crate::core::{AfterPrompt, AppCore, PanelAction, PanelId, ToastKind};
use crate::{hint, mapping, paint, settings};
use egui::{Context, Id, LayerId, Order, Pos2, Rect};
use ph_core::ChannelId;
use std::path::{Path, PathBuf};

/// Audio extensions the sampler can decode.
pub const AUDIO_EXT: [&str; 7] = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg"];
/// Audio formats we recognise but cannot decode yet.
const LATER_EXT: [&str; 4] = ["m4a", "aac", "wma", "opus"];
/// The browser tab that lists library folders.
const LIBRARY_TAB: usize = 3;

/// A place a drop can land, recorded by the panels as they draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DropZone {
    Browser,
    Rack,
    RackRow(ChannelId),
    Playlist,
    PianoRoll,
}

/// What the person is dragging over the window right now, for tests and the hint.
#[derive(Clone, Debug, PartialEq)]
pub struct DropHover {
    /// The hint text ("Drop to add 3 samples as channels").
    pub text: String,
    /// The highlighted panels and rows.
    pub rects: Vec<Rect>,
}

/// What one dropped path is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Audio(PathBuf),
    Midi(PathBuf),
    /// An FL Studio project file (`.flp`): imported as a new project.
    Flp(PathBuf),
    /// A plain folder: becomes a library folder.
    Folder(PathBuf),
    /// A project folder (the path is the folder).
    Project(PathBuf),
    /// Not usable; the reason is shown to the person.
    Skip(String, &'static str),
}

pub fn classify(path: &Path) -> Item {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    if path.is_dir() {
        return if path.join("project.json").is_file() {
            Item::Project(path.to_path_buf())
        } else {
            Item::Folder(path.to_path_buf())
        };
    }
    if !path.exists() {
        return Item::Skip(name, "not found");
    }
    if name.eq_ignore_ascii_case("project.json") {
        return Item::Project(path.parent().unwrap_or(path).to_path_buf());
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if AUDIO_EXT.contains(&ext.as_str()) {
        Item::Audio(path.to_path_buf())
    } else if ext == "mid" || ext == "midi" {
        Item::Midi(path.to_path_buf())
    } else if ext == "flp" {
        Item::Flp(path.to_path_buf())
    } else if LATER_EXT.contains(&ext.as_str()) {
        Item::Skip(
            name,
            "that audio format is not supported yet (use WAV, AIFF, FLAC, MP3 or OGG)",
        )
    } else {
        Item::Skip(name, "not an audio, MIDI, FL Studio or project file")
    }
}

/// Everything dropped (or hovering), sorted by what it is, in drop order.
#[derive(Default, Debug, PartialEq)]
pub struct Plan {
    pub audio: Vec<PathBuf>,
    pub midi: Vec<PathBuf>,
    pub flp: Vec<PathBuf>,
    pub folders: Vec<PathBuf>,
    pub projects: Vec<PathBuf>,
    pub skipped: Vec<(String, &'static str)>,
}

impl Plan {
    pub fn from_paths(paths: &[PathBuf]) -> Plan {
        let mut plan = Plan::default();
        for p in paths {
            match classify(p) {
                Item::Audio(p) => plan.audio.push(p),
                Item::Midi(p) => plan.midi.push(p),
                Item::Flp(p) => plan.flp.push(p),
                Item::Folder(p) => plan.folders.push(p),
                Item::Project(p) => plan.projects.push(p),
                Item::Skip(n, why) => plan.skipped.push((n, why)),
            }
        }
        plan
    }
}

type Zones = [(Rect, DropZone)];

/// The topmost recorded zone under `pos` that `want` accepts.
fn zone_at(
    zones: &Zones,
    pos: Option<Pos2>,
    want: impl Fn(DropZone) -> bool,
) -> Option<(Rect, DropZone)> {
    let pos = pos?;
    zones
        .iter()
        .rev()
        .find(|(r, z)| r.contains(pos) && want(*z))
        .copied()
}

fn first_zone(zones: &Zones, want: DropZone) -> Option<Rect> {
    zones
        .iter()
        .rev()
        .find(|(_, z)| *z == want)
        .map(|(r, _)| *r)
}

fn audio_zone(z: DropZone) -> bool {
    matches!(
        z,
        DropZone::Rack | DropZone::RackRow(_) | DropZone::Playlist
    )
}

fn midi_zone(z: DropZone) -> bool {
    matches!(z, DropZone::Playlist | DropZone::PianoRoll)
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn folder_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
}

/// What a drop of `plan` would do at `pos`: the hint text and what to highlight.
pub fn describe(core: &AppCore, plan: &Plan, pos: Option<Pos2>) -> Option<DropHover> {
    let zones = &core.drop_zones;
    let mut parts: Vec<String> = Vec::new();
    let mut rects: Vec<Rect> = Vec::new();
    if let Some(p) = plan.projects.first() {
        parts.push(format!("Drop to open project {}", folder_name(p)));
        if plan.projects.len() + plan.audio.len() + plan.midi.len() + plan.folders.len() > 1 {
            parts.push("only the first project is opened".into());
        }
    } else if let Some(p) = plan.flp.first() {
        parts.push(format!(
            "FL Studio: drop to import {} as a new project",
            folder_name(p)
        ));
        if plan.flp.len() + plan.audio.len() + plan.midi.len() + plan.folders.len() > 1 {
            parts.push("only the first FL Studio project is imported".into());
        }
    } else {
        if !plan.audio.is_empty() {
            let n = plan.audio.len();
            match zone_at(zones, pos, audio_zone) {
                Some((r, DropZone::RackRow(c))) if n == 1 => {
                    let name = core
                        .project
                        .channels
                        .get(&c)
                        .map_or("the channel".into(), |c| c.name.clone());
                    parts.push(format!("Drop to replace {name}'s sample"));
                    rects.push(r);
                }
                Some((r, DropZone::Playlist)) => {
                    parts.push(format!(
                        "Drop to add {} as channels (audio clips arrive in a later milestone)",
                        count(n, "sample", "samples")
                    ));
                    rects.push(r);
                }
                hit => {
                    parts.push(format!(
                        "Drop to add {} as {}",
                        count(n, "sample", "samples"),
                        if n == 1 { "a channel" } else { "channels" }
                    ));
                    // highlight the rack panel (a row hit still adds channels when n > 1)
                    let rack = match (hit, pos) {
                        (Some(_), Some(p)) => zones
                            .iter()
                            .rev()
                            .find(|(r, z)| *z == DropZone::Rack && r.contains(p))
                            .map(|(r, _)| *r),
                        _ => first_zone(zones, DropZone::Rack),
                    };
                    rects.extend(rack.or_else(|| first_zone(zones, DropZone::Rack)));
                }
            }
        }
        if !plan.midi.is_empty() {
            let n = plan.midi.len();
            parts.push(if n == 1 {
                "MIDI: drop to import as a new pattern".into()
            } else {
                format!("MIDI: drop to import {n} files as new patterns")
            });
            let rect = zone_at(zones, pos, midi_zone)
                .map(|(r, _)| r)
                .or_else(|| first_zone(zones, DropZone::Playlist))
                .or_else(|| first_zone(zones, DropZone::PianoRoll));
            rects.extend(rect);
        }
        if !plan.folders.is_empty() {
            let n = plan.folders.len();
            parts.push(if n == 1 {
                format!(
                    "Drop to add {} to the library",
                    folder_name(&plan.folders[0])
                )
            } else {
                format!("Drop to add {n} folders to the library")
            });
            rects.extend(first_zone(zones, DropZone::Browser));
        }
    }
    if !plan.skipped.is_empty() {
        parts.push(format!(
            "{} will be skipped",
            count(plan.skipped.len(), "file", "files")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some(DropHover {
        text: parts.join(" \u{b7} "),
        rects,
    })
}

/// Runs once per frame after the panels were drawn: shows the hover state and carries out a
/// drop.
pub fn frame(core: &mut AppCore, ctx: &Context) {
    let (hovered, dropped, pos) = ctx.input(|i| {
        (
            i.raw
                .hovered_files
                .iter()
                .filter_map(|h| h.path.clone())
                .collect::<Vec<_>>(),
            i.raw
                .dropped_files
                .iter()
                .map(|d| d.path().to_path_buf())
                .collect::<Vec<_>>(),
            i.pointer.latest_pos(),
        )
    });
    core.drop_hover = None;
    if !dropped.is_empty() {
        let plan = Plan::from_paths(&dropped);
        let zone = zone_at(&core.drop_zones, pos, audio_zone).map(|(_, z)| z);
        execute(core, plan, zone);
        ctx.request_repaint();
    } else if !hovered.is_empty() {
        let plan = Plan::from_paths(&hovered);
        if let Some(h) = describe(core, &plan, pos) {
            let accent = core.theme.colors.accent;
            let p = ctx.layer_painter(LayerId::new(
                Order::Foreground,
                Id::new("ph_drop_highlight"),
            ));
            for r in &h.rects {
                paint::glow_rect(&p, *r, 3, accent, 1.0);
                p.rect_stroke(
                    r.shrink(1.0),
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(2.0, accent),
                    egui::StrokeKind::Inside,
                );
                p.rect_filled(*r, egui::CornerRadius::same(3), accent.gamma_multiply(0.07));
            }
            hint::announce(
                ctx,
                ph_control::UiHint {
                    name: h.text.clone(),
                    value: String::new(),
                    gestures: String::new(),
                },
            );
            core.drop_hover = Some(h);
        }
        ctx.request_repaint();
    }
    core.drop_zones.clear();
}

/// Turns a drop into commands (or a question) and notices.
pub fn execute(core: &mut AppCore, plan: Plan, zone: Option<DropZone>) {
    if !plan.skipped.is_empty() {
        let shown: Vec<String> = plan
            .skipped
            .iter()
            .take(4)
            .map(|(n, why)| format!("{n} ({why})"))
            .collect();
        let more = plan.skipped.len().saturating_sub(4);
        core.toast(
            ToastKind::Warn,
            format!(
                "Skipped {}{}",
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ),
        );
    }
    if plan.projects.is_empty()
        && let Some(file) = plan.flp.first()
    {
        let others = plan.flp.len() - 1 + plan.audio.len() + plan.midi.len() + plan.folders.len();
        if others > 0 {
            core.toast(
                ToastKind::Warn,
                format!(
                    "Importing {} only; the other {others} dropped items were ignored",
                    folder_name(file)
                ),
            );
        }
        core.ask_or_do(AfterPrompt::ImportFlp {
            path: file.to_string_lossy().into_owned(),
        });
        return;
    }
    if let Some(dir) = plan.projects.first() {
        let others =
            plan.projects.len() - 1 + plan.audio.len() + plan.midi.len() + plan.folders.len();
        if others > 0 {
            core.toast(
                ToastKind::Warn,
                format!(
                    "Opening {} only; the other {others} dropped items were ignored",
                    folder_name(dir)
                ),
            );
        }
        core.ask_or_do(AfterPrompt::OpenProject {
            path: dir.to_string_lossy().into_owned(),
        });
        return;
    }
    if !plan.audio.is_empty() {
        let paths: Vec<String> = plan
            .audio
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let channel = match zone {
            Some(DropZone::RackRow(c)) if paths.len() == 1 => Some(c),
            _ => None,
        };
        core.send(mapping::sample_import(&paths, channel));
    }
    for m in &plan.midi {
        core.send(mapping::import_midi(&m.to_string_lossy()));
    }
    for f in &plan.folders {
        core.add_library_folder(f);
    }
}

impl AppCore {
    /// Adds a library folder for this session and remembers it in the settings file. The
    /// browser opens on the Library tab.
    pub fn add_library_folder(&mut self, dir: &Path) {
        if self.paths.library_roots.iter().any(|r| r == dir) {
            self.toast(
                ToastKind::Info,
                format!("{} is already in the library", folder_name(dir)),
            );
        } else {
            self.paths.library_roots.push(dir.to_path_buf());
            self.browser.add_library(dir);
            if let Some(c) = self.paths.config_dir.clone() {
                let mut prefs = settings::load_prefs(Some(&c));
                if !prefs.library_folders.iter().any(|f| f == dir) {
                    prefs.library_folders.push(dir.to_path_buf());
                }
                let _ = settings::save_prefs(&c, &prefs);
            }
            self.toast(
                ToastKind::Info,
                format!("Added {} to the library", folder_name(dir)),
            );
        }
        self.browser.tab = LIBRARY_TAB;
        self.panel_requests
            .push((PanelId::Browser, PanelAction::Open));
    }
}
