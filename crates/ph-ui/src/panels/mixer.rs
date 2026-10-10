//! Mixer: insert strips with meters, faders, pan, mute and solo; effect slots (10 per insert),
//! sends, route-to-master and sidechain for the selected insert, and generic effect panels
//! built from `ph_engine::catalog` descriptors.

use crate::core::{AppCore, EffectEdit, PanelAction, RenameTarget};
use crate::mapping;
use crate::theme::rgb;
use crate::widgets::{self, Edit, fader, knob, param_grid, toggle};
use crate::workspace::WindowId;
use egui::{
    Align2, CornerRadius, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use ph_core::{EffectSpec, InsertId, MixerInsert, ObjectPath};

pub const STRIP_W: f32 = 84.0;
pub const FADER_MAX: f64 = 1.5;
pub const MAX_EFFECTS: usize = 10;

#[derive(Default)]
pub struct MixerUi {
    /// Shown inserts, from 0 (master).
    pub strips: usize,
}

pub fn volume_db(v: f32) -> String {
    if v <= 0.0005 {
        "-inf".into()
    } else {
        format!("{:.1} dB", 20.0 * v.log10())
    }
}

/// Insert label: its name, or the number when unnamed.
fn insert_label(i: usize, m: &MixerInsert) -> String {
    if m.name.is_empty() {
        format!("{i}")
    } else {
        m.name.clone()
    }
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::Mixer);
    let project = core.project.clone();
    let live = core.live();
    let n = project.mixer.len();
    let strip_h = (ui.available_height() - 22.0).clamp(220.0, 380.0);

    // strips on the left, effects / sends / the selected effect's knobs on the right
    // inside the window's scroll area the available size is unbounded, so use the window's own
    // size, but never squeeze the strips and the detail pane below a usable minimum
    let view = if core.view_size.x <= 1.0 {
        ui.available_size()
    } else {
        core.view_size
    };
    let full = egui::vec2(view.x.max(460.0), view.y.max(300.0));
    let detail_w = (full.x * 0.42).clamp(300.0, 560.0).min(full.x * 0.6);
    let strips_w = (full.x - detail_w - 10.0).max(120.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(strips_w, full.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("mixer_strips")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            for i in 0..n {
                                strip(ui, core, i, strip_h, &live);
                            }
                        });
                    });
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(detail_w, full.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| detail(ui, core),
        );
    });
}

fn strip(ui: &mut Ui, core: &mut AppCore, i: usize, h: f32, live: &crate::live::Live) {
    let theme = core.theme.clone();
    let m = core.project.mixer[i].clone();
    let selected = core.sel.insert == i as u32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, h), Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect.shrink(1.0),
        CornerRadius::same(4),
        if selected {
            theme.colors.panel_alt
        } else {
            theme.colors.panel
        },
    );
    if selected {
        p.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(4),
            Stroke::new(1.5, theme.colors.accent),
            StrokeKind::Inside,
        );
    }
    if core
        .agent_select
        .contains(&ObjectPath::Insert(InsertId(i as u32)))
    {
        p.rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(2.0, theme.colors.agent),
            StrokeKind::Inside,
        );
    }
    if let Some((who, at)) = core.touches.get(&ObjectPath::Insert(InsertId(i as u32))) {
        let k = (1.0 - ((core.now - at) / crate::piano_roll::FLASH_SECS) as f32).clamp(0.0, 1.0);
        if k > 0.0 {
            p.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(2.0, theme.colors.agent.gamma_multiply(k)),
                StrokeKind::Inside,
            );
            p.text(
                rect.right_top() + Vec2::new(-4.0, 2.0),
                Align2::RIGHT_TOP,
                who.trim_start_matches("agent:"),
                FontId::proportional(9.0),
                theme.colors.agent,
            );
        }
    }
    // header: number + name
    let head = Rect::from_min_size(
        rect.min + Vec2::new(2.0, 2.0),
        Vec2::new(STRIP_W - 4.0, 34.0),
    );
    let hr = ui.interact(head, ui.id().with(("strip", i)), Sense::click());
    p.rect_filled(
        head,
        CornerRadius::same(3),
        m.color
            .map_or(theme.colors.widget, rgb)
            .gamma_multiply(0.55),
    );
    p.text(
        head.left_top() + Vec2::new(4.0, 2.0),
        Align2::LEFT_TOP,
        if i == 0 {
            "M".to_string()
        } else {
            format!("{i}")
        },
        FontId::proportional(9.0),
        theme.colors.text_dim,
    );
    p.text(
        head.center() + Vec2::new(0.0, 6.0),
        Align2::CENTER_CENTER,
        insert_label(i, &m),
        FontId::proportional(11.0),
        theme.colors.text,
    );
    if hr.clicked() {
        core.sel.insert = i as u32;
        core.sel.effect = None;
    }
    if hr.double_clicked() {
        core.open_rename(RenameTarget::Insert(i as u32));
    }
    hr.context_menu(|ui| {
        if ui.button("Rename…").clicked() {
            core.open_rename(RenameTarget::Insert(i as u32));
            ui.close();
        }
    });

    // meter + fader
    let body_top = head.bottom() + 6.0;
    let body_h = rect.bottom() - body_top - 62.0;
    let meter_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 8.0, body_top),
        Vec2::new(18.0, body_h),
    );
    let peak = live.peak.get(i).copied().unwrap_or([0.0; 2]);
    let rms = live.rms.get(i).copied().unwrap_or([0.0; 2]);
    widgets::paint_meter(ui, meter_rect, peak, rms, &theme);
    let fader_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 32.0, body_top),
        Vec2::new(40.0, body_h),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(fader_rect));
    let default = if i == 0 { 1.0 } else { 0.8 };
    let (_, e) = fader(
        &mut child,
        ui.id().with(("fader", i)),
        m.volume as f64,
        0.0,
        FADER_MAX,
        default,
        fader_rect.size(),
        &theme,
    );
    if let Some(v) = e.commit() {
        core.send(mapping::mixer_volume(i as u32, v as f32));
    }
    let shown = e.shown(m.volume as f64) as f32;
    ui.painter().text(
        Pos2::new(rect.center().x, rect.bottom() - 54.0),
        Align2::CENTER_CENTER,
        volume_db(shown),
        FontId::proportional(10.0),
        theme.colors.text_dim,
    );
    // pan knob + buttons
    let kr = Rect::from_min_size(
        Pos2::new(rect.left() + 4.0, rect.bottom() - 44.0),
        Vec2::splat(24.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(kr));
    let (_, e) = knob(
        &mut child,
        ui.id().with(("pan", i)),
        m.pan as f64,
        -1.0,
        1.0,
        0.0,
        24.0,
        &theme,
    );
    if let Some(v) = e.commit() {
        core.send(mapping::mixer_pan(i as u32, v as f32));
    }
    let br = Rect::from_min_size(
        Pos2::new(rect.left() + 32.0, rect.bottom() - 46.0),
        Vec2::new(48.0, 20.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
    child.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if toggle(ui, m.muted, "M", theme.colors.warn, &theme).clicked() {
            core.send(mapping::mixer_mute(i as u32, !m.muted));
        }
        if i > 0 {
            let solo = core.solo_inserts.contains(&(i as u32));
            if toggle(ui, solo, "S", theme.colors.ok, &theme).clicked() {
                core.send(mapping::mixer_solo(i as u32, !solo));
            }
        }
    });
    let nfx = m.effects.len();
    ui.painter().text(
        Pos2::new(rect.center().x, rect.bottom() - 10.0),
        Align2::CENTER_CENTER,
        if nfx == 0 {
            String::new()
        } else {
            format!("{nfx} fx")
        },
        FontId::proportional(9.0),
        theme.colors.text_dim,
    );
}

fn detail(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let project = core.project.clone();
    let ins = core.sel.insert as usize;
    let Some(m) = project.mixer.get(ins).cloned() else {
        return;
    };
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt("mixer_detail")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}  (insert {ins})", insert_label(ins, &m))).strong(),
                );
                if ins > 0 {
                    let mut route = m.route_to_master;
                    if ui.checkbox(&mut route, "Route to master").changed() {
                        core.send(mapping::route_to_master(ins as u32, route));
                    }
                    ui.label("Sidechain from");
                    let cur = m.sidechain_from.map_or("none".to_string(), |s| {
                        format!(
                            "{} {}",
                            s.0,
                            project
                                .mixer
                                .get(s.0 as usize)
                                .map_or("", |x| x.name.as_str())
                        )
                    });
                    egui::ComboBox::from_id_salt("sidechain")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(m.sidechain_from.is_none(), "none")
                                .clicked()
                            {
                                core.send(mapping::sidechain(ins as u32, None));
                            }
                            for (j, other) in project.mixer.iter().enumerate().skip(1) {
                                if j != ins
                                    && ui
                                        .selectable_label(
                                            m.sidechain_from == Some(InsertId(j as u32)),
                                            format!("{j} {}", other.name),
                                        )
                                        .clicked()
                                {
                                    core.send(mapping::sidechain(ins as u32, Some(j as u32)));
                                }
                            }
                        });
                }
            });
            ui.separator();
            {
                // ---- effect slots
                widgets::section_label(ui, "EFFECTS", &theme);
                for (idx, slot) in m.effects.iter().enumerate() {
                    let name = match &slot.effect {
                        EffectSpec::Builtin { name, .. } => name.clone(),
                        EffectSpec::Plugin { plugin_id, .. } => plugin_id.clone(),
                    };
                    ui.horizontal(|ui| {
                        let sel = core.sel.effect == Some(idx);
                        if ui
                            .selectable_label(sel, format!("{}. {}", idx + 1, name))
                            .clicked()
                        {
                            core.sel.effect = Some(idx);
                            core.window_requests.push((
                                WindowId::Effect(InsertId(ins as u32), idx as u32),
                                PanelAction::Open,
                            ));
                        }
                        if toggle(ui, slot.bypass, "Byp", theme.colors.warn, &theme).clicked() {
                            core.send(mapping::effect_bypass(ins as u32, idx, !slot.bypass));
                        }
                        let (_, e) = knob(
                            ui,
                            Id::new(("fxmix", ins, idx)),
                            slot.mix as f64,
                            0.0,
                            1.0,
                            1.0,
                            22.0,
                            &theme,
                        );
                        if let Some(v) = e.commit() {
                            core.send(mapping::effect_mix(ins as u32, idx, v as f32));
                        }
                        ui.label(RichText::new("mix").size(10.0).color(theme.colors.text_dim));
                        if idx > 0 && ui.small_button("^").on_hover_text("Move up").clicked() {
                            core.send(mapping::effect_move(ins as u32, idx, idx - 1));
                            core.effect_edits.push(EffectEdit::Move {
                                insert: ins as u32,
                                from: idx as u32,
                                to: idx as u32 - 1,
                            });
                            core.sel.effect = Some(idx - 1);
                        }
                        if idx + 1 < m.effects.len()
                            && ui.small_button("v").on_hover_text("Move down").clicked()
                        {
                            core.send(mapping::effect_move(ins as u32, idx, idx + 1));
                            core.effect_edits.push(EffectEdit::Move {
                                insert: ins as u32,
                                from: idx as u32,
                                to: idx as u32 + 1,
                            });
                            core.sel.effect = Some(idx + 1);
                        }
                        if ui.small_button("x").on_hover_text("Remove").clicked() {
                            core.send(mapping::effect_remove(ins as u32, idx));
                            core.effect_edits.push(EffectEdit::Remove {
                                insert: ins as u32,
                                slot: idx as u32,
                            });
                            core.sel.effect = None;
                        }
                    });
                }
                if m.effects.len() < MAX_EFFECTS {
                    ui.menu_button("+ Add effect", |ui| {
                        for name in ph_engine::catalog::builtin_effects() {
                            if ui.button(*name).clicked() {
                                core.send(mapping::effect_add(ins as u32, name));
                                core.sel.effect = Some(m.effects.len());
                                ui.close();
                            }
                        }
                    });
                } else {
                    ui.label(
                        RichText::new("Insert is full (10 effects)").color(theme.colors.text_dim),
                    );
                }

                // ---- sends
                ui.add_space(8.0);
                widgets::section_label(ui, "SENDS", &theme);
                for s in &m.sends {
                    ui.horizontal(|ui| {
                        let to = s.to.0;
                        ui.label(format!(
                            "-> {} {}",
                            to,
                            project
                                .mixer
                                .get(to as usize)
                                .map_or("", |x| x.name.as_str())
                        ));
                        let (_, e) = knob(
                            ui,
                            Id::new(("send", ins, to)),
                            s.level as f64,
                            0.0,
                            2.0,
                            0.5,
                            22.0,
                            &theme,
                        );
                        if let Some(v) = e.commit() {
                            core.send(mapping::send_level(ins as u32, to, v as f32));
                        }
                        ui.label(volume_db(e.shown(s.level as f64) as f32));
                        if ui.small_button("x").clicked() {
                            core.send(mapping::send_remove(ins as u32, to));
                        }
                    });
                }
                ui.menu_button("+ Add send", |ui| {
                    for (j, other) in project.mixer.iter().enumerate() {
                        if j != ins
                            && j != 0
                            && !m.sends.iter().any(|s| s.to.0 as usize == j)
                            && ui.button(format!("{j} {}", other.name)).clicked()
                        {
                            core.send(mapping::send_add(ins as u32, j as u32, 0.5));
                            ui.close();
                        }
                    }
                });

                ui.add_space(6.0);
                ui.label(
                    RichText::new("Click an effect to open its own window.")
                        .size(11.0)
                        .color(theme.colors.text_dim),
                );
            }
        });
}

/// The contents of an effect window: the slot's bypass and mix, and every parameter knob.
pub fn effect_window(ui: &mut Ui, core: &mut AppCore, insert: u32, index: usize) {
    let theme = core.theme.clone();
    let Some(slot) = core
        .project
        .mixer
        .get(insert as usize)
        .and_then(|m| m.effects.get(index))
        .cloned()
    else {
        ui.label("This effect no longer exists.");
        return;
    };
    ui.horizontal(|ui| {
        if toggle(ui, slot.bypass, "Bypass", theme.colors.warn, &theme).clicked() {
            core.send(mapping::effect_bypass(insert, index, !slot.bypass));
        }
        let (_, e) = knob(
            ui,
            Id::new(("fxwin_mix", insert, index)),
            slot.mix as f64,
            0.0,
            1.0,
            1.0,
            22.0,
            &theme,
        );
        if let Some(v) = e.commit() {
            core.send(mapping::effect_mix(insert, index, v as f32));
        }
        ui.label(RichText::new("mix").size(10.0).color(theme.colors.text_dim));
    });
    ui.separator();
    // the window itself scrolls (see `windows::show_content`)
    effect_panel(ui, core, insert, index, &slot.effect);
}

fn effect_panel(ui: &mut Ui, core: &mut AppCore, insert: u32, index: usize, effect: &EffectSpec) {
    let theme = core.theme.clone();
    let Ok(desc) = ph_engine::catalog::effect_descriptor(effect) else {
        ui.label("This effect has no parameter descriptor.");
        return;
    };
    ui.label(RichText::new(&desc.name).strong());
    let overrides = match effect {
        EffectSpec::Builtin { params, .. } => params.clone(),
        _ => Default::default(),
    };
    // Group equaliser-style prefixed params (b1_, b2_...) into rows of their own.
    let changes = param_grid(
        ui,
        Id::new(("fx", insert, index)),
        &desc,
        &|_| true,
        &|info| overrides.get(&info.id).copied().unwrap_or(info.default),
        &theme,
    );
    for (param, value) in changes {
        core.send(mapping::effect_param(insert, index, &param, value));
    }
    let _ = Edit::None;
}
