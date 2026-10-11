//! The egui side of the floating workspace: draws each window of the [`Workspace`] model as an
//! `egui::Area` with our own flat title bar, and reports drags and clicks back into the model.
//!
//! Nothing here keeps window state of its own. Positions, sizes, stacking and visibility all
//! live in the model; the only thing remembered between frames is where a drag started.

use crate::core::{AppCore, PanelId, ScrollInfo};
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::paint;
use crate::panels;
use crate::theme::{TRACK_LABEL, caps_sized};
use crate::workspace::{Edges, Rect as WRect, TITLE_H, WindowId, Workspace};
use egui::{
    Align, CornerRadius, CursorIcon, FontId, Id, Layout, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2,
};
use ph_core::InstrumentSpec;

/// Transparent grab margin around a window, so its edges can be dragged from just outside.
const MARGIN: f32 = 4.0;
/// Width of the inner part of the resize band.
const BAND_IN: f32 = 3.0;
const CORNER: f32 = 12.0;
const BUTTON: f32 = 20.0;
/// Title-bar buttons: options, detach, maximize, close.
const BUTTONS: usize = 4;

/// The name shown for an effect: its descriptor's name ("EQ"), else its raw id.
pub fn effect_name(effect: &ph_core::EffectSpec) -> String {
    let raw = match effect {
        ph_core::EffectSpec::Builtin { name, .. } => name.as_str(),
        ph_core::EffectSpec::Plugin { plugin_id, .. } => plugin_id.as_str(),
    };
    match ph_engine::catalog::effect_descriptor(effect) {
        Ok(d) if !d.name.is_empty() => d.name,
        _ if !raw.is_empty() => raw.to_string(),
        _ => "Effect".to_string(),
    }
}

/// The insert's name, or "Insert N" (or "Master") when it has none.
pub fn insert_title(core: &AppCore, i: u32) -> String {
    match core.project.mixer.get(i as usize) {
        Some(m) if !m.name.trim().is_empty() => m.name.clone(),
        _ if i == 0 => "Master".into(),
        _ => format!("Insert {i}"),
    }
}

/// The title of a window; object windows are named after their object. Never empty.
pub fn title_of(core: &AppCore, id: WindowId) -> String {
    let t = match id {
        WindowId::Playlist => PanelId::Playlist.title().into(),
        WindowId::ChannelRack => PanelId::ChannelRack.title().into(),
        WindowId::PianoRoll => PanelId::PianoRoll.title().into(),
        WindowId::Mixer => PanelId::Mixer.title().into(),
        WindowId::History => PanelId::History.title().into(),
        WindowId::Agents => PanelId::Agents.title().into(),
        WindowId::Instrument(c) => match core.project.channels.get(&c) {
            Some(ch) => format!(
                "{} - {}",
                if ch.name.trim().is_empty() {
                    format!("Channel {}", c.0)
                } else {
                    ch.name.clone()
                },
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
            let fx = core
                .project
                .mixer
                .get(i.0 as usize)
                .and_then(|m| m.effects.get(slot as usize))
                .map(|s| effect_name(&s.effect))
                .unwrap_or_else(|| "Effect".into());
            format!("{} - {fx}", insert_title(core, i.0))
        }
    };
    if t.trim().is_empty() {
        "Window".into()
    } else {
        t
    }
}

/// The contents of a window; the panels are unchanged from the docked shell. Windows whose
/// content can outgrow them sit in a scroll area, so nothing is cut off. The piano roll, the
/// playlist and the channel rack (its step grid) pan and zoom on their own.
pub fn show_content(ui: &mut Ui, core: &mut AppCore, id: WindowId) {
    // (scroll sideways too, scroll area id) - instrument and effect windows wrap their knob
    // rows instead of scrolling sideways
    let scroll_x = match id {
        WindowId::Instrument(_) | WindowId::Effect(..) => Some(false),
        WindowId::Mixer | WindowId::History | WindowId::Agents => Some(true),
        WindowId::Playlist | WindowId::ChannelRack | WindowId::PianoRoll => None,
    };
    let Some(x) = scroll_x else {
        show_raw(ui, core, id);
        return;
    };
    core.view_size = ui.available_size();
    let out = egui::ScrollArea::new([x, true])
        .auto_shrink(false)
        .id_salt(("ph_scroll", id))
        .show(ui, |ui| show_raw(ui, core, id));
    core.scrolls.insert(
        id,
        ScrollInfo {
            offset: out.state.offset,
            content: out.content_size,
            viewport: out.inner_rect.size(),
        },
    );
}

fn show_raw(ui: &mut Ui, core: &mut AppCore, id: WindowId) {
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
    reset: bool,
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
        // keep the title readable when a window in front covers its usual place
        let title_w = title_of(core, id).chars().count() as f32 * 8.5;
        let title_x = ws.title_start(id, title_w, BUTTON * BUTTONS as f32 + 10.0);
        let (ev, layer) = show_window(
            &ctx,
            work,
            to_screen(work.min, eff),
            id,
            eff,
            state.maximized,
            front == Some(id),
            title_x,
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
        if ev.reset {
            ws.reset_window(id);
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
    title_x: f32,
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
            // body, border, title bar. A focused window has an accent edge and a soft outer
            // glow that fades in over 120 ms; background windows have neither.
            let c = &theme.colors;
            let focus = crate::motion::ease_bool(ctx, area_id.with("focus"), is_front, 0.12);
            let round = if maximized {
                CornerRadius::ZERO
            } else {
                CornerRadius::same(theme.metrics.radius_lg as u8)
            };
            if focus > 0.01 && !maximized {
                paint::glow_rect(&p, win, round.nw, c.accent, focus);
            }
            p.rect_filled(win, round, c.bg2);
            let title_rect = Rect::from_min_size(win.min, Vec2::new(win.width(), TITLE_H));
            p.rect_filled(
                title_rect,
                CornerRadius {
                    nw: round.nw,
                    ne: round.ne,
                    sw: 0,
                    se: 0,
                },
                paint::over(c.bg1, c.bg3, focus),
            );
            if focus > 0.01 {
                p.line_segment(
                    [title_rect.left_bottom(), title_rect.right_bottom()],
                    Stroke::new(2.0, c.accent.gamma_multiply(focus)),
                );
            } else {
                p.line_segment(
                    [title_rect.left_bottom(), title_rect.right_bottom()],
                    Stroke::new(1.0, c.line_soft),
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
                Stroke::new(1.0, paint::over(c.line, c.accent_dim, focus)),
                StrokeKind::Inside,
            );

            // title: drag to move, double-click to maximize
            let buttons_w = BUTTON * BUTTONS as f32 + 10.0;
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
            let tr = tr.hint(
                &title,
                "",
                "drag to move, double-click to maximize, right-click for options",
            );
            // the fixed windows are named in capitals; object windows carry their (typed) name
            let rich = if id.is_object() {
                egui::RichText::new(&title).font(FontId::new(
                    theme.metrics.font_title,
                    crate::fonts::body_medium(),
                ))
            } else {
                caps_sized(&title, theme.metrics.font_title, TRACK_LABEL).font(FontId::new(
                    theme.metrics.font_title,
                    crate::fonts::cond_bold(),
                ))
            };
            let galley = egui::WidgetText::from(rich).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            );
            let tp = Pos2::new(
                title_rect.left() + title_x,
                title_rect.center().y - galley.size().y / 2.0,
            );
            p.with_clip_rect(drag_rect.intersect(clip)).galley(
                tp,
                galley,
                if is_front { c.text } else { c.dim },
            );
            tr.context_menu(|ui| {
                window_options(ui, core, id, maximized, &mut ev, &title);
            });

            // buttons, right to left: close, maximize, detach, options
            let btn = |ui: &mut Ui, slot: f32, what: &str| {
                let r = Rect::from_min_size(
                    Pos2::new(
                        title_rect.right() - 5.0 - BUTTON * (slot + 1.0),
                        title_rect.top() + (TITLE_H - BUTTON) / 2.0,
                    ),
                    Vec2::splat(BUTTON),
                );
                let resp = ui.interact(r, area_id.with(what), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(theme.metrics.radius_md as u8),
                        if what == "close" {
                            c.clip.gamma_multiply(0.45)
                        } else {
                            c.bg4
                        },
                    );
                }
                (resp, r.shrink(2.0))
            };
            let ink = |r: &egui::Response| if r.hovered() { c.text } else { c.dim };
            let (cl, g) = btn(ui, 0.0, "close");
            cl.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close"));
            Icon::Close.paint(&p, g, ink(&cl));
            ev.close |= cl
                .hint(
                    "Close",
                    "",
                    "closes the window; its contents stay in the project",
                )
                .clicked();
            let (m, g) = btn(ui, 1.0, "maximize");
            m.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    if maximized { "Restore" } else { "Maximize" },
                )
            });
            (if maximized {
                Icon::Restore
            } else {
                Icon::Maximize
            })
            .paint(&p, g, ink(&m));
            ev.maximize |= m
                .hint(
                    if maximized { "Restore" } else { "Maximize" },
                    "",
                    "fills the workspace; double-click the title does the same",
                )
                .clicked();
            let (d, g) = btn(ui, 2.0, "detach");
            d.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Detach"));
            Icon::Detach.paint(&p, g, ink(&d));
            ev.detach |= d
                .hint("Detach", "", "opens the window in its own OS window")
                .clicked();
            let (o, g) = btn(ui, 3.0, "options");
            o.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Window options")
            });
            Icon::Options.paint(&p, g, ink(&o));
            let o = o.hint("Window options", "", "move, size and per-window settings");
            egui::Popup::menu(&o).show(|ui| {
                window_options(ui, core, id, maximized, &mut ev, &title);
            });

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

/// What each window is for (the Help entry of its options menu).
pub fn window_help(id: WindowId) -> &'static str {
    match id {
        WindowId::Playlist => {
            "Playlist: arrange patterns on tracks. Click an empty spot to place the selected pattern, drag to move, drag an edge to resize."
        }
        WindowId::ChannelRack => {
            "Channel rack: one row per channel. Click the pads to toggle steps, drag across to paint, right-click a row for more."
        }
        WindowId::PianoRoll => {
            "Piano roll: edit the selected channel's notes. Left-click draws, right-click deletes, Ctrl+wheel zooms time, Alt+wheel zooms keys."
        }
        WindowId::Mixer => {
            "Mixer: faders, pan, mute and solo for every insert; select one to edit its effects and sends."
        }
        WindowId::History => {
            "History: every change, by author. Undo mine, or undo everything an agent did."
        }
        WindowId::Agents => {
            "Agents: who is connected over MCP, what each may do, and what they changed."
        }
        WindowId::Instrument(_) => "Instrument: the sound source of one channel.",
        WindowId::Effect(..) => "Effect: the parameters of one effect on a mixer insert.",
    }
}

/// The options menu of a title bar (the `...` button, or right-click on the title): the same
/// entries for every window, then the ones that window has of its own.
fn window_options(
    ui: &mut Ui,
    core: &mut AppCore,
    id: WindowId,
    maximized: bool,
    ev: &mut Events,
    title: &str,
) {
    use crate::widgets::menu_item;
    if menu_item(ui, if maximized { "Restore" } else { "Maximize" }, "").clicked() {
        ev.maximize = true;
        ui.close();
    }
    if menu_item(ui, "Detach to own window", "").clicked() {
        ev.detach = true;
        ui.close();
    }
    if menu_item(ui, "Reset size and position", "").clicked() {
        ev.reset = true;
        ui.close();
    }
    match id {
        WindowId::PianoRoll => {
            ui.separator();
            let mut ghosts = core.roll.ghosts;
            if ui
                .checkbox(&mut ghosts, crate::theme::caps("Ghost notes"))
                .changed()
            {
                core.roll.ghosts = ghosts;
            }
            ui.menu_button(crate::theme::caps("Snap"), |ui| {
                for (name, ticks) in crate::piano_roll::SNAPS {
                    if ui
                        .selectable_label(core.roll.snap == *ticks, crate::theme::caps(name))
                        .clicked()
                    {
                        core.roll.snap = *ticks;
                        ui.close();
                    }
                }
            });
            ui.menu_button(crate::theme::caps("Zoom"), |ui| {
                for (preset, label, gesture) in crate::piano_roll::ZoomPreset::MENU {
                    if menu_item(ui, label, gesture).clicked() {
                        core.roll.zoom_request = Some(*preset);
                        ui.close();
                    }
                }
            });
        }
        WindowId::ChannelRack => {
            ui.separator();
            let mut v = core.rack.velocity_view;
            if ui
                .checkbox(&mut v, crate::theme::caps("Velocity view"))
                .changed()
            {
                core.rack.velocity_view = v;
            }
        }
        WindowId::Playlist => {
            ui.separator();
            ui.menu_button(crate::theme::caps("Snap"), |ui| {
                for (name, ticks) in crate::piano_roll::SNAPS {
                    let t = if *ticks == 0 {
                        core.playlist.bar
                    } else {
                        *ticks
                    };
                    if ui
                        .selectable_label(core.playlist.snap == t, crate::theme::caps(name))
                        .clicked()
                    {
                        core.playlist.snap = t;
                        ui.close();
                    }
                }
            });
        }
        _ => {}
    }
    ui.separator();
    if menu_item(ui, "Help", "").clicked() {
        crate::hint::request_help(
            ui.ctx(),
            ph_control::UiHint {
                name: title.to_string(),
                value: String::new(),
                gestures: window_help(id).to_string(),
            },
        );
        ui.close();
    }
    if menu_item(ui, "Close", "").clicked() {
        ev.close = true;
        ui.close();
    }
}
