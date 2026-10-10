//! The egui side of the floating workspace: draws each window of the [`Workspace`] model as an
//! `egui::Area` with our own flat title bar, and reports drags and clicks back into the model.
//!
//! Nothing here keeps window state of its own. Positions, sizes, stacking and visibility all
//! live in the model; the only thing remembered between frames is where a drag started.

use crate::core::{AppCore, PanelId};
use crate::panels;
use crate::workspace::{Edges, Rect as WRect, TITLE_H, WindowId, Workspace};
use egui::{
    Align, Align2, CornerRadius, CursorIcon, FontId, Id, Layout, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2,
};
use ph_core::InstrumentSpec;

/// Transparent grab margin around a window, so its edges can be dragged from just outside.
const MARGIN: f32 = 4.0;
/// Width of the inner part of the resize band.
const BAND_IN: f32 = 3.0;
const CORNER: f32 = 12.0;
const BUTTON: f32 = 20.0;

/// The title of a window; object windows are named after their object.
pub fn title_of(core: &AppCore, id: WindowId) -> String {
    match id {
        WindowId::Playlist => PanelId::Playlist.title().into(),
        WindowId::ChannelRack => PanelId::ChannelRack.title().into(),
        WindowId::PianoRoll => PanelId::PianoRoll.title().into(),
        WindowId::Mixer => PanelId::Mixer.title().into(),
        WindowId::History => PanelId::History.title().into(),
        WindowId::Agents => PanelId::Agents.title().into(),
        WindowId::Instrument(c) => match core.project.channels.get(&c) {
            Some(ch) => format!(
                "{} - {}",
                ch.name,
                match ch.instrument {
                    InstrumentSpec::Sampler { .. } => "Sampler",
                    InstrumentSpec::Synth { .. } => "Haus Synth",
                    InstrumentSpec::MultiSampler { .. } => "Haus Sampler",
                    InstrumentSpec::Plugin { .. } => "Plugin",
                }
            ),
            None => "Instrument".into(),
        },
        WindowId::Effect(i, slot) => {
            let insert = core.project.mixer.get(i.0 as usize);
            let fx = insert
                .and_then(|m| m.effects.get(slot as usize))
                .map(|s| match &s.effect {
                    ph_core::EffectSpec::Builtin { name, .. } => name.clone(),
                    ph_core::EffectSpec::Plugin { plugin_id, .. } => plugin_id.clone(),
                })
                .unwrap_or_else(|| "Effect".into());
            match insert {
                Some(m) if !m.name.is_empty() => format!("{fx} - {}", m.name),
                _ => format!("{fx} - insert {}", i.0),
            }
        }
    }
}

/// The contents of a window; the panels are unchanged from the docked shell.
pub fn show_content(ui: &mut Ui, core: &mut AppCore, id: WindowId) {
    match id {
        WindowId::Playlist => panels::playlist::show(ui, core),
        WindowId::ChannelRack => panels::rack::show(ui, core),
        WindowId::PianoRoll => panels::piano_roll::show(ui, core),
        WindowId::Mixer => panels::mixer::show(ui, core),
        WindowId::History => panels::history::show(ui, core),
        WindowId::Agents => panels::agents::show(ui, core),
        WindowId::Instrument(c) => panels::inspector::show(ui, core, c),
        WindowId::Effect(i, slot) => panels::mixer::effect_window(ui, core, i.0, slot as usize),
    }
}

/// What one window reported this frame.
#[derive(Default)]
struct Events {
    pressed: bool,
    close: bool,
    maximize: bool,
    detach: bool,
    move_to: Option<(f32, f32)>,
    resize: Option<(Edges, WRect)>,
}

#[derive(Clone, Copy)]
struct Grab {
    pointer: Pos2,
    rect: WRect,
}

fn to_screen(origin: Pos2, r: WRect) -> Rect {
    Rect::from_min_size(origin + Vec2::new(r.x, r.y), Vec2::new(r.w, r.h))
}

/// Draws every visible, attached window into the workspace area of `ui` and applies what the
/// user did to the model.
pub fn show_workspace(ui: &mut Ui, ws: &mut Workspace, core: &mut AppCore) -> Rect {
    let work = ui.max_rect();
    ws.set_area(WRect::new(0.0, 0.0, work.width(), work.height()));
    let ctx = ui.ctx().clone();
    let front = ws.front();
    let mut layers = Vec::new();
    let mut pressed_top = None;
    let mut actions: Vec<(WindowId, Events)> = Vec::new();
    for id in ws.visible_in_z_order() {
        let Some(state) = ws.get(id).copied() else {
            continue;
        };
        if state.detached {
            continue;
        }
        let eff = ws.effective_rect(id).unwrap();
        let (ev, layer) = show_window(
            &ctx,
            work,
            to_screen(work.min, eff),
            id,
            eff,
            state.maximized,
            front == Some(id),
            core,
        );
        if ev.pressed {
            pressed_top = Some(id);
        }
        layers.push((id, layer));
        actions.push((id, ev));
    }
    for (id, ev) in actions {
        if ev.close {
            ws.hide(id);
            continue;
        }
        if ev.maximize {
            ws.maximize_toggle(id);
        }
        if ev.detach {
            ws.set_detached(id, true);
        }
        if let Some((x, y)) = ev.move_to {
            ws.drag_move(id, x, y);
        }
        if let Some((edges, want)) = ev.resize {
            ws.drag_resize(id, edges, want);
        }
    }
    if let Some(id) = pressed_top {
        ws.focus(id);
    }
    // The model's stacking order is the truth. Every change to it raises exactly one window
    // (show, focus, toggle, maximize), so mirroring a change of the front window into egui's
    // layer order keeps the two in step. (egui itself orders a batch of raised layers by their
    // old order, so reordering several at once cannot be done.)
    let key = Id::new("ph_last_front");
    let last: Option<WindowId> = ctx.data(|d| d.get_temp(key)).flatten();
    let now = ws.front();
    if now != last
        && let Some(id) = now
        && let Some((_, l)) = layers.iter().find(|(i, _)| *i == id)
    {
        ctx.move_to_top(*l);
    }
    ctx.data_mut(|d| d.insert_temp(key, now));
    work
}

#[allow(clippy::too_many_arguments)]
fn show_window(
    ctx: &egui::Context,
    work: Rect,
    win: Rect,
    id: WindowId,
    model_rect: WRect,
    maximized: bool,
    is_front: bool,
    core: &mut AppCore,
) -> (Events, egui::LayerId) {
    let theme = core.theme.clone();
    let title = title_of(core, id);
    let mut ev = Events::default();
    let area_id = Id::new(("ph_window", id));
    let out = egui::Area::new(area_id)
        .order(Order::Middle)
        .fixed_pos(win.min - Vec2::splat(MARGIN))
        .movable(false)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(work));
            ui.allocate_rect(win.expand(MARGIN), Sense::hover());
            let clip = ui.clip_rect();
            let p = ui.painter().clone();
            let round = if maximized {
                CornerRadius::ZERO
            } else {
                CornerRadius::same(6)
            };
            // body, border, title bar
            p.rect_filled(win, round, theme.colors.panel);
            let title_rect = Rect::from_min_size(win.min, Vec2::new(win.width(), TITLE_H));
            p.rect_filled(
                title_rect,
                CornerRadius {
                    nw: round.nw,
                    ne: round.ne,
                    sw: 0,
                    se: 0,
                },
                theme.colors.panel_alt,
            );
            if is_front {
                p.line_segment(
                    [title_rect.left_bottom(), title_rect.right_bottom()],
                    Stroke::new(2.0, theme.colors.accent),
                );
            }

            // contents
            let content = Rect::from_min_max(
                win.min + Vec2::new(1.0, TITLE_H + 1.0),
                win.max - Vec2::new(1.0, 1.0),
            );
            // form-like windows get a margin; the editors draw edge to edge
            let content = match id {
                WindowId::Instrument(_)
                | WindowId::Effect(..)
                | WindowId::History
                | WindowId::Agents => content.shrink(8.0),
                _ => content,
            };
            if content.width() > 4.0 && content.height() > 4.0 {
                let mut child = ui.new_child(
                    UiBuilder::new()
                        .max_rect(content)
                        .id_salt(("ph_content", id))
                        .layout(Layout::top_down(Align::Min)),
                );
                child.set_clip_rect(content.intersect(clip));
                show_content(&mut child, core, id);
            }
            p.rect_stroke(
                win,
                round,
                Stroke::new(
                    1.0,
                    if is_front {
                        theme.colors.accent_dim
                    } else {
                        theme.colors.border
                    },
                ),
                StrokeKind::Inside,
            );

            // title: drag to move, double-click to maximize
            let buttons_w = BUTTON * 3.0 + 8.0;
            let drag_rect = Rect::from_min_max(
                title_rect.min,
                Pos2::new(title_rect.max.x - buttons_w, title_rect.max.y),
            );
            let tr = ui.interact(drag_rect, area_id.with("title"), Sense::click_and_drag());
            if tr.drag_started()
                && let Some(pt) = ui.input(|i| i.pointer.press_origin())
            {
                ui.data_mut(|d| {
                    d.insert_temp(
                        area_id.with("grab"),
                        Grab {
                            pointer: pt,
                            rect: model_rect,
                        },
                    )
                });
            }
            if tr.dragged()
                && !maximized
                && let (Some(pt), Some(g)) = (
                    tr.interact_pointer_pos(),
                    ui.data(|d| d.get_temp::<Grab>(area_id.with("grab"))),
                )
            {
                let d = pt - g.pointer;
                ev.move_to = Some((g.rect.x + d.x, g.rect.y + d.y));
            }
            if tr.double_clicked() {
                ev.maximize = true;
            }
            p.with_clip_rect(drag_rect.intersect(clip)).text(
                Pos2::new(title_rect.left() + 10.0, title_rect.center().y),
                Align2::LEFT_CENTER,
                &title,
                FontId::proportional(12.5),
                if is_front {
                    theme.colors.text
                } else {
                    theme.colors.text_dim
                },
            );

            // buttons, right to left: close, maximize, detach
            let btn = |ui: &mut Ui, slot: f32, what: &str| {
                let r = Rect::from_min_size(
                    Pos2::new(
                        title_rect.right() - 4.0 - BUTTON * (slot + 1.0),
                        title_rect.top() + (TITLE_H - BUTTON) / 2.0,
                    ),
                    Vec2::splat(BUTTON),
                );
                let resp = ui.interact(r, area_id.with(what), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(4),
                        if what == "close" {
                            theme.colors.error.gamma_multiply(0.7)
                        } else {
                            theme.colors.widget_hover
                        },
                    );
                }
                (resp, r.shrink(6.0))
            };
            let stroke = Stroke::new(1.3, theme.colors.text_dim);
            let (c, g) = btn(ui, 0.0, "close");
            p.line_segment([g.left_top(), g.right_bottom()], stroke);
            p.line_segment([g.left_bottom(), g.right_top()], stroke);
            ev.close |= c.on_hover_text("Close").clicked();
            let (m, g) = btn(ui, 1.0, "maximize");
            if maximized {
                p.rect_stroke(
                    g.shrink2(Vec2::new(0.0, 1.5)),
                    CornerRadius::same(1),
                    stroke,
                    StrokeKind::Inside,
                );
                p.line_segment([g.left_top() + Vec2::new(2.5, 0.0), g.right_top()], stroke);
            } else {
                p.rect_stroke(g, CornerRadius::same(1), stroke, StrokeKind::Inside);
            }
            ev.maximize |= m
                .on_hover_text(if maximized { "Restore" } else { "Maximize" })
                .clicked();
            let (d, g) = btn(ui, 2.0, "detach");
            p.rect_stroke(
                Rect::from_min_size(g.min + Vec2::new(0.0, 3.0), g.size() - Vec2::splat(3.0)),
                CornerRadius::same(1),
                stroke,
                StrokeKind::Inside,
            );
            p.line_segment([g.center() + Vec2::new(0.0, -1.0), g.right_top()], stroke);
            p.line_segment(
                [g.right_top(), g.right_top() + Vec2::new(-3.5, 0.0)],
                stroke,
            );
            p.line_segment([g.right_top(), g.right_top() + Vec2::new(0.0, 3.5)], stroke);
            ev.detach |= d.on_hover_text("Open in its own window").clicked();

            // resize bands on the edges and corners (not while maximized)
            if !maximized {
                resize_handles(ui, area_id, win, model_rect, &mut ev);
            }
        });
    // a press anywhere inside the window raises it, but only when it is the layer under the
    // pointer (a window in front of this one swallows the click)
    let layer = out.response.layer_id;
    if ctx.input(|i| i.pointer.any_pressed())
        && ctx.rect_contains_pointer(layer, win.intersect(work))
    {
        ev.pressed = true;
    }
    (ev, layer)
}

fn resize_handles(ui: &mut Ui, area_id: Id, win: Rect, model_rect: WRect, ev: &mut Events) {
    let o = MARGIN;
    let i = BAND_IN;
    let band = |a: Pos2, b: Pos2| Rect::from_two_pos(a, b);
    let handles: [(&str, Rect, Edges, CursorIcon); 8] = [
        (
            "l",
            band(
                Pos2::new(win.left() - o, win.top() + CORNER),
                Pos2::new(win.left() + i, win.bottom() - CORNER),
            ),
            Edges::new(true, false, false, false),
            CursorIcon::ResizeHorizontal,
        ),
        (
            "r",
            band(
                Pos2::new(win.right() - i, win.top() + CORNER),
                Pos2::new(win.right() + o, win.bottom() - CORNER),
            ),
            Edges::new(false, true, false, false),
            CursorIcon::ResizeHorizontal,
        ),
        (
            "t",
            band(
                Pos2::new(win.left() + CORNER, win.top() - o),
                Pos2::new(win.right() - CORNER, win.top() + i),
            ),
            Edges::new(false, false, true, false),
            CursorIcon::ResizeVertical,
        ),
        (
            "b",
            band(
                Pos2::new(win.left() + CORNER, win.bottom() - i),
                Pos2::new(win.right() - CORNER, win.bottom() + o),
            ),
            Edges::new(false, false, false, true),
            CursorIcon::ResizeVertical,
        ),
        (
            "tl",
            band(
                Pos2::new(win.left() - o, win.top() - o),
                Pos2::new(win.left() + CORNER, win.top() + CORNER),
            ),
            Edges::new(true, false, true, false),
            CursorIcon::ResizeNwSe,
        ),
        (
            "tr",
            band(
                Pos2::new(win.right() - CORNER, win.top() - o),
                Pos2::new(win.right() + o, win.top() + CORNER),
            ),
            Edges::new(false, true, true, false),
            CursorIcon::ResizeNeSw,
        ),
        (
            "bl",
            band(
                Pos2::new(win.left() - o, win.bottom() - CORNER),
                Pos2::new(win.left() + CORNER, win.bottom() + o),
            ),
            Edges::new(true, false, false, true),
            CursorIcon::ResizeNeSw,
        ),
        (
            "br",
            band(
                Pos2::new(win.right() - CORNER, win.bottom() - CORNER),
                Pos2::new(win.right() + o, win.bottom() + o),
            ),
            Edges::new(false, true, false, true),
            CursorIcon::ResizeNwSe,
        ),
    ];
    for (name, rect, edges, icon) in handles {
        let hid = area_id.with(("resize", name));
        let r = ui.interact(rect, hid, Sense::drag()).on_hover_cursor(icon);
        if r.drag_started()
            && let Some(pt) = ui.input(|i| i.pointer.press_origin())
        {
            ui.data_mut(|d| {
                d.insert_temp(
                    hid,
                    Grab {
                        pointer: pt,
                        rect: model_rect,
                    },
                )
            });
        }
        if r.dragged()
            && let (Some(pt), Some(g)) = (
                r.interact_pointer_pos(),
                ui.data(|d| d.get_temp::<Grab>(hid)),
            )
        {
            let d = pt - g.pointer;
            let mut want = g.rect;
            if edges.left {
                want.x += d.x;
                want.w -= d.x;
            }
            if edges.right {
                want.w += d.x;
            }
            if edges.top {
                want.y += d.y;
                want.h -= d.y;
            }
            if edges.bottom {
                want.h += d.y;
            }
            ev.resize = Some((edges, want));
        }
    }
}
