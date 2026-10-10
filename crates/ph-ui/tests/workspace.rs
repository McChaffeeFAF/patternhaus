//! The floating workspace through real egui pointer and key events: dragging, resizing,
//! raising, maximizing, the F-key toggles and opening instrument and effect windows.

mod common;

use common::*;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use ph_core::{ChannelId, InsertId};
use ph_ui::PhApp;
use ph_ui::workspace::{TITLE_H, WindowId};
use std::time::Duration;

type H = Harness<'static, PhApp>;

fn start() -> (H, Ids, tempfile::TempDir) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 500, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    for _ in 0..4 {
        h.step();
    }
    (h, ids, fx.dir)
}

fn frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn button(h: &mut H, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
    h.step();
}

fn click(h: &mut H, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    button(h, pos, true);
    button(h, pos, false);
    frames(h, 6);
}

fn double_click(h: &mut H, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    button(h, pos, true);
    button(h, pos, false);
    button(h, pos, true);
    button(h, pos, false);
    // let egui's double-click window (0.3 s) lapse so the next gesture starts fresh
    frames(h, 14);
}

fn drag(h: &mut H, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    button(h, from, true);
    for i in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        h.step();
    }
    button(h, to, false);
    frames(h, 6);
}

/// Screen position of a point `(dx, dy)` inside a window's rectangle.
fn at(h: &H, id: WindowId, dx: f32, dy: f32) -> Pos2 {
    let r = h.state().ws.effective_rect(id).expect("window exists");
    h.state().work_rect.min + Vec2::new(r.x + dx, r.y + dy)
}

fn rect(h: &H, id: WindowId) -> ph_ui::workspace::Rect {
    h.state().ws.get(id).expect("window exists").rect
}

fn key(h: &mut H, k: Key) {
    h.key_press(k);
    h.step();
    h.step();
}

#[test]
fn dragging_a_title_bar_moves_the_window() {
    let (mut h, _, _dir) = start();
    let rack = WindowId::ChannelRack;
    let before = rect(&h, rack);
    let from = at(&h, rack, 60.0, TITLE_H / 2.0);
    drag(&mut h, from, from + Vec2::new(220.0, 140.0));
    let after = rect(&h, rack);
    assert!(
        (after.x - before.x - 220.0).abs() < 1.0 && (after.y - before.y - 140.0).abs() < 1.0,
        "{before:?} -> {after:?}"
    );
    assert_eq!((after.w, after.h), (before.w, before.h));
    assert!(h.state().ws.is_front(rack));
}

#[test]
fn dragging_toward_an_edge_snaps_and_the_title_stays_reachable() {
    let (mut h, _, _dir) = start();
    let rack = WindowId::ChannelRack;
    let r0 = rect(&h, rack);
    // 5 pt short of the workspace's left edge: snaps flush to it
    let from = at(&h, rack, 60.0, TITLE_H / 2.0);
    drag(&mut h, from, from + Vec2::new(-r0.x + 5.0, 0.0));
    assert_eq!(rect(&h, rack).x, 0.0);
    // dragged far out of the bottom: the title bar is still inside the workspace
    let from = at(&h, rack, 60.0, TITLE_H / 2.0);
    drag(&mut h, from, from + Vec2::new(0.0, 5000.0));
    let area = h.state().ws.area();
    assert!(rect(&h, rack).y <= area.h - TITLE_H);
}

#[test]
fn dragging_an_edge_resizes() {
    let (mut h, _, _dir) = start();
    let rack = WindowId::ChannelRack;
    let r0 = rect(&h, rack);
    // right edge
    let from = at(&h, rack, r0.w - 1.0, r0.h / 2.0);
    drag(&mut h, from, from + Vec2::new(90.0, 0.0));
    let r1 = rect(&h, rack);
    assert!((r1.w - r0.w - 90.0).abs() < 1.0, "{r0:?} -> {r1:?}");
    assert_eq!(r1.x, r0.x);
    // bottom-right corner
    let from = at(&h, rack, r1.w - 2.0, r1.h - 2.0);
    drag(&mut h, from, from + Vec2::new(-40.0, 60.0));
    let r2 = rect(&h, rack);
    assert!((r2.w - r1.w + 40.0).abs() < 1.0 && (r2.h - r1.h - 60.0).abs() < 1.0);
    // left edge: the far edge stays put
    let from = at(&h, rack, 1.0, r2.h / 2.0);
    drag(&mut h, from, from + Vec2::new(30.0, 0.0));
    let r3 = rect(&h, rack);
    assert!((r3.right() - r2.right()).abs() < 1.0 && (r3.x - r2.x - 30.0).abs() < 1.0);
    // dragged below the minimum, it stops at the minimum
    let from = at(&h, rack, r3.w - 1.0, r3.h / 2.0);
    drag(&mut h, from, from + Vec2::new(-2000.0, 0.0));
    assert_eq!(rect(&h, rack).w, rack.min_size().0);
}

#[test]
fn clicking_a_back_window_raises_it() {
    let (mut h, _, _dir) = start();
    let rack = WindowId::ChannelRack;
    let roll = WindowId::PianoRoll;
    // park the rack where the roll will cover its lower part, then open the roll on top
    let from = at(&h, rack, 60.0, TITLE_H / 2.0);
    drag(&mut h, from, from + Vec2::new(300.0, 300.0));
    key(&mut h, Key::F7);
    assert!(h.state().ws.is_front(roll), "F7 opens the roll in front");
    let rr = rect(&h, rack);
    let ro = rect(&h, roll);
    // a spot on the rack just below the roll
    let dy = ro.bottom() + 20.0 - rr.y;
    assert!(
        dy > TITLE_H && dy < rr.h,
        "the rack pokes out below the roll"
    );
    let spot = at(&h, rack, 100.0, dy);
    click(&mut h, spot);
    assert!(h.state().ws.is_front(rack), "clicking the rack raised it");
    // and egui agrees about what is under the pointer
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new(("ph_window", rack)));
    assert_eq!(h.ctx.layer_id_at(spot), Some(layer));
    // the order the model holds
    assert_eq!(
        h.state().ws.visible_in_z_order(),
        vec![WindowId::Playlist, roll, rack]
    );
}

#[test]
fn double_clicking_a_title_maximizes_and_restores() {
    let (mut h, _, _dir) = start();
    let rack = WindowId::ChannelRack;
    let before = rect(&h, rack);
    let p = at(&h, rack, 100.0, TITLE_H / 2.0);
    double_click(&mut h, p);
    assert!(h.state().ws.get(rack).unwrap().maximized);
    assert_eq!(
        h.state().ws.effective_rect(rack),
        Some(h.state().ws.area()),
        "fills the workspace"
    );
    let p = at(&h, rack, 100.0, TITLE_H / 2.0);
    double_click(&mut h, p);
    assert!(!h.state().ws.get(rack).unwrap().maximized);
    assert_eq!(rect(&h, rack), before, "restored to where it was");
}

#[test]
fn the_title_bar_buttons_close_and_maximize() {
    let (mut h, _, _dir) = start();
    let rack = WindowId::ChannelRack;
    let r = rect(&h, rack);
    // maximize is the middle button, close the right-most
    let p = at(&h, rack, r.w - 4.0 - 30.0, TITLE_H / 2.0);
    click(&mut h, p);
    assert!(h.state().ws.get(rack).unwrap().maximized);
    let w = h.state().ws.area().w;
    let p = at(&h, rack, w - 14.0, TITLE_H / 2.0);
    click(&mut h, p);
    assert!(!h.state().ws.is_visible(rack), "close hides the window");
}

#[test]
fn f_keys_show_raise_and_hide() {
    let (mut h, _, _dir) = start();
    let (roll, mixer) = (WindowId::PianoRoll, WindowId::Mixer);
    assert!(!h.state().ws.is_visible(roll));
    key(&mut h, Key::F7);
    assert!(h.state().ws.is_front(roll), "first press shows it");
    key(&mut h, Key::F9);
    assert!(h.state().ws.is_front(mixer));
    key(&mut h, Key::F7);
    assert!(
        h.state().ws.is_visible(roll) && h.state().ws.is_front(roll),
        "a visible window behind another is raised, not hidden"
    );
    key(&mut h, Key::F7);
    assert!(!h.state().ws.is_visible(roll), "in front: hidden");
    // F8 is the browser dock
    assert!(h.state().ws.browser.visible);
    key(&mut h, Key::F8);
    assert!(!h.state().ws.browser.visible);
    key(&mut h, Key::F8);
    assert!(h.state().ws.browser.visible);
    // F5 raises the maximized playlist behind the rack; again hides it
    key(&mut h, Key::F5);
    assert!(h.state().ws.is_front(WindowId::Playlist));
    key(&mut h, Key::F5);
    assert!(!h.state().ws.is_visible(WindowId::Playlist));
}

#[test]
fn clicking_a_channel_opens_its_instrument_window_and_escape_closes_it() {
    let (mut h, ids, _dir) = start();
    let rack = WindowId::ChannelRack;
    // the name sits left of the steps, in the channel's row
    let (_, steps, _) = h
        .state()
        .core
        .rack
        .geometry
        .iter()
        .find(|(c, ..)| *c == ids.kick)
        .cloned()
        .unwrap();
    let r = h.state().ws.effective_rect(rack).unwrap();
    let name = Pos2::new(
        h.state().work_rect.min.x + r.x + 1.0 + 32.0 + 40.0,
        steps.center().y,
    );
    click(&mut h, name);
    let kick = WindowId::Instrument(ids.kick);
    assert!(h.state().ws.is_front(kick), "kick's window opened in front");
    // a second channel: both stay open
    let (_, steps, _) = h
        .state()
        .core
        .rack
        .geometry
        .iter()
        .find(|(c, ..)| *c == ids.bass)
        .cloned()
        .unwrap();
    // the kick window may cover the rack, so move it out of the way first
    let from = at(&h, kick, 80.0, TITLE_H / 2.0);
    drag(&mut h, from, from + Vec2::new(400.0, 200.0));
    let name = Pos2::new(
        h.state().work_rect.min.x + r.x + 1.0 + 32.0 + 40.0,
        steps.center().y,
    );
    click(&mut h, name);
    let bass = WindowId::Instrument(ids.bass);
    assert!(h.state().ws.is_visible(kick) && h.state().ws.is_visible(bass));
    assert!(h.state().ws.is_front(bass));
    // Escape closes the front instrument window only
    key(&mut h, Key::Escape);
    assert!(!h.state().ws.is_visible(bass));
    assert!(h.state().ws.is_visible(kick));
    // the rack was clicked last, so it holds the focus now and Escape leaves everything alone
    key(&mut h, Key::Escape);
    assert!(h.state().ws.is_visible(kick) && h.state().ws.is_visible(rack));
    // focus the kick window by clicking its title, then Escape closes it
    let p = at(&h, kick, 40.0, TITLE_H / 2.0);
    click(&mut h, p);
    key(&mut h, Key::Escape);
    assert!(!h.state().ws.is_visible(kick));
    assert!(
        h.state().ws.is_visible(rack),
        "escape never closes the rack"
    );
}

#[test]
fn clicking_an_effect_slot_opens_its_window_and_deleting_the_channel_closes_instrument_windows() {
    let (mut h, ids, _dir) = start();
    // an agent-style request opens windows the same way a click does
    h.state_mut().core.window_requests.push((
        WindowId::Effect(InsertId(4), 1),
        ph_ui::core::PanelAction::Open,
    ));
    h.state_mut().core.window_requests.push((
        WindowId::Instrument(ids.bass),
        ph_ui::core::PanelAction::Open,
    ));
    frames(&mut h, 3);
    assert!(h.state().ws.is_visible(WindowId::Effect(InsertId(4), 1)));
    assert!(h.state().ws.is_visible(WindowId::Instrument(ids.bass)));
    // an effect slot that does not exist cannot be opened
    h.state_mut().core.window_requests.push((
        WindowId::Effect(InsertId(4), 9),
        ph_ui::core::PanelAction::Open,
    ));
    frames(&mut h, 2);
    assert!(!h.state().ws.is_visible(WindowId::Effect(InsertId(4), 9)));
    // deleting the channel through a command closes its window (driven by the snapshot)
    h.state_mut()
        .core
        .send(ph_ui::mapping::channel_remove(ids.bass));
    for _ in 0..40 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
        if h.state().ws.get(WindowId::Instrument(ids.bass)).is_none() {
            break;
        }
    }
    assert!(h.state().ws.get(WindowId::Instrument(ids.bass)).is_none());
    assert!(h.state().ws.is_visible(WindowId::Effect(InsertId(4), 1)));
    let _ = ChannelId(0);
}
