//! Browser: factory kits, Haus Synth presets, and files in the project and library folders.
//! Click to preview, double-click or drag to the rack to make a channel.

use crate::browser_model::{self as bm, Dragged, Kind, Node, RefStyle};
use crate::core::{AppCore, AppPaths};
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::mapping;
use crate::widgets::{self, icon_button, menu_item};
use egui::{Rect, RichText, Sense, Ui};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct BrowserState {
    pub factory: Vec<Node>,
    pub presets: Vec<Node>,
    pub project: Vec<Node>,
    pub library: Vec<(PathBuf, Vec<Node>)>,
    pub filter: String,
    pub selected: Option<String>,
    /// The sample reference whose waveform the preview shows.
    pub preview_ref: Option<String>,
    /// The tab shown (see [`TABS`]).
    pub tab: usize,
    /// Folders that are open, by their path in the tree.
    pub open: HashSet<String>,
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
            preview_ref: None,
            tab: 0,
            open: ["samples/drums".to_string()].into_iter().collect(),
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

    /// Shows another library folder (the caller keeps `AppPaths` and the settings in step).
    pub fn add_library(&mut self, dir: &std::path::Path) {
        if !self.paths.library_roots.iter().any(|r| r == dir) {
            self.paths.library_roots.push(dir.to_path_buf());
        }
        self.rescan();
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

/// The browser's tabs, in order.
pub const TABS: [&str; 4] = ["Samples", "Presets", "Project", "Library"];

/// One visible line of the tree.
struct Row<'a> {
    node: &'a Node,
    depth: usize,
    key: String,
}

/// Flattens the open part of the tree that passes the filter.
fn flatten<'a>(
    nodes: &'a [Node],
    filter: &str,
    open: &HashSet<String>,
    depth: usize,
    path: &str,
    out: &mut Vec<Row<'a>>,
) {
    for n in nodes {
        if !matches(n, filter) {
            continue;
        }
        let key = format!("{path}/{}", n.name);
        out.push(Row {
            node: n,
            depth,
            key: key.clone(),
        });
        if n.is_folder() && (!filter.is_empty() || open.contains(&key)) {
            flatten(&n.children, filter, open, depth + 1, &key, out);
        }
    }
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::Global);
    let theme = core.theme.clone();
    let c = &theme.colors;

    // tabs
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, name) in TABS.iter().enumerate() {
            if widgets::tab(ui, &theme, name, core.browser.tab == i).clicked() {
                core.browser.tab = i;
            }
        }
    });
    ui.add_space(2.0);
    // search
    ui.horizontal(|ui| {
        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 22.0), Sense::hover());
        Icon::Search.paint(ui.painter(), icon_rect, c.dim);
        ui.add(
            egui::TextEdit::singleline(&mut core.browser.filter)
                .hint_text("Search")
                .desired_width((ui.available_width() - 34.0).max(40.0)),
        )
        .hint(
            "Search",
            &core.browser.filter.clone(),
            "type to filter the tree",
        );
        if icon_button(
            ui,
            &theme,
            Icon::Loop,
            false,
            "Rescan",
            "reads the folders again",
        )
        .clicked()
        {
            core.browser.rescan();
        }
    });
    ui.add_space(2.0);

    let filter = core.browser.filter.to_lowercase();
    let tab = core.browser.tab.min(TABS.len() - 1);
    let owned: Vec<Node>;
    let (nodes, root): (&[Node], &str) = match tab {
        0 => (&core.browser.factory, "samples"),
        1 => (&core.browser.presets, "presets"),
        2 => (&core.browser.project, "project"),
        _ => {
            // every library root as a top-level folder
            owned = core
                .browser
                .library
                .iter()
                .map(|(root, children)| Node {
                    name: root
                        .file_name()
                        .map_or_else(|| root.to_string_lossy(), |n| n.to_string_lossy())
                        .into_owned(),
                    kind: Kind::Folder,
                    children: children.clone(),
                })
                .collect();
            (&owned, "library")
        }
    };
    let open_set = core.browser.open.clone();
    let selected = core.browser.selected.clone();
    let mut rows = Vec::new();
    flatten(nodes, &filter, &open_set, 0, root, &mut rows);

    let mut actions: Vec<Action> = Vec::new();
    let mut toggles: Vec<String> = Vec::new();
    // leave room for the preview waveform and the hint line under the tree
    let tree_h = (ui.available_height() - 96.0).max(80.0);
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.label(
            RichText::new(if filter.is_empty() {
                "Nothing here yet."
            } else {
                "Nothing matches."
            })
            .color(c.dim),
        );
    }
    egui::ScrollArea::vertical()
        .id_salt(("browser_tree", tab))
        .auto_shrink(false)
        .max_height(tree_h)
        .show_rows(ui, ROW_H, rows.len(), |ui, range| {
            for row in &rows[range] {
                let open = open_set.contains(&row.key) || !filter.is_empty();
                tree_row(
                    ui,
                    row,
                    selected.as_deref(),
                    open,
                    &theme,
                    &mut actions,
                    &mut toggles,
                );
            }
        });
    for k in toggles {
        if !core.browser.open.remove(&k) {
            core.browser.open.insert(k);
        }
    }

    // the preview waveform of the selected sample
    ui.add_space(4.0);
    preview(ui, core, &theme);
    ui.label(
        RichText::new("Click to preview, double-click or drag to the rack to add.")
            .size(11.0)
            .color(c.faint),
    );

    for a in actions {
        match a {
            Action::Preview(key, kind) => {
                core.browser.selected = Some(key);
                core.browser.preview_ref = match &kind {
                    Kind::Sample { reference } => Some(reference.clone()),
                    _ => None,
                };
                core.audition(&kind);
            }
            Action::Add(name, kind) => core.add_channel_from(&name, &kind),
            Action::AddKit(samples) => {
                let reqs: Vec<_> = samples
                    .iter()
                    .take(16)
                    .map(|(n, r)| {
                        let inst = ph_core::InstrumentSpec::Sampler {
                            slices: Vec::new(),
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

const ROW_H: f32 = 22.0;

enum Action {
    Preview(String, Kind),
    Add(String, Kind),
    AddKit(Vec<(String, String)>),
}

fn tree_row(
    ui: &mut Ui,
    row: &Row<'_>,
    selected: Option<&str>,
    open: bool,
    theme: &crate::theme::Theme,
    actions: &mut Vec<Action>,
    toggles: &mut Vec<String>,
) {
    let c = &theme.colors;
    let n = row.node;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        Sense::click_and_drag(),
    );
    let is_sel = selected == Some(row.key.as_str());
    let p = ui.painter();
    if is_sel {
        p.rect_filled(
            rect,
            egui::CornerRadius::same(2),
            c.accent.gamma_multiply(0.16),
        );
        p.rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())),
            egui::CornerRadius::ZERO,
            c.accent,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, egui::CornerRadius::same(2), c.bg3);
    }
    let mut x = rect.left() + 6.0 + row.depth as f32 * 14.0;
    if n.is_folder() {
        let chev = if open { Icon::Down } else { Icon::Right };
        chev.paint(
            p,
            Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), egui::vec2(16.0, 16.0)),
            c.dim,
        );
    }
    x += 16.0;
    let icon = match n.kind {
        Kind::Folder => Icon::Folder,
        Kind::Sample { .. } => Icon::Sample,
        Kind::Preset { .. } => Icon::Preset,
    };
    icon.paint(
        p,
        Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), egui::vec2(16.0, 16.0)),
        match n.kind {
            Kind::Folder => c.warn,
            Kind::Sample { .. } => c.accent,
            Kind::Preset { .. } => c.agent,
        },
    );
    x += 22.0;
    // names are the user's, in the case they were typed
    crate::paint::text_fit_on(
        &p.with_clip_rect(rect),
        &resp,
        (egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER),
        &n.name,
        egui::FontId::new(13.0, crate::fonts::body()),
        if is_sel { c.accent_hot } else { c.text },
        rect.right() - x - 6.0,
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, n.name.as_str()));
    let gestures = match &n.kind {
        Kind::Folder => "click to open or close; right-click to add its samples",
        Kind::Sample { .. } => "click to preview, double-click or drag to the rack to add",
        Kind::Preset { .. } => "click to hear it, double-click or drag to the rack to add",
    };
    let kind_name = match &n.kind {
        Kind::Folder => "Folder",
        Kind::Sample { .. } => "Sample",
        Kind::Preset { .. } => "Preset",
    };
    let resp = resp.hint(&n.name, kind_name, gestures);
    if n.is_folder() {
        if resp.clicked() {
            toggles.push(row.key.clone());
        }
        let samples = n.samples();
        if !samples.is_empty() {
            resp.context_menu(|ui| {
                if menu_item(
                    ui,
                    &format!("Add {} sample(s) as channels", samples.len().min(16)),
                    "",
                )
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
        actions.push(Action::Preview(row.key.clone(), n.kind.clone()));
    }
    resp.context_menu(|ui| {
        if menu_item(ui, "Add as channel", "").clicked() {
            actions.push(Action::Add(n.name.clone(), n.kind.clone()));
            ui.close();
        }
        crate::hint::standard_entries(
            ui,
            false,
            &ph_control::UiHint {
                name: n.name.clone(),
                value: kind_name.into(),
                gestures: gestures.into(),
            },
        );
    });
}

/// The waveform of the selected sample, or a hint when nothing is selected.
fn preview(ui: &mut Ui, core: &mut AppCore, theme: &crate::theme::Theme) {
    let c = &theme.colors;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), Sense::hover());
    crate::paint::well(ui.painter(), rect, 3, c.lcd, theme);
    let reference = core.browser.preview_ref.clone();
    let p = ui.painter();
    match reference {
        Some(r) => {
            let (assets, project_dir) = (
                core.paths.assets_dir.clone(),
                core.paths.project_dir.clone(),
            );
            match core.inspector.wave(&r, &assets, &project_dir) {
                Some(w) => {
                    let mid = rect.center().y;
                    let half = rect.height() * 0.5 - 4.0;
                    let mut mesh = egui::Mesh::default();
                    for (i, (lo, hi)) in w.peaks.iter().enumerate() {
                        let x = rect.left()
                            + 3.0
                            + (i as f32 + 0.5) / w.peaks.len() as f32 * (rect.width() - 6.0);
                        mesh.add_colored_rect(
                            Rect::from_min_max(
                                egui::pos2(x - 0.5, mid - hi.max(0.01) * half),
                                egui::pos2(x + 0.5, mid - lo.min(-0.01) * half),
                            ),
                            c.accent,
                        );
                    }
                    p.add(egui::Shape::mesh(mesh));
                    p.text(
                        rect.right_top() + egui::vec2(-5.0, 3.0),
                        egui::Align2::RIGHT_TOP,
                        format!("{:.2} s", w.frames as f64 / w.sample_rate.max(1.0)),
                        egui::FontId::new(10.0, crate::fonts::mono()),
                        c.dim,
                    );
                }
                None => {
                    p.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "no preview",
                        egui::FontId::new(11.0, crate::fonts::body()),
                        c.faint,
                    );
                }
            }
        }
        None => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Select a sample to see it here",
                egui::FontId::new(11.0, crate::fonts::body()),
                c.faint,
            );
        }
    }
    resp.hint("Sample preview", "", "the waveform of the selected sample");
}
