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
    start_with(false)
}

/// As [`start`]; with `gpu` the harness renders on the GPU when an adapter exists (pictures).
fn start_with(gpu: bool) -> Rig {
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
    let gpu = gpu
        && std::panic::catch_unwind(|| {
            Harness::builder()
                .with_size(Vec2::new(64.0, 64.0))
                .wgpu()
                .build_ui(|ui| {
                    ui.label("probe");
                })
        })
        .is_ok();
    let b = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 60.0);
    let b = if gpu { b.wgpu() } else { b };
    let mut h = b.build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
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

/// ---- the audio editor ---------------------------------------------------------------------
use ph_ui::workspace::WindowId;

impl Rig {
    /// Opens the editor on the clip's sample (through the command, like an agent would) and
    /// waits for the samples to be read.
    fn open_editor(&mut self) {
        run(
            self.h.state(),
            "editor.open",
            json!({ "sample": self.sample.0 }),
        );
        frames(&mut self.h, 4);
        let ok = pump_until(self.h.state_mut(), 8000, |a| {
            a.core
                .waves
                .lock()
                .unwrap()
                .request("samples/loop.wav", true)
                .is_some_and(|l| l.data.is_some())
        });
        assert!(ok, "the editor read the samples");
        self.h.state_mut().ws.hide(WindowId::Playlist);
        frames(&mut self.h, 6);
        assert!(
            self.h
                .state()
                .ws
                .is_visible(WindowId::AudioEditor(self.sample))
        );
    }

    fn editor_sample(&self) -> SampleId {
        self.h
            .state()
            .ws
            .ids()
            .find_map(|w| match w {
                WindowId::AudioEditor(s) if self.h.state().ws.is_visible(w) => Some(s),
                _ => None,
            })
            .expect("an editor window is open")
    }

    /// Waits for the samples of the file the editor shows now (a new file after an edit is
    /// read in the background), and draws.
    fn wait_loaded(&mut self) {
        let s = self.editor_sample();
        let path = self.h.state().core.project.samples[&s].path.clone();
        let ok = pump_until(self.h.state_mut(), 8000, |a| {
            a.core
                .waves
                .lock()
                .unwrap()
                .request(&path, true)
                .is_some_and(|l| l.data.is_some())
        });
        assert!(ok, "the editor read {path}");
        frames(&mut self.h, 3);
    }

    /// The editor window's view, for the sample it shows now.
    fn editor(&self) -> ph_ui::audio_editor::EditorState {
        self.h.state().core.audio_editors[&self.editor_sample()].clone()
    }

    /// Selects `a..b` frames with a real drag across the waveform.
    fn select_frames(&mut self, a: u64, b: u64) {
        self.wait_loaded();
        let st = self.editor();
        let (xa, xb) = (
            st.canvas.left() + st.x_of(a as f64),
            st.canvas.left() + st.x_of(b as f64),
        );
        let y = st.canvas.center().y;
        self.drag(Pos2::new(xa, y), Pos2::new(xb, y), Modifiers::NONE);
    }

    fn press_button(&mut self, label: &str) {
        use egui_kittest::kittest::Queryable;
        self.wait_loaded();
        self.h.get_by_label(label).click();
        after_input(&mut self.h);
    }
}

#[test]
fn selecting_and_trimming_writes_a_new_file_and_the_window_follows() {
    let mut r = start();
    r.open_editor();
    let original = std::fs::read(r.dir.path().join("samples/loop.wav")).unwrap();
    assert_eq!(r.editor().frames, 192_000);
    // a drag across the waveform selects (frames are rounded to pixels)
    r.select_frames(48_000, 96_000);
    let st = r.editor();
    let (a, b) = st.sel.expect("a selection");
    assert!((a as i64 - 48_000).abs() < 2 * st.fpp as i64 + 1, "{a}");
    assert!((b as i64 - 96_000).abs() < 2 * st.fpp as i64 + 1, "{b}");
    // Trim: one command, a new sample, the original byte for byte
    let hist = r.history();
    r.press_button("Trim");
    assert_eq!(r.history(), hist + 1);
    let p = r.h.state().core.project.clone();
    assert_eq!(p.samples.len(), 2, "the original stays registered");
    let new = r.editor_sample();
    assert_ne!(new, r.sample, "the window follows the new file");
    assert!(!r.h.state().ws.is_visible(WindowId::AudioEditor(r.sample)));
    assert_eq!(p.samples[&new].frames, b - a);
    assert_eq!(
        std::fs::read(r.dir.path().join("samples/loop.wav")).unwrap(),
        original,
        "the original file is untouched"
    );
    // the clip plays the trimmed file now
    assert_eq!(r.audio().sample, new);
    // an undo takes the project back and the window walks back with it
    r.h.state_mut().core.send(ph_ui::mapping::undo_mine());
    settle(r.h.state_mut());
    frames(&mut r.h, 4);
    assert_eq!(r.audio().sample, r.sample);
    assert_eq!(
        r.editor_sample(),
        r.sample,
        "the editor shows the original again"
    );
}

#[test]
fn the_edit_buttons_do_what_they_say() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut r = start();
    r.open_editor();
    // reverse the whole sample (no selection): same length, a new sample
    r.press_button("Reverse");
    let s1 = r.editor_sample();
    let p = r.h.state().core.project.clone();
    assert_eq!(p.samples[&s1].frames, 192_000);
    assert!(matches!(
        &p.samples[&s1].provenance.origin,
        ph_core::Origin::Edited { operation, .. } if operation == "reverse"
    ));
    // select, copy, then paste at the caret: the sample grows by the copied piece
    r.select_frames(4_000, 24_000);
    r.press_button("Copy");
    assert!(r.h.state().core.audio_clip.is_some());
    // the caret goes near the end: click the waveform there
    r.wait_loaded();
    let st = r.editor();
    let x = st.canvas.left() + st.x_of(190_000.0);
    r.click(Pos2::new(x, st.canvas.center().y), PointerButton::Primary);
    assert!(r.editor().sel.is_none());
    r.press_button("Paste");
    let s2 = r.editor_sample();
    let p = r.h.state().core.project.clone();
    assert!(p.samples[&s2].frames > 192_000, "{}", p.samples[&s2].frames);
    // the pasted piece is selected
    assert!(r.editor().sel.is_some());
    // fade out the pasted selection, normalise it, silence it: each one command
    let hist = r.history();
    r.press_button("Fade out");
    r.press_button("Normalise");
    r.press_button("Silence");
    assert_eq!(r.history(), hist + 3);
    // cut removes the selection and puts it on the clipboard
    r.select_frames(4_000, 10_000);
    r.press_button("Cut");
    assert!(r.h.state().core.audio_clip.is_some());
    // the Rec button records the armed inputs (M2/T8), so it is live
    assert!(!r.h.get_by_label("Rec").accesskit_node().is_disabled());
}

#[test]
fn the_audition_plays_outside_the_project_and_stops() {
    let mut r = start();
    r.open_editor();
    r.press_button("Audition");
    assert!(r.editor().playing.is_some());
    let p = r.h.state().core.project.clone();
    assert_eq!(p.clips.len(), 1, "an audition adds nothing to the project");
    r.press_button("Stop audition");
    assert!(r.editor().playing.is_none());
}

#[test]
fn the_slicer_detects_places_moves_and_deletes_markers_and_builds_the_instrument() {
    let mut r = start();
    r.open_editor();
    r.press_button("Slicer");
    // the loop has a thump on every half second: detection finds them
    r.press_button("Detect");
    let st = r.editor();
    assert!(
        st.markers.len() >= 5 && st.markers.len() <= 12,
        "{:?}",
        st.markers
    );
    let near_beats = st
        .markers
        .iter()
        .filter(|m| (**m % 24_000).min(24_000 - **m % 24_000) < 2_000)
        .count();
    assert!(
        near_beats >= 5,
        "markers sit on the thumps: {:?}",
        st.markers
    );
    // clear them and place two by hand in the marker lane
    r.press_button("Clear markers");
    assert!(r.editor().markers.is_empty());
    for f in [40_000u64, 100_000] {
        let st = r.editor();
        let x = st.marker_lane.left() + st.x_of(f as f64);
        r.click(
            Pos2::new(x, st.marker_lane.center().y),
            PointerButton::Primary,
        );
    }
    let st = r.editor();
    assert_eq!(st.markers.len(), 2);
    assert!((st.markers[0] as i64 - 40_000).abs() < 600);
    // drag the first one later
    let x0 = st.marker_lane.left() + st.x_of(st.markers[0] as f64);
    let y = st.marker_lane.center().y;
    r.drag(Pos2::new(x0, y), Pos2::new(x0 + 60.0, y), Modifiers::NONE);
    let moved = r.editor().markers[0];
    assert!(moved > 40_000 + 3_000, "{moved}");
    // right-click deletes the second
    let st = r.editor();
    let x1 = st.marker_lane.left() + st.x_of(st.markers[1] as f64);
    r.click(Pos2::new(x1, y), PointerButton::Secondary);
    assert_eq!(r.editor().markers.len(), 1);
    // make the instrument: two slices, one per key, with a pattern that replays the loop
    let hist = r.history();
    r.press_button("Make instrument");
    assert_eq!(r.history(), hist + 1, "one undo step");
    let p = r.h.state().core.project.clone();
    let ch = p
        .channels
        .values()
        .find(|c| {
            matches!(&c.instrument,
                ph_core::InstrumentSpec::Sampler { slices, .. } if !slices.is_empty())
        })
        .expect("a sliced sampler channel");
    let ph_core::InstrumentSpec::Sampler {
        slices, root_key, ..
    } = &ch.instrument
    else {
        unreachable!()
    };
    assert_eq!(slices, &vec![0, moved]);
    assert_eq!(*root_key, 36);
    assert_eq!(p.patterns.len(), 1, "the pattern that replays the loop");
    let pat = p.patterns.values().next().unwrap();
    assert_eq!(pat.notes[&ch.id].len(), 2);
}

#[test]
fn keys_cut_copy_and_zoom_work_over_the_waveform() {
    let mut r = start();
    r.open_editor();
    r.select_frames(10_000, 30_000);
    let st = r.editor();
    r.h.event(Event::PointerMoved(st.canvas.center()));
    r.h.step();
    r.h.key_down_modifiers(Modifiers::COMMAND, egui::Key::C);
    frames(&mut r.h, 3);
    assert!(r.h.state().core.audio_clip.is_some());
    let hist = r.history();
    r.h.key_press(egui::Key::Delete);
    after_input(&mut r.h);
    assert_eq!(r.history(), hist + 1);
    let new = r.editor_sample();
    let (a, b) = r.h.state().core.audio_editors[&new].sel.unwrap_or((0, 0));
    let _ = (a, b);
    let removed = 192_000 - r.h.state().core.project.samples[&new].frames;
    assert!((19_990..=20_010).contains(&removed), "{removed}");
    // zoom with the buttons, then fit again
    let before = r.editor().fpp;
    r.press_button("Zoom +");
    assert!(r.editor().fpp < before);
    r.press_button("Fit");
    assert!((r.editor().fpp - before).abs() < before * 0.2);
}

/// Frame cost with many audio clips on screen (the numbers the report quotes; `--nocapture`).
#[test]
fn audio_clips_keep_the_frame_time_low() {
    use std::time::Instant;
    let mut r = start();
    r.h.state_mut().core.playlist.track_h = 52.0;
    let measure = |h: &mut H, label: &str, f: &mut dyn FnMut(&mut H, usize)| -> f64 {
        let mut times = Vec::new();
        for i in 0..60 {
            f(h, i);
            let t = Instant::now();
            h.step();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        println!(
            "PERF {label}: avg {avg:.2} ms, p95 {:.2} ms, max {:.2} ms",
            times[56], times[59]
        );
        avg
    };
    // still frames with the one clip, then with 24 clips on 12 tracks, all waveforms cached
    let one = measure(&mut r.h, "playlist, 1 audio clip, still", &mut |_, _| {});
    for i in 0..23u64 {
        run(
            r.h.state(),
            "audio.add_clip",
            json!({ "track": 4 + (i % 12), "start": (i / 12) * 7680 + 3840 * (i % 3),
                    "sample": r.sample.0, "length": 5760 }),
        );
    }
    settle(r.h.state_mut());
    frames(&mut r.h, 4);
    let drawn = r.h.state().core.playlist.audio.drawn_waveforms;
    assert!(drawn > 1000, "the clips drew their waveforms: {drawn}");
    let still = measure(&mut r.h, "playlist, 24 audio clips, still", &mut |_, _| {});
    // scrolling sideways every frame recomputes the columns of every visible clip
    let scrolling = measure(
        &mut r.h,
        "playlist, 24 audio clips, scrolling every frame",
        &mut |h, i| h.state_mut().core.playlist.x0 = (i * 300) as f64,
    );
    // zooming every frame changes the level and the columns
    let zooming = measure(
        &mut r.h,
        "playlist, 24 audio clips, zooming every frame",
        &mut |h, i| {
            let z = 0.02 + 0.04 * ((i % 20) as f64 / 20.0);
            h.state_mut().core.playlist.px_per_tick = z;
        },
    );
    let budget = if cfg!(debug_assertions) { 120.0 } else { 16.0 };
    assert!(
        [one, still, scrolling, zooming].iter().all(|t| *t < budget),
        "frames too slow: {one:.1} / {still:.1} / {scrolling:.1} / {zooming:.1} ms"
    );
}

// Pictures for the report: set `PH_T6_SHOTS` to a folder. Nothing is written otherwise.
#[test]
fn screenshots() {
    let Some(dir) = std::env::var_os("PH_T6_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (theme, tag) in [("neon-slate", "dark"), ("red-line", "red")] {
        let mut r = start_with(true);
        r.h.state_mut().set_theme(theme);
        // fades, a reversed copy and a pattern clip for company
        run(
            r.h.state(),
            "audio.set_fade",
            json!({ "clip": r.clip.0, "fade_in": { "length": 960, "curve": "s_curve" },
                    "fade_out": { "length": 1920, "curve": "exp" } }),
        );
        let second = run(
            r.h.state(),
            "audio.add_clip",
            json!({ "track": 4, "start": 3840 * 2, "sample": r.sample.0, "length": 3840 }),
        );
        run(
            r.h.state(),
            "audio.set_reverse",
            json!({ "clip": second.result["clip"], "reverse": true }),
        );
        run(
            r.h.state(),
            "audio.set_gain",
            json!({ "clip": second.result["clip"], "gain_db": -6.0 }),
        );
        let pat = run(r.h.state(), "pattern.add", json!({})).result["pattern"].clone();
        run(
            r.h.state(),
            "playlist.add_clip",
            json!({ "track": 0, "pattern": pat, "start": 0, "length": 3840 }),
        );
        settle(r.h.state_mut());
        r.h.state_mut().core.playlist.track_h = 64.0;
        frames(&mut r.h, 6);
        snap(&mut r.h, &dir, &format!("playlist-audio-{tag}"));
        // the clip's settings menu
        let rect = r.rect();
        r.click(rect.center(), PointerButton::Secondary);
        frames(&mut r.h, 6);
        snap(&mut r.h, &dir, &format!("clip-menu-{tag}"));
        r.h.key_press(egui::Key::Escape);
        frames(&mut r.h, 4);
        // the audio editor with a selection and slice markers
        let sample = r.sample;
        run(r.h.state(), "editor.open", json!({ "sample": sample.0 }));
        frames(&mut r.h, 4);
        let ready = pump_until(r.h.state_mut(), 8000, |a| {
            a.core
                .waves
                .lock()
                .unwrap()
                .request("samples/loop.wav", true)
                .is_some_and(|l| l.data.is_some())
        });
        assert!(ready);
        {
            let st =
                r.h.state_mut()
                    .core
                    .audio_editors
                    .entry(sample)
                    .or_default();
            st.slicer = true;
            st.markers = vec![24_000, 48_000, 72_000, 96_000, 120_000, 144_000, 168_000];
            st.select(30_000, 70_000, 192_000);
        }
        r.h.state_mut()
            .ws
            .hide(ph_ui::workspace::WindowId::Playlist);
        frames(&mut r.h, 8);
        snap(&mut r.h, &dir, &format!("audio-editor-{tag}"));
    }
}

fn snap(h: &mut H, dir: &std::path::Path, name: &str) {
    for _ in 0..3 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    match h.render() {
        Ok(img) => img.save(dir.join(format!("{name}.png"))).unwrap(),
        Err(e) => eprintln!("no picture for {name}: {e}"),
    }
}
