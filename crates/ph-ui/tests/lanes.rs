//! The controller lane editor under the piano roll (M3/T11), through real egui events offscreen
//! and a real session: clicks and drags in the lane become `cc.*` commands, every gesture is one
//! undo step, the lane follows the grid's zoom and scroll, and an agent's edits show up live.
//! Waits settle on the session's snapshot, never on fixed frame counts.
//!
//! Pictures of the lane area are written to `local/reports/lane-editor/` (git-ignored) with
//! `PH_SCREENSHOTS=1` and a GPU adapter.

mod common;

use common::*;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_core::{CcPoint, Controller, CurveKind};
use ph_ui::PhApp;
use ph_ui::mapping;
use ph_ui::piano_roll::PrAction;
use ph_ui::piano_roll::lanes::{self, LaneKind};
use std::time::Duration;

type H = Harness<'static, PhApp>;

const MOD: Controller = Controller::Cc(1);

fn harness(app: PhApp) -> H {
    let mut h = Harness::builder()
        .with_size(Vec2::new(1400.0, 760.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(
            |ui, app: &mut PhApp| {
                let ctx = ui.ctx().clone();
                if !ph_ui::fonts::ensure(&ctx) {
                    return;
                }
                app.core.theme.clone().apply(&ctx);
                let now = ctx.input(|i| i.time);
                app.core.update(now);
                ph_ui::panels::piano_roll::show(ui, &mut app.core);
            },
            app,
        );
    frames(&mut h, 6);
    h
}

fn start() -> (H, Ids, tempfile::TempDir) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.bassline);
    app.core.sel.channel = Some(ids.bass);
    app.core.fix_selection();
    // a roll fitted to the pattern, 1/16 snap (the default)
    let mut h = harness(app);
    frames(&mut h, 4);
    (h, ids, fx.dir)
}

fn frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(6));
    }
}

/// Steps until the session's snapshot satisfies `done` (and every command has been answered).
fn until(h: &mut H, what: &str, mut done: impl FnMut(&PhApp) -> bool) {
    let t0 = std::time::Instant::now();
    loop {
        h.step();
        let app = h.state();
        if app.core.dispatch.in_flight() == 0 && done(app) {
            return;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(6));
    }
}

fn run(h: &mut H, r: ph_commands::Request) -> serde_json::Value {
    h.state()
        .core
        .studio
        .session
        .execute(&ph_commands::Caller::human(), r.clone())
        .unwrap_or_else(|e| panic!("{}: {e}", r.command))
        .result
}

fn pts(h: &H, c: Controller) -> Vec<CcPoint> {
    let core = &h.state().core;
    let (p, ch) = (core.sel.pattern.unwrap(), core.sel.channel.unwrap());
    core.project.pattern(p).unwrap().lane(ch, c).points
}

fn add(h: &mut H, c: Controller, points: &[(u64, f64)]) {
    let core = &h.state().core;
    let (p, ch) = (core.sel.pattern.unwrap(), core.sel.channel.unwrap());
    let specs: Vec<ph_core::CcPointSpec> = points
        .iter()
        .map(|(t, v)| ph_core::CcPointSpec {
            controller: c,
            tick: *t,
            value: *v,
            curve: CurveKind::Linear,
            tension: 0.0,
        })
        .collect();
    run(h, mapping::cc_add_points(p, ch, c, &specs));
    until(h, "points", |a| {
        a.core.project.pattern(p).unwrap().lane(ch, c).points.len() >= specs.len()
    });
}

/// Shows `c`'s lane (the roll's selector does this on a click; here by state).
fn show_lane(h: &mut H, c: Controller) {
    let core = &mut h.state_mut().core;
    let (p, ch) = (core.sel.pattern.unwrap(), core.sel.channel.unwrap());
    let project = core.project.clone();
    core.roll
        .lane
        .set_kind(LaneKind::Controller(c), &project, p, ch);
    frames(h, 3);
}

/// Where a point sits on screen.
fn at(h: &H, c: Controller, tick: u64, value: f64) -> Pos2 {
    let roll = &h.state().core.roll;
    let env = roll.lane_env().unwrap();
    let v = roll.lane.pos_of(&env, c, tick, value);
    roll.lane.body.min + v
}

fn move_to(h: &mut H, p: Pos2) {
    h.event(Event::PointerMoved(p));
    frames(h, 2);
}

fn button(h: &mut H, p: Pos2, b: PointerButton, pressed: bool) {
    h.event(Event::PointerButton {
        pos: p,
        button: b,
        pressed,
        modifiers: Modifiers::NONE,
    });
    h.step();
}

fn hold(h: &mut H, m: Modifiers) {
    h.event(Event::ModifiersChanged(m));
    h.step();
}

fn click(h: &mut H, p: Pos2) {
    move_to(h, p);
    button(h, p, PointerButton::Primary, true);
    button(h, p, PointerButton::Primary, false);
    frames(h, 2);
}

fn right_click(h: &mut H, p: Pos2) {
    move_to(h, p);
    button(h, p, PointerButton::Secondary, true);
    button(h, p, PointerButton::Secondary, false);
    frames(h, 3);
}

/// Press at the first position, follow the path, release at the last.
fn stroke(h: &mut H, path: &[Pos2]) {
    move_to(h, path[0]);
    button(h, path[0], PointerButton::Primary, true);
    for p in &path[1..] {
        h.event(Event::PointerMoved(*p));
        h.step();
        std::thread::sleep(Duration::from_millis(4));
    }
    button(h, *path.last().unwrap(), PointerButton::Primary, false);
    frames(h, 2);
}

fn undo(h: &mut H) {
    h.state_mut().core.send(mapping::undo_mine());
}

const SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: false,
};
const CTRL: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: true,
};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn a_click_adds_a_point_through_cc_add_points_and_undo_removes_it() {
    let (mut h, ..) = start();
    show_lane(&mut h, MOD);
    assert!(pts(&h, MOD).is_empty());
    let sent = h.state().core.dispatch.sent;
    let p = at(&h, MOD, 970, 0.5);
    click(&mut h, p);
    until(&mut h, "the point", |a| {
        a.core.project.patterns.values().any(|p| p.cc_count() == 1)
    });
    let p = pts(&h, MOD);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].tick, 960, "970 snaps to the 1/16 grid at 960");
    assert!(near(p[0].value, 0.5), "value {}", p[0].value);
    assert_eq!(p[0].curve, CurveKind::Linear);
    // one gesture, one command
    assert_eq!(h.state().core.dispatch.sent, sent + 1);
    undo(&mut h);
    until(&mut h, "undo", |a| {
        a.core.project.patterns.values().all(|p| p.cc_count() == 0)
    });
}

#[test]
fn dragging_a_point_moves_it_in_time_and_value_as_one_undo_step() {
    let (mut h, ..) = start();
    add(&mut h, MOD, &[(480, 0.25), (1920, 0.75)]);
    show_lane(&mut h, MOD);
    let before = pts(&h, MOD);
    let path = [
        at(&h, MOD, 1920, 0.75),
        at(&h, MOD, 2200, 0.7),
        at(&h, MOD, 2400, 0.3),
    ];
    stroke(&mut h, &path);
    until(&mut h, "the move", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .iter()
            .any(|x| x.tick == 2400)
    });
    let after = pts(&h, MOD);
    assert_eq!(after.len(), 2, "moved, not copied");
    assert_eq!(after[0].tick, 480, "the other point is untouched");
    assert_eq!(after[1].tick, 2400);
    assert!(near(after[1].value, 0.3), "value {}", after[1].value);
    // the moved point stays selected (found again by its tick)
    until(&mut h, "selection", |a| {
        a.core.roll.lane.selection.len() == 1
    });
    // a single undo puts both back
    undo(&mut h);
    until(&mut h, "undo", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        let l = a.core.project.pattern(p).unwrap().lane(ch, MOD).points;
        l.len() == 2 && l.iter().any(|x| x.tick == 1920)
    });
    let back = pts(&h, MOD);
    assert_eq!(back.len(), before.len());
    assert!(near(back[1].value, 0.75));
}

#[test]
fn a_freehand_stroke_is_thinned_and_is_one_undo_step() {
    let (mut h, ..) = start();
    h.state_mut().core.roll.snap = 60; // 1/64: a sample every few pixels
    show_lane(&mut h, MOD);
    // a triangle: up for a beat, down for a beat, sampled densely
    let mut path = Vec::new();
    for i in 0..=48 {
        let t = 480 + i * 40;
        let v = if i <= 24 {
            0.2 + 0.6 * i as f64 / 24.0
        } else {
            0.8 - 0.6 * (i - 24) as f64 / 24.0
        };
        path.push(at(&h, MOD, t, v));
    }
    let sent = h.state().core.dispatch.sent;
    stroke(&mut h, &path);
    until(&mut h, "the stroke", |a| {
        a.core.project.patterns.values().any(|p| p.cc_count() > 0)
    });
    let written = pts(&h, MOD);
    assert!(
        (3..=12).contains(&written.len()),
        "a triangle thins to a few points, got {}",
        written.len()
    );
    assert!(
        written.len() < path.len() / 3,
        "thinned from {}",
        path.len()
    );
    assert!(written.windows(2).all(|w| w[0].tick < w[1].tick));
    let peak = written.iter().map(|p| p.value).fold(0.0, f64::max);
    assert!(near(peak, 0.8), "the peak survives thinning: {peak}");
    assert_eq!(
        h.state().core.dispatch.sent,
        sent + 1,
        "one command for the whole stroke"
    );
    undo(&mut h);
    until(&mut h, "undo", |a| {
        a.core.project.patterns.values().all(|p| p.cc_count() == 0)
    });
}

#[test]
fn shift_drag_writes_a_straight_ramp() {
    let (mut h, ..) = start();
    show_lane(&mut h, MOD);
    hold(&mut h, SHIFT);
    let path = [
        at(&h, MOD, 0, 0.2),
        at(&h, MOD, 960, 0.5),
        at(&h, MOD, 1920, 0.8),
    ];
    stroke(&mut h, &path);
    hold(&mut h, Modifiers::NONE);
    until(&mut h, "the ramp", |a| {
        a.core.project.patterns.values().any(|p| p.cc_count() == 2)
    });
    let p = pts(&h, MOD);
    assert_eq!(
        p.len(),
        2,
        "a ramp is two points, however the mouse got there"
    );
    assert_eq!((p[0].tick, p[1].tick), (0, 1920));
    assert!(near(p[0].value, 0.2) && near(p[1].value, 0.8));
    undo(&mut h);
    until(&mut h, "undo", |a| {
        a.core.project.patterns.values().all(|p| p.cc_count() == 0)
    });
}

#[test]
fn the_segment_menu_changes_the_curve_and_a_diamond_drag_changes_the_tension() {
    let (mut h, ..) = start();
    add(&mut h, MOD, &[(0, 0.2), (1920, 0.8)]);
    show_lane(&mut h, MOD);
    // the menu: right-click the segment between the points, choose Hold
    let p = at(&h, MOD, 600, 0.9);
    right_click(&mut h, p);
    h.get_by_label("Hold").click();
    frames(&mut h, 2);
    until(&mut h, "the curve", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .first()
            .is_some_and(|x| x.curve == CurveKind::Hold)
    });
    let p = pts(&h, MOD);
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].curve, CurveKind::Hold);
    assert_eq!((p[0].tick, p[1].tick), (0, 1920), "ticks and values kept");
    // back to linear, then drag the diamond in the middle of the segment upward
    h.state_mut().core.roll.lane.menu_point = None;
    let env = h.state().core.roll.lane_env().unwrap();
    let id = pts(&h, MOD)[0].id;
    let outs = h
        .state_mut()
        .core
        .roll
        .lane
        .set_curve(&env, id, CurveKind::Linear);
    h.state_mut().core.apply_roll(outs);
    until(&mut h, "linear", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .first()
            .is_some_and(|x| x.curve == CurveKind::Linear)
    });
    frames(&mut h, 2);
    let handle = {
        let roll = &h.state().core.roll;
        let env = roll.lane_env().unwrap();
        let hs = roll.lane.handles(&env);
        assert_eq!(hs.len(), 1, "one diamond for the one linear segment");
        roll.lane.body.min + hs[0].1
    };
    stroke(
        &mut h,
        &[
            handle,
            handle + Vec2::new(0.0, -10.0),
            handle + Vec2::new(0.0, -22.0),
        ],
    );
    until(&mut h, "tension", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .first()
            .is_some_and(|x| x.tension != 0.0)
    });
    let p = pts(&h, MOD);
    assert!(
        p[0].tension > 0.0,
        "up bends the curve up: {}",
        p[0].tension
    );
    assert_eq!(p[0].curve, CurveKind::Linear);
    assert_eq!(p.len(), 2);
}

#[test]
fn right_click_deletes_a_point_and_a_box_selects_and_delete_removes_them() {
    let (mut h, ..) = start();
    add(
        &mut h,
        MOD,
        &[(0, 0.2), (480, 0.4), (960, 0.6), (1440, 0.8), (1920, 0.5)],
    );
    show_lane(&mut h, MOD);
    let p = at(&h, MOD, 960, 0.6);
    right_click(&mut h, p);
    until(&mut h, "the delete", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .len()
            == 4
    });
    assert!(pts(&h, MOD).iter().all(|p| p.tick != 960));
    // rubber band (Ctrl + drag on empty space) around the two middle points
    let a = at(&h, MOD, 480, 0.4) + Vec2::new(-12.0, 14.0);
    let b = at(&h, MOD, 1440, 0.8) + Vec2::new(12.0, -14.0);
    hold(&mut h, CTRL);
    stroke(&mut h, &[a, (a + b.to_vec2()) * 0.5, b]);
    hold(&mut h, Modifiers::NONE);
    assert_eq!(h.state().core.roll.lane.selection.len(), 2);
    assert!(h.state().core.roll.lane.focused);
    // Delete removes the selection, in one command
    let outs = h.state_mut().core.roll.action(PrAction::DeleteSelected);
    h.state_mut().core.apply_roll(outs);
    until(&mut h, "box delete", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .len()
            == 2
    });
    undo(&mut h);
    until(&mut h, "undo", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .len()
            == 4
    });
}

#[test]
fn ctrl_a_selects_the_lane_points_when_the_lane_has_focus_and_the_notes_otherwise() {
    let (mut h, ..) = start();
    add(&mut h, MOD, &[(0, 0.2), (960, 0.6)]);
    show_lane(&mut h, MOD);
    let p = at(&h, MOD, 0, 0.2);
    click(&mut h, p);
    assert!(h.state().core.roll.lane.focused);
    let _ = h.state_mut().core.roll.action(PrAction::SelectAll);
    assert_eq!(h.state().core.roll.lane.selection.len(), 2);
    assert!(h.state().core.roll.selection.is_empty(), "notes untouched");
    // a click in the note grid moves the focus back
    let g = h.state().core.roll.last_grid;
    click(&mut h, g.center());
    assert!(!h.state().core.roll.lane.focused);
}

#[test]
fn lanes_with_points_are_tabs_and_the_selector_switches_between_lanes() {
    let (mut h, ..) = start();
    add(&mut h, MOD, &[(0, 0.5)]);
    add(&mut h, Controller::PitchBend, &[(0, 0.0), (960, 0.5)]);
    frames(&mut h, 3);
    assert_eq!(h.state().core.roll.lane.kind, LaneKind::Velocity);
    h.get_by_label("CC1").click();
    frames(&mut h, 2);
    assert_eq!(h.state().core.roll.lane.kind, LaneKind::Controller(MOD));
    assert_eq!(h.state().core.roll.lane.points.len(), 1);
    h.get_by_label("Bend").click();
    frames(&mut h, 2);
    assert_eq!(
        h.state().core.roll.lane.kind,
        LaneKind::Controller(Controller::PitchBend)
    );
    assert_eq!(h.state().core.roll.lane.points.len(), 2);
    // lanes with points come first in the selector, then the usual controllers
    let kinds = h.state().core.roll.lane.selector_entries();
    assert_eq!(kinds[0], LaneKind::Controller(MOD));
    assert_eq!(kinds[1], LaneKind::Controller(Controller::PitchBend));
    assert!(kinds.contains(&LaneKind::Velocity));
    assert!(kinds.contains(&LaneKind::Controller(Controller::Cc(11))));
    assert!(kinds.contains(&LaneKind::Controller(Controller::Cc(64))));
    // "Other CC": any number
    let (p, ch, project) = {
        let c = &h.state().core;
        (
            c.sel.pattern.unwrap(),
            c.sel.channel.unwrap(),
            c.project.clone(),
        )
    };
    h.state_mut().core.roll.lane.set_kind(
        LaneKind::Controller(Controller::Cc(74)),
        &project,
        p,
        ch,
    );
    assert_eq!(
        h.state().core.roll.lane.tab_kinds().last(),
        Some(&LaneKind::Controller(Controller::Cc(74))),
        "the lane being edited has a tab even when empty"
    );
    // Velocity is still there, and still the old editor
    h.get_by_label("Velocity").click();
    frames(&mut h, 2);
    assert_eq!(h.state().core.roll.lane.kind, LaneKind::Velocity);
}

#[test]
fn the_bend_lane_is_centred_and_values_read_in_the_controllers_units() {
    let (mut h, ..) = start();
    let bend = Controller::PitchBend;
    add(&mut h, bend, &[(0, 0.0), (1920, 0.0)]);
    show_lane(&mut h, bend);
    let roll = &h.state().core.roll;
    let body = roll.lane.body;
    // zero sits on the middle of the body; full up and full down at the ends
    let mid = roll.lane.y_of(bend, 0.0);
    assert!(
        (mid - roll.lane.h / 2.0).abs() < 0.6,
        "zero at {mid} of {}",
        roll.lane.h
    );
    assert!(roll.lane.y_of(bend, 1.0) < mid && roll.lane.y_of(bend, -1.0) > mid);
    assert!(near(roll.lane.value_at_y(bend, mid), 0.0));
    // CC lanes run 0 at the bottom to 127 at the top
    assert!(roll.lane.y_of(MOD, 1.0) < roll.lane.y_of(MOD, 0.0));
    // units
    assert_eq!(lanes::units(bend, -1.0), "-8192");
    assert_eq!(lanes::units(bend, 1.0), "+8191");
    assert_eq!(lanes::units(bend, 0.0), "+0");
    assert_eq!(lanes::units(MOD, 1.0), "127");
    assert_eq!(lanes::units(MOD, 0.5), "64");
    // the drawn curve of the flat lane runs along the middle line
    let want_y = body.top() + mid;
    let found = shapes(&h).iter().any(|s| match s {
        egui::Shape::Path(p) if p.points.len() >= 8 => p
            .points
            .iter()
            .filter(|q| q.x > body.left() + 4.0)
            .all(|q| (q.y - want_y).abs() < 1.0),
        _ => false,
    });
    assert!(found, "a flat line at the bend lane's centre {want_y}");
    // a flat lane at zero reads +0 under the pointer too
    let pos = at(&h, bend, 960, 0.0);
    let env = h.state().core.roll.lane_env().unwrap();
    let text = h.state().core.roll.lane.readout(
        &env,
        pos - h.state().core.roll.lane.body.min,
        Default::default(),
        &h.state().core.project,
    );
    assert!(text.ends_with("+0"), "{text}");
}

fn shapes(h: &H) -> Vec<egui::Shape> {
    let mut out = Vec::new();
    for cs in &h.output().shapes {
        let mut stack = vec![&cs.shape];
        while let Some(s) = stack.pop() {
            match s {
                egui::Shape::Vec(v) => stack.extend(v.iter()),
                other => out.push(other.clone()),
            }
        }
    }
    out
}

#[test]
fn the_evaluated_curve_is_what_the_engine_plays() {
    // every curve kind: the lane's evaluation is the shared value_at, sent as the wire value
    let mk = |tick, value, curve, tension| CcPoint {
        id: ph_core::PointId(tick as u32 + 1),
        controller: MOD,
        tick,
        value,
        curve,
        tension,
    };
    let pts = vec![
        mk(0, 0.0, CurveKind::Smooth, 0.0),
        mk(480, 0.8, CurveKind::SCurve, 0.4),
        mk(960, 0.2, CurveKind::Linear, -0.5),
        mk(1440, 0.9, CurveKind::Hold, 0.0),
        mk(1920, 0.1, CurveKind::Linear, 0.0),
    ];
    let lane = ph_core::CcLane {
        controller: MOD,
        points: pts.clone(),
    };
    for t in (0..2400).step_by(37) {
        let want = lane.value_at(t as f64).map(|v| f64::from(MOD.wire(v)));
        assert_eq!(lanes::evaluate(MOD, &pts, t as f64), want, "tick {t}");
    }
    // the drawn polyline samples that evaluation (a state with those points, no window)
    let mut st = lanes::LaneState::default();
    st.kind = LaneKind::Controller(MOD);
    st.points = pts.clone();
    st.h = 100.0;
    let env = lanes::Env {
        view: ph_ui::piano_roll::View {
            x0: 0.0,
            px_per_tick: 0.25,
            y0: 84.0,
            key_h: 16.0,
        },
        snap: 240,
        bar: 3840,
        pattern_len: 3840,
        pattern: ph_core::PatternId(0),
        channel: ph_core::ChannelId(0),
    };
    let line = st.curve_line(&env, MOD, 600.0);
    assert!(line.len() > 100);
    for q in &line {
        let t = env.view.tick_at(q.x);
        let v = lanes::evaluate(MOD, &pts, t).unwrap();
        assert!((st.y_of(MOD, v) - q.y).abs() < 1e-3, "x {} tick {t}", q.x);
    }
    // the hold draws as a step: two samples a hair apart at tick 1920 - 480 differ in height
    let step: Vec<_> = line
        .windows(2)
        .filter(|w| w[1].x - w[0].x < 0.1 && (w[1].y - w[0].y).abs() > 5.0)
        .collect();
    assert!(
        !step.is_empty(),
        "the hold's jump is drawn as a vertical step"
    );
}

#[test]
fn the_lane_follows_the_grids_zoom_and_scroll() {
    let (mut h, ..) = start();
    add(&mut h, MOD, &[(960, 0.5), (1920, 0.8)]);
    show_lane(&mut h, MOD);
    let grid_x = |h: &H, tick: f64| {
        let r = &h.state().core.roll;
        r.last_grid.left() + r.view.x_of(tick)
    };
    assert!((at(&h, MOD, 960, 0.5).x - grid_x(&h, 960.0)).abs() < 0.01);
    // zoom in and scroll right through the roll's own view (what the wheel does)
    h.state_mut().core.roll.view.zoom_x(2.0, 300.0);
    h.state_mut().core.roll.view.x0 += 100.0;
    frames(&mut h, 2);
    for t in [960u64, 1920] {
        let x = at(&h, MOD, t, 0.5).x;
        assert!((x - grid_x(&h, t as f64)).abs() < 0.01, "tick {t}");
        // and a point is drawn there
        let dot = shapes(&h).iter().any(|s| match s {
            egui::Shape::Circle(c) => (c.center.x - x).abs() < 0.6 && c.radius == 4.0,
            _ => false,
        });
        let visible =
            x >= h.state().core.roll.lane.body.left() && x <= h.state().core.roll.lane.body.right();
        assert!(dot || !visible, "a point is drawn at x {x}");
    }
    // a click picks the point in the zoomed view
    let click_at = at(&h, MOD, 1920, 0.8);
    if click_at.x < h.state().core.roll.lane.body.right() {
        click(&mut h, click_at);
        assert_eq!(h.state().core.roll.lane.selection.len(), 1);
    }
}

#[test]
fn an_agents_edit_shows_up_in_the_open_lane_and_is_marked() {
    let (mut h, ..) = start();
    show_lane(&mut h, MOD);
    assert!(h.state().core.roll.lane.points.is_empty());
    let (p, ch) = {
        let c = &h.state().core;
        (c.sel.pattern.unwrap(), c.sel.channel.unwrap())
    };
    let agent = ph_commands::Caller::agent("lane-test", ph_commands::Grant::full());
    h.state()
        .core
        .studio
        .session
        .execute(
            &agent,
            ph_commands::Request::new(
                "cc.shape",
                serde_json::json!({"pattern": p, "channel": ch, "controller": "mod wheel",
                                   "start": 0, "end": 1920, "from": 0.3, "peak": 0.9, "to": 0.4}),
            ),
        )
        .expect("the agent's cc.shape");
    until(&mut h, "the agent's lane", |a| {
        a.core.roll.lane.points.len() == 3
    });
    // its points are in the lane, ringed as touched, and the tab appeared
    assert!(
        h.state()
            .core
            .touches
            .keys()
            .any(|k| matches!(k, ph_core::ObjectPath::CcPoint(..)))
    );
    frames(&mut h, 2);
    let ring = shapes(&h)
        .iter()
        .any(|s| matches!(s, egui::Shape::Circle(c) if c.radius == 9.0));
    assert!(ring, "the agent's points carry a ring");
    assert!(
        h.state()
            .core
            .roll
            .lane
            .tab_kinds()
            .contains(&LaneKind::Controller(MOD))
    );
    h.get_by_label("CC1");
}

#[test]
fn a_collapsed_lane_area_keeps_its_header_and_the_grid_grows() {
    let (mut h, ..) = start();
    let before = h.state().core.roll.last_grid.height();
    h.get_by_label("Lanes -").click();
    frames(&mut h, 3);
    assert!(!h.state().core.roll.lane.open);
    let after = h.state().core.roll.last_grid.height();
    assert!(
        after > before,
        "the grid took the room: {before} -> {after}"
    );
    h.get_by_label("Lanes +").click();
    frames(&mut h, 3);
    assert!(h.state().core.roll.lane.open);
}

/// Pictures of the lane area, only with `PH_SCREENSHOTS=1` and a GPU adapter.
#[test]
fn lane_snapshots() {
    if !std::env::var_os("PH_SCREENSHOTS").is_some_and(|v| v == "1") {
        return;
    }
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/reports/lane-editor");
    std::fs::create_dir_all(&dir).unwrap();
    let (mut h, ..) = start();
    add(
        &mut h,
        MOD,
        &[(0, 0.2), (960, 0.8), (1920, 0.4), (2880, 0.9), (3840, 0.3)],
    );
    add(
        &mut h,
        Controller::PitchBend,
        &[(0, 0.0), (960, 0.6), (1920, -0.5), (2880, 0.0)],
    );
    for (name, c) in [("mod", MOD), ("bend", Controller::PitchBend)] {
        show_lane(&mut h, c);
        frames(&mut h, 3);
        match h.render() {
            Ok(img) => img.save(dir.join(format!("lane_{name}.png"))).unwrap(),
            Err(e) => {
                eprintln!("SKIP: no GPU adapter ({e})");
                return;
            }
        }
    }
}

#[test]
fn a_stroke_drawn_right_to_left_is_ordered_and_written() {
    let (mut h, ..) = start();
    h.state_mut().core.roll.snap = 60;
    show_lane(&mut h, MOD);
    let mut path = Vec::new();
    for i in 0..=36 {
        let t = 1920 - i * 40;
        path.push(at(&h, MOD, t, 0.2 + 0.6 * i as f64 / 36.0));
    }
    stroke(&mut h, &path);
    until(&mut h, "the stroke", |a| {
        a.core.project.patterns.values().any(|p| p.cc_count() > 0)
    });
    let p = pts(&h, MOD);
    assert_eq!(p.len(), 2, "a straight stroke is two points either way");
    assert_eq!((p[0].tick, p[1].tick), (480, 1920));
    assert!(near(p[0].value, 0.8) && near(p[1].value, 0.2), "{p:?}");
    // drawing back over a stroke replaces what is under it, still in order
    let mut path = Vec::new();
    for t in [600u64, 900, 1200, 1500, 1200, 900, 700] {
        path.push(at(&h, MOD, t, 0.5));
    }
    stroke(&mut h, &path);
    until(&mut h, "the second stroke", |a| {
        let p = a.core.sel.pattern.unwrap();
        let ch = a.core.sel.channel.unwrap();
        a.core
            .project
            .pattern(p)
            .unwrap()
            .lane(ch, MOD)
            .points
            .iter()
            .any(|x| x.tick == 600)
    });
    let p = pts(&h, MOD);
    assert!(p.windows(2).all(|w| w[0].tick < w[1].tick), "{p:?}");
}

#[test]
fn a_still_click_on_a_point_selects_it_and_writes_nothing() {
    let (mut h, ..) = start();
    // 0.2 is not a whole CC step (25.4 / 127): a rewrite would change it
    add(&mut h, MOD, &[(0, 0.2), (960, 0.6)]);
    show_lane(&mut h, MOD);
    let ids: Vec<_> = pts(&h, MOD).iter().map(|p| p.id).collect();
    let sent = h.state().core.dispatch.sent;
    let p = at(&h, MOD, 960, 0.6);
    click(&mut h, p);
    assert_eq!(h.state().core.roll.lane.selection.len(), 1);
    assert_eq!(h.state().core.dispatch.sent, sent, "a click is not an edit");
    // pressing a tension diamond without dragging is not an edit either
    let handle = {
        let roll = &h.state().core.roll;
        let env = roll.lane_env().unwrap();
        roll.lane.body.min + roll.lane.handles(&env)[0].1
    };
    click(&mut h, handle + Vec2::new(2.0, 2.0));
    assert_eq!(h.state().core.dispatch.sent, sent);
    // the panel itself does not act on Delete: the keymap routes it (once) through the roll
    h.event(Event::Key {
        key: egui::Key::Delete,
        physical_key: Some(egui::Key::Delete),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    frames(&mut h, 2);
    assert_eq!(h.state().core.dispatch.sent, sent);
    let after: Vec<_> = pts(&h, MOD).iter().map(|p| p.id).collect();
    assert_eq!(after, ids, "same points, same ids");
}
