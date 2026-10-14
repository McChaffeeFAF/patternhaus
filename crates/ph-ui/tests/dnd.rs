//! Dropping files from the operating system, simulated offscreen: `RawInput.hovered_files` and
//! `dropped_files` are injected into a kittest harness, nothing is shown and no audio device is
//! opened. Every drop must become `sample.import`, `file.import_midi` or `file.open` as the
//! human; the copy into `samples/` happens in the Host.

mod common;

use common::*;
use egui::{Event, Pos2, Vec2};
use egui_kittest::Harness;
use ph_core::{ChannelId, InstrumentSpec, Project};
use ph_formats::encode::{DITHER_SEED, WavFormat, encode_wav};
use ph_ui::PhApp;
use ph_ui::core::{AfterPrompt, Dialog, PanelId, ToastKind};
use ph_ui::workspace::WindowId;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

type H = Harness<'static, PhApp>;

#[derive(Debug)]
struct TestFile(PathBuf);

impl egui::DroppedFile for TestFile {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

struct Rig {
    h: H,
    ids: Ids,
    /// The project folder.
    dir: tempfile::TempDir,
    /// Where the "Explorer" files live, outside the project.
    drops: tempfile::TempDir,
    config: tempfile::TempDir,
}

fn wav(freq: f32) -> Vec<u8> {
    let tone: Vec<f32> = (0..2000)
        .map(|i| (i as f32 * freq * 0.01).sin() * 0.5)
        .collect();
    encode_wav(&[tone], 44_100, WavFormat::Pcm16, DITHER_SEED).unwrap()
}

fn start() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let st = studio(dir.path(), Project::empty("Drop test"));
    let ids = build_beat(&st.session);
    let ctx = egui::Context::default();
    let mut app = PhApp::new(
        &ctx,
        st,
        ph_ui::AppPaths {
            project_dir: dir.path().to_path_buf(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            config_dir: Some(config.path().to_path_buf()),
        },
        ph_ui::Hooks::default(),
        ph_engine::EngineConfig::default(),
        None,
    )
    .expect("app starts");
    pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    for _ in 0..4 {
        h.step();
    }
    Rig {
        h,
        ids,
        dir,
        drops: tempfile::tempdir().unwrap(),
        config,
    }
}

impl Rig {
    fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.drops.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.h.step();
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    fn app(&mut self) -> &mut PhApp {
        self.h.state_mut()
    }

    fn move_to(&mut self, pos: Pos2) {
        self.h.event(Event::PointerMoved(pos));
        self.frames(2);
    }

    /// A drag of `paths` hovers over `pos`.
    fn hover(&mut self, paths: &[&Path], pos: Pos2) {
        self.h.input_mut().hovered_files = paths
            .iter()
            .map(|p| egui::HoveredFile {
                path: Some(p.to_path_buf()),
                ..Default::default()
            })
            .collect();
        self.move_to(pos);
    }

    /// The files are released at `pos`; waits for the commands to finish.
    fn drop_at(&mut self, paths: &[&Path], pos: Pos2) {
        self.move_to(pos);
        let input = self.h.input_mut();
        input.hovered_files.clear();
        input.dropped_files = paths
            .iter()
            .map(|p| Arc::new(TestFile(p.to_path_buf())) as egui::DroppedFileHandle)
            .collect();
        self.h.step();
        self.settle();
    }

    fn settle(&mut self) {
        // generous bounds: a loaded machine (parallel builds) delays the session thread
        for _ in 0..600 {
            self.frames(1);
            if self.h.state().core.dispatch.in_flight() == 0 {
                break;
            }
        }
        // the Change stream: until the UI's snapshot is the session's project
        if let Ok(target) = self.h.state().core.studio.session.project() {
            for _ in 0..600 {
                let p = &self.h.state().core.project;
                if Arc::ptr_eq(p, &target) || **p == *target {
                    break;
                }
                self.frames(1);
            }
        }
        self.frames(6);
    }

    fn hint(&self) -> String {
        ph_ui::hint::current(&self.h.ctx)
            .map(|h| h.name)
            .unwrap_or_default()
    }

    fn row_center(&mut self, c: ChannelId) -> Pos2 {
        let g = &self.app().core.rack.geometry;
        let (_, r, _) = g.iter().find(|(id, ..)| *id == c).expect("row drawn");
        r.center()
    }

    fn window_point(&mut self, id: WindowId, fx: f32, fy: f32) -> Pos2 {
        let app = self.app();
        let r = app.ws.effective_rect(id).expect("window shown");
        let o = app.work_rect.min;
        Pos2::new(o.x + r.x + r.w * fx, o.y + r.y + r.h * fy)
    }

    fn sampler(&mut self, c: ChannelId) -> Option<String> {
        match &self.app().core.project.channels[&c].instrument {
            InstrumentSpec::Sampler { sample, .. } => sample.clone(),
            _ => None,
        }
    }

    fn toasts(&mut self) -> Vec<(ToastKind, String)> {
        self.app()
            .core
            .toasts
            .iter()
            .map(|t| (t.kind, t.text.clone()))
            .collect()
    }

    fn channels(&mut self) -> usize {
        self.app().core.project.channels.len()
    }
}

/// The rack window's empty area (below the rows).
fn empty_rack_area(r: &mut Rig) -> Pos2 {
    r.window_point(WindowId::ChannelRack, 0.5, 0.97)
}

#[test]
fn audio_files_on_the_rack_become_one_channel_each_and_one_undo_step() {
    let mut r = start();
    let a = r.file("kick.wav", &wav(1.0));
    let b = r.file("snare.wav", &wav(2.0));
    let c = r.file("hat.wav", &wav(3.0));
    let pos = empty_rack_area(&mut r);
    r.drop_at(&[&a, &b, &c], pos);
    assert_eq!(r.channels(), 7);
    let order = r.app().core.project.channel_order.clone();
    let new: Vec<_> = order[4..]
        .iter()
        .map(|id| r.app().core.project.channels[id].name.clone())
        .collect();
    assert_eq!(
        new,
        ["kick", "snare", "hat"],
        "named after the files, in order"
    );
    let refs: Vec<_> = order[4..].iter().map(|id| r.sampler(*id)).collect();
    assert_eq!(
        refs,
        [
            Some("samples/kick.wav".into()),
            Some("samples/snare.wav".into()),
            Some("samples/hat.wav".into())
        ]
    );
    for n in ["kick", "snare", "hat"] {
        assert!(r.dir.path().join(format!("samples/{n}.wav")).exists());
    }
    // one undo step takes all three back; the copied files stay
    r.app().core.send(ph_ui::mapping::undo_mine());
    r.settle();
    assert_eq!(r.channels(), 4);
    assert!(r.dir.path().join("samples/kick.wav").exists());
}

#[test]
fn one_audio_file_on_a_channel_row_replaces_its_sample() {
    let mut r = start();
    let a = r.file("snare2.wav", &wav(5.0));
    let kick = r.ids.kick;
    let pos = r.row_center(kick);
    r.hover(&[&a], pos);
    assert_eq!(r.hint(), "Drop to replace Kick's sample");
    r.drop_at(&[&a], pos);
    assert_eq!(r.channels(), 4, "no new channel");
    assert_eq!(r.sampler(kick).as_deref(), Some("samples/snare2.wav"));
    assert_eq!(r.app().core.project.channels[&kick].name, "Kick");
    assert!(r.dir.path().join("samples/snare2.wav").exists());
    // several files on a row are not a replace: they make channels
    let b = r.file("b.wav", &wav(6.0));
    let c = r.file("c.wav", &wav(7.0));
    let pos = r.row_center(kick);
    r.hover(&[&b, &c], pos);
    assert_eq!(r.hint(), "Drop to add 2 samples as channels");
    r.drop_at(&[&b, &c], pos);
    assert_eq!(r.channels(), 6);
    assert_eq!(r.sampler(kick).as_deref(), Some("samples/snare2.wav"));
}

#[test]
fn audio_on_the_playlist_adds_channels_and_the_hint_says_why() {
    let mut r = start();
    r.app().open_panel(PanelId::Playlist);
    r.frames(4);
    let a = r.file("loop.wav", &wav(1.5));
    let pos = r.window_point(WindowId::Playlist, 0.5, 0.5);
    r.hover(&[&a], pos);
    assert!(
        r.hint().contains("audio clips arrive in a later milestone"),
        "{}",
        r.hint()
    );
    r.drop_at(&[&a], pos);
    assert_eq!(r.channels(), 5);
}

#[test]
fn a_midi_file_on_the_playlist_or_piano_roll_is_imported_as_a_pattern() {
    let mut r = start();
    // a real .mid, exported by the app and moved outside the project
    let beat = r.ids.beat;
    r.app().core.send(ph_commands::Request::new(
        "file.export_midi",
        serde_json::json!({ "path": "kick.mid", "pattern": beat }),
    ));
    r.settle();
    let bytes = std::fs::read(r.dir.path().join("kick.mid")).unwrap();
    let mid = r.file("groove.mid", &bytes);
    r.app().open_panel(PanelId::Playlist);
    r.app().open_panel(PanelId::PianoRoll);
    r.frames(4);
    let before = r.app().core.project.patterns.len();
    let pos = r.window_point(WindowId::Playlist, 0.5, 0.5);
    r.hover(&[&mid], pos);
    assert_eq!(r.hint(), "MIDI: drop to import as a new pattern");
    r.drop_at(&[&mid], pos);
    assert_eq!(r.app().core.project.patterns.len(), before + 1);
    assert!(
        r.app()
            .core
            .project
            .patterns
            .values()
            .any(|p| p.name == "groove")
    );
    // and over the piano roll
    let pos = r.window_point(WindowId::PianoRoll, 0.5, 0.5);
    r.hover(&[&mid], pos);
    assert_eq!(r.hint(), "MIDI: drop to import as a new pattern");
    r.drop_at(&[&mid], pos);
    assert_eq!(r.app().core.project.patterns.len(), before + 2);
}

#[test]
fn a_folder_on_the_browser_becomes_a_remembered_library_folder() {
    let mut r = start();
    r.app().open_panel(PanelId::Browser);
    r.frames(4);
    r.file("Kicks/boom.wav", &wav(1.0));
    let folder = r.drops.path().join("Kicks");
    r.hover(&[&folder], Pos2::new(60.0, 400.0));
    assert_eq!(r.hint(), "Drop to add Kicks to the library");
    assert!(
        !r.app().core.drop_hover.as_ref().unwrap().rects.is_empty(),
        "the browser is highlighted"
    );
    r.drop_at(&[&folder], Pos2::new(60.0, 400.0));
    let core = &r.app().core;
    assert!(core.paths.library_roots.contains(&folder));
    assert_eq!(core.browser.library.len(), 1);
    assert_eq!(core.browser.library[0].1[0].name, "boom");
    assert_eq!(core.browser.tab, 3, "the Library tab is showing");
    // remembered in the settings file
    let prefs = ph_ui::settings::load_prefs(Some(r.config.path()));
    assert_eq!(prefs.library_folders, vec![folder.clone()]);
    // dropping it again does not duplicate it
    r.drop_at(&[&folder], Pos2::new(60.0, 400.0));
    assert_eq!(r.app().core.paths.library_roots.len(), 1);
}

fn save_other_project(r: &Rig) -> PathBuf {
    let other = r.drops.path().join("Other Song");
    ph_formats::save_project(&Project::empty("Other Song"), &other).unwrap();
    other
}

#[test]
fn a_project_folder_opens_asking_first_when_there_are_unsaved_changes() {
    let mut r = start();
    let other = save_other_project(&r);
    r.app().core.dirty = true;
    r.drop_at(&[&other], Pos2::new(800.0, 500.0));
    // unsaved changes: the T5c question, nothing opened yet
    match &r.app().core.dialog {
        Some(Dialog::Unsaved {
            then: AfterPrompt::OpenProject { path },
        }) => assert_eq!(Path::new(path), other),
        d => panic!("expected the unsaved-changes question, got {d:?}"),
    }
    assert_eq!(r.app().core.project.settings.title, "Drop test");
    // Don't save: it opens
    r.app().core.dialog = None;
    r.app().core.ready.push(AfterPrompt::OpenProject {
        path: other.to_string_lossy().into_owned(),
    });
    r.settle();
    assert_eq!(r.app().core.project.settings.title, "Other Song");

    // a clean project opens at once, and a dropped project.json means its folder
    let mut r = start();
    let other = save_other_project(&r);
    r.app().core.dirty = false;
    r.drop_at(&[&other.join("project.json")], Pos2::new(800.0, 500.0));
    assert!(r.app().core.dialog.is_none());
    assert_eq!(r.app().core.project.settings.title, "Other Song");
}

#[test]
fn unsupported_files_are_skipped_with_a_notice_naming_them() {
    let mut r = start();
    let txt = r.file("notes.txt", b"hello");
    let zip = r.file("pack.zip", b"PK");
    let m4a = r.file("song.m4a", b"....");
    let sent = r.app().core.dispatch.sent;
    r.drop_at(&[&txt, &zip, &m4a], Pos2::new(800.0, 500.0));
    assert_eq!(r.app().core.dispatch.sent, sent, "no command was sent");
    let t = r.toasts();
    let (kind, text) = t.last().unwrap();
    assert_eq!(*kind, ToastKind::Warn);
    for want in [
        "notes.txt",
        "pack.zip",
        "song.m4a",
        "not an audio, MIDI, FL Studio or project file",
    ] {
        assert!(text.contains(want), "{text}");
    }
    assert_eq!(r.channels(), 4);
}

#[test]
fn name_clashes_get_a_number_and_identical_files_are_reused() {
    let mut r = start();
    let pos = empty_rack_area(&mut r);
    let a = r.file("kick.wav", &wav(1.0));
    let b = r.file("other/kick.wav", &wav(2.0));
    r.drop_at(&[&a], pos);
    r.drop_at(&[&b], pos);
    r.drop_at(&[&a], pos); // the same bytes again
    let order = r.app().core.project.channel_order.clone();
    let refs: Vec<_> = order[4..]
        .iter()
        .map(|id| r.sampler(*id).unwrap())
        .collect();
    assert_eq!(
        refs,
        [
            "samples/kick.wav",
            "samples/kick (2).wav",
            "samples/kick.wav"
        ]
    );
    let files = std::fs::read_dir(r.dir.path().join("samples"))
        .unwrap()
        .count();
    assert_eq!(files, 2, "the identical file was not copied twice");
}

#[test]
fn a_file_that_will_not_decode_is_skipped_and_named() {
    let mut r = start();
    let good = r.file("good.wav", &wav(1.0));
    let bad = r.file("broken.wav", b"this is not audio");
    let pos = empty_rack_area(&mut r);
    r.drop_at(&[&bad, &good], pos);
    assert_eq!(r.channels(), 5, "only the good one became a channel");
    assert!(!r.dir.path().join("samples/broken.wav").exists());
    let t = r.toasts();
    assert!(
        t.iter()
            .any(|(k, s)| *k == ToastKind::Warn && s.contains("broken.wav")),
        "{t:?}"
    );
}

#[test]
fn hovering_highlights_the_target_and_leaving_clears_it() {
    let mut r = start();
    let a = r.file("one.wav", &wav(1.0));
    let pos = empty_rack_area(&mut r);
    r.hover(&[&a], pos);
    assert_eq!(r.hint(), "Drop to add 1 sample as a channel");
    let hover = r.app().core.drop_hover.clone().expect("highlight state");
    assert_eq!(hover.rects.len(), 1);
    assert!(hover.rects[0].contains(pos), "the rack window is lit");
    // the drag leaves the window
    r.h.input_mut().hovered_files.clear();
    r.frames(3);
    assert!(r.app().core.drop_hover.is_none());
    assert_eq!(r.hint(), "");
}

#[test]
fn an_flp_file_asks_where_to_import_then_asks_about_unsaved_changes() {
    let mut r = start();
    let flp = r.file("song.flp", &ph_formats::flp::build::sample_project());
    let pos = Pos2::new(800.0, 500.0);
    r.hover(&[&flp], pos);
    assert_eq!(
        r.hint(),
        "FL Studio: drop to import song.flp as a new project"
    );
    r.app().core.dirty = true;
    r.drop_at(&[&flp], pos);
    // first the placement question
    match &r.app().core.dialog {
        Some(Dialog::ImportFlp { path }) => assert_eq!(Path::new(path), flp),
        d => panic!("expected the placement question, got {d:?}"),
    }
    assert_eq!(r.app().core.project.settings.title, "Drop test");
    // choosing a place, with unsaved changes, asks the T5c question
    r.app().core.dialog = None;
    r.app().core.ask_or_do(AfterPrompt::ImportFlp {
        path: flp.to_string_lossy().into_owned(),
        next_to_flp: true,
    });
    match &r.app().core.dialog {
        Some(Dialog::Unsaved {
            then: AfterPrompt::ImportFlp { path, next_to_flp },
        }) => {
            assert_eq!(Path::new(path), flp);
            assert!(*next_to_flp);
        }
        d => panic!("expected the unsaved-changes question, got {d:?}"),
    }
    assert_eq!(r.app().core.project.settings.title, "Drop test");
    // Don't save: it imports, beside the file as asked
    r.app().core.dialog = None;
    r.app().core.ready.push(AfterPrompt::ImportFlp {
        path: flp.to_string_lossy().into_owned(),
        next_to_flp: true,
    });
    r.settle();
    assert_eq!(r.app().core.project.settings.title, "Synthetic song");
    assert_eq!(r.app().core.project.channels.len(), 3);
    let imported = r.drops.path().join("song (Patternhaus)");
    assert!(imported.join("import-report.md").is_file());
    // the UI's project folder followed the import
    assert_eq!(r.app().core.paths.project_dir, imported);

    // a clean project imports at once once a place is chosen (the host has no projects folder
    // in tests, so both choices land beside the file)
    let mut r = start();
    let flp = r.file("again.flp", &ph_formats::flp::build::sample_project());
    r.app().core.dirty = false;
    r.drop_at(&[&flp], pos);
    assert!(matches!(
        r.app().core.dialog,
        Some(Dialog::ImportFlp { .. })
    ));
    r.app().core.dialog = None;
    r.app().core.ask_or_do(AfterPrompt::ImportFlp {
        path: flp.to_string_lossy().into_owned(),
        next_to_flp: false,
    });
    r.settle();
    assert!(r.app().core.dialog.is_none());
    assert_eq!(r.app().core.project.settings.title, "Synthetic song");
}

#[test]
fn opening_a_project_moves_the_ui_project_folder() {
    let mut r = start();
    let other = save_other_project(&r);
    r.app().core.dirty = false;
    r.app().core.ask_or_do(AfterPrompt::OpenProject {
        path: other.to_string_lossy().into_owned(),
    });
    r.settle();
    assert_eq!(r.app().core.paths.project_dir, other);
}
