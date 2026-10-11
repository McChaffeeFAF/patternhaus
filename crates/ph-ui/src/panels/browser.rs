//! Browser: factory kits, Haus Synth presets, and files in the project and library folders.
//! Click to preview, double-click or drag to the rack to make a channel.

use crate::browser_model::{self as bm, Dragged, Kind, Node, RefStyle};
use crate::core::{AppCore, AppPaths};
use crate::mapping;
use egui::{CollapsingHeader, RichText, Sense, Ui};
use std::path::PathBuf;

pub struct BrowserState {
    pub factory: Vec<Node>,
    pub presets: Vec<Node>,
    pub project: Vec<Node>,
    pub library: Vec<(PathBuf, Vec<Node>)>,
    pub filter: String,
    pub selected: Option<String>,
    paths: AppPaths,
}

impl BrowserState {
    pub fn new(paths: &AppPaths) -> Self {
        let mut s = Self {
            factory: Vec::new(),
            presets: Vec::new(),
            project: Vec::new(),
            library: Vec::new(),
            filter: String::new(),
            selected: None,
            paths: paths.clone(),
        };
        s.rescan();
        s
    }

    pub fn rescan(&mut self) {
        let p = &self.paths;
        self.factory = bm::factory_samples(&p.assets_dir);
        self.presets = bm::synth_presets(&ph_engine::catalog::synth_presets());
        self.project = bm::scan(
            &p.project_dir,
            &RefStyle::RelativeTo(p.project_dir.clone()),
            5,
        );
        self.library = p
            .library_roots
            .iter()
            .map(|r| (r.clone(), bm::scan(r, &RefStyle::Absolute, 6)))
            .collect();
    }

    pub fn set_project_dir(&mut self, dir: PathBuf) {
        self.paths.project_dir = dir;
        self.rescan();
    }
}

/// Does this node or anything below it match the filter?
fn matches(n: &Node, f: &str) -> bool {
    f.is_empty() || n.name.to_lowercase().contains(f) || n.children.iter().any(|c| matches(c, f))
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::Global);
    let theme = core.theme.clone();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut core.browser.filter)
                .hint_text("Filter")
                .desired_width(140.0),
        );
        if ui.small_button("Rescan").clicked() {
            core.browser.rescan();
        }
    });
    ui.label(
        RichText::new("Click to preview, double-click or drag to the channel rack to add.")
            .size(10.0)
            .color(theme.colors.dim),
    );
    ui.separator();
    let filter = core.browser.filter.to_lowercase();
    let mut actions: Vec<Action> = Vec::new();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            section(
                ui,
                "Factory samples",
                &core.browser.factory,
                &filter,
                &mut actions,
                true,
                &core.browser.selected,
            );
            section(
                ui,
                "Haus Synth presets",
                &core.browser.presets,
                &filter,
                &mut actions,
                false,
                &core.browser.selected,
            );
            section(
                ui,
                "Project files",
                &core.browser.project,
                &filter,
                &mut actions,
                false,
                &core.browser.selected,
            );
            for (root, nodes) in &core.browser.library {
                let title = format!(
                    "Library: {}",
                    root.file_name()
                        .map_or_else(|| root.to_string_lossy(), |n| n.to_string_lossy())
                );
                section(
                    ui,
                    &title,
                    nodes,
                    &filter,
                    &mut actions,
                    false,
                    &core.browser.selected,
                );
            }
        });
    for a in actions {
        match a {
            Action::Preview(key, kind) => {
                core.browser.selected = Some(key);
                core.audition(&kind);
            }
            Action::Add(name, kind) => core.add_channel_from(&name, &kind),
            Action::AddKit(samples) => {
                let reqs: Vec<_> = samples
                    .iter()
                    .take(16)
                    .map(|(n, r)| {
                        let inst = ph_core::InstrumentSpec::Sampler {
                            sample: Some(r.clone()),
                            root_key: 60,
                            params: Default::default(),
                        };
                        mapping::channel_add(&bm::channel_name(n), &inst, None)
                    })
                    .collect();
                core.send_batch(reqs);
            }
        }
    }
}

enum Action {
    Preview(String, Kind),
    Add(String, Kind),
    AddKit(Vec<(String, String)>),
}

fn section(
    ui: &mut Ui,
    title: &str,
    nodes: &[Node],
    filter: &str,
    actions: &mut Vec<Action>,
    open: bool,
    selected: &Option<String>,
) {
    CollapsingHeader::new(RichText::new(title).strong())
        .default_open(open)
        .show(ui, |ui| {
            if nodes.is_empty() {
                ui.label(RichText::new("(nothing here)").weak());
            }
            for n in nodes {
                node(ui, n, filter, actions, selected, title);
            }
        });
}

fn node(
    ui: &mut Ui,
    n: &Node,
    filter: &str,
    actions: &mut Vec<Action>,
    selected: &Option<String>,
    path: &str,
) {
    if !matches(n, filter) {
        return;
    }
    let key = format!("{path}/{}", n.name);
    if n.is_folder() {
        let header = CollapsingHeader::new(&n.name)
            .id_salt(&key)
            .open(if filter.is_empty() { None } else { Some(true) })
            .show(ui, |ui| {
                for c in &n.children {
                    node(ui, c, filter, actions, selected, &key);
                }
            });
        let samples = n.samples();
        if !samples.is_empty() {
            header.header_response.context_menu(|ui| {
                if ui
                    .button(format!(
                        "Add {} sample(s) as channels",
                        samples.len().min(16)
                    ))
                    .clicked()
                {
                    actions.push(Action::AddKit(
                        samples
                            .iter()
                            .map(|(a, b)| (a.to_string(), b.to_string()))
                            .collect(),
                    ));
                    ui.close();
                }
            });
        }
        return;
    }
    let is_sel = selected.as_deref() == Some(key.as_str());
    let text = if matches!(n.kind, Kind::Preset { .. }) {
        format!("~ {}", n.name)
    } else {
        n.name.clone()
    };
    let resp = ui.add(egui::Button::selectable(is_sel, text).sense(Sense::click_and_drag()));
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            Dragged {
                name: n.name.clone(),
                kind: n.kind.clone(),
            },
        );
    }
    if resp.double_clicked() {
        actions.push(Action::Add(n.name.clone(), n.kind.clone()));
    } else if resp.clicked() {
        actions.push(Action::Preview(key, n.kind.clone()));
    }
    resp.context_menu(|ui| {
        if ui.button("Add as channel").clicked() {
            actions.push(Action::Add(n.name.clone(), n.kind.clone()));
            ui.close();
        }
    });
}
