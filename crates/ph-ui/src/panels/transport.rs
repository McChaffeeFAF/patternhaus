//! Transport bar: play/stop, pattern or song mode, tempo, position, metronome, loop, meters
//! and engine health, and the pattern selector.

use crate::core::{AppCore, Dialog};
use crate::mapping;
use crate::widgets::{self, toggle};
use egui::{DragValue, RichText, Ui};
use ph_core::{PPQ, Tick};
use ph_engine::PlayMode;

/// `bar:beat:tick`, 1-based bars and beats.
pub fn format_position(pos: Tick, ticks_per_bar: Tick) -> String {
    let bar = pos / ticks_per_bar.max(1);
    let within = pos % ticks_per_bar.max(1);
    format!("{:03}:{}:{:03}", bar + 1, within / PPQ + 1, within % PPQ)
}

pub fn format_time(pos: Tick, bpm: f64) -> String {
    let secs = ph_core::ticks_to_seconds(pos, bpm);
    format!("{:02}:{:04.1}", (secs / 60.0) as u32, secs % 60.0)
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let live = core.live();
    let project = core.project.clone();
    let c = &theme.colors;
    ui.horizontal_wrapped(|ui| {
        // play / stop
        let playing = live.status.playing;
        let play = toggle(
            ui,
            playing,
            if playing { "Stop" } else { "Play" },
            c.ok,
            &theme,
        );
        if play.clicked() {
            core.send(mapping::transport("toggle"));
        }
        play.on_hover_text("Space");
        // pattern / song
        let pattern_mode = matches!(live.status.mode, PlayMode::Pattern(_));
        if toggle(ui, pattern_mode, "PAT", c.accent, &theme)
            .on_hover_text("Pattern mode loops the selected pattern (L)")
            .clicked()
        {
            core.send(mapping::set_mode_pattern(core.sel.pattern));
        }
        if toggle(ui, !pattern_mode, "SONG", c.accent, &theme)
            .on_hover_text("Song mode plays the playlist (L)")
            .clicked()
        {
            core.send(mapping::set_mode_song());
        }
        if toggle(ui, live.status.metronome, "Metro", c.warn, &theme).clicked() {
            core.send(mapping::set_metronome(!live.status.metronome));
        }
        let looping = live.status.loop_range.is_some();
        if toggle(ui, looping, "Loop", c.accent, &theme)
            .on_hover_text("Loop the song, or the region shift-dragged on the playlist ruler")
            .clicked()
        {
            let end = project
                .song_length()
                .max(project.settings.time_signature.ticks_per_bar() * 4);
            core.send(mapping::set_loop(if looping {
                None
            } else {
                Some((0, end))
            }));
        }
        ui.separator();

        // tempo: drag or type; commit when released or Enter
        let bpm = project.settings.bpm;
        let id = ui.id().with("tempo");
        let mut v: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(bpm);
        let r = ui.add(
            DragValue::new(&mut v)
                .range(10.0..=999.0)
                .speed(0.25)
                .fixed_decimals(2)
                .suffix(" BPM"),
        );
        if r.changed() {
            ui.data_mut(|d| d.insert_temp(id, v));
        }
        if (r.drag_stopped() || r.lost_focus())
            && let Some(v) = ui.data_mut(|d| d.remove_temp::<f64>(id))
            && (v - bpm).abs() > 1e-9
        {
            core.send(mapping::set_tempo(v));
        }
        r.on_hover_text("Drag or type a tempo");

        // position
        let bar = project.settings.time_signature.ticks_per_bar();
        ui.label(
            RichText::new(format_position(live.status.position, bar))
                .monospace()
                .size(16.0)
                .color(c.playhead),
        );
        ui.label(
            RichText::new(format_time(live.status.position, bpm))
                .monospace()
                .color(c.dim),
        );
        ui.separator();

        // pattern selector
        let names: Vec<_> = project
            .patterns_in_order()
            .map(|p| (p.id, p.name.clone()))
            .collect();
        let cur = core
            .sel
            .pattern
            .and_then(|id| names.iter().find(|(i, _)| *i == id))
            .map_or("(no pattern)".to_string(), |(_, n)| n.clone());
        egui::ComboBox::from_id_salt("pattern_select")
            .selected_text(cur)
            .width(120.0)
            .show_ui(ui, |ui| {
                for (id, name) in &names {
                    if ui
                        .selectable_label(core.sel.pattern == Some(*id), name)
                        .clicked()
                    {
                        core.select_pattern(*id);
                    }
                }
            });
        if ui.small_button("+").on_hover_text("New pattern").clicked() {
            core.send(mapping::pattern_add());
        }
        if ui
            .small_button("Clone")
            .on_hover_text("Clone pattern (Ctrl+L)")
            .clicked()
            && let Some(p) = core.sel.pattern
        {
            core.send(mapping::pattern_clone(p));
        }
        if ui.small_button("Rename").clicked()
            && let Some(p) = core.sel.pattern
        {
            core.open_rename(crate::core::RenameTarget::Pattern(p));
        }
        ui.separator();

        // typing keyboard
        let on = core.typing.enabled;
        if toggle(
            ui,
            on,
            &format!("Keys oct {}", core.typing.octave),
            c.accent,
            &theme,
        )
        .on_hover_text(
            "Typing keyboard plays the selected channel (Ctrl+T). - and = shift the octave.",
        )
        .clicked()
        {
            core.typing.enabled = !on;
        }
        ui.separator();

        // history shortcuts
        if ui
            .small_button("Undo")
            .on_hover_text("Undo your last change (Ctrl+Z)")
            .clicked()
        {
            core.send(mapping::undo_mine());
        }
        if ui.small_button("Redo").on_hover_text("Ctrl+Y").clicked() {
            core.send(mapping::redo_mine());
        }
        if ui.small_button("Save").on_hover_text("Ctrl+S").clicked() {
            core.send(mapping::file_save());
        }
        if ui.small_button("Export").on_hover_text("Ctrl+R").clicked() {
            core.dialog = Some(Dialog::Export {
                path: "exports/mix.wav".into(),
                overwrite: false,
            });
        }
        ui.separator();

        // engine health
        let master = (
            live.peak.first().copied().unwrap_or([0.0; 2]),
            live.rms.first().copied().unwrap_or([0.0; 2]),
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(110.0, 16.0), egui::Sense::hover());
        widgets::paint_meter(ui, rect, master.0, master.1, &theme);
        let load = live.stats.dsp_load;
        let col = if load > 0.8 {
            c.error
        } else if load > 0.5 {
            c.warn
        } else {
            c.dim
        };
        ui.label(RichText::new(format!("DSP {:.0}%", load * 100.0)).color(col))
            .on_hover_text(format!(
                "{} frames at {} Hz",
                live.stats.buffer_frames, live.stats.sample_rate
            ));
        if live.stats.overloads > 0 {
            ui.label(RichText::new(format!("{} overloads", live.stats.overloads)).color(c.error));
        }
        if let Some(n) = &core.audio_notice {
            ui.label(RichText::new(n).color(c.warn));
        }
    });

    // non-blocking notice: the engine thinks a bigger buffer would help
    if let Some(frames) = live.stats.suggest_buffer_frames
        && !core.notice_dismissed
    {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Audio is struggling at {} frames. Use a larger buffer ({frames})?",
                    live.stats.buffer_frames
                ))
                .color(c.warn),
            );
            if ui.small_button(format!("Use {frames}")).clicked() {
                core.set_buffer(frames);
            }
            if ui.small_button("Dismiss").clicked() {
                core.notice_dismissed = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_formats_as_bar_beat_tick() {
        assert_eq!(format_position(0, 3840), "001:1:000");
        assert_eq!(format_position(960, 3840), "001:2:000");
        assert_eq!(format_position(3840 + 960 * 2 + 17, 3840), "002:3:017");
    }

    #[test]
    fn time_formats_minutes_and_seconds() {
        // 120 bpm: a quarter note is 0.5 s
        assert_eq!(format_time(960 * 2, 120.0), "00:01.0");
        assert_eq!(format_time(960 * 2 * 61, 120.0), "01:01.0");
    }
}
