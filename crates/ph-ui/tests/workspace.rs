//! The floating workspace through real egui pointer and key events: dragging, resizing,
//! raising, maximizing, the F-key toggles and opening instrument and effect windows.

mod common;

use common::*;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
    // the rack was clicked last, so it is in front, but Escape still closes the front-most
    // instrument window in z-order
    assert!(h.state().ws.is_front(rack));
    key(&mut h, Key::Escape);
    assert!(
        !h.state().ws.is_visible(kick),
        "escape reached under the rack"
    );
    key(&mut h, Key::Escape);
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

fn run_cmd(h: &mut H, req: ph_commands::Request) {
    h.state()
        .core
        .studio
        .session
        .execute(&ph_commands::Caller::human(), req)
        .expect("command runs");
}

fn settle_frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn every_window_kind_has_a_title() {
    let (h, ids, _dir) = start();
    let core = &h.state().core;
    let mut all = vec![
        WindowId::Playlist,
        WindowId::ChannelRack,
        WindowId::PianoRoll,
        WindowId::Mixer,
        WindowId::History,
        WindowId::Agents,
        WindowId::Instrument(ids.kick),
        WindowId::Instrument(ids.bass),
        WindowId::Effect(InsertId(4), 0),
        WindowId::Effect(InsertId(4), 1),
        WindowId::Effect(InsertId(5), 0),
    ];
    // objects that do not exist still get a title
    all.push(WindowId::Instrument(ChannelId(999)));
    all.push(WindowId::Effect(InsertId(4), 9));
    all.push(WindowId::Effect(InsertId(99), 0));
    for id in all {
        let t = ph_ui::windows::title_of(core, id);
        assert!(!t.trim().is_empty(), "{id:?} has a blank title");
    }
    // effect titles read "<insert name> - <effect name>"
    let eq = ph_ui::windows::title_of(core, WindowId::Effect(InsertId(4), 0));
    assert!(eq.starts_with("Insert 4 - "), "{eq}");
    assert!(!eq.ends_with(" - "), "{eq}");
    let rev = ph_ui::windows::title_of(core, WindowId::Effect(InsertId(5), 0));
    assert!(rev.starts_with("Reverb - "), "named insert: {rev}");
}

#[test]
fn a_title_hidden_under_a_window_in_front_moves_into_view() {
    let (mut h, ids, _dir) = start();
    let fx = WindowId::Effect(InsertId(4), 0);
    let inst = WindowId::Instrument(ids.bass);
    h.state_mut().ws.show(fx);
    h.state_mut().ws.show(inst);
    frames(&mut h, 3);
    // put the instrument window over the left of the effect window's title bar
    let r = rect(&h, fx);
    h.state_mut().ws.drag_move(inst, r.x + 150.0 - 500.0, r.y);
    h.state_mut().ws.focus(inst);
    frames(&mut h, 3);
    let ws = &h.state().ws;
    assert!(ws.is_front(inst));
    assert!(
        ws.title_start(fx, 100.0, 68.0) > 10.0,
        "the title starts past the cover"
    );
}

#[test]
fn effect_windows_follow_their_effect_when_the_ui_or_an_agent_reorders() {
    let (mut h, _ids, _dir) = start();
    let eq = WindowId::Effect(InsertId(4), 0);
    let comp = WindowId::Effect(InsertId(4), 1);
    h.state_mut().core.sel.insert = 4;
    h.state_mut().ws.show(eq);
    h.state_mut().ws.show(comp);
    frames(&mut h, 3);
    // give each window its own place
    let p = at(&h, eq, 60.0, TITLE_H / 2.0);
    drag(&mut h, p, p + Vec2::new(0.0, 250.0));
    let p = at(&h, comp, 60.0, TITLE_H / 2.0);
    drag(&mut h, p, p + Vec2::new(200.0, 300.0));
    let (eq_rect, comp_rect) = (rect(&h, eq), rect(&h, comp));
    assert_ne!(eq_rect, comp_rect);
    // an agent moves the EQ below the compressor: slots swap, windows follow with their rects
    run_cmd(&mut h, ph_ui::mapping::effect_move(4, 0, 1));
    settle_frames(&mut h, 6);
    assert_eq!(
        rect(&h, comp),
        eq_rect,
        "the EQ's window now sits at slot 1"
    );
    assert_eq!(
        rect(&h, eq),
        comp_rect,
        "the compressor's window is at slot 0"
    );
    assert!(h.state().ws.is_visible(eq) && h.state().ws.is_visible(comp));
    // undo puts them back again (a snapshot from outside the UI)
    run_cmd(&mut h, ph_ui::mapping::undo_mine());
    settle_frames(&mut h, 6);
    assert_eq!(rect(&h, eq), eq_rect);
    assert_eq!(rect(&h, comp), comp_rect);
    // the UI's own "move down" button remaps directly, with the same result
    h.state_mut().ws.show(WindowId::Mixer);
    frames(&mut h, 3);
    let downs: Vec<_> = h.query_all_by_label("v").collect();
    assert!(!downs.is_empty(), "the mixer shows the move-down buttons");
    downs[0].click();
    settle_frames(&mut h, 8);
    assert_eq!(rect(&h, comp), eq_rect);
    assert_eq!(rect(&h, eq), comp_rect);
    // removing the compressor closes its window (and only its window)
    run_cmd(&mut h, ph_ui::mapping::effect_remove(4, 0));
    settle_frames(&mut h, 6);
    assert!(
        h.state().ws.get(eq).is_some(),
        "the EQ's window moved to slot 0"
    );
    assert!(h.state().ws.get(comp).is_none(), "nothing left in slot 1");
}

fn ui_cmd(h: &mut H, cmd: &str, params: serde_json::Value) {
    run_cmd(h, ph_commands::Request::new(cmd, params));
    settle_frames(h, 4);
}

#[test]
fn agents_open_instrument_and_effect_windows_and_select_opens_them() {
    let (mut h, ids, _dir) = start();
    let inst = WindowId::Instrument(ids.snare);
    let fx = WindowId::Effect(InsertId(4), 1);
    assert!(!h.state().ws.is_visible(inst));
    ui_cmd(
        &mut h,
        "ui.open_panel",
        serde_json::json!({ "panel": format!("instrument:{}", ids.snare.0) }),
    );
    assert!(
        h.state().ws.is_front(inst),
        "instrument window opened in front"
    );
    ui_cmd(
        &mut h,
        "ui.open_panel",
        serde_json::json!({ "panel": "effect:4:1" }),
    );
    assert!(h.state().ws.is_front(fx), "effect window opened in front");
    ui_cmd(
        &mut h,
        "ui.close_panel",
        serde_json::json!({ "panel": "effect:4:1" }),
    );
    assert!(!h.state().ws.is_visible(fx));
    ui_cmd(
        &mut h,
        "ui.close_panel",
        serde_json::json!({ "panel": format!("instrument:{}", ids.snare.0) }),
    );
    assert!(!h.state().ws.is_visible(inst));
    // selecting a channel opens its instrument window
    let hat = WindowId::Instrument(ids.hat);
    ui_cmd(
        &mut h,
        "ui.select",
        serde_json::json!({ "paths": [format!("channel/{}", ids.hat.0)] }),
    );
    assert!(h.state().ws.is_front(hat));
    assert_eq!(h.state().core.sel.channel, Some(ids.hat));
    // the piano roll request selects the pattern and the channel it names
    ui_cmd(
        &mut h,
        "ui.show_piano_roll",
        serde_json::json!({ "pattern": ids.bassline.0, "channel": ids.bass.0 }),
    );
    assert!(h.state().ws.is_front(WindowId::PianoRoll));
    assert_eq!(h.state().core.sel.pattern, Some(ids.bassline));
    assert_eq!(h.state().core.sel.channel, Some(ids.bass));
    assert!(
        h.state()
            .core
            .agent_select
            .contains(&ph_core::ObjectPath::Channel(ids.bass))
    );
}

#[test]
fn small_windows_scroll_instead_of_clipping() {
    let (mut h, ids, _dir) = start();
    let w = WindowId::Instrument(ids.bass);
    h.state_mut().ws.show(w);
    frames(&mut h, 4);
    // shrink it to the minimum size by dragging the bottom-right corner
    let r = rect(&h, w);
    let corner = at(&h, w, r.w - 2.0, r.h - 2.0);
    drag(&mut h, corner, corner - Vec2::new(400.0, 400.0));
    frames(&mut h, 4);
    let info = h
        .state()
        .core
        .scrolls
        .get(&w)
        .copied()
        .expect("scroll area");
    assert!(
        info.content.y > info.viewport.y + 20.0,
        "the content is taller than the window: {info:?}"
    );
    assert_eq!(info.offset.y, 0.0);
    assert!(
        info.content.x <= info.viewport.x + 1.0,
        "knob rows wrap instead of overflowing sideways: {info:?}"
    );
    // a real mouse wheel over the window scrolls it
    let over = at(&h, w, 100.0, TITLE_H + 80.0);
    h.event(Event::PointerMoved(over));
    h.step();
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -120.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    frames(&mut h, 6);
    let after = h.state().core.scrolls.get(&w).copied().unwrap();
    assert!(after.offset.y > 20.0, "scrolled down: {after:?}");
}

#[test]
fn the_mixer_and_other_windows_scroll_both_ways_but_the_editors_do_not_wrap_in_a_scroll_area() {
    let (mut h, _ids, _dir) = start();
    h.state_mut().ws.show(WindowId::Mixer);
    h.state_mut().ws.show(WindowId::History);
    h.state_mut().ws.show(WindowId::Agents);
    frames(&mut h, 4);
    for w in [WindowId::Mixer, WindowId::History, WindowId::Agents] {
        assert!(h.state().core.scrolls.contains_key(&w), "{w:?} scrolls");
    }
    // the editors keep their own panning and zooming, with their toolbars pinned
    for w in [
        WindowId::Playlist,
        WindowId::ChannelRack,
        WindowId::PianoRoll,
    ] {
        assert!(
            !h.state().core.scrolls.contains_key(&w),
            "{w:?} pans itself"
        );
    }
    // a mixer window smaller than its content scrolls sideways
    let r = rect(&h, WindowId::Mixer);
    let corner = at(&h, WindowId::Mixer, r.w - 2.0, r.h - 2.0);
    drag(&mut h, corner, corner - Vec2::new(900.0, 400.0));
    frames(&mut h, 4);
    let i = h.state().core.scrolls[&WindowId::Mixer];
    assert!(
        i.content.x > i.viewport.x && i.content.y > i.viewport.y,
        "{i:?}"
    );
}

#[test]
fn the_solo_display_follows_the_session_when_an_agent_solos() {
    let (mut h, ids, _dir) = start();
    h.state_mut().ws.show(WindowId::Mixer);
    frames(&mut h, 3);
    assert!(h.state().core.solo_channels.is_empty());
    // an agent solos a channel and an insert; no snapshot is published for either
    let agent =
        ph_commands::Caller::agent("tester", ph_commands::Grant(vec![ph_commands::Scope::Full]));
    for r in [
        ph_ui::mapping::channel_solo(ids.kick, true),
        ph_ui::mapping::mixer_solo(3, true),
    ] {
        h.state().core.studio.session.execute(&agent, r).unwrap();
    }
    settle_frames(&mut h, 6);
    assert!(h.state().core.solo_channels.contains(&ids.kick));
    assert!(h.state().core.solo_inserts.contains(&3));
    // and un-solo
    h.state()
        .core
        .studio
        .session
        .execute(&agent, ph_ui::mapping::channel_solo(ids.kick, false))
        .unwrap();
    settle_frames(&mut h, 6);
    assert!(h.state().core.solo_channels.is_empty());
}

struct FakeAgents;

impl ph_ui::AgentDirectory for FakeAgents {
    fn clients(&self) -> Vec<ph_ui::ClientInfo> {
        let c = |name: &str, live: bool| ph_ui::ClientInfo {
            name: name.into(),
            scopes: vec![ph_commands::Scope::Compose],
            live,
            connected_for: 42,
            idle_for: 3,
            commands: 9,
        };
        // remembered first on purpose: the panel must still put live clients on top
        vec![c("Old Agent", false), c("Live Agent", true)]
    }
    fn pending_grants(&self) -> Vec<ph_ui::GrantRequest> {
        vec![]
    }
    fn resolve_grant(&self, _: u64, _: Option<Vec<ph_commands::Scope>>) {}
}

#[test]
fn the_agents_window_lists_live_connections_before_remembered_ones() {
    let (mut h, _ids, _dir) = start();
    h.state_mut().core.hooks.agents = Some(std::sync::Arc::new(FakeAgents));
    h.state_mut().ws.show(WindowId::Agents);
    frames(&mut h, 4);
    let live = h.get_by_label("Live Agent");
    let old = h.get_by_label("Old Agent");
    assert!(
        live.rect().min.y < old.rect().min.y,
        "live clients are listed first"
    );
    assert!(h.query_by_label("disconnected").is_some());
    assert!(
        h.query_by_label_contains("9 command(s)").is_some(),
        "live rows show the command count"
    );
    let _ = (live, old);
}
