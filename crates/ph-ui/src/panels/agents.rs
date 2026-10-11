//! Agents panel: connected clients and their scopes, the grant prompt for new clients, and what
//! each agent did recently. The connection data comes from T4 through the `AgentDirectory`
//! hook; recent actions come from the history log.

use crate::core::AppCore;
use crate::mapping;
use crate::widgets::{badge, section_label};
use egui::{RichText, Ui};
use ph_commands::{Author, Scope};

pub fn scope_name(s: Scope) -> &'static str {
    match s {
        Scope::Read => "read",
        Scope::Compose => "compose",
        Scope::Mix => "mix",
        Scope::Full => "full",
    }
}

/// Live connections first (in the order given), then the remembered ones.
pub fn live_first(mut clients: Vec<crate::core::ClientInfo>) -> Vec<crate::core::ClientInfo> {
    clients.sort_by_key(|c| !c.live);
    clients
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let dir = core.hooks.agents.clone();

    // grant prompts first: they block an agent until answered
    if let Some(d) = &dir {
        for g in d.pending_grants() {
            egui::Frame::group(ui.style())
                .fill(theme.colors.bg3)
                .show(ui, |ui| {
                    ui.label(RichText::new(format!("{} wants to connect", g.client)).strong());
                    ui.label(format!(
                        "Requested: {}",
                        g.requested
                            .iter()
                            .map(|s| scope_name(*s))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Allow").clicked() {
                            d.resolve_grant(g.id, Some(g.requested.clone()));
                        }
                        if ui.button("Read only").clicked() {
                            d.resolve_grant(g.id, Some(vec![Scope::Read]));
                        }
                        if ui.button("Deny").clicked() {
                            d.resolve_grant(g.id, None);
                        }
                    });
                });
        }
    }

    section_label(ui, "CONNECTED", &theme);
    match &dir {
        Some(d) => {
            let clients = live_first(d.clients());
            if !clients.iter().any(|c| c.live) {
                ui.label(RichText::new("No agent is connected.").color(theme.colors.dim));
            }
            let mut shown_remembered = false;
            for c in clients {
                if !c.live && !shown_remembered {
                    shown_remembered = true;
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Remembered, not connected")
                            .size(11.0)
                            .color(theme.colors.dim),
                    );
                }
                ui.horizontal(|ui| {
                    let fill = if c.live {
                        theme.colors.agent
                    } else {
                        theme.colors.agent.gamma_multiply(0.35)
                    };
                    badge(ui, &c.name, fill);
                    for s in &c.scopes {
                        badge(
                            ui,
                            scope_name(*s),
                            if c.live {
                                theme.colors.bg4
                            } else {
                                theme.colors.bg4.gamma_multiply(0.5)
                            },
                        );
                    }
                    if c.live {
                        ui.label(
                            RichText::new(format!(
                                "connected {} s, {} command(s), idle {} s",
                                c.connected_for, c.commands, c.idle_for
                            ))
                            .color(theme.colors.dim),
                        );
                    } else {
                        ui.label(
                            RichText::new("disconnected")
                                .italics()
                                .color(theme.colors.dim.gamma_multiply(0.7)),
                        );
                    }
                });
            }
        }
        None => {
            ui.label(
                RichText::new("The local agent endpoint is not running in this build.")
                    .color(theme.colors.dim),
            );
        }
    }

    ui.add_space(10.0);
    section_label(ui, "ACTIVITY BY AUTHOR", &theme);
    let history = core.history.clone();
    let mut authors: Vec<Author> = history.iter().map(|e| e.author.clone()).collect();
    authors.sort();
    authors.dedup();
    if authors.is_empty() {
        ui.label(RichText::new("No edits yet.").color(theme.colors.dim));
    }
    for a in authors {
        let mine: Vec<_> = history.iter().filter(|e| e.author == a).collect();
        ui.horizontal(|ui| {
            badge(
                ui,
                &a.to_string(),
                if a == Author::Human {
                    theme.colors.accent
                } else {
                    theme.colors.agent
                },
            );
            ui.label(format!("{} change(s)", mine.len()));
            if a != Author::Human && ui.small_button("Undo all").clicked() {
                core.send(mapping::undo_author(&a.to_string(), "stop"));
            }
        });
        for e in mine.iter().rev().take(5) {
            ui.label(
                RichText::new(format!(
                    "   {}  {}",
                    e.txn,
                    e.summary.clone().unwrap_or_else(|| e
                        .commands
                        .first()
                        .map(|c| c.command.clone())
                        .unwrap_or_default())
                ))
                .size(11.0)
                .color(theme.colors.dim),
            );
        }
    }
    if !core.agent_select.is_empty() {
        ui.add_space(10.0);
        section_label(ui, "AGENT IS POINTING AT", &theme);
        for p in &core.agent_select {
            ui.label(p.to_string());
        }
    }
}
