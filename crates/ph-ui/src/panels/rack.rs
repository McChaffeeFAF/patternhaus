//! Channel rack and step sequencer.

use crate::browser_model::Dragged;
use crate::core::{AppCore, PanelAction, PanelId, RenameTarget};
use crate::mapping;
use crate::steps::{self, Step, StepLayout};
use crate::theme::{lerp, rgb};
use crate::widgets::{self, Edit, knob, toggle};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use ph_core::{ChannelId, InstrumentSpec, PatternId, STEP};
use std::collections::BTreeSet;

pub const ROW_H: f32 = 30.0;
const PALETTE: [u32; 12] = [
    0xe5484d, 0xf76b15, 0xffc53d, 0x46a758, 0x12a594, 0x3e63dd, 0x8e4ec6, 0xd6409f, 0x5f9ea0,
    0x9acd32, 0xdaa520, 0x8b8d98,
];

#[derive(Clone, Debug)]
struct Paint {
    pattern: PatternId,
    channel: ChannelId,
    value: bool,
    touched: BTreeSet<u32>,
}

#[derive(Clone, Debug)]
struct VelDrag {
    pattern: PatternId,
    channel: ChannelId,
    step: u32,
    value: f32,
}

#[derive(Default)]
pub struct RackState {
    /// Show per-step velocity bars instead of buttons.
    pub velocity_view: bool,
    paint: Option<Paint>,
    vel: Option<VelDrag>,
    reorder: Option<ChannelId>,
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    super::track_hover(ui, core, crate::keymap::Context::ChannelRack);
    let theme = core.theme.clone();
    let project = core.project.clone();
    let live = core.live();
    let Some(pattern) = core.sel.pattern else {
        ui.horizontal(|ui| {
            ui.label("No pattern yet.");
            if ui.button("New pattern").clicked() {
                core.send(mapping::pattern_add());
            }
        });
        return;
    };
    let n_steps = steps::step_count(&project, pattern);
    let mut pat_steps = n_steps;

    // ---- toolbar
    ui.horizontal(|ui| {
        ui.menu_button("+ Channel", |ui| {
            if ui.button("Sampler (empty)").clicked() {
                core.add_channel(
                    "Sampler",
                    &InstrumentSpec::Sampler {
                        sample: None,
                        root_key: 60,
                        params: Default::default(),
                    },
                );
                ui.close();
            }
            ui.menu_button("Haus Synth", |ui| {
                for cat in &core.browser.presets.clone() {
                    ui.menu_button(&cat.name, |ui| {
                        for p in &cat.children {
                            if ui.button(&p.name).clicked() {
                                core.add_channel_from(&p.name, &p.kind);
                                ui.close();
                            }
                        }
                    });
                }
            });
        });
        ui.label("Steps");
        let r = ui.add(egui::DragValue::new(&mut pat_steps).range(1..=256));
        if r.drag_stopped() || r.lost_focus() {
            if pat_steps != n_steps {
                core.send(mapping::pattern_length_steps(pattern, pat_steps));
            }
        }
        if toggle(
            ui,
            core.rack.velocity_view,
            "Velocity",
            theme.colors.accent,
            &theme,
        )
        .on_hover_text("Drag a lit step up or down to set its velocity")
        .clicked()
        {
            core.rack.velocity_view = !core.rack.velocity_view;
        }
        ui.label(
            egui::RichText::new(
                project
                    .patterns
                    .get(&pattern)
                    .map_or(String::new(), |p| format!("Pattern: {}", p.name)),
            )
            .color(theme.colors.text_dim),
        );
    });
    ui.separator();

    let channels: Vec<ph_core::Channel> = project.channels_in_order().cloned().collect();
    if channels.is_empty() {
        ui.add_space(12.0);
        ui.label("The rack is empty. Drag a sample or preset from the browser (F8) onto this panel, or use + Channel.");
    }

    // playhead step
    let play_step = live
        .status
        .playing
        .then(|| match live.status.mode {
            ph_engine::PlayMode::Pattern(p) if p == pattern => {
                let len = project.pattern_length(pattern).unwrap_or(STEP * 16).max(1);
                Some(steps::step_of_tick(live.status.position % len))
            }
            _ => None,
        })
        .flatten();

    let panel_rect = ui.available_rect_before_wrap();
    let mut removed: Option<ChannelId> = None;
    let mut reorder_to: Option<(ChannelId, usize)> = None;
    let layout = StepLayout {
        origin_x: 0.0,
        w: 22.0,
        gap: 6.0,
    };
    egui::ScrollArea::both()
        .auto_shrink(false)
        .id_salt("rack_scroll")
        .show_rows(ui, ROW_H, channels.len(), |ui, range| {
            for idx in range {
                let ch = &channels[idx];
                let row_rect = ui
                    .allocate_space(Vec2::new(layout.total_width(n_steps) + 340.0, ROW_H))
                    .1;
                let selected = core.sel.channel == Some(ch.id);
                let p = ui.painter();
                p.rect_filled(
                    row_rect.shrink2(Vec2::new(0.0, 1.0)),
                    CornerRadius::same(3),
                    if selected {
                        theme.colors.panel_alt
                    } else {
                        theme.colors.panel
                    },
                );
                let mut x = row_rect.left() + 4.0;
                let mid = row_rect.center().y;

                // grip (reorder)
                let grip = Rect::from_center_size(Pos2::new(x + 6.0, mid), Vec2::new(12.0, 22.0));
                let gr = ui.interact(grip, ui.id().with(("grip", ch.id)), Sense::drag());
                for dy in [-5.0, 0.0, 5.0] {
                    p.circle_filled(Pos2::new(x + 6.0, mid + dy), 1.3, theme.colors.text_dim);
                }
                if gr.drag_started() {
                    core.rack.reorder = Some(ch.id);
                }
                if core.rack.reorder == Some(ch.id) {
                    if let Some(pp) = ui.input(|i| i.pointer.latest_pos()) {
                        let target = (((pp.y - panel_rect.top()) / ROW_H).max(0.0) as usize)
                            .min(channels.len() - 1);
                        let y = row_rect.top() + (target as f32 - idx as f32) * ROW_H;
                        ui.painter().line_segment(
                            [
                                Pos2::new(row_rect.left(), y),
                                Pos2::new(row_rect.left() + 300.0, y),
                            ],
                            Stroke::new(2.0, theme.colors.accent),
                        );
                        if gr.drag_stopped() || ui.input(|i| i.pointer.any_released()) {
                            reorder_to = Some((ch.id, target));
                            core.rack.reorder = None;
                        }
                    }
                }
                x += 16.0;

                // colour swatch + name
                let sw = Rect::from_min_size(Pos2::new(x, mid - 9.0), Vec2::new(8.0, 18.0));
                p.rect_filled(sw, CornerRadius::same(2), rgb(ch.color));
                x += 12.0;
                let name_rect = Rect::from_min_size(
                    Pos2::new(x, row_rect.top() + 2.0),
                    Vec2::new(104.0, ROW_H - 4.0),
                );
                let nr = ui.interact(name_rect, ui.id().with(("name", ch.id)), Sense::click());
                ui.painter().text(
                    Pos2::new(name_rect.left() + 2.0, mid),
                    Align2::LEFT_CENTER,
                    &ch.name,
                    FontId::proportional(13.0),
                    if ch.muted {
                        theme.colors.text_dim
                    } else {
                        theme.colors.text
                    },
                );
                if nr.clicked() {
                    core.sel.channel = Some(ch.id);
                    core.fix_selection();
                }
                if nr.double_clicked() {
                    core.panel_requests
                        .push((PanelId::PianoRoll, PanelAction::Open));
                }
                channel_menu(&nr, core, ch, &mut removed);
                // agent touch badge
                if let Some((who, at)) = core.touches.get(&ph_core::ObjectPath::Channel(ch.id)) {
                    let k = (1.0 - ((core.now - at) / crate::piano_roll::FLASH_SECS) as f32)
                        .clamp(0.0, 1.0);
                    if k > 0.0 {
                        ui.painter().rect_stroke(
                            row_rect.shrink2(Vec2::new(0.0, 1.0)),
                            CornerRadius::same(3),
                            Stroke::new(2.0, theme.colors.agent.gamma_multiply(k)),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            Pos2::new(name_rect.right() - 2.0, row_rect.top() + 1.0),
                            Align2::RIGHT_TOP,
                            who.trim_start_matches("agent:"),
                            FontId::proportional(9.0),
                            theme.colors.agent,
                        );
                    }
                }
                if core
                    .agent_select
                    .contains(&ph_core::ObjectPath::Channel(ch.id))
                {
                    ui.painter().rect_stroke(
                        row_rect,
                        CornerRadius::same(3),
                        Stroke::new(2.0, theme.colors.agent),
                        StrokeKind::Inside,
                    );
                }
                x += 108.0;

                // mute / solo
                let solo = core.solo_channels.contains(&ch.id);
                let mrect = Rect::from_min_size(Pos2::new(x, mid - 10.0), Vec2::new(22.0, 20.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(mrect));
                if toggle(&mut child, ch.muted, "M", theme.colors.warn, &theme).clicked() {
                    core.send(mapping::channel_mute(ch.id, !ch.muted));
                }
                x += 24.0;
                let srect = Rect::from_min_size(Pos2::new(x, mid - 10.0), Vec2::new(22.0, 20.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(srect));
                if toggle(&mut child, solo, "S", theme.colors.ok, &theme).clicked() {
                    if solo {
                        core.solo_channels.remove(&ch.id);
                    } else {
                        core.solo_channels.insert(ch.id);
                    }
                    core.send(mapping::channel_solo(ch.id, !solo));
                }
                x += 26.0;

                // volume + pan knobs
                let krect = Rect::from_min_size(Pos2::new(x, mid - 13.0), Vec2::splat(26.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(krect));
                let (_, e) = knob(
                    &mut child,
                    ui.id().with(("vol", ch.id)),
                    ch.volume as f64,
                    0.0,
                    1.0,
                    0.78,
                    26.0,
                    &theme,
                );
                if let Some(v) = e.commit() {
                    core.send(mapping::channel_volume(ch.id, v as f32));
                }
                x += 30.0;
                let krect = Rect::from_min_size(Pos2::new(x, mid - 13.0), Vec2::splat(26.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(krect));
                let (_, e) = knob(
                    &mut child,
                    ui.id().with(("pan", ch.id)),
                    ch.pan as f64,
                    -1.0,
                    1.0,
                    0.0,
                    26.0,
                    &theme,
                );
                if let Some(v) = e.commit() {
                    core.send(mapping::channel_pan(ch.id, v as f32));
                }
                x += 32.0;

                // mixer insert
                let irect = Rect::from_min_size(Pos2::new(x, mid - 10.0), Vec2::new(40.0, 20.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(irect));
                let mut ins = ch.insert.0;
                let r = child.add(egui::DragValue::new(&mut ins).range(0..=32));
                r.clone().on_hover_text("Mixer insert (0 = master)");
                if (r.drag_stopped() || r.lost_focus()) && ins != ch.insert.0 {
                    core.send(mapping::channel_insert(ch.id, ins));
                }
                x += 48.0;

                // steps
                let steps_rect = Rect::from_min_size(
                    Pos2::new(x, row_rect.top() + 3.0),
                    Vec2::new(layout.total_width(n_steps), ROW_H - 6.0),
                );
                let lay = StepLayout {
                    origin_x: steps_rect.left(),
                    ..layout
                };
                step_row(ui, core, ch, pattern, n_steps, steps_rect, lay, play_step);
            }
        });

    if let Some(c) = removed {
        core.send(mapping::channel_remove(c));
    }
    if let Some((c, idx)) = reorder_to {
        core.send(mapping::channel_move(c, idx));
    }

    // dropping a browser entry on the rack makes a channel
    let released = ui.input(|i| i.pointer.any_released());
    if released {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if panel_rect.contains(pos) {
                if let Some(d) = egui::DragAndDrop::take_payload::<Dragged>(ui.ctx()) {
                    core.add_channel_from(&d.name, &d.kind);
                }
            }
        }
    }
    if egui::DragAndDrop::has_payload_of_type::<Dragged>(ui.ctx()) {
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            if panel_rect.contains(pos) {
                ui.painter().rect_stroke(
                    panel_rect,
                    CornerRadius::same(4),
                    Stroke::new(2.0, theme.colors.accent),
                    StrokeKind::Inside,
                );
            }
        }
    }
}

fn channel_menu(
    resp: &egui::Response,
    core: &mut AppCore,
    ch: &ph_core::Channel,
    removed: &mut Option<ChannelId>,
) {
    resp.context_menu(|ui| {
        core.sel.channel = Some(ch.id);
        if ui.button("Rename…").clicked() {
            core.open_rename(RenameTarget::Channel(ch.id));
            ui.close();
        }
        ui.menu_button("Colour", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(150.0);
                for c in PALETTE {
                    let (r, p) = ui.allocate_painter(Vec2::splat(22.0), Sense::click());
                    p.rect_filled(r.rect.shrink(1.0), CornerRadius::same(4), rgb(c));
                    if r.clicked() {
                        core.send(mapping::channel_color(ch.id, c));
                        ui.close();
                    }
                }
            });
        });
        if ui.button("Clone").clicked() {
            core.send(mapping::channel_clone(ch.id));
            ui.close();
        }
        ui.menu_button("Route to insert", |ui| {
            ui.set_max_height(300.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for i in 0..=32u32 {
                    let name = core
                        .project
                        .mixer
                        .get(i as usize)
                        .map_or(String::new(), |m| m.name.clone());
                    if ui
                        .selectable_label(ch.insert.0 == i, format!("{i}  {name}"))
                        .clicked()
                    {
                        core.send(mapping::channel_insert(ch.id, i));
                        ui.close();
                    }
                }
            });
        });
        if ui.button("Open in piano roll").clicked() {
            core.panel_requests
                .push((PanelId::PianoRoll, PanelAction::Open));
            ui.close();
        }
        if ui.button("Instrument settings").clicked() {
            core.panel_requests
                .push((PanelId::Inspector, PanelAction::Open));
            ui.close();
        }
        ui.separator();
        if ui
            .button(egui::RichText::new("Delete").color(Color32::from_rgb(255, 107, 107)))
            .clicked()
        {
            *removed = Some(ch.id);
            ui.close();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn step_row(
    ui: &mut Ui,
    core: &mut AppCore,
    ch: &ph_core::Channel,
    pattern: PatternId,
    n_steps: u32,
    rect: Rect,
    lay: StepLayout,
    play_step: Option<u32>,
) {
    let theme = core.theme.clone();
    let row = steps::row(&core.project, pattern, ch.id, n_steps);
    let resp = ui.interact(
        rect,
        ui.id().with(("steps", ch.id)),
        Sense::click_and_drag(),
    );
    let colour = rgb(ch.color);

    // input
    let (pos, primary_pressed, secondary_pressed, released, down) = ui.input(|i| {
        (
            i.pointer.latest_pos(),
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.pointer.any_released(),
            i.pointer.any_down(),
        )
    });
    let over = |p: Pos2| {
        lay.step_at(p.x, n_steps)
            .filter(|_| p.y >= rect.top() && p.y <= rect.bottom())
    };
    if let Some(p) = pos {
        if resp.hovered()
            && (primary_pressed || secondary_pressed)
            && core.rack.paint.is_none()
            && core.rack.vel.is_none()
        {
            if let Some(s) = over(p) {
                core.sel.channel = Some(ch.id);
                core.fix_selection();
                if core.rack.velocity_view && primary_pressed && row[s as usize].is_on() {
                    core.rack.vel = Some(VelDrag {
                        pattern,
                        channel: ch.id,
                        step: s,
                        value: ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0),
                    });
                } else {
                    let value = if secondary_pressed {
                        false
                    } else {
                        !row[s as usize].is_on()
                    };
                    core.rack.paint = Some(Paint {
                        pattern,
                        channel: ch.id,
                        value,
                        touched: [s].into(),
                    });
                    // let the user hear the pad (not for turning steps off)
                    if value {
                        core.preview(ch.id, default_key(ch), true);
                    }
                }
            }
        }
        if down {
            if let Some(pt) = &mut core.rack.paint {
                if pt.channel == ch.id {
                    if let Some(s) = over(Pos2::new(p.x, rect.center().y)) {
                        pt.touched.insert(s);
                    }
                }
            }
            if let Some(v) = &mut core.rack.vel {
                if v.channel == ch.id {
                    v.value = ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0);
                }
            }
        }
    }
    if released {
        if core.rack.paint.as_ref().is_some_and(|p| p.channel == ch.id) {
            let pt = core.rack.paint.take().unwrap();
            core.preview(ch.id, default_key(ch), false);
            if pt.touched.len() == 1 {
                let s = *pt.touched.iter().next().unwrap();
                // a single press toggles; right-click only ever clears
                if row[s as usize].is_on() != pt.value {
                    core.send(mapping::step_toggle(pt.pattern, pt.channel, s));
                }
            } else {
                core.send(mapping::step_set_row(
                    pt.pattern,
                    pt.channel,
                    &steps::painted(&row, &pt.touched, pt.value),
                ));
            }
        }
        if core.rack.vel.as_ref().is_some_and(|v| v.channel == ch.id) {
            let v = core.rack.vel.take().unwrap();
            core.send(mapping::step_velocity(
                v.pattern,
                v.channel,
                v.step,
                v.value.max(0.01),
            ));
        }
    }

    // draw
    let pt = core.rack.paint.as_ref().filter(|p| p.channel == ch.id);
    let vel = core.rack.vel.as_ref().filter(|v| v.channel == ch.id);
    let painter = ui.painter();
    for (i, s) in row.iter().enumerate() {
        let i = i as u32;
        let x = lay.x_of(i);
        let r = Rect::from_min_size(
            Pos2::new(x, rect.top()),
            Vec2::new(lay.w - 2.0, rect.height()),
        );
        let group_dark = (i / 4) % 2 == 0;
        let base = if group_dark {
            theme.colors.key_white
        } else {
            theme.colors.row_alt
        };
        let mut on = s.is_on();
        if let Some(p) = pt {
            if p.touched.contains(&i) {
                on = p.value;
            }
        }
        painter.rect_filled(r, CornerRadius::same(3), base);
        if core.rack.velocity_view {
            if let Step::On { velocity, .. } = s {
                let v = match vel {
                    Some(d) if d.step == i => d.value,
                    _ => *velocity,
                };
                let h = r.height() * v;
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(r.left(), r.bottom() - h), r.right_bottom()),
                    CornerRadius::same(3),
                    lerp(colour.gamma_multiply(0.5), colour, v),
                );
            }
        } else if on {
            let v = if let Step::On { velocity, .. } = s {
                *velocity
            } else {
                0.78
            };
            painter.rect_filled(
                r,
                CornerRadius::same(3),
                lerp(colour.gamma_multiply(0.55), colour, v),
            );
        } else if matches!(s, Step::Other) {
            painter.circle_filled(r.center(), 2.5, theme.colors.text_dim);
        }
        if play_step == Some(i) {
            painter.rect_stroke(
                r,
                CornerRadius::same(3),
                Stroke::new(1.5, theme.colors.playhead),
                StrokeKind::Inside,
            );
        }
    }
    let _ = widgets::section_label;
    let _ = Edit::None;
}

fn default_key(ch: &ph_core::Channel) -> u8 {
    match &ch.instrument {
        InstrumentSpec::Sampler { root_key, .. } => *root_key,
        _ => 60,
    }
}
