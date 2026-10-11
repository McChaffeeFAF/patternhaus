//! The toolbar below the top bar: tool icons, snap, history and file buttons, and the master
//! stereo meter with its dB readout. Which items show, and in what order, is the user's
//! (View > Customise toolbar, or right-click the toolbar) and is saved with the layout.

use crate::core::{AppCore, Dialog};
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::mapping;
use crate::piano_roll::{self, Tool, ZoomPreset};
use crate::widgets::{self, icon_button, icon_button_coloured};
use egui::{FontId, Id, RichText, Sense, Ui, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolbarItem {
    Draw,
    Select,
    Delete,
    Slice,
    Mute,
    Zoom,
    Snap,
    Ghosts,
    Undo,
    Redo,
    Save,
    Export,
    MasterMeter,
}

impl ToolbarItem {
    pub const ALL: &'static [ToolbarItem] = &[
        ToolbarItem::Draw,
        ToolbarItem::Select,
        ToolbarItem::Delete,
        ToolbarItem::Slice,
        ToolbarItem::Mute,
        ToolbarItem::Zoom,
        ToolbarItem::Snap,
        ToolbarItem::Ghosts,
        ToolbarItem::Undo,
        ToolbarItem::Redo,
        ToolbarItem::Save,
        ToolbarItem::Export,
        ToolbarItem::MasterMeter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolbarItem::Draw => "Draw tool",
            ToolbarItem::Select => "Select tool",
            ToolbarItem::Delete => "Delete tool",
            ToolbarItem::Slice => "Slice tool",
            ToolbarItem::Mute => "Mute tool",
            ToolbarItem::Zoom => "Zoom presets",
            ToolbarItem::Snap => "Snap",
            ToolbarItem::Ghosts => "Ghost notes",
            ToolbarItem::Undo => "Undo",
            ToolbarItem::Redo => "Redo",
            ToolbarItem::Save => "Save",
            ToolbarItem::Export => "Export",
            ToolbarItem::MasterMeter => "Master meter",
        }
    }

    pub fn icon(self) -> Option<Icon> {
        Some(match self {
            ToolbarItem::Draw => Icon::Draw,
            ToolbarItem::Select => Icon::Select,
            ToolbarItem::Delete => Icon::Delete,
            ToolbarItem::Slice => Icon::Slice,
            ToolbarItem::Mute => Icon::Mute,
            ToolbarItem::Zoom => Icon::Zoom,
            ToolbarItem::Snap => Icon::Snap,
            ToolbarItem::Ghosts => Icon::Ghost,
            ToolbarItem::Undo => Icon::Undo,
            ToolbarItem::Redo => Icon::Redo,
            ToolbarItem::Save => Icon::Save,
            ToolbarItem::Export => Icon::Export,
            ToolbarItem::MasterMeter => return None,
        })
    }

    /// Items of one group sit together; a gap separates groups.
    fn group(self) -> u8 {
        match self {
            ToolbarItem::Draw
            | ToolbarItem::Select
            | ToolbarItem::Delete
            | ToolbarItem::Slice
            | ToolbarItem::Mute => 0,
            ToolbarItem::Zoom | ToolbarItem::Snap | ToolbarItem::Ghosts => 1,
            ToolbarItem::Undo | ToolbarItem::Redo | ToolbarItem::Save | ToolbarItem::Export => 2,
            ToolbarItem::MasterMeter => 3,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub item: ToolbarItem,
    pub visible: bool,
}

/// The toolbar's items in order, each shown or hidden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolbarConfig {
    pub items: Vec<Entry>,
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        Self {
            items: ToolbarItem::ALL
                .iter()
                .map(|&item| Entry {
                    item,
                    visible: true,
                })
                .collect(),
        }
    }
}

impl ToolbarConfig {
    /// Drops duplicates and adds items a saved layout predates, so every item exists once.
    pub fn normalize(&mut self) {
        let mut seen = Vec::new();
        self.items.retain(|e| {
            if seen.contains(&e.item) {
                false
            } else {
                seen.push(e.item);
                true
            }
        });
        for &item in ToolbarItem::ALL {
            if !seen.contains(&item) {
                self.items.push(Entry {
                    item,
                    visible: true,
                });
            }
        }
    }

    pub fn set_visible(&mut self, item: ToolbarItem, visible: bool) {
        if let Some(e) = self.items.iter_mut().find(|e| e.item == item) {
            e.visible = visible;
        }
    }

    pub fn is_visible(&self, item: ToolbarItem) -> bool {
        self.items.iter().any(|e| e.item == item && e.visible)
    }

    /// Moves an item one place earlier (`-1`) or later (`1`) among all items.
    pub fn shift(&mut self, item: ToolbarItem, by: i32) {
        let Some(i) = self.items.iter().position(|e| e.item == item) else {
            return;
        };
        let j = (i as i32 + by).clamp(0, self.items.len() as i32 - 1) as usize;
        let e = self.items.remove(i);
        self.items.insert(j, e);
    }

    /// The visible items, in order.
    pub fn shown(&self) -> impl Iterator<Item = ToolbarItem> + '_ {
        self.items.iter().filter(|e| e.visible).map(|e| e.item)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let live = core.live();
    let items: Vec<ToolbarItem> = core.toolbar.shown().collect();
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut last_group = None;
            for item in items {
                if last_group.is_some_and(|g| g != item.group()) {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 18.0), Sense::hover());
                    ui.painter().rect_filled(r, 0.0, theme.colors.line);
                    ui.add_space(6.0);
                }
                last_group = Some(item.group());
                match item {
                    ToolbarItem::Draw => tool(
                        ui,
                        core,
                        Tool::Draw,
                        Icon::Draw,
                        "Draw",
                        "left-click draws, right-click deletes (Alt+P)",
                    ),
                    ToolbarItem::Select => tool(
                        ui,
                        core,
                        Tool::Select,
                        Icon::Select,
                        "Select",
                        "box select; Alt+drag lassos (Alt+E)",
                    ),
                    ToolbarItem::Delete => tool(
                        ui,
                        core,
                        Tool::Delete,
                        Icon::Delete,
                        "Delete",
                        "drag over notes to delete them (Alt+D)",
                    ),
                    ToolbarItem::Slice => tool(
                        ui,
                        core,
                        Tool::Slice,
                        Icon::Slice,
                        "Slice",
                        "click a note to split it at the snapped position; Alt = no snap",
                    ),
                    ToolbarItem::Mute => tool(
                        ui,
                        core,
                        Tool::Mute,
                        Icon::Mute,
                        "Mute",
                        "click or drag over notes to mute or unmute them",
                    ),
                    ToolbarItem::Zoom => zoom_menu(ui, core),
                    ToolbarItem::Snap => {
                        ui.label(
                            crate::theme::caps_sized(
                                "Snap",
                                theme.metrics.font_caption,
                                crate::theme::TRACK_CAPTION,
                            )
                            .color(theme.colors.dim),
                        );
                        piano_roll::snap_combo(ui, "toolbar_snap", &mut core.roll.snap, &theme)
                            .hint_menu(
                                "Snap",
                                &piano_roll::snap_name(core.roll.snap),
                                "chooses the grid notes snap to",
                            );
                    }
                    ToolbarItem::Ghosts => {
                        if icon_button(
                            ui,
                            &theme,
                            Icon::Ghost,
                            core.roll.ghosts,
                            "Ghost notes",
                            "show the other channels' notes",
                        )
                        .clicked()
                        {
                            core.roll.ghosts = !core.roll.ghosts;
                        }
                    }
                    ToolbarItem::Undo => {
                        if icon_button(
                            ui,
                            &theme,
                            Icon::Undo,
                            false,
                            "Undo",
                            "undo your last change (Ctrl+Z)",
                        )
                        .clicked()
                        {
                            core.send(mapping::undo_mine());
                        }
                    }
                    ToolbarItem::Redo => {
                        if icon_button(ui, &theme, Icon::Redo, false, "Redo", "Ctrl+Y").clicked() {
                            core.send(mapping::redo_mine());
                        }
                    }
                    ToolbarItem::Save => {
                        if icon_button_coloured(
                            ui,
                            &theme,
                            Icon::Save,
                            core.dirty,
                            true,
                            "Save",
                            "save the project (Ctrl+S)",
                            theme.colors.warn,
                        )
                        .clicked()
                        {
                            core.send(mapping::file_save());
                        }
                    }
                    ToolbarItem::Export => {
                        if icon_button(
                            ui,
                            &theme,
                            Icon::Export,
                            false,
                            "Export",
                            "render the project to audio (Ctrl+R)",
                        )
                        .clicked()
                        {
                            core.dialog = Some(Dialog::Export {
                                path: "exports/mix.wav".into(),
                                overwrite: false,
                            });
                        }
                    }
                    ToolbarItem::MasterMeter => {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let peak = live.peak.first().copied().unwrap_or([0.0; 2]);
                            let rms = live.rms.first().copied().unwrap_or([0.0; 2]);
                            let top = peak[0].max(peak[1]);
                            let db = widgets::db_from_linear(top);
                            ui.label(
                                RichText::new(if top < 0.001 {
                                    "-inf dB".to_string()
                                } else {
                                    format!("{db:+.1} dB")
                                })
                                .font(FontId::new(
                                    theme.metrics.font_readout,
                                    crate::fonts::mono(),
                                ))
                                .color(if top > 1.0 {
                                    theme.colors.clip
                                } else {
                                    theme.colors.accent
                                }),
                            );
                            widgets::meter(
                                ui,
                                Id::new("master_meter"),
                                Vec2::new(150.0, 14.0),
                                peak,
                                rms,
                                &theme,
                            )
                            .hint(
                                "Master meter",
                                &format!("{db:+.1} dB peak"),
                                "click to clear the clip LED",
                            );
                        });
                    }
                }
            }
        })
        .response;
    let _ = resp.interact(Sense::click()).context_menu(|ui| {
        if crate::widgets::btn(ui, "Customise toolbar…").clicked() {
            core.dialog = Some(Dialog::Toolbar);
            ui.close();
        }
        if crate::widgets::btn(ui, "Reset toolbar").clicked() {
            core.toolbar.reset();
            ui.close();
        }
    });
}

fn tool(ui: &mut Ui, core: &mut AppCore, tool: Tool, icon: Icon, name: &str, gestures: &str) {
    let theme = core.theme.clone();
    if icon_button(ui, &theme, icon, core.roll.tool == tool, name, gestures).clicked() {
        core.roll.tool = tool;
    }
}

fn zoom_menu(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let r = icon_button(
        ui,
        &theme,
        Icon::Zoom,
        false,
        "Zoom",
        "presets: fit, one bar, one beat, 1/64 detail",
    );
    egui::Popup::menu(&r).show(|ui| {
        for (preset, label, gesture) in ZoomPreset::MENU {
            let text = format!("{label}   {gesture}");
            if ui.button(crate::theme::caps(&text)).clicked() {
                core.roll.zoom_request = Some(*preset);
                ui.close();
            }
        }
    });
}

/// View > Customise toolbar.
pub fn dialog(ctx: &egui::Context, core: &mut AppCore) -> bool {
    let mut open = true;
    let theme = core.theme.clone();
    egui::Window::new("Customise toolbar")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Show, hide and reorder the toolbar's items. Changes apply at once and are saved with the layout.");
            ui.add_space(4.0);
            let snapshot = core.toolbar.items.clone();
            for e in snapshot {
                ui.horizontal(|ui| {
                    let mut v = e.visible;
                    if ui.checkbox(&mut v, crate::theme::caps(e.item.label())).changed() {
                        core.toolbar.set_visible(e.item, v);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, &theme, Icon::Right, false, "Move later", "").clicked() {
                            core.toolbar.shift(e.item, 1);
                        }
                        if icon_button(ui, &theme, Icon::Left, false, "Move earlier", "").clicked() {
                            core.toolbar.shift(e.item, -1);
                        }
                    });
                });
            }
            ui.separator();
            if widgets::btn(ui, "Reset to default").clicked() {
                core.toolbar.reset();
            }
        });
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_appears_once_by_default() {
        let c = ToolbarConfig::default();
        assert_eq!(c.items.len(), ToolbarItem::ALL.len());
        assert!(c.items.iter().all(|e| e.visible));
    }

    #[test]
    fn items_hide_and_reorder() {
        let mut c = ToolbarConfig::default();
        c.set_visible(ToolbarItem::Mute, false);
        assert!(!c.is_visible(ToolbarItem::Mute));
        assert!(!c.shown().any(|i| i == ToolbarItem::Mute));
        c.shift(ToolbarItem::Undo, -1);
        let order: Vec<_> = c.shown().collect();
        let ui = order.iter().position(|i| *i == ToolbarItem::Undo).unwrap();
        let gh = order
            .iter()
            .position(|i| *i == ToolbarItem::Ghosts)
            .unwrap();
        assert!(ui < gh, "undo moved before ghosts");
        c.shift(ToolbarItem::Draw, -5);
        assert_eq!(c.items[0].item, ToolbarItem::Draw, "stops at the ends");
    }

    #[test]
    fn a_saved_toolbar_from_an_older_version_gains_new_items() {
        let mut c = ToolbarConfig {
            items: vec![
                Entry {
                    item: ToolbarItem::Save,
                    visible: false,
                },
                Entry {
                    item: ToolbarItem::Save,
                    visible: true,
                },
            ],
        };
        c.normalize();
        assert_eq!(c.items.len(), ToolbarItem::ALL.len());
        assert!(!c.items[0].visible, "the first copy wins");
    }

    #[test]
    fn the_toolbar_round_trips_through_ron() {
        let mut c = ToolbarConfig::default();
        c.set_visible(ToolbarItem::Zoom, false);
        c.shift(ToolbarItem::Export, -3);
        let text = ron::ser::to_string(&c).unwrap();
        let back: ToolbarConfig = ron::from_str(&text).unwrap();
        assert_eq!(back, c);
    }
}
