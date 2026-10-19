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
        crate::widgets::leave_room(ui, 330.0, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .font(egui::FontId::new(13.0, crate::fonts::body_medium()))
                        .color(theme.colors.text),
                )
                .truncate(),
            )
        });
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
            .hint_menu("Playlist snap", &cur, "chooses the grid clips snap to");
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
        let on = core.playlist.audio.slice_tool;
        if crate::widgets::toggle(ui, on, "Split", theme.colors.accent, &theme)
            .tip("Slice tool: click an audio clip to cut it in two at the grid")
            .clicked()
        {
            core.playlist.audio.slice_tool = !on;
        }
        ui.label(
            egui::RichText::new("Click: place \u{b7} drag: move \u{b7} edge: resize \u{b7} right-click: delete \u{b7} shift-drag ruler: loop \u{b7} audio: edges trim, Alt+drag slips, handles fade, right-click settings")
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
        waves: &core.waves,
    };
    let res = playlist::show(ui, &mut core.playlist, &cx);
    browser_drop(ui, core);
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

/// A sample dragged from the browser lands as an audio clip under the pointer (the track and
/// the grid-snapped position), with a ghost while it is over the grid.
fn browser_drop(ui: &mut Ui, core: &mut AppCore) {
    use crate::browser_model::{Dragged, Kind};
    let ctx = ui.ctx().clone();
    if !egui::DragAndDrop::has_payload_of_type::<Dragged>(&ctx) {
        return;
    }
    let grid = core.playlist.last_grid;
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos())) else {
        return;
    };
    if !grid.contains(pos) || !ui.clip_rect().contains(pos) {
        return;
    }
    let rel = pos - grid.min;
    let track = core
        .playlist
        .track_at(rel.y)
        .clamp(0, core.playlist.n_tracks as i32 - 1);
    let mods = ctx.input(|i| i.modifiers);
    let grid_ticks = if mods.alt { 1 } else { core.playlist.grid() };
    let start = crate::piano_roll::snap_floor(core.playlist.tick_at(rel.x), grid_ticks);
    let sample = egui::DragAndDrop::payload::<Dragged>(&ctx)
        .and_then(|d| matches!(d.kind, Kind::Sample { .. }).then(|| d.clone()));
    let Some(d) = sample else {
        return;
    };
    // the ghost: the registered sample's length when it is known, else a bar
    let len = match &d.kind {
        Kind::Sample { reference } => core
            .project
            .sample_by_path(reference)
            .map(|e| {
                ph_core::seconds_to_ticks(e.seconds(), core.project.settings.bpm).round() as u64
            })
            .unwrap_or(core.playlist.bar),
        _ => core.playlist.bar,
    };
    let r = core
        .playlist
        .clip_rect(track, start, len.max(1))
        .translate(grid.min.to_vec2());
    let accent = core.theme.colors.accent;
    let p = ui.painter().with_clip_rect(grid);
    p.rect_filled(
        r.shrink2(egui::vec2(0.0, 1.0)),
        egui::CornerRadius::same(3),
        accent.gamma_multiply(0.3),
    );
    p.rect_stroke(
        r.shrink2(egui::vec2(0.0, 1.0)),
        egui::CornerRadius::same(3),
        egui::Stroke::new(1.0, accent),
        egui::StrokeKind::Inside,
    );
    if ctx.input(|i| i.pointer.any_released())
        && let Some(d) = egui::DragAndDrop::take_payload::<Dragged>(&ctx)
        && let Kind::Sample { reference } = &d.kind
    {
        core.send(crate::mapping_audio::add_clip_path(
            track as u32,
            start,
            reference,
        ));
    }
}
