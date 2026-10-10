//! Piano roll panel: toolbar around the `piano_roll` widget.

use crate::core::AppCore;
use crate::live::pattern_position;
use crate::piano_roll::{self, DrawCtx, PrAction, SNAPS, Tool};
use crate::widgets::toggle;
use egui::Ui;

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::PianoRoll);
    let theme = core.theme.clone();
    let project = core.project.clone();
    core.fix_selection();
    let (Some(pattern), Some(channel)) = (core.roll.pattern, core.roll.channel) else {
        ui.label("Add a channel to the rack, then open it here.");
        return;
    };

    ui.horizontal(|ui| {
        let ch_name = project
            .channels
            .get(&channel)
            .map_or("?", |c| c.name.as_str());
        let pat_name = project
            .patterns
            .get(&pattern)
            .map_or("?", |p| p.name.as_str());
        ui.label(egui::RichText::new(format!("{ch_name}  ·  {pat_name}")).strong());
        ui.separator();
        for (tool, label, tip) in [
            (
                Tool::Draw,
                "Draw",
                "Left-click draws, right-click deletes (Alt+P)",
            ),
            (
                Tool::Select,
                "Select",
                "Box select (Alt+E, or Ctrl+drag); Alt+drag in empty space lassos",
            ),
            (Tool::Delete, "Delete", "Drag over notes to delete (Alt+D)"),
            (
                Tool::Slice,
                "Slice",
                "Click a note to split it at the snapped position (Alt = no snap)",
            ),
            (
                Tool::Mute,
                "Mute",
                "Click or drag over notes to mute or unmute them",
            ),
        ] {
            if toggle(
                ui,
                core.roll.tool == tool,
                label,
                theme.colors.accent,
                &theme,
            )
            .on_hover_text(tip)
            .clicked()
            {
                core.roll.tool = tool;
            }
        }
        ui.separator();
        ui.label("Snap");
        let cur = SNAPS
            .iter()
            .find(|(_, t)| *t == core.roll.snap)
            .map_or("custom", |(n, _)| *n);
        egui::ComboBox::from_id_salt("roll_snap")
            .selected_text(cur)
            .width(96.0)
            .show_ui(ui, |ui| {
                for (name, ticks) in SNAPS {
                    ui.selectable_value(&mut core.roll.snap, *ticks, *name);
                }
            });
        if toggle(ui, core.roll.ghosts, "Ghosts", theme.colors.accent, &theme)
            .on_hover_text("Show notes of the other channels")
            .clicked()
        {
            core.roll.ghosts = !core.roll.ghosts;
        }
        ui.separator();
        for (label, a, tip) in [
            ("Quantize", PrAction::Quantize, "Ctrl+Q"),
            ("Duplicate", PrAction::Duplicate, "Ctrl+B"),
            (
                "-1",
                PrAction::TransposeDown,
                "Transpose down a semitone (Down)",
            ),
            ("+1", PrAction::TransposeUp, "Transpose up a semitone (Up)"),
            ("-12", PrAction::OctaveDown, "Octave down (Shift+Down)"),
            ("+12", PrAction::OctaveUp, "Octave up (Shift+Up)"),
        ] {
            if ui.small_button(label).on_hover_text(tip).clicked() {
                let outs = core.roll.action(a);
                core.apply_roll(outs);
            }
        }
        ui.label(
            egui::RichText::new(format!("{} selected", core.roll.selection.len()))
                .color(theme.colors.text_dim),
        );
    });

    let live = core.live();
    let len = project.pattern_length(pattern).unwrap_or(3840);
    let playhead = if live.status.playing {
        pattern_position(&live.status, len, pattern)
    } else {
        None
    };
    let agent_notes: std::collections::BTreeSet<ph_core::NoteId> = core
        .agent_select
        .iter()
        .filter_map(|p| match p {
            ph_core::ObjectPath::Note(_, _, n) => Some(*n),
            _ => None,
        })
        .collect();
    let flashes = core.flashes.clone();
    let cx = DrawCtx {
        project: &project,
        theme: &theme,
        playhead,
        flashes: &flashes,
        now: core.now,
        agent_selected: &agent_notes,
    };
    let outs = piano_roll::show(ui, &mut core.roll, &cx);
    if !outs.is_empty() {
        core.apply_roll(outs);
    }
    if !flashes.is_empty()
        && flashes
            .iter()
            .any(|f| core.now - f.at < piano_roll::FLASH_SECS)
    {
        ui.ctx().request_repaint();
    }
}
