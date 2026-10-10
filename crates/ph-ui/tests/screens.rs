//! Offscreen rendering (egui_kittest on wgpu, no window): screenshots for the docs, the key
//! handling through real egui events, and frame-cost measurements.

mod common;

use common::*;
use egui::Key;
use egui_kittest::Harness;
use ph_ui::core::PanelId;
use ph_ui::{PhApp, demo_agent, mapping};
use std::path::PathBuf;
use std::time::{Duration, Instant};

type H = Harness<'static, PhApp>;

fn harness(app: PhApp, w: f32, h: f32) -> H {
    Harness::builder()
        .with_size(egui::vec2(w, h))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app)
}

fn shots_dir() -> Option<PathBuf> {
    std::env::var_os("PH_SCREENSHOTS").map(|_| {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/M1");
        std::fs::create_dir_all(&d).unwrap();
        d
    })
}

fn shot(h: &mut H, name: &str) {
    // let replies and animations land, then render offscreen
    for _ in 0..6 {
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
    let img = h.render().expect("offscreen render");
    assert!(img.width() > 100 && img.height() > 100);
    // not blank: plenty of pixels differ from the corner colour
    let corner = *img.get_pixel(0, 0);
    assert!(
        img.pixels().filter(|p| **p != corner).count() > 5000,
        "{name} rendered blank"
    );
    if let Some(dir) = shots_dir() {
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn focus(h: &mut H, p: PanelId) {
    h.state_mut().open_panel(p);
    h.step();
}

#[test]
fn screenshots_of_the_beat() {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let _keep = fx.dir;
    let mut app = fx.app;
    pump_until(&mut app, 500, |a| a.core.project.channels.len() == 4);
    app.core.sel.channel = Some(ids.kick);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.insert = 4;
    app.core.sel.effect = Some(1);
    app.core.fix_selection();
    let mut h = harness(app, 1600.0, 1000.0);

    // the default layout: browser, rack, playlist, mixer
    shot(&mut h, "01-overview-dark");
    focus(&mut h, PanelId::PianoRoll);
    // an agent edits the kick while we look: its notes flash with an author badge
    for k in 0..3 {
        demo_agent::step(&h.state().core.studio.session, k);
    }
    shot(&mut h, "02-piano-roll-agent-flash");
    h.state_mut().core.sel.channel = Some(ids.bass);
    h.state_mut().core.sel.pattern = Some(ids.bassline);
    h.state_mut().core.fix_selection();
    shot(&mut h, "02b-piano-roll-bass");
    focus(&mut h, PanelId::Inspector);
    h.state_mut().core.sel.channel = Some(ids.bass);
    shot(&mut h, "03-instrument-synth");
    h.state_mut().core.sel.channel = Some(ids.kick);
    shot(&mut h, "04-instrument-sampler-waveform");
    focus(&mut h, PanelId::History);
    shot(&mut h, "05-history");
    focus(&mut h, PanelId::Agents);
    shot(&mut h, "06-agents");
    focus(&mut h, PanelId::Playlist);
    shot(&mut h, "07-playlist");
    focus(&mut h, PanelId::Mixer);
    shot(&mut h, "08-mixer-effects");

    // conflict dialog on top of the history: the human edits the agent's newest note, then
    // asks to undo the agent
    focus(&mut h, PanelId::History);
    let app = h.state_mut();
    let pat = app.core.project.pattern_order[0];
    let ch = app.core.project.channel_order[0];
    let agent_note = app
        .core
        .project
        .pattern(pat)
        .unwrap()
        .notes
        .get(&ch)
        .and_then(|m| m.values().find(|n| n.key == 48))
        .map(|n| n.id)
        .expect("the agent's note is in the first pattern");
    app.core
        .send(mapping::note_velocity(pat, ch, &[agent_note], 0.3));
    settle(app);
    app.core.send(mapping::undo_author("agent:demo", "stop"));
    settle(app);
    assert!(
        app.core.conflict.is_some(),
        "the conflict dialog should be open"
    );
    shot(&mut h, "09-conflict-dialog");

    // light theme
    h.state_mut().core.conflict = None;
    h.state_mut().set_theme("light");
    focus(&mut h, PanelId::PianoRoll);
    shot(&mut h, "10-light-theme");
}

fn wait(h: &mut H, f: &dyn Fn(&PhApp) -> bool) -> bool {
    for _ in 0..80 {
        h.step();
        if f(h.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn docked(h: &H, p: PanelId) -> bool {
    h.state().dock.iter_all_tabs().any(|(_, t)| *t == p)
}

#[test]
fn keys_drive_commands_through_real_egui_events() {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let _keep = fx.dir;
    let mut app = fx.app;
    pump_until(&mut app, 500, |a| a.core.project.channels.len() == 4);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    h.step();

    // Space -> transport.toggle
    h.key_press(Key::Space);
    assert!(
        wait(&mut h, &|a| a.core.live().status.playing),
        "space starts playback"
    );
    h.key_press(Key::Space);
    assert!(
        wait(&mut h, &|a| !a.core.live().status.playing),
        "space stops it"
    );

    // F7: the piano roll starts as a background tab, so the first press brings it to the front
    // and the second hides it; the third shows it again.
    assert!(docked(&h, PanelId::PianoRoll));
    h.key_press(Key::F7);
    h.step();
    h.step();
    assert!(docked(&h, PanelId::PianoRoll));
    h.key_press(Key::F7);
    h.step();
    h.step();
    assert!(
        !docked(&h, PanelId::PianoRoll),
        "second F7 hides the front piano roll"
    );
    h.key_press(Key::F7);
    h.step();
    h.step();
    assert!(docked(&h, PanelId::PianoRoll));

    // Ctrl+Z reverts the human's last change, Ctrl+Y brings it back
    let before = h.state().core.project.settings.bpm;
    h.state_mut().core.send(mapping::set_tempo(150.0));
    assert!(wait(&mut h, &|a| a.core.project.settings.bpm == 150.0));
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    assert!(
        wait(&mut h, &|a| a.core.project.settings.bpm == before),
        "ctrl+z undoes the tempo change"
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    assert!(
        wait(&mut h, &|a| a.core.project.settings.bpm == 150.0),
        "ctrl+y redoes it"
    );

    // typing keyboard: Z plays a note on the selected channel (no panic, no command sent)
    let sent = h.state().core.dispatch.sent;
    h.key_press(Key::Z);
    h.step();
    assert_eq!(
        h.state().core.dispatch.sent,
        sent,
        "playing a note is not an edit"
    );
}

#[test]
fn big_project_stays_interactive() {
    let dir = tempfile::tempdir().unwrap();
    let t0 = Instant::now();
    let project = big_project();
    println!(
        "PERF built 100 channels / 50 patterns / 10k notes in {:.0} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    let st = studio(dir.path(), project);
    let ctx = egui::Context::default();
    let t = Instant::now();
    let app = PhApp::new(
        &ctx,
        st,
        ph_ui::AppPaths {
            project_dir: dir.path().into(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            config_dir: None,
        },
        ph_ui::Hooks::default(),
        ph_engine::EngineConfig::default(),
        None,
    )
    .unwrap();
    println!(
        "PERF app open: {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(app.core.project.channels.len(), 100);
    assert_eq!(app.core.project.patterns.len(), 50);
    let mut h = harness(app, 1600.0, 1000.0);
    h.state_mut().core.sel.pattern = Some(ph_core::PatternId(1));
    h.state_mut().core.fix_selection();
    for _ in 0..5 {
        h.step();
    }
    let report = |h: &mut H, label: &str| {
        let mut times = Vec::new();
        for _ in 0..40 {
            let t = Instant::now();
            h.step();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        println!(
            "PERF {label}: avg {avg:.2} ms, p95 {:.2} ms, max {:.2} ms (CPU: layout + paint)",
            times[37], times[39]
        );
        avg
    };
    let a = report(&mut h, "100 channels, 50 patterns, default layout");
    focus(&mut h, PanelId::PianoRoll);
    h.state_mut().core.sel.pattern = Some(ph_core::PatternId(0));
    h.state_mut().core.sel.channel = Some(ph_core::ChannelId(3));
    h.state_mut().core.fix_selection();
    let b = report(&mut h, "10 000-note piano roll in the full layout");
    let t = Instant::now();
    let img = h.render().expect("render");
    println!(
        "PERF offscreen wgpu render of the full 1600x1000 window: {:.1} ms ({}x{})",
        t.elapsed().as_secs_f64() * 1000.0,
        img.width(),
        img.height()
    );
    let t = Instant::now();
    let _ = h.render().expect("render");
    println!(
        "PERF second offscreen wgpu render: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    // generous in debug test builds; the release numbers are what the report quotes
    let budget = if cfg!(debug_assertions) { 120.0 } else { 16.0 };
    assert!(
        a < budget && b < budget,
        "frames too slow: {a:.1} / {b:.1} ms"
    );
}
