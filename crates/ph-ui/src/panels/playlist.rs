//! Playlist panel: toolbar around the `playlist` widget.

use crate::core::{AppCore, RenameTarget};
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::mapping;
use crate::piano_roll::{SNAPS, ZoomPreset};
use crate::playlist::{self, DrawCtx};
use crate::theme::caps;
use crate::widgets::icon_button;
use egui::Ui;

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::Playlist);
    let theme = core.theme.clone();
    let project = core.project.clone();
    let live = core.live();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let name = core
            .sel
            .pattern
            .and_then(|p| project.patterns.get(&p))
            .map_or("(none)".to_string(), |p| p.name.clone());
        ui.label(crate::widgets::caption(&theme, "Place"));
        ui.label(
            egui::RichText::new(name)
                .font(egui::FontId::new(13.0, crate::fonts::body_medium()))
                .color(theme.colors.text),
        );
        ui.separator();
        ui.label(crate::widgets::caption(&theme, "Snap"));
        let bar = core.playlist.bar;
        let cur = SNAPS
            .iter()
            .find(|(n, t)| (*t == core.playlist.snap) || (*n == "Bar" && core.playlist.snap == bar))
            .map_or("custom".to_string(), |(n, _)| (*n).to_string());
        egui::ComboBox::from_id_salt("playlist_snap")
            .selected_text(caps(&cur))
            .width(78.0)
            .show_ui(ui, |ui| {
                for (n, t) in SNAPS {
                    let ticks = if *t == 0 { bar } else { *t };
                    if ui.selectable_label(*n == cur, caps(n)).clicked() {
                        core.playlist.snap = ticks;
                    }
                }
            })
            .response
            .hint("Playlist snap", &cur, "chooses the grid clips snap to");
        if icon_button(ui, &theme, Icon::Minus, false, "Zoom out", "Ctrl+wheel").clicked() {
            core.playlist.zoom_x(0.8, 100.0);
        }
        if icon_button(ui, &theme, Icon::Plus, false, "Zoom in", "Ctrl+wheel").clicked() {
            core.playlist.zoom_x(1.25, 100.0);
        }
        let zr = icon_button(ui, &theme, Icon::Zoom, false, "Zoom presets", "fit the song, one bar, one beat, 1/64 detail");
        egui::Popup::menu(&zr).show(|ui| {
            let w = (core.view_size.x - 140.0).max(200.0);
            let len = project.song_length().max(project.settings.time_signature.ticks_per_bar());
            for (preset, label, _) in ZoomPreset::MENU {
                if ui.button(caps(label)).clicked() {
                    core.playlist.zoom_to(*preset, w, len);
                    ui.close();
                }
            }
        });
        ui.label(
            egui::RichText::new("Click: place \u{b7} drag: move \u{b7} edge: resize \u{b7} right-click: delete \u{b7} shift-drag ruler: loop")
                .size(11.5)
                .color(theme.colors.faint),
        );
    });

    let cx = DrawCtx {
        project: &project,
        theme: &theme,
        position: live.status.position,
        loop_range: live.status.loop_range,
        place_pattern: core.sel.pattern,
        touches: &core.touches,
        agent_selected: &core.agent_select,
        now: core.now,
    };
    let res = playlist::show(ui, &mut core.playlist, &cx);
    if !res.out.is_empty() {
        core.apply_playlist(res.out);
    }
    if let Some((t, m)) = res.mute_track {
        core.send(mapping::track_mute(t, m));
    }
    if let Some(t) = res.rename_track {
        core.open_rename(RenameTarget::Track(t));
    }
}
