//! Playlist panel: toolbar around the `playlist` widget.

use crate::core::{AppCore, RenameTarget};
use crate::mapping;
use crate::piano_roll::SNAPS;
use crate::playlist::{self, DrawCtx};
use egui::Ui;

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::Playlist);
    let theme = core.theme.clone();
    let project = core.project.clone();
    let live = core.live();

    ui.horizontal(|ui| {
        let name = core
            .sel
            .pattern
            .and_then(|p| project.patterns.get(&p))
            .map_or("(none)".to_string(), |p| p.name.clone());
        ui.label(format!("Place: {name}"));
        ui.separator();
        ui.label("Snap");
        let cur = SNAPS
            .iter()
            .find(|(n, t)| (*t == core.playlist.snap) || (*n == "Bar" && core.playlist.snap == core.playlist.bar))
            .map_or("custom", |(n, _)| *n);
        egui::ComboBox::from_id_salt("playlist_snap")
            .selected_text(cur)
            .width(96.0)
            .show_ui(ui, |ui| {
                for (n, t) in SNAPS {
                    let ticks = if *t == 0 { core.playlist.bar } else { *t };
                    ui.selectable_value(&mut core.playlist.snap, ticks, *n);
                }
            });
        if ui.small_button("-").on_hover_text("Zoom out (Ctrl+wheel)").clicked() {
            core.playlist.zoom_x(0.8, 100.0);
        }
        if ui.small_button("+").on_hover_text("Zoom in").clicked() {
            core.playlist.zoom_x(1.25, 100.0);
        }
        ui.label(
            egui::RichText::new("Click: place · drag: move · edge: resize · right-click: delete · shift-drag ruler: loop")
                .size(10.0)
                .color(theme.colors.text_dim),
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
