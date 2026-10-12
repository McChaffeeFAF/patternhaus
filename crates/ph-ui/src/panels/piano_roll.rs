//! Piano roll panel: toolbar around the `piano_roll` widget.

use crate::core::AppCore;
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::live::pattern_position;
use crate::piano_roll::{self, DrawCtx, PrAction};
use crate::widgets::{btn, icon_button};
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
        ui.spacing_mut().item_spacing.x = 5.0;
        let ch_name = project
            .channels
            .get(&channel)
            .map_or("?", |c| c.name.as_str());
        let pat_name = project
            .patterns
            .get(&pattern)
            .map_or("?", |p| p.name.as_str());
        // the buttons after the title keep their room; a long title is shortened
        crate::widgets::leave_room(ui, 430.0, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{ch_name}  \u{b7}  {pat_name}"))
                        .font(egui::FontId::new(13.0, crate::fonts::body_medium()))
                        .color(theme.colors.text),
                )
                .truncate(),
            )
        });
        ui.add_space(6.0);
        for (icon, a, name, gestures) in [
            (
                Icon::Quantize,
                PrAction::Quantize,
                "Quantize",
                "snap selected notes to the grid (Ctrl+Q)",
            ),
            (
                Icon::Clone,
                PrAction::Duplicate,
                "Duplicate",
                "copy the selection after itself (Ctrl+B)",
            ),
        ] {
            if icon_button(ui, &theme, icon, false, name, gestures).clicked() {
                let outs = core.roll.action(a);
                core.apply_roll(outs);
            }
        }
        ui.add_space(4.0);
        for (label, a, tip) in [
            (
                "-1",
                PrAction::TransposeDown,
                "Transpose down a semitone (Down)",
            ),
            ("+1", PrAction::TransposeUp, "Transpose up a semitone (Up)"),
            ("-12", PrAction::OctaveDown, "Octave down (Shift+Down)"),
            ("+12", PrAction::OctaveUp, "Octave up (Shift+Up)"),
        ] {
            if btn(ui, label).tip(tip).clicked() {
                let outs = core.roll.action(a);
                core.apply_roll(outs);
            }
        }
        ui.label(
            egui::RichText::new(format!("{} selected", core.roll.selection.len()))
                .font(egui::FontId::new(11.5, crate::fonts::mono()))
                .color(theme.colors.dim),
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
