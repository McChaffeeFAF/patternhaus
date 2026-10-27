//! Agent preview mode in the UI (M2/T9): an agent's proposal shows as ghost notes in the piano
//! roll and ghost clips in the playlist, drawn in the agent's colour, with Accept and Reject.
//!
//! Nothing here edits the project: Accept and Reject are the `proposal.accept` and
//! `proposal.reject` commands, sent like any other, and the ghosts are read from the previews
//! the session keeps. A proposal that other edits made stale cannot be accepted; it can only
//! be dismissed.

use crate::core::AppCore;
use crate::widgets::{badge, btn, btn_enabled, section_label};
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Shape, Stroke, StrokeKind, Ui, Vec2};
use ph_commands::{ProposalInfo, ProposalStatus, Request};
use ph_core::{ChannelId, PatternId, Tick};
use serde_json::json;

pub fn accept(id: u64) -> Request {
    Request::new("proposal.accept", json!({ "id": id }))
}

pub fn reject(id: u64) -> Request {
    Request::new("proposal.reject", json!({ "id": id }))
}

/// A note a pending proposal would add (or change), to draw in the piano roll.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewNote {
    pub channel: ChannelId,
    pub key: u8,
    pub start: Tick,
    pub length: Tick,
    /// The note would be removed rather than added.
    pub removed: bool,
}

/// A playlist clip a pending proposal would add (or change) or remove.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewClip {
    pub track: u32,
    pub start: Tick,
    pub length: Tick,
    pub label: String,
    pub removed: bool,
    pub author: String,
}

/// Proposals waiting for a decision that can still be applied.
pub fn live(proposals: &[ProposalInfo]) -> impl Iterator<Item = &ProposalInfo> {
    proposals
        .iter()
        .filter(|p| p.pending() && p.stale.is_none())
}

/// The ghost notes of every live proposal in `pattern`.
pub fn roll_previews(proposals: &[ProposalInfo], pattern: PatternId) -> Vec<PreviewNote> {
    let mut out = Vec::new();
    for p in live(proposals) {
        for g in p.ghost.notes.iter().filter(|g| g.pattern == pattern) {
            out.push(PreviewNote {
                channel: g.channel,
                key: g.note.key,
                start: g.note.start,
                length: g.note.length,
                removed: false,
            });
        }
        for (pid, ch, nid) in p.ghost.removed_notes.iter().filter(|r| r.0 == pattern) {
            if let Some(n) = p
                .base
                .patterns
                .get(pid)
                .and_then(|pat| pat.notes.get(ch))
                .and_then(|m| m.get(nid))
            {
                out.push(PreviewNote {
                    channel: *ch,
                    key: n.key,
                    start: n.start,
                    length: n.length,
                    removed: true,
                });
            }
        }
    }
    out
}

/// The ghost clips of every live proposal.
pub fn playlist_previews(proposals: &[ProposalInfo]) -> Vec<PreviewClip> {
    let mut out = Vec::new();
    for p in live(proposals) {
        let author = p.author.to_string();
        for c in &p.ghost.clips {
            out.push(PreviewClip {
                track: c.track,
                start: c.start,
                length: c.length,
                label: p.preview.clip_label(c),
                removed: false,
                author: author.clone(),
            });
        }
        for id in &p.ghost.removed_clips {
            if let Some(c) = p.base.clips.get(id) {
                out.push(PreviewClip {
                    track: c.track,
                    start: c.start,
                    length: c.length,
                    label: p.base.clip_label(c),
                    removed: true,
                    author: author.clone(),
                });
            }
        }
    }
    out
}

/// Paints one preview box: a translucent fill and a dashed-looking outline in the agent colour.
/// A removal is struck through instead.
pub fn paint_ghost_box(painter: &egui::Painter, r: Rect, agent: Color32, removed: bool) {
    if removed {
        painter.rect_stroke(
            r,
            CornerRadius::same(2),
            Stroke::new(1.0, agent.gamma_multiply(0.7)),
            StrokeKind::Inside,
        );
        painter.line_segment([r.left_top(), r.right_bottom()], Stroke::new(1.0, agent));
        return;
    }
    painter.rect_filled(r, CornerRadius::same(2), agent.gamma_multiply(0.30));
    // dashes along the top and bottom edges, solid ends
    let step = 6.0;
    let mut x = r.left();
    while x < r.right() {
        let x2 = (x + step * 0.6).min(r.right());
        for y in [r.top(), r.bottom()] {
            painter.add(Shape::line_segment(
                [Pos2::new(x, y), Pos2::new(x2, y)],
                Stroke::new(1.5, agent),
            ));
        }
        x += step;
    }
    for x in [r.left(), r.right()] {
        painter.add(Shape::line_segment(
            [Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
            Stroke::new(1.5, agent),
        ));
    }
}

fn counts(p: &ProposalInfo) -> String {
    let g = &p.ghost;
    let notes = g.notes.len() + g.more_notes;
    let mut parts = Vec::new();
    if notes > 0 {
        parts.push(format!("{notes} note{}", if notes == 1 { "" } else { "s" }));
    }
    if !g.clips.is_empty() {
        parts.push(format!(
            "{} clip{}",
            g.clips.len(),
            if g.clips.len() == 1 { "" } else { "s" }
        ));
    }
    let removed = g.removed_notes.len() + g.removed_clips.len();
    if removed > 0 {
        parts.push(format!("{removed} removed"));
    }
    if parts.is_empty() {
        "settings".into()
    } else {
        parts.join(", ")
    }
}

/// One proposal as a card: who, what, and the buttons. `compact` is the one-line strip.
fn card(ui: &mut Ui, core: &mut AppCore, p: &ProposalInfo, compact: bool) {
    let theme = core.theme.clone();
    let stale = p.stale.is_some();
    egui::Frame::group(ui.style())
        .fill(theme.colors.bg3)
        .stroke(Stroke::new(1.5, theme.colors.agent.gamma_multiply(0.8)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                badge(ui, &p.author.to_string(), theme.colors.agent, &theme);
                crate::widgets::leave_room(ui, if compact { 190.0 } else { 0.0 }, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}  ({})", p.title, counts(p))).strong(),
                        )
                        .truncate(),
                    )
                });
                if compact || stale {
                    buttons(ui, core, p, stale);
                }
            });
            if let Some(why) = &p.stale {
                ui.label(
                    RichText::new(format!("Stale, it can't be applied: {why}"))
                        .size(11.5)
                        .color(theme.colors.warn),
                );
            } else if !compact {
                if let Some(s) = &p.summary {
                    ui.add(
                        egui::Label::new(RichText::new(s).size(11.5).color(theme.colors.dim))
                            .truncate(),
                    );
                }
                ui.horizontal(|ui| {
                    buttons(ui, core, p, false);
                    ui.label(
                        RichText::new("Shown as ghosts in the piano roll and playlist")
                            .size(11.0)
                            .color(theme.colors.faint),
                    );
                });
            }
        });
}

fn buttons(ui: &mut Ui, core: &mut AppCore, p: &ProposalInfo, stale: bool) {
    if !stale {
        if btn_enabled(ui, true, "Accept").clicked() {
            core.send(accept(p.id));
        }
        if btn(ui, "Reject").clicked() {
            core.send(reject(p.id));
        }
    } else if btn(ui, "Dismiss").clicked() {
        core.send(reject(p.id));
    }
}

/// A strip at the top of a panel for the first pending proposal (with a count of the rest).
/// Draws nothing when there is none.
pub fn bar(ui: &mut Ui, core: &mut AppCore) {
    let pending: Vec<ProposalInfo> = core
        .proposals
        .iter()
        .filter(|p| p.pending())
        .cloned()
        .collect();
    let Some(first) = pending.first() else {
        return;
    };
    card(ui, core, first, true);
    if pending.len() > 1 {
        ui.label(
            RichText::new(format!(
                "{} more waiting in the Agents panel",
                pending.len() - 1
            ))
            .size(11.0)
            .color(core.theme.colors.dim),
        );
    }
}

/// The Agents panel's section: pending proposals as cards, then the recent decisions.
pub fn section(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let list = core.proposals.clone();
    if list.is_empty() {
        return;
    }
    section_label(ui, "PROPOSALS", &theme);
    for p in list.iter().filter(|p| p.pending()) {
        card(ui, core, p, false);
        ui.add_space(4.0);
    }
    let decided: Vec<&ProposalInfo> = list.iter().filter(|p| !p.pending()).collect();
    for p in decided.iter().rev().take(5) {
        let (word, colour) = match &p.status {
            ProposalStatus::Accepted { .. } => ("accepted", theme.colors.ok),
            ProposalStatus::Rejected { .. } => ("rejected", theme.colors.dim),
            ProposalStatus::Stale { .. } => ("stale", theme.colors.warn),
            ProposalStatus::Pending => continue,
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(word.to_uppercase()).size(10.5).color(colour));
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{}: {}", p.author, p.title))
                        .size(11.5)
                        .color(theme.colors.dim),
                )
                .truncate(),
            );
        });
    }
    ui.add_space(10.0);
}

/// The top-left corner and size of a ghost box in a view (shared by the roll and playlist
/// painters); kept here so both clamp the same way.
pub fn at_least_px(r: Rect, min_w: f32) -> Rect {
    Rect::from_min_size(r.min, Vec2::new(r.width().max(min_w), r.height()))
}
