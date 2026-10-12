//! Offscreen rendering (egui_kittest on wgpu, no window): screenshots for the docs, the key
//! handling through real egui events, and frame-cost measurements.

mod common;

use common::*;
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_core::InsertId;
use ph_ui::core::PanelId;
use ph_ui::workspace::WindowId;
use ph_ui::{PhApp, demo_agent, mapping};
use std::path::PathBuf;
use std::time::{Duration, Instant};

type H = Harness<'static, PhApp>;

/// A harness that renders on the GPU when an adapter exists (a CI box without Vulkan still runs
/// the logic, it just cannot take pictures).
fn harness_ppp(app: PhApp, w: f32, h: f32, ppp: f32) -> H {
    harness_dt(app, w, h, ppp, 0.25)
}

/// As [`harness_ppp`] with the time each step advances (a clock-driven screen needs it small).
fn harness_dt(app: PhApp, w: f32, h: f32, ppp: f32, dt: f32) -> H {
    let build = |app: PhApp, gpu: bool| {
        let b = Harness::builder()
            .with_size(egui::vec2(w, h))
            .with_step_dt(dt)
            .with_pixels_per_point(ppp);
        let b = if gpu { b.wgpu() } else { b };
        b.build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app)
    };
    // Building the GPU renderer panics when there is no adapter; the app is moved in, so probe
    // with a throw-away harness first.
    let gpu = std::panic::catch_unwind(|| {
        Harness::builder()
            .with_size(egui::vec2(64.0, 64.0))
            .wgpu()
            .build_ui(|ui| {
                ui.label("probe");
            })
    })
    .is_ok();
    build(app, gpu)
}

/// Offscreen wgpu setup that picks the discrete GPU (kittest's own default prefers a software
/// rasteriser so snapshots are reproducible), for timing the real renderer.
fn gpu_setup() -> eframe::egui_wgpu::WgpuSetup {
    use eframe::egui_wgpu::{self, wgpu};
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup
        .instance_descriptor
        .backends
        .remove(wgpu::Backends::BROWSER_WEBGPU);
    setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, _| {
        let pick = adapters
            .iter()
            .find(|a| a.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
            .or_else(|| adapters.first())
            .ok_or_else(|| "no adapter".to_string())?;
        println!("PERF gpu adapter: {:?}", pick.get_info().name);
        Ok(pick.clone())
    }));
    egui_wgpu::WgpuSetup::CreateNew(setup)
}

fn harness_gpu(app: PhApp, w: f32, h: f32) -> H {
    Harness::builder()
        .with_size(egui::vec2(w, h))
        .with_pixels_per_point(1.0)
        .wgpu_setup(gpu_setup())
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app)
}

fn harness(app: PhApp, w: f32, h: f32) -> H {
    harness_ppp(app, w, h, 1.0)
}

fn shots_dir() -> Option<PathBuf> {
    // only PH_SCREENSHOTS=1 writes the tracked PNGs: a plain `cargo test` leaves the tree clean
    (std::env::var_os("PH_SCREENSHOTS").is_some_and(|v| v == "1")).then(|| {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/M1");
        std::fs::create_dir_all(&d).unwrap();
        d
    })
}

fn shot(h: &mut H, name: &str) {
    shot_after(h, name, 6);
}

/// As [`shot`], with `steps` frames first (a screen that animates by the clock needs few).
fn shot_after(h: &mut H, name: &str, steps: usize) {
    // let replies and animations land, then render offscreen
    for _ in 0..steps {
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
    let Ok(img) = h.render() else {
        eprintln!("SKIP {name}: no GPU adapter for offscreen rendering");
        return;
    };
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

/// Hides every floating window except these (the playlist background stays), then shows them in
/// the given order, the last one in front.
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
    h.step();
}

/// The whole tour in one theme: `prefix` names the files (`neon-overview.png`, ...).
fn tour(theme: &str, prefix: &str) {
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
    h.state_mut().set_theme(theme);
    let name = |what: &str| format!("{prefix}-{what}");

    // the default layout: browser docked, playlist as the background, the rack floating
    shot(&mut h, &name("overview"));
    focus(&mut h, PanelId::PianoRoll);
    // an agent edits the kick while we look: its notes flash with an author badge
    for k in 0..3 {
        demo_agent::step(&h.state().core.studio.session, k);
    }
    shot(&mut h, &name("piano-roll-agent-flash"));
    h.state_mut().core.sel.channel = Some(ids.bass);
    h.state_mut().core.sel.pattern = Some(ids.bassline);
    h.state_mut().core.fix_selection();
    shot(&mut h, &name("piano-roll-bass"));

    // the same roll zoomed to 1/64: a run of 1/64 notes, the finest grid tier solid
    {
        let notes: Vec<ph_ui::mapping::NewNote> = (0..8u64)
            .map(|i| ph_ui::mapping::NewNote {
                key: 48 + (i % 4) as u8 * 3,
                start: 960 * 3 + i * 60,
                length: 60,
                velocity: 0.55 + 0.06 * i as f32,
                muted: false,
            })
            .collect();
        let app = h.state_mut();
        app.core
            .send(ph_ui::mapping::note_add(ids.bassline, ids.bass, &notes));
        settle(app);
        app.core.roll.snap = 60;
        app.core.roll.zoom_request = Some(ph_ui::piano_roll::ZoomPreset::Detail64);
        h.step();
        let app = h.state_mut();
        app.core.roll.view.x0 = 960.0 * 3.0 - 90.0;
        app.core.roll.view.y0 = 58.0;
    }
    h.state_mut().ws.maximize_toggle(WindowId::PianoRoll);
    shot(&mut h, &name("piano-roll-1-64"));
    h.state_mut().ws.maximize_toggle(WindowId::PianoRoll);
    h.state_mut().core.roll.snap = 240;
    h.state_mut().core.roll.view.px_per_tick = 0.12;
    only(&mut h, &[WindowId::Instrument(ids.bass)]);
    shot(&mut h, &name("instrument-synth"));
    only(&mut h, &[WindowId::Instrument(ids.kick)]);
    shot(&mut h, &name("instrument-sampler-waveform"));
    focus(&mut h, PanelId::History);
    shot(&mut h, &name("history"));
    focus(&mut h, PanelId::Agents);
    shot(&mut h, &name("agents"));
    only(&mut h, &[]);
    focus(&mut h, PanelId::Playlist);
    shot(&mut h, &name("playlist"));
    // the mixer keeps strips and routing; an effect slot opens its own window with the knobs
    only(&mut h, &[WindowId::Mixer, WindowId::Effect(InsertId(4), 1)]);
    shot(&mut h, &name("mixer-effects"));

    // a busy workspace: overlapping windows, two instrument windows open side by side
    only(
        &mut h,
        &[
            WindowId::ChannelRack,
            WindowId::PianoRoll,
            WindowId::Instrument(ids.kick),
            WindowId::Instrument(ids.bass),
            WindowId::Effect(InsertId(4), 0),
        ],
    );
    {
        let ws = &mut h.state_mut().ws;
        ws.drag_move(WindowId::ChannelRack, 20.0, 20.0);
        ws.drag_move(WindowId::PianoRoll, 330.0, 170.0);
        ws.drag_move(WindowId::Instrument(ids.kick), 20.0, 300.0);
        ws.drag_move(WindowId::Instrument(ids.bass), 560.0, 60.0);
        ws.drag_move(WindowId::Effect(InsertId(4), 0), 900.0, 330.0);
        ws.focus(WindowId::Instrument(ids.bass));
    }
    shot(&mut h, &name("busy-workspace"));

    // the right-click menu every control shares
    only(&mut h, &[WindowId::ChannelRack]);
    {
        let knob = h.get_by_label("Kick volume").rect().center();
        h.event(egui::Event::PointerMoved(knob));
        h.step();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: knob,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
    }
    shot(&mut h, &name("context-menu"));
    // Escape closes it again
    h.key_press(egui::Key::Escape);
    h.step();
    h.step();

    // the About box
    h.state_mut().core.dialog = Some(ph_ui::core::Dialog::About);
    shot(&mut h, &name("about"));
    h.state_mut().core.dialog = None;

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
    shot(&mut h, &name("conflict-dialog"));
}

#[test]
fn screenshots_neon_slate() {
    tour("neon-slate", "neon");
}

#[test]
fn screenshots_red_line() {
    tour("red-line", "red");
}

/// The loading screen, mid-way, in both themes.
#[test]
fn screenshots_of_the_splash() {
    for (theme, prefix) in [("neon-slate", "neon"), ("red-line", "red")] {
        let (fx, _) = fixture(false);
        let _keep = fx.dir;
        let mut app = fx.app;
        app.show_splash();
        let mut h = harness_dt(app, 1200.0, 760.0, 1.0, 0.04);
        h.state_mut().set_theme(theme);
        shot_after(&mut h, &format!("{prefix}-splash"), 3);
    }
}

#[test]
fn renders_sharply_at_double_dpi() {
    let (fx, _) = fixture(true);
    let _keep = fx.dir;
    let mut app = fx.app;
    pump_until(&mut app, 500, |a| a.core.project.channels.len() == 4);
    let mut h = harness_ppp(app, 1000.0, 640.0, 2.0);
    shot(&mut h, "neon-hidpi-2x");
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

fn shown(h: &H, p: PanelId) -> bool {
    h.state().panel_visible(p)
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

    // F7: the piano roll starts hidden, so the first press shows it in front and the second
    // hides it; the third shows it again.
    assert!(!shown(&h, PanelId::PianoRoll));
    h.key_press(Key::F7);
    h.step();
    h.step();
    assert!(shown(&h, PanelId::PianoRoll));
    h.key_press(Key::F7);
    h.step();
    h.step();
    assert!(
        !shown(&h, PanelId::PianoRoll),
        "second F7 hides the front piano roll"
    );
    h.key_press(Key::F7);
    h.step();
    h.step();
    assert!(shown(&h, PanelId::PianoRoll));

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
    let mut h = harness_gpu(app, 1600.0, 1000.0);
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
    let a = report(&mut h, "100 channels, 50 patterns, default workspace");
    focus(&mut h, PanelId::PianoRoll);
    h.state_mut().core.sel.pattern = Some(ph_core::PatternId(0));
    h.state_mut().core.sel.channel = Some(ph_core::ChannelId(3));
    h.state_mut().core.fix_selection();
    let b = report(&mut h, "10 000-note piano roll in a floating window");
    if h.render().is_ok() {
        // GPU side: scroll the 10 000-note roll every frame and render the whole window offscreen
        let mut gpu = Vec::new();
        for i in 0..30 {
            h.state_mut().core.roll.view.x0 = (i * 5000) as f64;
            h.step();
            let t = Instant::now();
            let _ = h.render().expect("render");
            gpu.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let first = gpu.remove(0);
        gpu.sort_by(|a, b| a.total_cmp(b));
        println!(
            "PERF offscreen wgpu render (full 1600x1000 window, scrolling 10k-note roll): first {first:.1} ms (device warm-up), then avg {:.2} ms, max {:.2} ms",
            gpu.iter().sum::<f64>() / gpu.len() as f64,
            gpu.last().unwrap()
        );
    }
    // generous in debug test builds; the release numbers are what the report quotes
    let budget = if cfg!(debug_assertions) { 120.0 } else { 16.0 };
    assert!(
        a < budget && b < budget,
        "frames too slow: {a:.1} / {b:.1} ms"
    );
}
