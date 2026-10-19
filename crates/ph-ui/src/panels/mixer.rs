//! Mixer: insert strips with meters, faders, pan, mute and solo; effect slots (10 per insert),
//! sends, route-to-master and sidechain for the selected insert, and generic effect panels
//! built from `ph_engine::catalog` descriptors.

use crate::core::{AppCore, EffectEdit, PanelAction, RenameTarget};
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::mapping;
use crate::theme::caps;
use crate::theme::rgb;
use crate::widgets::{self, Edit, fader, icon_button, knob, menu_item, toggle};
use crate::workspace::WindowId;
use egui::{
    Align2, CornerRadius, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use ph_core::{EffectSpec, InsertId, MixerInsert, ObjectPath};

pub const STRIP_W: f32 = 88.0;
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
/// Room the controls after an effect slot's name take: bypass, mix knob and caption, up, down, close.
const SLOT_CONTROLS_W: f32 = 190.0;
/// Room the controls after a send's name take: level knob, dB readout, remove.
const SEND_CONTROLS_W: f32 = 130.0;

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
    let c = &theme.colors;
    let m = core.project.mixer[i].clone();
    let selected = core.sel.insert == i as u32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, h), Sense::hover());
    let p = ui.painter().clone();
    let round = CornerRadius::same(theme.metrics.radius_lg as u8);
    p.rect_filled(
        rect.shrink(1.0),
        round,
        if selected { c.bg3 } else { c.bg2 },
    );
    p.rect_stroke(
        rect.shrink(1.0),
        round,
        Stroke::new(1.0, if selected { c.accent } else { c.line_soft }),
        StrokeKind::Inside,
    );
    if selected {
        crate::paint::glow_rect(&p, rect.shrink(1.0), round.nw, c.glow_soft, 0.7);
    }
    // colour cap
    let cap_col = m
        .color
        .map_or(c.pattern[i % c.pattern.len()].gamma_multiply(0.8), rgb);
    p.rect_filled(
        Rect::from_min_size(
            rect.min + Vec2::new(5.0, 3.0),
            Vec2::new(STRIP_W - 10.0, 3.0),
        ),
        CornerRadius::same(1),
        cap_col,
    );
    if core
        .agent_select
        .contains(&ObjectPath::Insert(InsertId(i as u32)))
    {
        p.rect_stroke(rect, round, Stroke::new(2.0, c.agent), StrokeKind::Inside);
    }
    if let Some((who, at)) = core.touches.get(&ObjectPath::Insert(InsertId(i as u32))) {
        let k = crate::motion::decay(core.now, *at, crate::piano_roll::FLASH_SECS);
        if k > 0.0 && !core.reduce_motion {
            p.rect_stroke(
                rect,
                round,
                Stroke::new(2.0, c.agent.gamma_multiply(k)),
                StrokeKind::Inside,
            );
            crate::paint::text_fit(
                &p,
                rect.right_top() + Vec2::new(-4.0, 8.0),
                Align2::RIGHT_TOP,
                who.trim_start_matches("agent:"),
                FontId::new(9.0, crate::fonts::cond()),
                c.agent,
                STRIP_W - 30.0,
            );
            ui.ctx().request_repaint();
        }
    }
    // number
    p.text(
        rect.left_top() + Vec2::new(8.0, 10.0),
        Align2::LEFT_TOP,
        if i == 0 {
            "M".to_string()
        } else {
            format!("{i:02}")
        },
        FontId::new(11.0, crate::fonts::mono()),
        if selected { c.accent } else { c.dim },
    );
    let body_top = rect.top() + 28.0;
    let foot = 78.0;
    let body_h = (rect.bottom() - body_top - foot).max(40.0);

    // the name runs vertically up the left edge; click selects, double-click renames
    let name_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 4.0, body_top),
        Vec2::new(18.0, body_h),
    );
    let nr = ui.interact(name_rect, ui.id().with(("strip", i)), Sense::click());
    let label = insert_label(i, &m).to_uppercase();
    let (galley, cut) = crate::paint::fit_galley(
        &p,
        &label,
        FontId::new(12.0, crate::fonts::cond_bold()),
        if selected { c.text } else { c.dim },
        name_rect.height() - 4.0,
    );
    let clip = p.with_clip_rect(name_rect.expand2(Vec2::new(2.0, 0.0)));
    clip.add(
        egui::epaint::TextShape::new(
            Pos2::new(name_rect.left() + 2.0, name_rect.bottom() - 2.0),
            galley,
            c.text,
        )
        .with_angle(-std::f32::consts::FRAC_PI_2),
    );
    let nr = if cut {
        nr.on_hover_text(insert_label(i, &m))
    } else {
        nr
    };
    let nr = nr.hint(
        &insert_label(i, &m),
        "",
        "click selects, double-click renames, right-click for more",
    );
    if nr.clicked() {
        core.sel.insert = i as u32;
        core.sel.effect = None;
    }
    if nr.double_clicked() {
        core.open_rename(RenameTarget::Insert(i as u32));
    }
    nr.context_menu(|ui| {
        if widgets::menu_item(ui, "Rename…", "").clicked() {
            core.open_rename(RenameTarget::Insert(i as u32));
            ui.close();
        }
        crate::hint::standard_entries(
            ui,
            false,
            &ph_control::UiHint {
                name: insert_label(i, &m),
                value: String::new(),
                gestures: "mixer insert".into(),
            },
        );
    });

    // stereo meter + fader
    let meter_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 26.0, body_top),
        Vec2::new(16.0, body_h),
    );
    let peak = live.peak.get(i).copied().unwrap_or([0.0; 2]);
    let rms = live.rms.get(i).copied().unwrap_or([0.0; 2]);
    widgets::paint_meter(
        ui,
        ui.id().with(("meter", i)),
        meter_rect,
        peak,
        rms,
        &theme,
    )
    .hint(
        &format!("{} meter", insert_label(i, &m)),
        &format!("{} peak", widgets::fmt_gain_db(peak[0].max(peak[1]) as f64)),
        "click to clear the clip LED",
    );
    let fader_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 46.0, body_top),
        Vec2::new(34.0, body_h),
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
        &widgets::Ctl::new(
            &format!("{} volume", insert_label(i, &m)),
            &widgets::fmt_gain_db,
        )
        .target(ph_core::ParamPath::InsertVolume(ph_core::InsertId(
            i as u32,
        ))),
        FADER_TICKS,
    );
    if let Some(v) = e.commit() {
        core.send(mapping::mixer_volume(i as u32, v as f32));
    }
    let shown = e.shown(m.volume as f64) as f32;
    // dB readout in a small well
    let readout = Rect::from_center_size(
        Pos2::new(rect.center().x, body_top + body_h + 12.0),
        Vec2::new(STRIP_W - 14.0, 17.0),
    );
    crate::paint::well(&p, readout, 3, c.lcd, &theme);
    p.text(
        readout.center(),
        Align2::CENTER_CENTER,
        volume_db(shown),
        FontId::new(11.0, crate::fonts::mono()),
        if shown > 1.0 { c.warn } else { c.accent },
    );
    // pan knob + buttons
    let kr = Rect::from_min_size(
        Pos2::new(rect.left() + 6.0, rect.bottom() - 51.0),
        Vec2::splat(26.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(kr));
    let (_, e) = knob(
        &mut child,
        ui.id().with(("pan", i)),
        m.pan as f64,
        -1.0,
        1.0,
        0.0,
        26.0,
        &theme,
        &widgets::Ctl::new(&format!("{} pan", insert_label(i, &m)), &widgets::fmt_pan)
            .target(ph_core::ParamPath::InsertPan(ph_core::InsertId(i as u32))),
    );
    if let Some(v) = e.commit() {
        core.send(mapping::mixer_pan(i as u32, v as f32));
    }
    let br = Rect::from_min_size(
        Pos2::new(rect.left() + 36.0, rect.bottom() - 48.0),
        Vec2::new(44.0, 20.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
    child.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if toggle(ui, m.muted, "M", theme.colors.warn, &theme)
            .on_hover_text("Mute this insert")
            .clicked()
        {
            core.send(mapping::mixer_mute(i as u32, !m.muted));
        }
        if i > 0 {
            let solo = core.solo_inserts.contains(&(i as u32));
            if toggle(ui, solo, "S", theme.colors.ok, &theme)
                .on_hover_text("Solo this insert")
                .clicked()
            {
                core.send(mapping::mixer_solo(i as u32, !solo));
            }
        }
    });
    let nfx = m.effects.len();
    p.text(
        Pos2::new(rect.center().x, rect.bottom() - 12.0),
        Align2::CENTER_CENTER,
        if nfx == 0 {
            "NO FX".to_string()
        } else {
            format!("{nfx} FX")
        },
        FontId::new(10.0, crate::fonts::cond()),
        if nfx == 0 { c.faint } else { c.dim },
    );
}

/// Fader scale: gain values at -inf..+3 dB with labels on the majors.
const FADER_TICKS: widgets::Ticks<'static> = &[
    (1.4125, "+3"),
    (1.0, "0"),
    (0.7079, ""),
    (0.5012, "-6"),
    (0.3548, ""),
    (0.2512, "-12"),
    (0.1259, "-18"),
    (0.0631, "-24"),
    (0.0316, ""),
];

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
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{}  (insert {ins})", insert_label(ins, &m)))
                            .font(FontId::new(15.0, crate::fonts::cond_bold())),
                    )
                    .truncate(),
                );
                if ins > 0 {
                    let mut route = m.route_to_master;
                    if ui
                        .checkbox(&mut route, caps("Route to master"))
                        .hint_menu(
                            "Route to master",
                            if route { "on" } else { "off" },
                            "sends this insert to the master; off for a send-only bus",
                        )
                        .changed()
                    {
                        core.send(mapping::route_to_master(ins as u32, route));
                    }
                    ui.label(caps("Sidechain from"));
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
                    let cur2 = cur.clone();
                    ui.scope(|ui| {
                        ui.set_max_width(220.0);
                        egui::ComboBox::from_id_salt("sidechain")
                            .selected_text(cur)
                            .truncate()
                            .show_ui(ui, |ui| {
                                ui.set_max_width(widgets::MENU_W);
                                if ui
                                    .selectable_label(m.sidechain_from.is_none(), "none")
                                    .clicked()
                                {
                                    core.send(mapping::sidechain(ins as u32, None));
                                }
                                for (j, other) in project.mixer.iter().enumerate().skip(1) {
                                    if j != ins
                                        && ui
                                            .add(
                                                egui::Button::selectable(
                                                    m.sidechain_from == Some(InsertId(j as u32)),
                                                    format!("{j} {}", other.name),
                                                )
                                                .truncate(),
                                            )
                                            .clicked()
                                    {
                                        core.send(mapping::sidechain(ins as u32, Some(j as u32)));
                                    }
                                }
                            })
                            .response
                            .hint_menu(
                                "Sidechain source",
                                &cur2,
                                "the insert whose signal drives sidechain compressors on this one",
                            );
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
                        if widgets::leave_room(ui, SLOT_CONTROLS_W, |ui| {
                            ui.add(
                                egui::Button::selectable(sel, format!("{}. {}", idx + 1, name))
                                    .truncate(),
                            )
                        })
                        .clicked()
                        {
                            core.sel.effect = Some(idx);
                            core.window_requests.push((
                                WindowId::Effect(InsertId(ins as u32), idx as u32),
                                PanelAction::Open,
                            ));
                        }
                        if widgets::icon_button_coloured(
                            ui,
                            &theme,
                            Icon::Bypass,
                            slot.bypass,
                            true,
                            "Bypass",
                            "click to bypass this effect",
                            theme.colors.warn,
                        )
                        .clicked()
                        {
                            core.send(mapping::effect_bypass(ins as u32, idx, !slot.bypass));
                        }
                        let (_, e) = knob(
                            ui,
                            Id::new(("fxmix", ins, idx)),
                            slot.mix as f64,
                            0.0,
                            1.0,
                            1.0,
                            24.0,
                            &theme,
                            &widgets::Ctl::new("Effect mix", &widgets::fmt_percent),
                        );
                        if let Some(v) = e.commit() {
                            core.send(mapping::effect_mix(ins as u32, idx, v as f32));
                        }
                        ui.label(widgets::caption(&theme, "Mix"));
                        if idx > 0
                            && icon_button(ui, &theme, Icon::Up, false, "Move up", "").clicked()
                        {
                            core.send(mapping::effect_move(ins as u32, idx, idx - 1));
                            core.effect_edits.push(EffectEdit::Move {
                                insert: ins as u32,
                                from: idx as u32,
                                to: idx as u32 - 1,
                            });
                            core.sel.effect = Some(idx - 1);
                        }
                        if idx + 1 < m.effects.len()
                            && icon_button(ui, &theme, Icon::Down, false, "Move down", "").clicked()
                        {
                            core.send(mapping::effect_move(ins as u32, idx, idx + 1));
                            core.effect_edits.push(EffectEdit::Move {
                                insert: ins as u32,
                                from: idx as u32,
                                to: idx as u32 + 1,
                            });
                            core.sel.effect = Some(idx + 1);
                        }
                        if icon_button(ui, &theme, Icon::Close, false, "Remove effect", "")
                            .clicked()
                        {
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
                    crate::widgets::menu_button(ui, "+ Add effect", |ui| {
                        for name in ph_engine::catalog::builtin_effects() {
                            if menu_item(ui, name, "").clicked() {
                                core.send(mapping::effect_add(ins as u32, name));
                                core.sel.effect = Some(m.effects.len());
                                ui.close();
                            }
                        }
                        ui.separator();
                        let at = m.effects.len();
                        crate::plugin_ui::plugin_menu(ui, core, false, |core, format, id| {
                            core.send(mapping::plugin_add_effect(ins as u32, format, id));
                            core.sel.effect = Some(at);
                        });
                    });
                } else {
                    ui.label(RichText::new("Insert is full (10 effects)").color(theme.colors.dim));
                }

                // ---- sends
                ui.add_space(8.0);
                widgets::section_label(ui, "SENDS", &theme);
                for s in &m.sends {
                    ui.horizontal(|ui| {
                        let to = s.to.0;
                        widgets::leave_room(ui, SEND_CONTROLS_W, |ui| {
                            ui.add(
                                egui::Label::new(format!(
                                    "-> {} {}",
                                    to,
                                    project
                                        .mixer
                                        .get(to as usize)
                                        .map_or("", |x| x.name.as_str())
                                ))
                                .truncate(),
                            )
                        });
                        let (_, e) = knob(
                            ui,
                            Id::new(("send", ins, to)),
                            s.level as f64,
                            0.0,
                            2.0,
                            0.5,
                            24.0,
                            &theme,
                            &widgets::Ctl::new("Send level", &widgets::fmt_gain_db).target(
                                ph_core::ParamPath::InsertSend {
                                    insert: InsertId(ins as u32),
                                    to: ph_core::InsertId(to),
                                },
                            ),
                        );
                        if let Some(v) = e.commit() {
                            core.send(mapping::send_level(ins as u32, to, v as f32));
                        }
                        ui.label(
                            RichText::new(volume_db(e.shown(s.level as f64) as f32))
                                .font(FontId::new(11.5, crate::fonts::mono()))
                                .color(theme.colors.accent),
                        );
                        if icon_button(ui, &theme, Icon::Close, false, "Remove send", "").clicked()
                        {
                            core.send(mapping::send_remove(ins as u32, to));
                        }
                    });
                }
                crate::widgets::menu_button(ui, "+ Add send", |ui| {
                    for (j, other) in project.mixer.iter().enumerate() {
                        if j != ins
                            && j != 0
                            && !m.sends.iter().any(|s| s.to.0 as usize == j)
                            && menu_item(ui, &format!("{j} {}", other.name), "").clicked()
                        {
                            core.send(mapping::send_add(ins as u32, j as u32, 0.5));
                            ui.close();
                        }
                    }
                });

                ui.add_space(6.0);
                ui.label(
                    RichText::new("Click an effect to open its own window.")
                        .size(11.5)
                        .color(theme.colors.dim),
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
        if toggle(ui, slot.bypass, "Bypass", theme.colors.warn, &theme)
            .on_hover_text("Bypass this effect")
            .clicked()
        {
            core.send(mapping::effect_bypass(insert, index, !slot.bypass));
        }
        let (_, e) = knob(
            ui,
            Id::new(("fxwin_mix", insert, index)),
            slot.mix as f64,
            0.0,
            1.0,
            1.0,
            26.0,
            &theme,
            &widgets::Ctl::new("Effect mix", &widgets::fmt_percent),
        );
        if let Some(v) = e.commit() {
            core.send(mapping::effect_mix(insert, index, v as f32));
        }
        ui.label(widgets::caption(&theme, "Mix"));
    });
    ui.separator();
    // the window itself scrolls (see `windows::show_content`)
    effect_panel(ui, core, insert, index, &slot.effect);
}

fn effect_panel(ui: &mut Ui, core: &mut AppCore, insert: u32, index: usize, effect: &EffectSpec) {
    if let EffectSpec::Plugin { .. } = effect {
        let target = ph_commands::PluginTarget::Effect {
            insert,
            slot: index as u32,
        };
        crate::plugin_ui::plugin_panel(ui, core, target);
        return;
    }
    let theme = core.theme.clone();
    let Ok(desc) = ph_engine::catalog::effect_descriptor(effect) else {
        ui.label("This effect has no parameter descriptor.");
        return;
    };
    ui.add(
        egui::Label::new(
            RichText::new(&desc.name)
                .font(FontId::new(15.0, crate::fonts::cond_bold()))
                .color(theme.colors.text),
        )
        .truncate(),
    );
    if desc.id == "ph.fx.analyzer" {
        analyzer_view(ui, Id::new(("fx_analyzer", insert, index)), &theme);
    }
    let overrides = match effect {
        EffectSpec::Builtin { params, .. } => params.clone(),
        _ => Default::default(),
    };
    // Group equaliser-style prefixed params (b1_, b2_...) into rows of their own.
    let changes = widgets::param_grid_with(
        ui,
        Id::new(("fx", insert, index)),
        &desc,
        &|_| true,
        &|info| overrides.get(&info.id).copied().unwrap_or(info.default),
        &|info| {
            Some(ph_core::ParamPath::EffectParam {
                insert: InsertId(insert),
                slot: index as u32,
                param: info.id.clone(),
            })
        },
        &theme,
    );
    for (param, value) in changes {
        core.send(mapping::effect_param(insert, index, &param, value));
    }
    let _ = Edit::None;
}

/// The analyser effect's window: live spectrum, short-term / momentary loudness, phase
/// correlation and peak meters, from the lock-free feed of the most recently activated
/// analyser (`ph_engine::catalog::analyzer_feed`).
fn analyzer_view(ui: &mut Ui, id: Id, theme: &crate::theme::Theme) {
    const DB_LO: f32 = -100.0;
    const DB_HI: f32 = 0.0;
    let c = &theme.colors;
    let Some(feed) = ph_engine::catalog::analyzer_feed() else {
        ui.label(widgets::caption(
            theme,
            "No analyser is running yet: play the project to see its spectrum.",
        ));
        return;
    };
    let width = ui.available_width().clamp(240.0, 520.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 120.0), Sense::hover());
    let p = ui.painter();
    crate::paint::well(p, rect, 3, c.lcd, theme);
    let inner = rect.shrink(4.0);
    for db in [-20.0f32, -40.0, -60.0, -80.0] {
        let y = inner.bottom() - inner.height() * (db - DB_LO) / (DB_HI - DB_LO);
        p.line_segment(
            [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
            Stroke::new(1.0, c.grid_minor),
        );
    }
    let spec = feed.spectrum();
    let n = spec.len();
    let points: Vec<Pos2> = spec
        .iter()
        .enumerate()
        .map(|(k, db)| {
            let x = inner.left() + inner.width() * (k as f32 + 0.5) / n as f32;
            let t = ((db - DB_LO) / (DB_HI - DB_LO)).clamp(0.0, 1.0);
            Pos2::new(x, inner.bottom() - inner.height() * t)
        })
        .collect();
    p.add(egui::Shape::line(points, Stroke::new(1.5, c.accent)));
    for (hz, label) in [(100.0f32, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let x = inner.left()
            + inner.width() * (hz / ANALYZER_F_LO).log10()
                / (ANALYZER_F_HI / ANALYZER_F_LO).log10();
        widgets::text_in(
            ui,
            Rect::from_min_size(
                Pos2::new(x - 12.0, inner.bottom() - 12.0),
                Vec2::new(24.0, 12.0),
            ),
            label,
            9.0,
            c.dim,
        );
    }
    ui.horizontal(|ui| {
        let lufs = |v: f32| {
            if v <= SILENT_LUFS {
                "-inf".to_string()
            } else {
                format!("{v:.1}")
            }
        };
        widgets::lcd(
            ui,
            theme,
            &format!("S {} LUFS", lufs(feed.short_term_lufs())),
            12.0,
            110.0,
            "Short-term loudness (3 s)",
        );
        widgets::lcd(
            ui,
            theme,
            &format!("M {} LUFS", lufs(feed.momentary_lufs())),
            12.0,
            110.0,
            "Momentary loudness (400 ms)",
        );
        widgets::lcd(
            ui,
            theme,
            &format!("Corr {:+.2}", feed.correlation()),
            12.0,
            90.0,
            "Stereo phase correlation",
        );
        let (mrect, _) = ui.allocate_exact_size(Vec2::new(70.0, 14.0), Sense::hover());
        let peak = feed.peak();
        widgets::paint_meter(ui, id.with("meter"), mrect, peak, peak, theme);
    });
    // keep the display moving while the window is open
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

const ANALYZER_F_LO: f32 = 20.0;
const ANALYZER_F_HI: f32 = 20000.0;
const SILENT_LUFS: f32 = -119.0;
