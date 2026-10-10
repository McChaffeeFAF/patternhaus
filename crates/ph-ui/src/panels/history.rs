//! History panel: per-author entries, "Undo mine", "Undo all by agent X", undo of a single
//! transaction, and the conflict dialog (DESIGN §3).

use crate::core::{AppCore, ConflictChoice};
use crate::mapping;
use crate::widgets::{badge, section_label};
use egui::{Color32, RichText, Ui};
use ph_commands::{EntryKind, HistoryEntry};
use std::collections::BTreeSet;
use std::time::SystemTime;

/// Transactions that are currently in effect: `Do` entries not reverted by a later `Undo`
/// (an `Undo` that was itself reverted by a `Redo` puts them back). Also returns the set of
/// `Undo` entries still in effect, which can be redone.
pub fn live_sets(history: &[HistoryEntry]) -> (BTreeSet<u64>, BTreeSet<u64>) {
    let mut live: BTreeSet<u64> = BTreeSet::new();
    let mut live_undos: BTreeSet<u64> = BTreeSet::new();
    for e in history {
        match &e.kind {
            EntryKind::Do => {
                live.insert(e.txn);
            }
            EntryKind::Undo { of } => {
                for t in of {
                    live.remove(t);
                }
                live_undos.insert(e.txn);
            }
            EntryKind::Redo { of } => {
                // a redo reverts the listed undo entries and restores what they undid
                for u in of {
                    live_undos.remove(u);
                    if let Some(UndoOf(restored)) =
                        history.iter().find(|x| x.txn == *u).map(undo_of)
                    {
                        live.extend(restored);
                    }
                }
            }
        }
    }
    (live, live_undos)
}

struct UndoOf(Vec<u64>);

fn undo_of(e: &HistoryEntry) -> UndoOf {
    match &e.kind {
        EntryKind::Undo { of } => UndoOf(of.clone()),
        _ => UndoOf(Vec::new()),
    }
}

pub fn clock(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let history = core.history.clone();
    let (live, live_undos) = live_sets(&history);
    let authors = core.authors();

    ui.horizontal_wrapped(|ui| {
        if ui.button("Undo mine").on_hover_text("Reverts your own last change (Ctrl+Z)").clicked() {
            core.send(mapping::undo_mine());
        }
        if ui.button("Redo mine").on_hover_text("Ctrl+Y").clicked() {
            core.send(mapping::redo_mine());
        }
        for a in &authors {
            if ui
                .button(format!("Undo all by {a}"))
                .on_hover_text("Reverts that author's work, newest first. Stops and asks if you changed the same things since.")
                .clicked()
            {
                core.send(mapping::undo_author(a, "stop"));
            }
        }
    });
    ui.separator();
    if history.is_empty() {
        ui.label(
            RichText::new("Nothing yet. Edit something and it appears here.")
                .color(theme.colors.text_dim),
        );
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            for e in history.iter().rev() {
                let is_live_do = matches!(e.kind, EntryKind::Do) && live.contains(&e.txn);
                let reverted = matches!(e.kind, EntryKind::Do) && !live.contains(&e.txn);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{:>4}", e.txn))
                            .monospace()
                            .color(theme.colors.text_dim),
                    );
                    let fill = match &e.author {
                        ph_commands::Author::Human => theme.colors.accent,
                        ph_commands::Author::Agent(_) => theme.colors.agent,
                    };
                    badge(ui, &e.author.to_string(), fill);
                    let text = e.summary.clone().unwrap_or_else(|| {
                        e.commands
                            .first()
                            .map_or("(no summary)".to_string(), |c| c.command.clone())
                    });
                    let label = match &e.kind {
                        EntryKind::Do => text,
                        EntryKind::Undo { of } => format!(
                            "undo of {}",
                            of.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
                        ),
                        EntryKind::Redo { of } => format!(
                            "redo of {}",
                            of.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
                        ),
                    };
                    let mut rt = RichText::new(label);
                    if reverted {
                        rt = rt.strikethrough().color(theme.colors.text_dim);
                    }
                    ui.label(rt);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if is_live_do
                            && ui
                                .small_button("Undo")
                                .on_hover_text("Revert only this transaction")
                                .clicked()
                        {
                            core.send(mapping::undo_txn(e.txn, "stop"));
                        }
                        if matches!(e.kind, EntryKind::Undo { .. })
                            && live_undos.contains(&e.txn)
                            && ui
                                .small_button("Redo")
                                .on_hover_text("Undo this undo")
                                .clicked()
                        {
                            core.send(mapping::undo_txn(e.txn, "stop"));
                        }
                        let when = core
                            .txn_times
                            .get(&e.txn)
                            .map_or("--:--:--".to_string(), |t| clock(*t));
                        ui.label(
                            RichText::new(when)
                                .monospace()
                                .size(10.0)
                                .color(theme.colors.text_dim),
                        );
                    });
                });
            }
        });
}

/// The modal conflict dialog. Returns nothing; it resolves through the core.
pub fn conflict_dialog(ctx: &egui::Context, core: &mut AppCore) {
    let Some(d) = core.conflict.clone() else {
        return;
    };
    let mut choice = None;
    egui::Window::new("Undo stopped: conflicts")
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Someone else changed {} thing(s) after the change you are undoing. Undo never overwrites another author's work silently.",
                d.conflicts.len()
            ));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                egui::Grid::new("conflicts").striped(true).show(ui, |ui| {
                    section_label(ui, "Object", &core.theme);
                    section_label(ui, "Fields", &core.theme);
                    section_label(ui, "Changed by", &core.theme);
                    ui.end_row();
                    for c in &d.conflicts {
                        ui.label(c.path.to_string());
                        ui.label(if c.fields.is_empty() { "(whole object)".to_string() } else { c.fields.join(", ") });
                        ui.label(c.changed_by.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "));
                        ui.end_row();
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Skip these objects").on_hover_text("Undo everything else, leave the conflicting objects as they are").clicked() {
                    choice = Some(ConflictChoice::Skip);
                }
                if ui
                    .button(RichText::new("Force").color(Color32::from_rgb(255, 180, 84)))
                    .on_hover_text("Overwrite their changes too")
                    .clicked()
                {
                    choice = Some(ConflictChoice::Force);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(ConflictChoice::Cancel);
                }
            });
        });
    if let Some(c) = choice {
        core.resolve_conflict(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Author, Request};
    use ph_core::PatchSet;

    fn entry(txn: u64, kind: EntryKind) -> HistoryEntry {
        HistoryEntry {
            txn,
            author: Author::Human,
            kind,
            commands: vec![Request::new("x", serde_json::json!({}))],
            patches: PatchSet::default(),
            summary: None,
        }
    }

    #[test]
    fn undo_and_redo_flip_liveness() {
        let h = vec![
            entry(1, EntryKind::Do),
            entry(2, EntryKind::Do),
            entry(3, EntryKind::Undo { of: vec![2] }),
        ];
        let (live, undos) = live_sets(&h);
        assert_eq!(live.into_iter().collect::<Vec<_>>(), vec![1]);
        assert!(undos.contains(&3));
        let mut h2 = h.clone();
        h2.push(entry(4, EntryKind::Redo { of: vec![3] }));
        let (live, undos) = live_sets(&h2);
        assert_eq!(live.into_iter().collect::<Vec<_>>(), vec![1, 2]);
        assert!(undos.is_empty());
    }

    #[test]
    fn clock_formats_time_of_day() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3600 * 5 + 62);
        assert_eq!(clock(t), "05:01:02");
    }
}
