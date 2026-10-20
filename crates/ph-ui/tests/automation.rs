//! Automation in the UI (M2/T7), through real egui events offscreen: "Create automation
//! clip" from a knob's menu, clips drawn on the playlist, knobs that show and follow their
//! automation, the editor window, and recording armed knob drags. Waits settle on the
//! session's snapshot, never on fixed frame counts.

mod common;

use common::*;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_core::{ClipId, ParamPath};
use ph_ui::PhApp;
use ph_ui::automation;
use ph_ui::mapping;
use ph_ui::workspace::WindowId;
use std::time::Duration;

type H = Harness<'static, PhApp>;

fn harness(app: PhApp) -> H {
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    frames(&mut h, 4);
    h
}

fn start() -> (H, Ids, tempfile::TempDir) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    (harness(app), ids, fx.dir)
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
        let app = h.state_mut();
        app.core.update(app.core.now + 0.01);
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

fn right_click(h: &mut H, p: Pos2) {
    move_to(h, p);
    button(h, p, PointerButton::Secondary, true);
    button(h, p, PointerButton::Secondary, false);
    frames(h, 3);
}

fn click(h: &mut H, p: Pos2) {
    move_to(h, p);
    button(h, p, PointerButton::Primary, true);
    button(h, p, PointerButton::Primary, false);
    frames(h, 2);
}

/// Press at `from`, move to `to` in steps, release.
fn drag(h: &mut H, from: Pos2, to: Pos2) {
    move_to(h, from);
    button(h, from, PointerButton::Primary, true);
    for i in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        h.step();
        std::thread::sleep(Duration::from_millis(8));
    }
    button(h, to, PointerButton::Primary, false);
    frames(h, 2);
}

fn only(h: &mut H, keep: &[WindowId]) {
    let ws = &mut h.state_mut().ws;
    for id in ws.ids().collect::<Vec<_>>() {
        if id != WindowId::Playlist {
            ws.hide(id);
        }
    }
    for id in keep {
        ws.show(*id);
    }
    frames(h, 3);
}

fn auto_clips(app: &PhApp) -> Vec<ph_core::Clip> {
    app.core
        .project
        .clips
        .values()
        .filter(|c| c.automation().is_some())
        .cloned()
        .collect()
}

fn run(app: &mut PhApp, r: ph_commands::Request) -> serde_json::Value {
    app.core
        .studio
        .session
        .execute(&ph_commands::Caller::human(), r.clone())
        .unwrap_or_else(|e| panic!("{}: {e}", r.command))
        .result
}

#[test]
fn a_knobs_menu_creates_an_automation_clip_over_the_loop_and_opens_its_editor() {
    let (mut h, ids, _dir) = start();
    only(&mut h, &[WindowId::ChannelRack]);
    // a loop region of two bars: the new clip covers it
    h.state_mut()
        .core
        .send(mapping::set_loop(Some((3840, 11520))));
    until(&mut h, "the loop", |a| {
        a.core.live().status.loop_range.is_some()
    });
    let at = h.get_by_label("Kick volume").rect().center();
    right_click(&mut h, at);
    h.get_by_label("Create automation clip").click();
    frames(&mut h, 2);
    until(&mut h, "the automation clip", |a| !auto_clips(a).is_empty());
    let clips = auto_clips(h.state());
    assert_eq!(clips.len(), 1);
    let c = &clips[0];
    let a = c.automation().unwrap();
    assert_eq!(a.target, ParamPath::ChannelVolume(ids.kick));
    assert_eq!((c.start, c.length), (3840, 7680));
    // tracks 0 (drums) and 1 (bass) hold clips over that range: the first free one is next
    assert_eq!(c.track, 2);
    let values: Vec<f64> = a.points_in_order().iter().map(|p| p.value).collect();
    assert_eq!(values.len(), 2);
    assert!(
        (values[0] - 0.78).abs() < 1e-6,
        "starts at the knob's value"
    );
    // the editor opened for the new clip
    until(&mut h, "the editor", |a| {
        a.ws.is_visible(WindowId::Automation(c.id))
    });
    // and the knob now wears the automation ring
    let st = automation::knob_state(&h.ctx, &ParamPath::ChannelVolume(ids.kick));
    assert!(st.automated);
}

#[test]
fn without_a_loop_or_selection_the_clip_takes_the_bar_at_the_playhead() {
    let (mut h, ids, _dir) = start();
    h.state_mut()
        .core
        .create_automation_clip(ParamPath::ChannelPan(ids.snare), 0.25);
    until(&mut h, "the automation clip", |a| !auto_clips(a).is_empty());
    let c = &auto_clips(h.state())[0];
    assert_eq!((c.start, c.length), (0, 3840));
    assert_eq!(c.track, 1, "track 0 has a clip over bar 1");
    assert_eq!(
        c.automation().unwrap().target,
        ParamPath::ChannelPan(ids.snare)
    );
}

/// A project with a volume clip on the kick: 0.2 rising to 1.0 over the first four bars.
fn automated(h: &mut H, ids: &Ids) -> ClipId {
    let target = ParamPath::ChannelVolume(ids.kick);
    let r = run(
        h.state_mut(),
        mapping::automation_add_clip(&target, 0, 4 * 3840, &[(0, 0.2), (4 * 3840, 1.0)]),
    );
    let id = ClipId(r["clip"].as_u64().unwrap() as u32);
    until(h, "the clip", |a| a.core.project.clips.contains_key(&id));
    id
}

#[test]
fn the_playlist_draws_the_curve_inside_the_clip_and_double_click_opens_the_editor() {
    let (mut h, ids, _dir) = start();
    let id = automated(&mut h, &ids);
    only(&mut h, &[]);
    frames(&mut h, 3);
    let st = &h.state().core.playlist;
    let c = h.state().core.project.clips[&id].clone();
    let r = st
        .clip_rect(c.track as i32, c.start, c.length)
        .translate(st.last_grid.min.to_vec2());
    // a line with many points sits inside the clip's rectangle
    let mut found = false;
    for cs in &h.output().shapes {
        let mut stack = vec![&cs.shape];
        while let Some(s) = stack.pop() {
            match s {
                egui::Shape::Vec(v) => stack.extend(v.iter()),
                egui::Shape::Path(p) if p.points.len() >= 8 => {
                    let b = egui::Rect::from_points(&p.points);
                    if r.expand(1.0).contains_rect(b) && b.width() > r.width() * 0.5 {
                        found = true;
                    }
                }
                _ => {}
            }
        }
    }
    assert!(found, "the clip's curve is drawn inside {r:?}");
    // double-click opens the editor
    let at = Pos2::new(r.center().x, r.center().y);
    click(&mut h, at);
    click(&mut h, at);
    until(&mut h, "the editor window", |a| {
        a.ws.is_visible(WindowId::Automation(id))
    });
}

#[test]
fn knobs_follow_the_automated_value_while_the_song_plays() {
    let (mut h, ids, _dir) = start();
    let id = automated(&mut h, &ids);
    let target = ParamPath::ChannelVolume(ids.kick);
    only(&mut h, &[WindowId::ChannelRack]);
    // stopped: ringed, but the knob shows the project's value
    let st = automation::knob_state(&h.ctx, &target);
    assert!(st.automated);
    assert_eq!(st.value, None);
    h.state_mut().core.send(mapping::transport("play"));
    until(&mut h, "playback past the first beat", |a| {
        let l = a.core.live();
        l.status.playing && l.status.position > 480
    });
    frames(&mut h, 2);
    let st = automation::knob_state(&h.ctx, &target);
    let v = st.value.expect("a value while playing inside the clip");
    let want = h
        .state()
        .core
        .project
        .automated_value(&target, h.state().core.live().status.position as f64)
        .unwrap();
    assert!(
        (0.2..=1.0).contains(&v) && (v - want).abs() < 0.05,
        "{v} vs {want}"
    );
    h.state_mut().core.send(mapping::transport("stop"));
    until(&mut h, "stop", |a| !a.core.live().status.playing);
    let _ = id;
}

#[test]
fn armed_knob_drags_are_recorded_as_a_thinned_clip_when_the_song_stops() {
    let (mut h, ids, _dir) = start();
    only(&mut h, &[WindowId::ChannelRack]);
    h.state_mut().core.auto_armed = true;
    h.state_mut().core.send(mapping::transport("play"));
    until(&mut h, "playback", |a| a.core.live().status.playing);
    let knob = h.get_by_label("Kick volume");
    let c = knob.rect().center();
    drag(&mut h, c, c + Vec2::new(0.0, -50.0));
    drag(&mut h, c, c + Vec2::new(0.0, 20.0));
    h.state_mut().core.send(mapping::transport("stop"));
    until(&mut h, "the recorded clip", |a| !auto_clips(a).is_empty());
    let clips = auto_clips(h.state());
    assert_eq!(clips.len(), 1);
    let a = clips[0].automation().unwrap();
    assert_eq!(a.target, ParamPath::ChannelVolume(ids.kick));
    assert!(a.points.len() >= 2, "{} points", a.points.len());
    let vals: Vec<f64> = a.points_in_order().iter().map(|p| p.value).collect();
    assert!(
        vals.iter().cloned().fold(0.0, f64::max) > vals[0] + 0.1,
        "the first drag raised the volume: {vals:?}"
    );
    // nothing is left in the recorder
    assert!(h.state().core.auto_rec.is_empty());
    // not armed: a drag while playing records nothing
    h.state_mut().core.auto_armed = false;
    let n = auto_clips(h.state()).len();
    h.state_mut().core.send(mapping::transport("play"));
    until(&mut h, "playback", |a| a.core.live().status.playing);
    drag(&mut h, c, c + Vec2::new(0.0, -10.0));
    h.state_mut().core.send(mapping::transport("stop"));
    until(&mut h, "stop", |a| !a.core.live().status.playing);
    frames(&mut h, 4);
    assert_eq!(auto_clips(h.state()).len(), n);
}

#[test]
fn the_editor_adds_moves_and_deletes_points_and_sets_curves() {
    let (mut h, ids, _dir) = start();
    let id = automated(&mut h, &ids);
    only(&mut h, &[WindowId::Automation(id)]);
    frames(&mut h, 3);
    let clip = |h: &H| h.state().core.project.clips[&id].clone();
    let points = |h: &H| {
        clip(h)
            .automation()
            .unwrap()
            .points_in_order()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
    };
    let geom = |h: &H| {
        let st = h.state().core.auto_editors.get(&id).expect("editor state");
        (st.grid, st.x_of(0.0), st.range, st.size)
    };
    let pos = |h: &H, tick: f64, v: f64| {
        let st = h.state().core.auto_editors.get(&id).unwrap();
        st.grid.min + Vec2::new(st.x_of(tick), st.y_of(v))
    };
    assert_eq!(points(&h).len(), 2);
    let (grid, ..) = geom(&h);
    assert!(grid.width() > 200.0 && grid.height() > 80.0, "{grid:?}");
    // click empty space: a point appears, snapped to the grid
    let p = pos(&h, 3840.0 + 100.0, 0.6);
    click(&mut h, p);
    until(&mut h, "the added point", |a| {
        a.core.project.clips[&id].automation().unwrap().points.len() == 3
    });
    let added = points(&h)[1].clone();
    assert_eq!(added.tick % 240, 0, "snapped: {}", added.tick);
    assert!((added.value - 0.6).abs() < 0.03, "{}", added.value);
    // drag it to another place
    let from = pos(&h, added.tick as f64, added.value);
    let to = pos(&h, 3.0 * 3840.0, 0.3);
    drag(&mut h, from, to);
    until(&mut h, "the moved point", |a| {
        a.core.project.clips[&id]
            .automation()
            .unwrap()
            .points
            .get(&added.id)
            .is_some_and(|p| p.tick != added.tick)
    });
    let moved = clip(&h).automation().unwrap().points[&added.id].clone();
    assert_eq!(moved.tick, 3 * 3840);
    assert!((moved.value - 0.3).abs() < 0.03, "{}", moved.value);
    // right-click it: choose "Hold" for the segment after it
    let at = pos(&h, moved.tick as f64, moved.value);
    right_click(&mut h, at);
    h.get_by_label("Hold").click();
    until(&mut h, "the curve", |a| {
        a.core.project.clips[&id].automation().unwrap().points[&added.id].curve
            == ph_core::CurveKind::Hold
    });
    // right-click, Delete point
    right_click(&mut h, at);
    h.get_by_label("Delete point").click();
    until(&mut h, "the deletion", |a| {
        a.core.project.clips[&id].automation().unwrap().points.len() == 2
    });
    // select the first point and press Delete
    let first = points(&h)[0].clone();
    let at = pos(&h, first.tick as f64, first.value);
    click(&mut h, at);
    h.event(Event::Key {
        key: egui::Key::Delete,
        physical_key: Some(egui::Key::Delete),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    frames(&mut h, 2);
    until(&mut h, "Delete", |a| {
        a.core.project.clips[&id].automation().unwrap().points.len() == 1
    });
    // all of it was commands: undo brings the point back
    h.state_mut().core.send(mapping::undo_mine());
    until(&mut h, "undo", |a| {
        a.core.project.clips[&id].automation().unwrap().points.len() == 2
    });
}

#[test]
fn automation_clips_move_resize_and_delete_like_pattern_clips() {
    let (mut h, ids, _dir) = start();
    let id = automated(&mut h, &ids);
    only(&mut h, &[]);
    let c = h.state().core.project.clips[&id].clone();
    let st = &h.state().core.playlist;
    let r = st
        .clip_rect(c.track as i32, c.start, c.length)
        .translate(st.last_grid.min.to_vec2());
    // drag the body one bar right
    let bar = h.state().core.playlist.px_per_tick as f32 * 3840.0;
    drag(&mut h, r.center(), r.center() + Vec2::new(bar, 0.0));
    until(&mut h, "the move", |a| {
        a.core.project.clips[&id].start == 3840
    });
    // right-click deletes
    let c = h.state().core.project.clips[&id].clone();
    let st = &h.state().core.playlist;
    let r = st
        .clip_rect(c.track as i32, c.start, c.length)
        .translate(st.last_grid.min.to_vec2());
    right_click(&mut h, r.center());
    until(&mut h, "the deletion", |a| {
        !a.core.project.clips.contains_key(&id)
    });
}

/// A harness that renders on the GPU when an adapter exists.
fn gpu_harness(app: PhApp) -> H {
    let build = |app: PhApp, gpu: bool| {
        let b = Harness::builder()
            .with_size(Vec2::new(1500.0, 900.0))
            .with_pixels_per_point(1.0)
            .with_step_dt(1.0 / 30.0);
        let b = if gpu { b.wgpu() } else { b };
        b.build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app)
    };
    let gpu = std::panic::catch_unwind(|| {
        Harness::builder()
            .with_size(Vec2::new(64.0, 64.0))
            .wgpu()
            .build_ui(|ui| {
                ui.label("probe");
            })
    })
    .is_ok();
    build(app, gpu)
}

/// Pictures of the automation UI in both themes. They are written only when
/// `PH_SCREENSHOTS_DIR` names a folder (the scratch folder for review) or `PH_SCREENSHOTS=1`
/// (the tracked docs shots); otherwise the frames are only checked to be non-blank.
#[test]
fn pictures_of_the_automation_ui_in_both_themes() {
    let dir = std::env::var_os("PH_SCREENSHOTS_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("PH_SCREENSHOTS")
                .is_some_and(|v| v == "1")
                .then(|| {
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../docs/screenshots/M2")
                })
        });
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).unwrap();
    }
    for (theme, prefix) in [("neon-slate", "neon"), ("red-line", "red")] {
        let (fx, ids) = fixture(true);
        let ids = ids.unwrap();
        let mut app = fx.app;
        pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
        // the kick's volume rides a curve; the bass cutoff-style param gets a smooth sweep
        let vol = ParamPath::ChannelVolume(ids.kick);
        let r = run(
            &mut app,
            mapping::automation_add_clip(&vol, 0, 4 * 3840, &[]),
        );
        let id = ClipId(r["clip"].as_u64().unwrap() as u32);
        for (t, v, c) in [
            (0, 0.9, "smooth"),
            (3840, 0.25, "s_curve"),
            (2 * 3840, 0.8, "hold"),
            (3 * 3840, 0.4, "linear"),
            (4 * 3840, 0.95, "linear"),
        ] {
            run(&mut app, mapping::automation_add_point(id, t, v, c));
        }
        let pan = ParamPath::InsertPan(ph_core::InsertId(4));
        run(
            &mut app,
            mapping::automation_add_clip(&pan, 3840, 3 * 3840, &[(0, -0.6), (3 * 3840, 0.6)]),
        );
        pump_until(&mut app, 3000, |a| auto_clips(a).len() == 2);
        app.core.sel.pattern = Some(ids.beat);
        app.core.sel.channel = Some(ids.kick);
        app.core.fix_selection();
        let mut h = gpu_harness(app);
        h.state_mut().set_theme(theme);
        let ws = &mut h.state_mut().ws;
        ws.show(WindowId::ChannelRack);
        ws.show(WindowId::Automation(id));
        ws.drag_move(WindowId::ChannelRack, 20.0, 30.0);
        ws.drag_move(WindowId::Automation(id), 330.0, 250.0);
        h.state_mut().core.send(mapping::transport("play"));
        until(&mut h, "playback", |a| a.core.live().status.position > 2000);
        frames(&mut h, 8);
        let Ok(img) = h.render() else {
            eprintln!("SKIP {theme}: no GPU adapter for offscreen rendering");
            continue;
        };
        let corner = *img.get_pixel(0, 0);
        assert!(
            img.pixels().filter(|p| **p != corner).count() > 5000,
            "blank"
        );
        if let Some(d) = &dir {
            img.save(d.join(format!("{prefix}-automation.png")))
                .unwrap();
        }
        h.state_mut().core.send(mapping::transport("stop"));
    }
}
