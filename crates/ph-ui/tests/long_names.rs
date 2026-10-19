//! The long-name guard. A project full of 80-character names (no spaces, with spaces, accented
//! and CJK) is shown in every window and docked panel in both themes; the frame's text shapes
//! are then checked for the two ways a long name goes wrong:
//!
//! 1. **Collision**: the *visible* rects of two texts (the glyph ink bounds at the shape's
//!    position, rotated if the shape is, intersected with the clip rect it was painted under)
//!    overlap by more than 2 px in both directions.
//! 2. **Bounds**: a text crosses the edge of the window being tested, or a user text is cut off
//!    by its clip rect instead of being shortened with "…".
//!
//! One window is shown at a time (the others hidden) so that overlapping *windows* never read
//! as overlapping *text*. The only allowlisted overlap is the glow copies `lcd_text` paints
//! under each LCD readout (same string, within 2.5 px of each other: copies at +-1 px).

mod common;

use common::*;
use egui::epaint::Shape;
use egui::{Pos2, Rect, Vec2};
use egui_kittest::Harness;
use ph_commands::{Caller, Request};
use ph_core::{InsertId, InstrumentSpec};
use ph_ui::workspace::WindowId;
use ph_ui::{PhApp, mapping};
use std::path::PathBuf;
use std::time::Duration;

type H = Harness<'static, PhApp>;

const NOSPACE: &str =
    "Lng-Channel-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const SPACED: &str =
    "Lng Channel with a rather long descriptive name that keeps going and going on";
const UNICODE: &str = "Lng \u{c9}dition sp\u{e9}ciale \u{f1}and\u{fa} \u{c5}\u{c4}\u{d6} \u{957f}\u{540d}\u{524d}\u{7684}\u{30c1}\u{30e3}\u{30f3}\u{30cd}\u{30eb}\u{540d}\u{524d}\u{30c6}\u{30b9}\u{30c8}\u{9577}\u{3044}\u{9577}\u{3044}";
const SAMPLE: &str =
    "C:/Users/someone/Music/Samples/Lng 80s Remix Tronicbo-long-file-name-that-goes-on-and-on.wav";
const AGENT: &str = "Lng-Agent-Client-With-An-Extremely-Long-Name-Version-1.2.3-Preview";

struct Fake;
impl ph_ui::AgentDirectory for Fake {
    fn clients(&self) -> Vec<ph_ui::ClientInfo> {
        let c = |name: &str, live: bool| ph_ui::ClientInfo {
            name: name.into(),
            scopes: vec![ph_commands::Scope::Compose, ph_commands::Scope::Mix],
            live,
            connected_for: 4200,
            idle_for: 3,
            commands: 90000,
        };
        vec![c(AGENT, true), c(UNICODE, false)]
    }
    fn pending_grants(&self) -> Vec<ph_ui::GrantRequest> {
        vec![ph_ui::GrantRequest {
            id: 1,
            client: AGENT.into(),
            requested: vec![ph_commands::Scope::Full],
        }]
    }
    fn resolve_grant(&self, _: u64, _: Option<Vec<ph_commands::Scope>>) {}
}

fn run(app: &mut PhApp, r: Request) -> serde_json::Value {
    app.core
        .studio
        .session
        .execute(&Caller::human(), r.clone())
        .unwrap_or_else(|e| panic!("{}: {e}", r.command))
        .result
}

struct Stress {
    app: PhApp,
    _dir: tempfile::TempDir,
    sampler: ph_core::ChannelId,
    synth: ph_core::ChannelId,
    /// A real (decodable) audio file with a long name, placed as an audio clip.
    audio: ph_core::SampleId,
}

/// The file name of the long-named audio clip.
const AUDIO_FILE: &str =
    "Lng Audio Clip Tronicbo with a very very long file name that goes on and on and on.wav";

fn stress() -> Stress {
    let (fx, _) = fixture(false);
    let mut app = fx.app;
    run(&mut app, mapping::set_tempo(124.0));
    run(
        &mut app,
        Request::new(
            "project.rename",
            serde_json::json!({ "title": format!("{SPACED} {NOSPACE}") }),
        ),
    );
    let mut patterns = vec![];
    for n in [SPACED, NOSPACE, UNICODE] {
        let id = ph_core::PatternId(
            run(&mut app, mapping::pattern_add())["pattern"]
                .as_u64()
                .unwrap() as u32,
        );
        run(&mut app, mapping::pattern_rename(id, n));
        patterns.push(id);
    }
    let sampler = |s: &str| InstrumentSpec::Sampler {
        slices: Vec::new(),
        sample: Some(s.into()),
        root_key: 60,
        params: Default::default(),
    };
    let synth = InstrumentSpec::Synth {
        preset: Some("bass/sub".into()),
        params: Default::default(),
    };
    let mut chans = vec![];
    for (i, (n, inst)) in [
        (NOSPACE, sampler(SAMPLE)),
        (SPACED, synth),
        (UNICODE, sampler("factory:drums/haus808/kick")),
    ]
    .into_iter()
    .enumerate()
    {
        let r = run(&mut app, mapping::channel_add(n, &inst, Some(1 + i as u32)));
        chans.push(ph_core::ChannelId(r["channel"].as_u64().unwrap() as u32));
    }
    let note = |k: u8, s: u64| mapping::NewNote {
        key: k,
        start: s,
        length: 480,
        velocity: 0.8,
        muted: false,
    };
    run(
        &mut app,
        mapping::note_add(patterns[0], chans[1], &[note(40, 0), note(43, 960)]),
    );
    run(
        &mut app,
        mapping::step_set_row(patterns[0], chans[0], &[true, false, false, false, true]),
    );
    for (i, n) in [UNICODE, NOSPACE, SPACED, NOSPACE].iter().enumerate() {
        run(&mut app, mapping::mixer_rename(1 + i as u32, n));
    }
    for (i, n) in [SPACED, NOSPACE, UNICODE].iter().enumerate() {
        run(&mut app, mapping::track_rename(i as u32, n));
        run(&mut app, mapping::clip_add(i as u32, patterns[i], 0, None));
        run(
            &mut app,
            mapping::clip_add(i as u32, patterns[i], 3840, Some(1920)),
        );
    }
    run(&mut app, mapping::effect_add(1, "eq"));
    run(&mut app, mapping::effect_add(2, "compressor"));
    run(&mut app, mapping::send_add(1, 2, 0.4));
    // an agent edits things: the history gets rows by a long-named author
    let agent = Caller::agent(AGENT, ph_commands::Grant(vec![ph_commands::Scope::Full]));
    let s = &app.core.studio.session;
    s.execute(&agent, mapping::channel_rename(chans[2], UNICODE))
        .unwrap();
    s.execute(&agent, mapping::pattern_rename(patterns[1], SPACED))
        .unwrap();
    app.core.hooks.agents = Some(std::sync::Arc::new(Fake));
    pump_until(&mut app, 800, |a| a.core.project.channels.len() == 3);
    app.core.sel.channel = Some(chans[0]);
    app.core.sel.pattern = Some(patterns[0]);
    app.core.sel.insert = 1;
    app.core.sel.effect = Some(0);
    app.core.fix_selection();
    // long file names in the browser's project tab (a filter opens every folder)
    let samples = fx.dir.path().join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    for n in [
        "Lng 80s Remix Tronicbo-long-file-name-that-goes-on-and-on-and-on.wav",
        "Lng-NoSpaces-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.wav",
        "Lng \u{c9}dition \u{957f}\u{540d}\u{524d}\u{7684}\u{30c1}\u{30e3}\u{30f3}\u{30cd}\u{30eb}\u{540d}\u{524d}\u{30c6}\u{30b9}\u{30c8}\u{9577}\u{3044}\u{9577}\u{3044}\u{540d}\u{524d}.wav",
    ] {
        std::fs::write(samples.join(n), b"x").unwrap();
    }
    // a real audio file with a long name, as an audio clip with a fade
    let tone: Vec<f32> = (0..96_000)
        .map(|i| (i as f32 * 0.03).sin() * 0.6 * (1.0 - i as f32 / 96_000.0))
        .collect();
    ph_formats::encode::write_wav(
        &samples.join(AUDIO_FILE),
        &[tone],
        48_000,
        ph_formats::encode::WavFormat::Float32,
    )
    .unwrap();
    let added = run(
        &mut app,
        Request::new(
            "audio.add_clip",
            serde_json::json!({ "track": 3, "start": 0, "path": format!("samples/{AUDIO_FILE}") }),
        ),
    );
    let audio = ph_core::SampleId(added["sample"].as_u64().unwrap() as u32);
    run(
        &mut app,
        Request::new(
            "audio.set_fade",
            serde_json::json!({ "clip": added["clip"], "fade_in": { "length": 480, "curve": "s_curve" } }),
        ),
    );
    pump_until(&mut app, 800, |a| {
        a.core.project.samples.contains_key(&audio)
    });
    app.core.browser.tab = 2;
    app.core.browser.filter = "lng".into();
    app.core.browser.rescan();
    app.core
        .toast(ph_ui::core::ToastKind::Info, format!("{SPACED} {NOSPACE}"));
    Stress {
        app,
        _dir: fx.dir,
        sampler: chans[0],
        synth: chans[1],
        audio,
    }
}

fn harness(app: PhApp) -> H {
    let build = |app: PhApp, gpu: bool| {
        let b = Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_step_dt(0.25)
            .with_pixels_per_point(1.0);
        let b = if gpu { b.wgpu() } else { b };
        b.build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app)
    };
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

/// One text shape as the user would see it.
#[derive(Debug, Clone)]
struct Seen {
    text: String,
    /// Where it lands after the clip rect.
    visible: Rect,
    /// Where it would land with no clipping.
    full: Rect,
}

fn collect(shapes: &[egui::epaint::ClippedShape]) -> Vec<Seen> {
    fn walk(s: &Shape, clip: Rect, out: &mut Vec<Seen>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
            Shape::Text(t) => {
                let text = t.galley.text().to_string();
                if text.trim().is_empty() {
                    return;
                }
                // the ink bounds of the glyphs, not the galley's layout rect (which carries line leading
                // and, in wrapped rows, empty space): two texts overlap when their ink does
                let local = t.galley.mesh_bounds.translate(t.pos.to_vec2());
                if !local.is_positive() {
                    return;
                }
                let full = if t.angle == 0.0 {
                    local
                } else {
                    // rotated about `pos`
                    let (sin, cos) = t.angle.sin_cos();
                    let rot = |p: Pos2| {
                        let d = p - t.pos;
                        t.pos + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
                    };
                    Rect::from_points(&[
                        rot(local.left_top()),
                        rot(local.right_top()),
                        rot(local.left_bottom()),
                        rot(local.right_bottom()),
                    ])
                };
                let visible = full.intersect(clip);
                if visible.width() > 0.0 && visible.height() > 0.0 {
                    out.push(Seen {
                        text,
                        visible,
                        full,
                    });
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for c in shapes {
        walk(&c.shape, c.clip_rect, &mut out);
    }
    out
}

const TOL: f32 = 2.0;

fn is_user(t: &str) -> bool {
    t.contains("Lng") || t.contains('\u{957f}') || t.contains("Tronicbo")
}

fn check(scene: &str, theme: &str, seen: &[Seen], window: Option<Rect>, fails: &mut Vec<String>) {
    for (i, a) in seen.iter().enumerate() {
        for b in &seen[i + 1..] {
            let o = a.visible.intersect(b.visible);
            if o.width() > TOL && o.height() > TOL {
                // allowlist: lcd_text's glow copies are the same string a pixel apart
                if a.text == b.text && (a.visible.min - b.visible.min).length() <= 2.5 {
                    continue;
                }
                fails.push(format!(
                    "[{theme}/{scene}] OVERLAP {:?} {:?}  with  {:?} {:?}",
                    a.text, a.visible, b.text, b.visible
                ));
            }
        }
        if is_user(&a.text) {
            let cut_x = a.full.width() - a.visible.width() > 1.0;
            let cut_y = a.full.height() - a.visible.height() > 1.0;
            if (cut_x || cut_y) && !a.text.ends_with('\u{2026}') {
                fails.push(format!(
                    "[{theme}/{scene}] CUT OFF {:?} full {:?} visible {:?}",
                    a.text, a.full, a.visible
                ));
            }
        }
        if let Some(w) = window
            && a.visible.intersects(w)
            && !w.expand(0.5).contains_rect(a.visible)
        {
            fails.push(format!(
                "[{theme}/{scene}] CROSSES WINDOW EDGE {:?} {:?} window {:?}",
                a.text, a.visible, w
            ));
        }
    }
}

fn only(h: &mut H, keep: &[WindowId]) {
    let ws = &mut h.state_mut().ws;
    // the docked browser sits under floating windows; its text is tested in its own scene
    ws.browser.visible = keep.is_empty();
    for id in ws.ids().collect::<Vec<_>>() {
        ws.hide(id);
    }
    for id in keep {
        ws.show(*id);
    }
}

fn step(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn shots() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/M1");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_theme(theme: &str, prefix: &str) {
    let s = stress();
    let (sampler, synth, audio) = (s.sampler, s.synth, s.audio);
    let _keep = s._dir;
    let mut h = harness(s.app);
    h.state_mut().set_theme(theme);
    let mut fails = vec![];

    let scenes: Vec<(&str, Vec<WindowId>)> = vec![
        ("rack", vec![WindowId::ChannelRack]),
        ("piano-roll", vec![WindowId::PianoRoll]),
        ("playlist", vec![WindowId::Playlist]),
        ("mixer", vec![WindowId::Mixer]),
        ("history", vec![WindowId::History]),
        ("agents", vec![WindowId::Agents]),
        ("sampler", vec![WindowId::Instrument(sampler)]),
        ("synth", vec![WindowId::Instrument(synth)]),
        ("effect", vec![WindowId::Effect(InsertId(1), 0)]),
        ("effect-2", vec![WindowId::Effect(InsertId(2), 0)]),
        ("audio-editor", vec![WindowId::AudioEditor(audio)]),
        ("audio-menu", vec![WindowId::Playlist]),
        ("docked-only", vec![]),
    ];
    for (name, wins) in scenes {
        only(&mut h, &wins);
        step(&mut h, 6);
        if name == "audio-menu" {
            // the clip's settings menu, with its long name at the top
            let clip = h
                .state()
                .core
                .project
                .clips
                .values()
                .find(|c| c.audio().is_some())
                .map(|c| c.id)
                .unwrap();
            h.state_mut()
                .core
                .playlist
                .open_audio_menu(clip, Pos2::new(420.0, 300.0));
            step(&mut h, 4);
        }
        if name == "audio-editor" {
            // the editor reads the file in the background: wait for it, then show the slicer
            // row too, with a marker or two
            pump_until(h.state_mut(), 8000, |a| {
                a.core
                    .waves
                    .lock()
                    .unwrap()
                    .request(&format!("samples/{AUDIO_FILE}"), true)
                    .is_some_and(|l| l.data.is_some())
            });
            let st = h.state_mut().core.audio_editors.entry(audio).or_default();
            st.slicer = true;
            st.markers = vec![20_000, 50_000];
            st.select(10_000, 30_000, 96_000);
            step(&mut h, 4);
        }
        let seen = collect(&h.output().shapes);
        let win = wins
            .first()
            .and_then(|w| h.state().ws.effective_rect(*w))
            .map(|r| {
                let o = h.state().work_rect.min;
                Rect::from_min_max(
                    o + Vec2::new(r.x, r.y),
                    o + Vec2::new(r.right(), r.bottom()),
                )
            });
        // the scene really shows long names (or the checks above would pass on an empty frame)
        let users = seen.iter().filter(|s| is_user(&s.text)).count();
        let need = if name == "audio-editor" { 2 } else { 3 };
        assert!(
            users >= need,
            "{theme}/{name}: only {users} user texts on screen"
        );
        check(name, theme, &seen, win, &mut fails);
    }
    // the picture for the docs: the rack and the mixer, the two places a long name hurt most
    only(&mut h, &[WindowId::ChannelRack, WindowId::Mixer]);
    h.state_mut()
        .ws
        .drag_move(WindowId::ChannelRack, 20.0, 20.0);
    h.state_mut().ws.drag_move(WindowId::Mixer, 200.0, 300.0);
    step(&mut h, 6);
    // only PH_SCREENSHOTS=1 writes the tracked PNGs: a plain `cargo test` leaves the tree clean
    if std::env::var_os("PH_SCREENSHOTS").is_some_and(|v| v == "1")
        && let Ok(img) = h.render()
    {
        img.save(shots().join(format!("{prefix}-long-names.png")))
            .unwrap();
    }
    assert!(
        fails.is_empty(),
        "{} long-name problem(s) in {theme}:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

#[test]
fn long_names_stay_inside_their_space_neon_slate() {
    run_theme("neon-slate", "neon");
}

#[test]
fn long_names_stay_inside_their_space_red_line() {
    run_theme("red-line", "red");
}
