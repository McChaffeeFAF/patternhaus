//! The plugin instrument catalogue in the UI (M3/T9), all through `catalogue.*` commands:
//!
//! - a **Catalogue** tab in the browser (shown when the session has a catalogue folder): the
//!   saved instruments with their tags, filtered by the search box; click shows the playing
//!   guide under the list, double-click or drag onto the channel rack adds a channel
//!   (`catalogue.add_to_channel`), right-click edits the guide;
//! - **Save to catalogue…** in a plugin instrument channel's menu: name, tags and a
//!   description (`catalogue.save`);
//! - a simple **guide editor**: range, mono/legato, velocity notes, keyswitches and CCs
//!   (`catalogue.update_guide`).
//!
//! The list is read as the human (`catalogue.list`, a read command) when the tab shows and
//! after every catalogue change; writes are sent like every other command.

use crate::browser_model::{Kind, Node};
use crate::core::{AppCore, Dialog};
use crate::widgets::btn;
use egui::{RichText, Ui};
use ph_commands::catalogue::{CcUse, Keyswitch, PlayingGuide, guide_text, note_name};
use ph_commands::{Caller, Request};
use ph_core::ChannelId;
use serde_json::{Value, json};

/// How often the list is re-read while the tab shows (seconds).
const REFRESH: f64 = 3.0;

/// The browser's view of the catalogue.
pub struct CatalogueState {
    pub nodes: Vec<Node>,
    /// The selected entry's guide, as text.
    pub guide: Option<String>,
    pub error: Option<String>,
    at: f64,
}

impl Default for CatalogueState {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            guide: None,
            error: None,
            at: f64::NEG_INFINITY,
        }
    }
}

impl CatalogueState {
    /// Re-read on the next frame the tab shows.
    pub fn mark_stale(&mut self) {
        self.at = f64::NEG_INFINITY;
    }
}

/// Whether this session has a catalogue (the tab shows only then).
pub fn available(core: &AppCore) -> bool {
    core.studio.catalogue_dir().is_some()
}

fn read(core: &AppCore, cmd: &str, params: Value) -> Result<Value, String> {
    core.studio
        .run(&Caller::human(), cmd, params)
        .map(|d| d.result)
        .map_err(|e| e.to_string())
}

/// One browser row per entry: the name, then its tags; a plugin that cannot load says so.
pub fn nodes(list: &Value) -> Vec<Node> {
    list["entries"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let entry = r["entry"].as_str()?.to_string();
                    let mut name = r["name"].as_str().unwrap_or(&entry).to_string();
                    let tags: Vec<&str> = r["tags"]
                        .as_array()
                        .map(|t| t.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if !tags.is_empty() {
                        name.push_str(&format!("  · {}", tags.join(", ")));
                    }
                    match r["status"].as_str() {
                        Some("ok") | Some("unknown") | None => {}
                        Some(s) => name.push_str(&format!("  ({s})")),
                    }
                    Some(Node {
                        name,
                        kind: Kind::Catalogue { entry },
                        children: Vec::new(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Re-reads the list when it is due (the tab is showing).
pub fn refresh(core: &mut AppCore) {
    if core.now - core.catalogue.at < REFRESH {
        return;
    }
    core.catalogue.at = core.now;
    match read(core, "catalogue.list", json!({})) {
        Ok(v) => {
            core.catalogue.nodes = nodes(&v);
            core.catalogue.error = None;
        }
        Err(e) => core.catalogue.error = Some(e),
    }
}

/// The selected entry: its guide shows under the list.
pub fn select(core: &mut AppCore, entry: &str) {
    core.catalogue.guide = Some(
        match read(core, "catalogue.get", json!({ "entry": entry })) {
            Ok(v) => {
                let mut t = v["guide_text"].as_str().unwrap_or("").to_string();
                if let Some(w) = v["warning"].as_str() {
                    t = format!("{t}\n{w}");
                }
                t
            }
            Err(e) => e,
        },
    );
}

/// The guide of the selected entry (under the browser list).
pub fn guide_panel(ui: &mut Ui, core: &AppCore) {
    let c = &core.theme.colors;
    if let Some(e) = &core.catalogue.error {
        ui.label(RichText::new(e).color(c.warn).size(11.0));
    }
    if let Some(g) = &core.catalogue.guide {
        ui.label(RichText::new(g).color(c.dim).size(11.0));
    }
}

/// `catalogue.add_to_channel` for an entry dropped or double-clicked.
pub fn add_request(entry: &str) -> Request {
    Request::new("catalogue.add_to_channel", json!({ "entry": entry }))
}

// ---- Save to catalogue… -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct SaveForm {
    pub channel: ChannelId,
    pub name: String,
    /// Comma-separated.
    pub tags: String,
    pub description: String,
}

impl SaveForm {
    pub fn new(channel: ChannelId, name: &str) -> Self {
        Self {
            channel,
            name: name.to_string(),
            tags: String::new(),
            description: String::new(),
        }
    }
}

fn split_tags(t: &str) -> Vec<String> {
    t.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn save_request(f: &SaveForm) -> Request {
    Request::new(
        "catalogue.save",
        json!({
            "channel": f.channel,
            "name": f.name.trim(),
            "tags": split_tags(&f.tags),
            "description": f.description.trim(),
        }),
    )
}

/// The "Save to catalogue" dialog; returns whether it stays open.
pub fn save_dialog(ctx: &egui::Context, core: &mut AppCore, f: &mut SaveForm) -> bool {
    let mut keep = true;
    super::app::modal(ctx, "Save to catalogue", |ui| {
        ui.label("Saves the plugin as it is set up now (its own state), to use in any project.");
        egui::Grid::new("cat_save").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(280.0));
            ui.end_row();
            ui.label("Tags");
            ui.add(
                egui::TextEdit::singleline(&mut f.tags)
                    .hint_text("strings, kontakt, warm")
                    .desired_width(280.0),
            );
            ui.end_row();
            ui.label("Description");
            ui.add(egui::TextEdit::multiline(&mut f.description).desired_rows(2));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if btn(ui, "Save").clicked() && !f.name.trim().is_empty() {
                core.send(save_request(f));
                core.catalogue.mark_stale();
                keep = false;
            }
            if btn(ui, "Cancel").clicked() {
                keep = false;
            }
        });
    });
    keep
}

// ---- guide editor -----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GuideForm {
    pub entry: String,
    pub name: String,
    pub guide: PlayingGuide,
}

/// Opens the guide editor on an entry (reads it first).
pub fn open_guide(core: &mut AppCore, entry: &str) {
    match read(core, "catalogue.get", json!({ "entry": entry })) {
        Ok(v) => {
            let guide = serde_json::from_value(v["guide"].clone()).unwrap_or_default();
            core.dialog = Some(Dialog::CatalogueGuide(GuideForm {
                entry: entry.to_string(),
                name: v["name"].as_str().unwrap_or(entry).to_string(),
                guide,
            }));
        }
        Err(e) => core.toast(crate::core::ToastKind::Error, e),
    }
}

pub fn guide_request(f: &GuideForm) -> Request {
    Request::new(
        "catalogue.update_guide",
        json!({ "entry": f.entry, "guide": f.guide }),
    )
}

fn key_field(ui: &mut Ui, key: &mut u8) {
    ui.add(
        egui::DragValue::new(key)
            .range(0..=127)
            .custom_formatter(|v, _| format!("{} ({v})", note_name(v as u8))),
    );
}

/// The guide editor; returns whether it stays open.
pub fn guide_dialog(ctx: &egui::Context, core: &mut AppCore, f: &mut GuideForm) -> bool {
    let mut keep = true;
    let mut cancel = false;
    let mut open = true;
    let g = &mut f.guide;
    egui::Window::new(format!("Playing guide: {}", f.name))
        .id(egui::Id::new("catalogue_guide"))
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            let mut has_range = g.range.is_some();
            ui.horizontal(|ui| {
                ui.checkbox(&mut has_range, "Range");
                let [mut lo, mut hi] = g.range.unwrap_or([21, 108]);
                if has_range {
                    key_field(ui, &mut lo);
                    ui.label("to");
                    key_field(ui, &mut hi);
                    g.range = Some([lo.min(hi), hi.max(lo)]);
                } else {
                    g.range = None;
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut g.mono, "Mono");
                ui.checkbox(&mut g.legato, "Legato");
            });
            ui.label("Velocity");
            ui.add(egui::TextEdit::singleline(&mut g.velocity).desired_width(380.0));
            ui.label("Notes");
            ui.add(egui::TextEdit::multiline(&mut g.notes).desired_rows(2));
            ui.separator();
            ui.label("Keyswitches");
            let mut drop_ks = None;
            for (i, k) in g.keyswitches.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    key_field(ui, &mut k.key);
                    ui.add(egui::TextEdit::singleline(&mut k.articulation).desired_width(220.0));
                    if btn(ui, "Remove").clicked() {
                        drop_ks = Some(i);
                    }
                });
            }
            if let Some(i) = drop_ks {
                g.keyswitches.remove(i);
            }
            if btn(ui, "Add keyswitch").clicked() {
                let key = g
                    .keyswitches
                    .last()
                    .map_or(24, |k| k.key.saturating_add(1).min(127));
                g.keyswitches.push(Keyswitch {
                    key,
                    articulation: String::new(),
                });
            }
            ui.separator();
            ui.label("Controllers");
            let mut drop_cc = None;
            for (i, c) in g.ccs.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut c.cc).range(0..=127).prefix("CC "));
                    ui.add(egui::TextEdit::singleline(&mut c.meaning).desired_width(220.0));
                    if btn(ui, "Remove").clicked() {
                        drop_cc = Some(i);
                    }
                });
            }
            if let Some(i) = drop_cc {
                g.ccs.remove(i);
            }
            if btn(ui, "Add controller").clicked() {
                g.ccs.push(CcUse {
                    cc: 1,
                    meaning: String::new(),
                });
            }
            ui.separator();
            ui.label(
                RichText::new(guide_text(g))
                    .size(11.0)
                    .color(core.theme.colors.dim),
            );
            ui.horizontal(|ui| {
                if btn(ui, "Save guide").clicked() {
                    keep = false;
                }
                if btn(ui, "Cancel").clicked() {
                    keep = false;
                    cancel = true;
                }
            });
        });
    if !keep && open && !cancel {
        // "Save guide"
        core.send(guide_request(f));
        core.catalogue.mark_stale();
        if let Some(e) = core.browser.selected.clone()
            && e.ends_with(&f.entry)
        {
            core.catalogue.guide = Some(guide_text(&f.guide));
        }
    }
    keep && open && !cancel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_requests() {
        let list = json!({ "entries": [
            { "entry": "strings", "name": "Strings", "tags": ["warm", "kontakt"], "status": "ok" },
            { "entry": "gone", "name": "Gone", "tags": [], "status": "missing" }
        ] });
        let n = nodes(&list);
        assert_eq!(n[0].name, "Strings  · warm, kontakt");
        assert_eq!(
            n[0].kind,
            Kind::Catalogue {
                entry: "strings".into()
            }
        );
        assert_eq!(n[1].name, "Gone  (missing)");
        let f = SaveForm {
            channel: ChannelId(3),
            name: " Pad ".into(),
            tags: "a, ,b ".into(),
            description: "soft".into(),
        };
        let r = save_request(&f);
        assert_eq!(r.command, "catalogue.save");
        assert_eq!(
            r.params,
            json!({ "channel": 3, "name": "Pad", "tags": ["a", "b"], "description": "soft" })
        );
        let g = GuideForm {
            entry: "strings".into(),
            name: "Strings".into(),
            guide: PlayingGuide {
                range: Some([40, 80]),
                ..Default::default()
            },
        };
        let r = guide_request(&g);
        assert_eq!(r.params["guide"]["range"], json!([40, 80]));
        assert_eq!(add_request("x").params, json!({ "entry": "x" }));
    }
}
