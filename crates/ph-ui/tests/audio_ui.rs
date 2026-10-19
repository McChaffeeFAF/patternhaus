//! Audio clips in the playlist, through real pointer events offscreen (no window, no audio
//! device): waveforms, trim, slip, fades, split, duplicate and the clip menu. Every gesture
//! must be one command, so one undo step. Generated audio only.

mod common;

use common::*;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Vec2};
use egui_kittest::Harness;
use ph_core::{Clip, ClipId, Project, SampleId};
use ph_formats::encode::{WavFormat, write_wav};
use ph_ui::PhApp;
use serde_json::json;
use std::time::Duration;

type H = Harness<'static, PhApp>;

/// A two-bar loop at 120 BPM (4 s, 48 kHz mono): a decaying 60 Hz thump on every beat.
pub fn loop_samples() -> Vec<f32> {
    let sr = 48_000usize;
    let mut x = vec![0.0f32; 4 * sr];
    for beat in 0..8 {
        let s0 = beat * sr / 2;
        for i in 0..(sr / 4) {
            let t = i as f32 / sr as f32;
            let amp = if beat % 2 == 0 { 0.9 } else { 0.5 };
            x[s0 + i] += amp * (std::f32::consts::TAU * 60.0 * t).sin() * (-t * 14.0).exp();
        }
    }
    x
}

struct Rig {
    h: H,
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    clip: ClipId,
    sample: SampleId,
}

fn run(app: &PhApp, cmd: &str, params: serde_json::Value) -> ph_commands::Executed {
    app.core
        .studio
        .session
        .execute(
            &ph_commands::Caller::human(),
            ph_commands::Request::new(cmd, params),
        )
        .unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(6));
    }
}

/// After a gesture: a few frames for the UI to send its commands, then wait for the session
/// to answer and the snapshot to arrive.
fn after_input(h: &mut H) {
    frames(h, 6);
    settle(h.state_mut());
    frames(h, 2);
}

/// A project with the generated loop as an audio clip on track 3 at bar 2, the waveform
/// decoded and the playlist drawn.
fn start() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("samples")).unwrap();
    write_wav(
        &dir.path().join("samples/loop.wav"),
        &[loop_samples()],
        48_000,
        WavFormat::Float32,
    )
    .unwrap();
    let st = studio(dir.path(), Project::empty("Audio UI"));
    let ctx = egui::Context::default();
    let mut app = PhApp::new(
        &ctx,
        st,
        ph_ui::AppPaths {
            project_dir: dir.path().to_path_buf(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            config_dir: None,
        },
        ph_ui::Hooks::default(),
        ph_engine::EngineConfig::default(),
        None,
    )
    .expect("app starts");
    run(&app, "project.set_tempo", json!({ "bpm": 120 }));
    let out = run(
        &app,
        "audio.add_clip",
        json!({ "track": 3, "start": 3840, "path": "samples/loop.wav" }),
    );
    let clip = ClipId(out.result["clip"].as_u64().unwrap() as u32);
    let sample = SampleId(out.result["sample"].as_u64().unwrap() as u32);
    pump_until(&mut app, 5000, |a| a.core.project.clips.contains_key(&clip));
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    // the channel rack floats over the top left of the playlist: out of the way
    h.state_mut()
        .ws
        .hide(ph_ui::workspace::WindowId::ChannelRack);
    frames(&mut h, 4);
    // the waveform is decoded in the background: wait for it, then draw
    let ready = pump_until(h.state_mut(), 8000, |a| {
        a.core.waves.lock().unwrap().is_ready("samples/loop.wav")
    });
    assert!(ready, "the waveform decoded");
    frames(&mut h, 3);
    Rig {
        h,
        dir,
        clip,
        sample,
    }
}

impl Rig {
    fn clip(&self) -> Clip {
        self.h.state().core.project.clips[&self.clip].clone()
    }
    fn audio(&self) -> ph_core::AudioClip {
        self.clip().audio().unwrap().clone()
    }
    /// The clip's rectangle on screen.
    fn rect(&self) -> Rect {
        let c = self.clip();
        let pl = &self.h.state().core.playlist;
        pl.clip_rect(c.track as i32, c.start, c.length)
            .translate(pl.last_grid.min.to_vec2())
    }
    fn history(&self) -> usize {
        self.h
            .state()
            .core
            .studio
            .session
            .history()
            .map(|h| h.len())
            .unwrap_or(0)
    }
    fn px_per_tick(&self) -> f32 {
        self.h.state().core.playlist.px_per_tick as f32
    }

    fn button(&mut self, pos: Pos2, b: PointerButton, pressed: bool, m: Modifiers) {
        self.h.event(Event::PointerButton {
            pos,
            button: b,
            pressed,
            modifiers: m,
        });
        self.h.step();
    }

    /// A drag with optional modifiers held throughout.
    fn drag(&mut self, from: Pos2, to: Pos2, m: Modifiers) {
        self.h.event(Event::ModifiersChanged(m));
        self.h.event(Event::PointerMoved(from));
        self.h.step();
        self.button(from, PointerButton::Primary, true, m);
        for i in 1..=6 {
            let p = from + (to - from) * (i as f32 / 6.0);
            self.h.event(Event::PointerMoved(p));
            self.h.step();
        }
        self.button(to, PointerButton::Primary, false, m);
        self.h.event(Event::ModifiersChanged(Modifiers::NONE));
        after_input(&mut self.h);
    }

    fn click(&mut self, p: Pos2, b: PointerButton) {
        self.h.event(Event::PointerMoved(p));
        self.h.step();
        self.button(p, b, true, Modifiers::NONE);
        self.button(p, b, false, Modifiers::NONE);
        after_input(&mut self.h);
    }
}

#[test]
fn an_audio_clip_draws_its_waveform_from_the_cached_peaks() {
    let mut r = start();
    let pl = &r.h.state().core.playlist;
    assert!(
        pl.audio.drawn_waveforms > 100,
        "{}",
        pl.audio.drawn_waveforms
    );
    assert_eq!(pl.audio.cols.len(), 1, "one clip's columns are cached");
    // scrolling off-screen draws nothing for it, zooming in redraws it
    let before = r.h.state().core.playlist.px_per_tick;
    r.h.state_mut().core.playlist.zoom_x(2.0, 100.0);
    frames(&mut r.h, 3);
    assert!(r.h.state().core.playlist.px_per_tick > before);
    assert!(r.h.state().core.playlist.audio.drawn_waveforms > 0);
}

#[test]
fn dragging_the_right_edge_trims_in_one_undo_step() {
    let mut r = start();
    let (hist, before) = (r.history(), r.clip());
    let rect = r.rect();
    let from = Pos2::new(rect.right() - 2.0, rect.center().y);
    // one bar (3840 ticks) shorter, snapped to the bar
    let to = from - Vec2::new(r.px_per_tick() * 3840.0, 0.0);
    r.drag(from, to, Modifiers::NONE);
    let c = r.clip();
    assert_eq!(c.start, before.start);
    assert_eq!(c.length, before.length - 3840);
    assert_eq!(r.audio().source_offset, 0, "the sound did not move");
    assert_eq!(r.history(), hist + 1, "one command, one undo step");
    // undo restores it
    r.h.state_mut().core.send(ph_ui::mapping::undo_mine());
    settle(r.h.state_mut());
    assert_eq!(r.clip().length, before.length);
}

#[test]
fn dragging_the_left_edge_trims_and_moves_the_source_offset() {
    let mut r = start();
    let before = r.clip();
    let rect = r.rect();
    let from = Pos2::new(rect.left() + 2.0, rect.center().y);
    let to = from + Vec2::new(r.px_per_tick() * 3840.0, 0.0);
    r.drag(from, to, Modifiers::NONE);
    let c = r.clip();
    assert_eq!(c.start, before.start + 3840);
    assert_eq!(c.length, before.length - 3840);
    // one bar at 120 BPM is 2 s = 96 000 frames at 48 kHz
    assert_eq!(r.audio().source_offset, 96_000);
}

#[test]
fn alt_drag_slips_the_sound_inside_the_clip() {
    let mut r = start();
    // trim in first, so there is file on both sides
    let rect = r.rect();
    let from = Pos2::new(rect.left() + 2.0, rect.center().y);
    r.drag(
        from,
        from + Vec2::new(r.px_per_tick() * 3840.0, 0.0),
        Modifiers::NONE,
    );
    let (hist, before, off) = (r.history(), r.clip(), r.audio().source_offset);
    assert_eq!(off, 96_000);
    let rect = r.rect();
    let from = Pos2::new(rect.center().x, rect.center().y + 6.0);
    // drag the sound 480 ticks later = 12 000 frames earlier in the file
    let to = from + Vec2::new(r.px_per_tick() * 480.0, 0.0);
    r.drag(from, to, Modifiers::ALT);
    let c = r.clip();
    assert_eq!(
        (c.start, c.length),
        (before.start, before.length),
        "the clip stays put"
    );
    assert_eq!(r.audio().source_offset, off - 12_000);
    assert_eq!(r.history(), hist + 1);
}

#[test]
fn a_fade_handle_sets_the_fade_length() {
    let mut r = start();
    let hist = r.history();
    let rect = r.rect();
    // the fade-in handle sits at the clip's left edge, on the label strip
    let from = Pos2::new(rect.left() + 1.0, rect.top() + 7.0);
    let to = from + Vec2::new(r.px_per_tick() * 960.0, 0.0);
    r.drag(from, to, Modifiers::NONE);
    let a = r.audio();
    assert!(
        (a.fade_in.length as i64 - 960).abs() <= 20,
        "{}",
        a.fade_in.length
    );
    assert_eq!(a.fade_out.length, 0);
    assert_eq!(r.clip().length, 7680, "a fade does not trim");
    assert_eq!(r.history(), hist + 1);
    // and the fade-out handle at the right edge
    let rect = r.rect();
    let from = Pos2::new(rect.right() - 1.0, rect.top() + 7.0);
    let to = from - Vec2::new(r.px_per_tick() * 480.0, 0.0);
    r.drag(from, to, Modifiers::NONE);
    let a = r.audio();
    assert!(
        (a.fade_out.length as i64 - 480).abs() <= 20,
        "{}",
        a.fade_out.length
    );
}

#[test]
fn the_split_tool_cuts_a_clip_at_the_grid() {
    let mut r = start();
    r.h.state_mut().core.playlist.audio.slice_tool = true;
    frames(&mut r.h, 2);
    let hist = r.history();
    let rect = r.rect();
    // a click a bit past the first bar line: snaps to the bar (tick 3840 after the start)
    let x = rect.left() + r.px_per_tick() * 3900.0;
    r.click(Pos2::new(x, rect.center().y), PointerButton::Primary);
    let clips: Vec<Clip> = r.h.state().core.project.clips.values().cloned().collect();
    assert_eq!(clips.len(), 2);
    let (left, right) = (&clips[0], &clips[1]);
    assert_eq!(left.length, 3840);
    assert_eq!(right.start, 3840 + 3840);
    assert_eq!(right.audio().unwrap().source_offset, 96_000);
    assert_eq!(r.history(), hist + 1);
}

#[test]
fn ctrl_b_duplicates_an_audio_clip_with_its_settings() {
    let mut r = start();
    run(
        r.h.state(),
        "audio.set_gain",
        json!({ "clip": r.clip.0, "gain_db": -4.0 }),
    );
    settle(r.h.state_mut());
    let rect = r.rect();
    r.click(rect.center(), PointerButton::Primary); // select it
    let hist = r.history();
    r.h.key_down_modifiers(Modifiers::COMMAND, egui::Key::B);
    after_input(&mut r.h);
    let clips: Vec<Clip> = r.h.state().core.project.clips.values().cloned().collect();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[1].start, clips[0].start + clips[0].length);
    assert_eq!(clips[1].audio().unwrap().gain_db, -4.0);
    assert_eq!(r.history(), hist + 1);
}

#[test]
fn the_clip_menu_changes_settings_one_command_each() {
    use egui_kittest::kittest::Queryable;
    let mut r = start();
    let rect = r.rect();
    r.click(rect.center(), PointerButton::Secondary);
    frames(&mut r.h, 4);
    assert!(
        r.h.state().core.playlist.audio.menu.is_some(),
        "the menu opened"
    );
    let hist = r.history();
    r.h.get_by_label("Reverse").click();
    after_input(&mut r.h);
    assert!(r.audio().reverse);
    // the stretch mode
    r.h.get_by_label("Stretch").click();
    after_input(&mut r.h);
    assert_eq!(r.audio().stretch, ph_core::StretchMode::Stretch);
    // Fit to tempo detects nothing here (the sample has no tempo), but must not break; set
    // one and fit
    run(
        r.h.state(),
        "sample.set_bpm",
        json!({ "sample": r.sample.0, "bpm": 120.0 }),
    );
    settle(r.h.state_mut());
    r.h.get_by_label("Fit to tempo").click();
    after_input(&mut r.h);
    let a = r.audio();
    assert!(a.tempo_sync);
    assert_eq!(
        r.clip().length,
        7680,
        "a two bar loop is two bars at 120 BPM"
    );
    assert!(r.history() >= hist + 2);
}

#[test]
fn delete_in_the_menu_removes_the_clip_and_a_pattern_clip_still_erases_on_right_click() {
    use egui_kittest::kittest::Queryable;
    let mut r = start();
    let rect = r.rect();
    r.click(rect.center(), PointerButton::Secondary);
    frames(&mut r.h, 4);
    r.h.get_by_label("Delete clip").click();
    after_input(&mut r.h);
    assert!(r.h.state().core.project.clips.is_empty());
}

#[test]
fn double_clicking_a_clip_opens_the_audio_editor() {
    let mut r = start();
    let rect = r.rect();
    r.h.event(Event::PointerMoved(rect.center()));
    r.h.step();
    for _ in 0..2 {
        r.button(rect.center(), PointerButton::Primary, true, Modifiers::NONE);
        r.button(
            rect.center(),
            PointerButton::Primary,
            false,
            Modifiers::NONE,
        );
    }
    after_input(&mut r.h);
    let id = ph_ui::workspace::WindowId::AudioEditor(r.sample);
    assert!(r.h.state().ws.is_visible(id), "the editor window is open");
}
