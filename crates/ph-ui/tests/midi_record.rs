//! MIDI learn, mapped controllers and recording in the UI (M2/T8), through real egui events
//! offscreen. The audio inputs are simulated and the MIDI source is a fake: no test opens a
//! device. Waits settle on the session's snapshot (and the engine's own state), never on
//! fixed frame counts.

mod common;

use common::*;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_core::ParamPath;
use ph_engine::midi::FakeMidi;
use ph_engine::record::SimulatedInputs;
use ph_ui::PhApp;
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

fn start() -> (H, Ids, tempfile::TempDir, FakeMidi) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    let fake = FakeMidi::new();
    fake.add_port("Scripted Keys");
    app.core.studio.midi.set_backend(Box::new(fake.clone()));
    (harness(app), ids, fx.dir, fake)
}

fn frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(6));
    }
}

/// Steps until `done` holds (and every command has been answered).
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

fn click_at(h: &mut H, p: Pos2, button: PointerButton) {
    move_to(h, p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    frames(h, 3);
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

fn run(app: &mut PhApp, r: ph_commands::Request) -> serde_json::Value {
    app.core
        .studio
        .session
        .execute(&ph_commands::Caller::human(), r.clone())
        .unwrap_or_else(|e| panic!("{}: {e}", r.command))
        .result
}

fn volume(app: &PhApp, id: ph_core::ChannelId) -> f32 {
    app.core.project.channels[&id].volume
}

#[test]
fn midi_learn_from_a_knobs_menu_maps_the_next_controller_and_it_moves_the_knob() {
    let (mut h, ids, _dir, fake) = start();
    only(&mut h, &[WindowId::ChannelRack]);
    until(&mut h, "the port to connect", |_| {
        fake.connected() == ["Scripted Keys"]
    });
    // right-click the kick's volume: MIDI learn is on the menu and enabled
    let at = h.get_by_label("Kick volume").rect().center();
    click_at(&mut h, at, PointerButton::Secondary);
    h.get_by_label("MIDI learn…").click();
    frames(&mut h, 3);
    let target = ParamPath::ChannelVolume(ids.kick);
    until(&mut h, "learning to start", |a| {
        a.core.studio.midi.learning() == Some(target.clone())
    });
    // a controller arrives and is mapped, with a note to the person
    assert!(fake.send("Scripted Keys", &[0xb0, 21, 10]));
    until(&mut h, "the mapping", |a| a.core.studio.midi.mappings().len() == 1);
    let m = &h.state().core.studio.midi.mappings()[0];
    assert_eq!((m.controller, m.target.clone()), (21, target.clone()));
    assert_eq!(h.state().core.studio.midi.learning(), None);
    until(&mut h, "the toast", |a| {
        a.core.toasts.iter().any(|t| t.text.contains("CC 21"))
    });
    // moving it moves the knob: the value arrives as an ordinary edit
    fake.send("Scripted Keys", &[0xb0, 21, 127]);
    until(&mut h, "volume 1.0", |a| (volume(a, ids.kick) - 1.0).abs() < 1e-6);
    fake.send("Scripted Keys", &[0xb0, 21, 0]);
    until(&mut h, "volume 0", |a| volume(a, ids.kick).abs() < 1e-6);
    // the same move is undoable like a knob drag
    run(h.state_mut(), ph_ui::mapping::undo_mine());
    until(&mut h, "undo", |a| (volume(a, ids.kick) - 1.0).abs() < 1e-6);
    // the menu now offers to remove the mapping
    let at = h.get_by_label("Kick volume").rect().center();
    click_at(&mut h, at, PointerButton::Secondary);
    assert!(h.query_by_label("Remove MIDI mapping (CC 21)").is_some());
    h.get_by_label("Remove MIDI mapping (CC 21)").click();
    frames(&mut h, 3);
    until(&mut h, "the mapping to go", |a| a.core.studio.midi.mappings().is_empty());
}

#[test]
fn cancelling_learn_and_unmappable_controls() {
    let (mut h, ids, _dir, _fake) = start();
    only(&mut h, &[WindowId::ChannelRack]);
    let at = h.get_by_label("Kick volume").rect().center();
    click_at(&mut h, at, PointerButton::Secondary);
    h.get_by_label("MIDI learn…").click();
    frames(&mut h, 3);
    let target = ParamPath::ChannelVolume(ids.kick);
    until(&mut h, "learning", |a| a.core.studio.midi.learning() == Some(target.clone()));
    // while learning, the same menu offers to cancel
    let at = h.get_by_label("Kick volume").rect().center();
    click_at(&mut h, at, PointerButton::Secondary);
    h.get_by_label("Cancel MIDI learn").click();
    frames(&mut h, 3);
    until(&mut h, "learning to end", |a| a.core.studio.midi.learning().is_none());
}

#[test]
fn the_keyboard_plays_the_selected_channel() {
    let (mut h, ids, _dir, fake) = start();
    until(&mut h, "the port to connect", |_| {
        fake.connected() == ["Scripted Keys"]
    });
    until(&mut h, "the target", |a| {
        a.core.studio.midi.target_channel() == Some(ids.kick)
    });
    // selecting another channel moves the keyboard to it
    h.state_mut().core.sel.channel = Some(ids.bass);
    until(&mut h, "the target to follow", |a| {
        a.core.studio.midi.target_channel() == Some(ids.bass)
    });
    // a note on the bass sounds: the engine's meters move while it is held
    fake.send("Scripted Keys", &[0x90, 40, 120]);
    let mut loud = false;
    let t0 = std::time::Instant::now();
    while t0.elapsed() < Duration::from_secs(5) && !loud {
        h.step();
        loud = h
            .state()
            .core
            .studio
            .engine
            .meters()
            .peak
            .iter()
            .any(|p| p[0] > 0.01 || p[1] > 0.01);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(loud, "the held note reaches the mixer");
    fake.send("Scripted Keys", &[0x80, 40, 0]);
}

#[test]
fn armed_automation_recording_writes_mapped_controller_moves_as_a_clip() {
    let (mut h, ids, _dir, fake) = start();
    until(&mut h, "the port to connect", |_| {
        fake.connected() == ["Scripted Keys"]
    });
    run(
        h.state_mut(),
        ph_commands::Request::new(
            "midi.map",
            serde_json::json!({ "controller": 7, "target": format!("channel/{}/volume", ids.kick.0) }),
        ),
    );
    let before = h.state().core.project.clips.len();
    h.state_mut().core.auto_armed = true;
    run(h.state_mut(), ph_ui::mapping::transport("play"));
    until(&mut h, "the song to play", |a| a.core.live().status.playing);
    for (i, v) in [20u8, 50, 90, 120].iter().enumerate() {
        fake.send("Scripted Keys", &[0xb0, 7, *v]);
        let want = *v as f32 / 127.0;
        until(&mut h, &format!("move {i}"), |a| {
            (volume(a, ids.kick) - want).abs() < 1e-3
        });
        std::thread::sleep(Duration::from_millis(60));
    }
    run(h.state_mut(), ph_ui::mapping::transport("stop"));
    until(&mut h, "the recorded automation clip", |a| {
        a.core.project.clips.len() == before + 1
    });
    let clip = h
        .state()
        .core
        .project
        .clips
        .values()
        .find_map(|c| c.automation().cloned())
        .expect("an automation clip");
    assert_eq!(clip.target, ParamPath::ChannelVolume(ids.kick));
    assert!(clip.points.len() >= 2, "{} points", clip.points.len());
}

#[test]
fn the_record_button_arms_records_and_places_a_take_through_commands() {
    let (mut h, _ids, dir, _fake) = start();
    // an input to record: a ramp, simulated
    let ramp: Vec<f32> = (0..8 * 48_000).map(|i| (i % 500) as f32 / 500.0).collect();
    h.state_mut()
        .core
        .studio
        .engine
        .set_input_provider(Some(Box::new(SimulatedInputs::new().with(
            "Scripted: In 1-2",
            48_000,
            vec![ramp.clone(), ramp],
            128,
        ))));
    // open the inputs popup: the input is listed
    h.get_by_label("IN").click();
    frames(&mut h, 3);
    until(&mut h, "the input list", |a| !a.core.rec.inputs.is_empty());
    frames(&mut h, 3);
    assert!(h.query_by_label_contains("Scripted: In 1-2").is_some());
    // arm it: a command, and the list follows
    h.get_by_label_contains("Scripted: In 1-2").click();
    until(&mut h, "armed", |a| a.core.rec.inputs.iter().any(|i| i.armed));
    assert_eq!(
        h.state().core.studio.engine.armed_inputs().len(),
        1,
        "the engine has it armed"
    );
    // REC starts, the song plays; Stop ends it and the take is in the project
    let clips = h.state().core.project.clips.len();
    h.get_by_label("REC").click();
    until(&mut h, "recording", |a| a.core.live().recording);
    until(&mut h, "the song to move", |a| a.core.live().status.position > 480);
    // closing the popup first: Stop is in the transport row
    h.event(Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    frames(&mut h, 2);
    let stop = h.get_by_label("Stop").rect().center();
    click_at(&mut h, stop, PointerButton::Primary);
    until(&mut h, "the take", |a| {
        a.core.project.clips.len() == clips + 1 && !a.core.rec.active
    });
    let app = h.state();
    let (clip, audio) = app
        .core
        .project
        .clips
        .values()
        .find_map(|c| c.audio().map(|a| (c, a)))
        .expect("an audio clip");
    assert_eq!(audio.source_offset, 128, "the clip is moved by the latency");
    let entry = app.core.project.sample(audio.sample).unwrap();
    assert!(entry.path.starts_with("samples/Take 1"), "{}", entry.path);
    assert!(dir.path().join(&entry.path).exists(), "the take is on disk");
    assert!(clip.length > 0);
    assert!(!app.core.live().recording);
    assert!(
        app.core.toasts.iter().any(|t| t.text.contains("recorded 1 take")),
        "{:?}",
        app.core.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
}

#[test]
fn recording_with_nothing_armed_says_so_and_does_not_stick() {
    let (mut h, _ids, _dir, _fake) = start();
    // song mode: nothing to record
    h.get_by_label("REC").click();
    until(&mut h, "the explanation", |a| {
        a.core.toasts.iter().any(|t| t.text.contains("nothing to record"))
    });
    assert!(!h.state().core.rec.active, "a failed start leaves the button off");
    assert!(!h.state().core.live().recording);
}
