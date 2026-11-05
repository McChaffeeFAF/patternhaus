//! The Haus Sampler window (M3/T5), offscreen through real egui events: the zone map's drags
//! (move, resize, several zones, rubber band) each become one undoable `sampler.zone_move`,
//! the inspector edits through `sampler.zone_set`, the keyboard strip switches articulation,
//! and drops of a generated SFZ, a sample folder (with its auto-map preview) and a two-preset
//! SF2 import through `sampler.import`. Save and collect run `sampler.save_native` and
//! `sampler.collect`. No window is shown and no audio or MIDI device is opened.
//!
//! With `PH_SCREENSHOTS=1`, the zone map is rendered at a few sizes in each theme into
//! `local/reports/m3-ui/` (git-ignored).

mod common;

use common::*;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_commands::{Caller, Request};
use ph_core::{
    ArticulationId, ChannelId, HausInstrument, InstrumentId, InstrumentSpec, KeyRange, Project,
    VelRange, ZoneId,
};
use ph_formats::encode::{DITHER_SEED, WavFormat, encode_wav};
use ph_ui::PhApp;
use ph_ui::core::Dialog;
use ph_ui::sampler_ui::Tab;
use ph_ui::workspace::WindowId;
use serde_json::{Value, json};
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

fn wav(freq: f32) -> Vec<u8> {
    let tone: Vec<f32> = (0..4000)
        .map(|i| (i as f32 * freq * 0.01).sin() * 0.5)
        .collect();
    encode_wav(&[tone], 44_100, WavFormat::Pcm16, DITHER_SEED).unwrap()
}

/// The zones of the test instrument, by role.
#[allow(dead_code)]
struct Zones {
    low_soft: ZoneId,
    low_loud: ZoneId,
    mid: ZoneId,
    rr1: ZoneId,
    rr2: ZoneId,
    release: ZoneId,
}

struct Rig {
    h: H,
    dir: tempfile::TempDir,
    drops: tempfile::TempDir,
    ch: ChannelId,
    inst: InstrumentId,
    z: Zones,
    staccato: ArticulationId,
}

fn exec(app: &PhApp, command: &str, params: Value) -> Value {
    app.core
        .studio
        .session
        .execute(&Caller::human(), Request::new(command, params))
        .unwrap_or_else(|e| panic!("{command}: {e}"))
        .result
}

fn harness(app: PhApp, w: f32, hgt: f32, gpu: bool) -> H {
    let b = Harness::builder()
        .with_size(Vec2::new(w, hgt))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 30.0);
    let b = if gpu { b.wgpu() } else { b };
    b.build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app)
}

fn gpu_available() -> bool {
    std::panic::catch_unwind(|| {
        Harness::builder()
            .with_size(egui::vec2(64.0, 64.0))
            .wgpu()
            .build_ui(|ui| {
                ui.label("probe");
            })
    })
    .is_ok()
}

/// A project with one Haus Sampler channel: two velocity layers, a middle zone, two round
/// robins, a release zone over the middle, two groups and two keyswitched articulations. The
/// sampler window is open and nothing else but the rack.
fn start_with(w: f32, hgt: f32, gpu: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("samples")).unwrap();
    for (n, f) in ["a", "b", "c", "d", "e", "f"].iter().zip(1..) {
        std::fs::write(dir.path().join(format!("samples/{n}.wav")), wav(f as f32)).unwrap();
    }
    let st = studio(dir.path(), Project::empty("Sampler test"));
    let ctx = egui::Context::default();
    let app = PhApp::new(
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
    let made = exec(
        &app,
        "sampler.new",
        json!({ "name": "Haus Keys", "group": "Sustain" }),
    );
    let inst = InstrumentId(made["instrument"].as_u64().unwrap() as u32);
    let ch = ChannelId(made["channel"].as_u64().unwrap() as u32);
    let g0 = made["group"].as_u64().unwrap();
    let g1 = exec(
        &app,
        "sampler.group_add",
        json!({ "instrument": inst, "name": "Staccato" }),
    )["group"]
        .as_u64()
        .unwrap();
    let zones = exec(
        &app,
        "sampler.zone_add",
        json!({ "instrument": inst, "zones": [
            { "group": g0, "sample": "samples/a.wav", "root_key": 54, "keys": [48, 59], "velocity": [0, 63] },
            { "group": g0, "sample": "samples/b.wav", "root_key": 54, "keys": [48, 59], "velocity": [64, 127] },
            { "group": g0, "sample": "samples/c.wav", "root_key": 66, "keys": [60, 71] },
            { "group": g1, "sample": "samples/d.wav", "root_key": 74, "keys": [72, 79], "round_robin": 0 },
            { "group": g1, "sample": "samples/e.wav", "root_key": 74, "keys": [72, 79], "round_robin": 1 },
            { "group": g0, "sample": "samples/f.wav", "root_key": 66, "keys": [60, 71], "trigger": "release" },
        ]}),
    )["zones"]
        .clone();
    let id = |n: usize| ZoneId(zones[n].as_u64().unwrap() as u32);
    exec(
        &app,
        "sampler.keyswitch_set",
        json!({ "instrument": inst, "name": "Sustain", "trigger": { "kind": "key", "key": 24 },
                "groups": [g0], "default": true }),
    );
    let st = exec(
        &app,
        "sampler.keyswitch_set",
        json!({ "instrument": inst, "name": "Staccato", "trigger": { "kind": "key", "key": 25 },
                "groups": [g1] }),
    );
    let staccato = ArticulationId(st["articulation"].as_u64().unwrap() as u32);
    let mut app = app;
    pump_until(&mut app, 5000, |a| {
        a.core
            .project
            .instruments
            .get(&inst)
            .is_some_and(|i| i.articulations.len() == 2)
    });
    let mut h = harness(app, w, hgt, gpu);
    {
        let ws = &mut h.state_mut().ws;
        for id in ws.ids().collect::<Vec<_>>() {
            if id != WindowId::Playlist {
                ws.hide(id);
            }
        }
    }
    h.state_mut()
        .core
        .window_requests
        .push((WindowId::Instrument(ch), ph_ui::core::PanelAction::Open));
    let mut r = Rig {
        h,
        dir,
        drops: tempfile::tempdir().unwrap(),
        ch,
        inst,
        z: Zones {
            low_soft: id(0),
            low_loud: id(1),
            mid: id(2),
            rr1: id(3),
            rr2: id(4),
            release: id(5),
        },
        staccato,
    };
    r.frames(6);
    r
}

fn start() -> Rig {
    start_with(1600.0, 1000.0, false)
}

impl Rig {
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.h.step();
            std::thread::sleep(Duration::from_millis(6));
        }
    }

    fn app(&mut self) -> &mut PhApp {
        self.h.state_mut()
    }

    fn settle(&mut self) {
        for _ in 0..600 {
            self.frames(1);
            if self.h.state().core.dispatch.in_flight() == 0 {
                break;
            }
        }
        if let Ok(target) = self.h.state().core.studio.session.project() {
            for _ in 0..600 {
                let p = &self.h.state().core.project;
                if Arc::ptr_eq(p, &target) || **p == *target {
                    break;
                }
                self.frames(1);
            }
        }
        self.frames(4);
    }

    fn instrument(&self) -> HausInstrument {
        self.h.state().core.project.instruments[&self.inst].clone()
    }

    fn zone(&self, z: ZoneId) -> ph_core::Zone {
        self.instrument().zones[&z].clone()
    }

    fn history(&self) -> usize {
        self.h.state().core.studio.session.history().unwrap().len()
    }

    fn view(&self) -> &ph_ui::sampler_ui::SamplerView {
        &self.h.state().core.sampler[&self.inst]
    }

    /// Where a zone is drawn (its band).
    fn zone_rect(&self, z: ZoneId) -> egui::Rect {
        self.view()
            .drawn
            .zones
            .iter()
            .find(|s| s.zone == z)
            .expect("zone drawn")
            .band
    }

    fn key_w(&self) -> f32 {
        self.view().drawn.map.unwrap().key_w
    }

    fn vel_h(&self) -> f32 {
        self.view().drawn.map.unwrap().vel_h()
    }

    fn button(&mut self, p: Pos2, pressed: bool, m: Modifiers) {
        self.h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: m,
        });
        self.h.step();
    }

    fn drag(&mut self, from: Pos2, to: Pos2, m: Modifiers) {
        self.h.event(Event::ModifiersChanged(m));
        self.h.event(Event::PointerMoved(from));
        self.h.step();
        self.button(from, true, m);
        for i in 1..=6 {
            let p = from + (to - from) * (i as f32 / 6.0);
            self.h.event(Event::PointerMoved(p));
            self.h.step();
        }
        self.button(to, false, m);
        self.h.event(Event::ModifiersChanged(Modifiers::NONE));
        self.settle();
    }

    fn click(&mut self, p: Pos2, m: Modifiers) {
        self.h.event(Event::ModifiersChanged(m));
        self.h.event(Event::PointerMoved(p));
        self.h.step();
        self.button(p, true, m);
        self.button(p, false, m);
        self.h.event(Event::ModifiersChanged(Modifiers::NONE));
        self.frames(3);
    }

    fn undo(&mut self) {
        self.app().core.send(ph_ui::mapping::undo_mine());
        self.settle();
    }

    fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.drops.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// The rack window's empty area (below the rows).
    fn rack_point(&mut self) -> Pos2 {
        let app = self.app();
        app.ws.show(WindowId::ChannelRack);
        let r = app.ws.effective_rect(WindowId::ChannelRack).unwrap();
        let o = app.work_rect.min;
        let p = Pos2::new(o.x + r.x + r.w * 0.5, o.y + r.y + r.h * 0.95);
        self.frames(3);
        p
    }

    fn drop_at(&mut self, paths: &[&Path], pos: Pos2) {
        self.h.event(Event::PointerMoved(pos));
        self.frames(2);
        let input = self.h.input_mut();
        input.hovered_files.clear();
        input.dropped_files = paths
            .iter()
            .map(|p| Arc::new(TestFile(p.to_path_buf())) as egui::DroppedFileHandle)
            .collect();
        self.h.step();
        self.settle();
    }

    fn hover(&mut self, paths: &[&Path], pos: Pos2) -> String {
        self.h.input_mut().hovered_files = paths
            .iter()
            .map(|p| egui::HoveredFile {
                path: Some(p.to_path_buf()),
                ..Default::default()
            })
            .collect();
        self.h.event(Event::PointerMoved(pos));
        self.frames(2);
        let text = self
            .h
            .state()
            .core
            .drop_hover
            .as_ref()
            .map(|h| h.text.clone())
            .unwrap_or_default();
        self.h.input_mut().hovered_files.clear();
        self.frames(1);
        text
    }

    fn sampler_channels(&self) -> Vec<(ChannelId, InstrumentId)> {
        let p = &self.h.state().core.project;
        p.channel_order
            .iter()
            .filter_map(|c| match p.channels[c].instrument {
                InstrumentSpec::MultiSampler {
                    instrument: Some(i),
                    ..
                } => Some((*c, i)),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn the_window_shows_the_zone_map_strip_and_tabs() {
    let mut r = start();
    let ch = r.ch;
    assert!(r.app().ws.is_visible(WindowId::Instrument(ch)));
    let rect = r.app().ws.get(WindowId::Instrument(ch)).unwrap().rect;
    assert!(rect.w >= 880.0 && rect.h >= 600.0, "opens big: {rect:?}");
    let v = r.view();
    assert_eq!(v.drawn.zones.len(), 6, "every zone is drawn");
    // round robins are stacked in one rectangle, the release zone outlined over the middle
    let (a, b) = (r.zone_rect(r.z.rr1), r.zone_rect(r.z.rr2));
    assert!((a.left() - b.left()).abs() < 0.1 && a.bottom() <= b.top() + 0.1);
    let rel = v
        .drawn
        .zones
        .iter()
        .find(|s| s.zone == r.z.release)
        .unwrap();
    assert!(rel.outlined);
    for name in [
        "Zone map",
        "Keyboard strip",
        "Groups",
        "Articulations",
        "Modulation",
        "Import report",
    ] {
        assert!(
            r.h.query_all_by_label(name).count() >= 1,
            "{name} is in the window"
        );
    }
    // the default articulation is the current one until a keyswitch is played
    assert!(
        r.h.query_all_by_label_contains("Articulation: Sustain")
            .count()
            >= 1
    );
}

#[test]
fn dragging_a_zone_moves_it_in_one_undo_step() {
    let mut r = start();
    let before = r.zone(r.z.mid);
    let hist = r.history();
    let from = r.zone_rect(r.z.mid).center() + Vec2::new(0.0, -20.0);
    let to = from + Vec2::new(r.key_w() * 2.0, 0.0);
    r.drag(from, to, Modifiers::NONE);
    let z = r.zone(r.z.mid);
    assert_eq!(z.keys, KeyRange { lo: 62, hi: 73 });
    assert_eq!(z.root_key, before.root_key + 2, "the root follows a move");
    assert_eq!(z.velocity, before.velocity);
    assert_eq!(r.history(), hist + 1, "one drag, one undo step");
    assert!(r.view().selected.contains(&r.z.mid));
    r.undo();
    assert_eq!(r.zone(r.z.mid), before, "undo restores it");

    // up and down: a velocity move
    let before = r.zone(r.z.low_soft);
    let from = r.zone_rect(r.z.low_soft).center();
    let to = from + Vec2::new(0.0, -r.vel_h() * 10.0);
    r.drag(from, to, Modifiers::NONE);
    assert_eq!(r.zone(r.z.low_soft).velocity, VelRange { lo: 10, hi: 73 });
    r.undo();
    assert_eq!(r.zone(r.z.low_soft), before);
}

#[test]
fn dragging_edges_resizes_and_keeps_the_root() {
    let mut r = start();
    let before = r.zone(r.z.mid);
    let hist = r.history();
    // select it first: its own edges then win over the release outline drawn on top of it
    let rect = r.zone_rect(r.z.mid);
    r.click(rect.center(), Modifiers::NONE);
    let from = Pos2::new(rect.right() - 2.0, rect.center().y - 30.0);
    r.drag(
        from,
        from - Vec2::new(r.key_w() * 3.0, 0.0),
        Modifiers::NONE,
    );
    let z = r.zone(r.z.mid);
    assert_eq!(z.keys, KeyRange { lo: 60, hi: 68 });
    assert_eq!(z.root_key, before.root_key, "an edge keeps the root");
    assert_eq!(r.history(), hist + 1);
    // the top edge of the soft layer: a velocity split
    let rect = r.zone_rect(r.z.low_soft);
    let from = Pos2::new(rect.center().x, rect.top() + 2.0);
    r.drag(
        from,
        from + Vec2::new(0.0, r.vel_h() * 13.0),
        Modifiers::NONE,
    );
    assert_eq!(r.zone(r.z.low_soft).velocity, VelRange { lo: 0, hi: 50 });
    assert_eq!(r.history(), hist + 2);
    r.undo();
    r.undo();
    assert_eq!(r.zone(r.z.mid), before);
    assert_eq!(r.zone(r.z.low_soft).velocity, VelRange { lo: 0, hi: 63 });
}

#[test]
fn several_zones_move_together_as_one_step() {
    let mut r = start();
    let hist = r.history();
    // click one, Shift-click another
    let a = r.zone_rect(r.z.low_soft).center();
    let b = r.zone_rect(r.z.low_loud).center();
    r.click(a, Modifiers::NONE);
    r.click(b, Modifiers::SHIFT);
    assert_eq!(
        r.view().selected.iter().copied().collect::<Vec<_>>(),
        vec![r.z.low_soft, r.z.low_loud]
    );
    r.drag(b, b - Vec2::new(r.key_w() * 4.0, 0.0), Modifiers::NONE);
    assert_eq!(r.zone(r.z.low_soft).keys, KeyRange { lo: 44, hi: 55 });
    assert_eq!(r.zone(r.z.low_loud).keys, KeyRange { lo: 44, hi: 55 });
    assert_eq!(r.history(), hist + 1);
    r.undo();
    assert_eq!(r.zone(r.z.low_loud).keys, KeyRange { lo: 48, hi: 59 });

    // a rubber band from empty space selects what it touches
    let map = r.view().drawn.map.unwrap();
    let rr = r.zone_rect(r.z.rr1);
    let from = Pos2::new(map.x_of(81.0), map.rect.top() + 2.0);
    assert!(!r.view().drawn.zones.iter().any(|s| s.band.contains(from)));
    assert!(rr.right() < from.x);
    let to = Pos2::new(map.x_of(73.5), map.rect.bottom() - 2.0);
    r.drag(from, to, Modifiers::NONE);
    let sel: Vec<ZoneId> = r.view().selected.iter().copied().collect();
    assert_eq!(sel, vec![r.z.rr1, r.z.rr2]);
    assert_eq!(r.history(), hist + 2, "a selection changes nothing");
}

#[test]
fn the_inspector_edits_the_selected_zones() {
    let mut r = start();
    r.click(r.zone_rect(r.z.mid).center(), Modifiers::NONE);
    assert_eq!(r.view().tab, Tab::Zones);
    let hist = r.history();
    let at = r.h.get_by_label("Zone volume").rect().center();
    r.drag(at, at + Vec2::new(-60.0, 0.0), Modifiers::NONE);
    let v = r.zone(r.z.mid).volume_db;
    assert!(v < -1.0, "volume went down: {v}");
    assert_eq!(r.history(), hist + 1, "one edit, one step");

    // two zones at once
    r.click(r.zone_rect(r.z.low_soft).center(), Modifiers::NONE);
    r.click(r.zone_rect(r.z.low_loud).center(), Modifiers::SHIFT);
    let at = r.h.get_by_label("Zone tune").rect().center();
    r.drag(at, at + Vec2::new(40.0, 0.0), Modifiers::NONE);
    let (a, b) = (r.zone(r.z.low_soft).tune, r.zone(r.z.low_loud).tune);
    assert!(a > 0 && a == b, "both retuned: {a} {b}");
    assert_eq!(r.history(), hist + 2);
    r.undo();
    assert_eq!(r.zone(r.z.low_loud).tune, 0);
    r.undo();
    assert_eq!(r.zone(r.z.mid).volume_db, 0.0);
}

#[test]
fn the_keyboard_strip_switches_articulation() {
    let mut r = start();
    {
        let inst = r.inst;
        let v = r.app().core.sampler.get_mut(&inst).unwrap();
        v.key0 = 20.0;
        v.key_w = 12.0;
    }
    r.frames(3);
    let strip = r.view().drawn.strip;
    let map = r.view().drawn.map.unwrap();
    let x = map.x_of(25.5);
    r.click(Pos2::new(x, strip.center().y + 6.0), Modifiers::NONE);
    assert_eq!(r.view().articulation, Some(r.staccato));
    assert!(
        r.h.query_all_by_label_contains("Articulation: Staccato")
            .count()
            >= 1
    );
    // the articulations tab adds one through sampler.keyswitch_set
    r.h.get_by_label("Articulations").click();
    r.frames(3);
    r.h.get_by_label("Add articulation").click();
    r.frames(2);
    r.settle();
    assert_eq!(r.instrument().articulations.len(), 3);
}

#[test]
fn group_and_instrument_settings_go_through_commands() {
    let mut r = start();
    r.h.get_by_label("Groups").click();
    r.frames(3);
    r.h.get_by_label("Add group").click();
    r.frames(2);
    r.settle();
    assert_eq!(r.instrument().groups.len(), 3);
    let at =
        r.h.get_by_label("Staccato voice limit (0 = none)")
            .rect()
            .center();
    r.drag(at, at + Vec2::new(30.0, 0.0), Modifiers::NONE);
    let g: Vec<_> = r.instrument().groups.values().cloned().collect();
    assert!(
        g[1].voice_limit.is_some_and(|v| v > 0),
        "{:?}",
        g[1].voice_limit
    );
    // the generic panel
    r.h.get_by_label("Instrument").click();
    r.frames(3);
    let at = r.h.get_by_label("Instrument voice limit").rect().center();
    r.drag(at, at + Vec2::new(-40.0, 0.0), Modifiers::NONE);
    assert!(r.instrument().settings.voice_limit < 256);
    // modulation: one route added
    r.h.get_by_label("Modulation").click();
    r.frames(3);
    r.h.get_by_label("Add route").click();
    r.frames(2);
    r.settle();
    assert_eq!(r.instrument().modulation.len(), 1);
    // the envelopes tab: turning the filter on
    r.h.get_by_label("Envelopes, filter, LFOs").click();
    r.frames(3);
    r.h.get_by_label("Filter on").click();
    r.frames(2);
    r.settle();
    let g0 = r.instrument().groups.values().next().unwrap().clone();
    assert!(g0.filter.is_some());
}

// ---- drop to create ---------------------------------------------------------------------------

const SFZ: &str = "<group> lovel=0 hivel=127\n\
<region> sample=samples/C3.wav lokey=36 hikey=54 pitch_keycenter=48\n\
<region> sample=samples/C4.wav lokey=55 hikey=84 pitch_keycenter=60\n";

#[test]
fn a_dropped_sfz_becomes_a_sampler_channel_with_its_window() {
    let mut r = start();
    r.file("Lib/samples/C3.wav", &wav(1.0));
    r.file("Lib/samples/C4.wav", &wav(2.0));
    let sfz = r.file("Lib/Piano.sfz", SFZ.as_bytes());
    let pos = r.rack_point();
    let text = r.hover(&[&sfz], pos);
    assert!(text.contains("Haus Sampler"), "{text}");
    let before = r.sampler_channels().len();
    r.drop_at(&[&sfz], pos);
    let list = r.sampler_channels();
    assert_eq!(list.len(), before + 1, "a new Haus Sampler channel");
    let (ch, inst) = *list.last().unwrap();
    let i = r.app().core.project.instruments[&inst].clone();
    assert_eq!(i.zones.len(), 2);
    assert_eq!(i.source.as_ref().unwrap().format, "sfz");
    r.frames(4);
    assert!(
        r.app().ws.is_visible(WindowId::Instrument(ch)),
        "its window opens"
    );
}

#[test]
fn a_dropped_sample_folder_shows_the_automap_preview_then_imports() {
    let mut r = start();
    for n in ["Pad_C3.wav", "Pad_C4.wav", "Pad_G4.wav", "readme.txt"] {
        r.file(&format!("Pads/{n}"), &wav(1.5));
    }
    let folder = r.drops.path().join("Pads");
    let pos = r.rack_point();
    let text = r.hover(&[&folder], pos);
    assert!(text.contains("Haus Sampler"), "{text}");
    let before = r.sampler_channels().len();
    r.drop_at(&[&folder], pos);
    let form = match &r.app().core.dialog {
        Some(Dialog::SamplerFolder(f)) => f.clone(),
        other => panic!("no preview: {other:?}"),
    };
    assert_eq!(form.rows.len(), 3);
    let roots: Vec<u8> = form.rows.iter().map(|x| x.root).collect();
    assert_eq!(roots, [48, 60, 67]);
    assert_eq!(form.rows[0].keys, KeyRange { lo: 0, hi: 54 });
    assert_eq!(r.sampler_channels().len(), before, "nothing imported yet");
    assert!(
        r.app().core.paths.library_roots.is_empty(),
        "not a library folder"
    );
    r.h.get_by_label("Import").click();
    r.frames(2);
    r.settle();
    assert_eq!(r.sampler_channels().len(), before + 1);
    let (_, inst) = *r.sampler_channels().last().unwrap();
    assert_eq!(r.app().core.project.instruments[&inst].zones.len(), 3);
}

fn two_preset_sf2() -> Vec<u8> {
    use ph_sampler_formats::build::sf2::{Sf2Builder, Sf2Sample, Sf2Zone};
    let mut b = Sf2Builder::new();
    let tone: Vec<f32> = (0..6000).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
    let s = b.add_sample(Sf2Sample::from_f32("tone", &tone, 44_100, 60));
    let piano = b.add_instrument("Piano", vec![Sf2Zone::new().sample(s)]);
    let kit = b.add_instrument("Kit", vec![Sf2Zone::new().keys(36, 40).sample(s)]);
    b.add_preset("Piano", 0, 0, vec![Sf2Zone::new().instrument(piano)]);
    b.add_preset("Kit", 128, 0, vec![Sf2Zone::new().instrument(kit)]);
    b.build()
}

#[test]
fn an_sf2_with_several_presets_asks_which_one() {
    let mut r = start();
    let sf2 = r.file("Banks/Two.sf2", &two_preset_sf2());
    let pos = r.rack_point();
    let before = r.sampler_channels().len();
    r.drop_at(&[&sf2], pos);
    let form = match &r.app().core.dialog {
        Some(Dialog::SamplerPreset(f)) => f.clone(),
        other => panic!("no preset question: {other:?}"),
    };
    assert_eq!(form.presets.len(), 2);
    assert_eq!(r.sampler_channels().len(), before);
    r.h.get_by_label("128:0  Kit").click();
    r.frames(2);
    r.h.get_by_label("Import preset").click();
    r.frames(2);
    r.settle();
    assert_eq!(r.sampler_channels().len(), before + 1);
    let (_, inst) = *r.sampler_channels().last().unwrap();
    let i = &r.app().core.project.instruments[&inst];
    assert_eq!(i.name, "Kit");
    assert_eq!(
        i.zones.values().next().unwrap().keys,
        KeyRange { lo: 36, hi: 40 }
    );
}

#[test]
fn the_browser_lists_instrument_files_and_imports_them() {
    let mut r = start();
    r.file("Lib/samples/C3.wav", &wav(1.0));
    r.file("Lib/samples/C4.wav", &wav(2.0));
    let sfz = r.file("Lib/Piano.sfz", SFZ.as_bytes());
    let root = r.drops.path().join("Lib");
    r.app().core.browser.add_library(&root);
    let lib = r.app().core.browser.library.clone();
    let node = lib[0]
        .1
        .iter()
        .find(|n| n.name == "Piano.sfz")
        .expect("listed")
        .clone();
    let ph_ui::browser_model::Kind::Instrument { path } = &node.kind else {
        panic!("not an instrument: {:?}", node.kind)
    };
    assert_eq!(Path::new(path), sfz);
    // the preview renders a few notes offline, outside the project
    let audio = ph_ui::sampler_ui::preview_audio(&sfz).expect("preview renders");
    let peak = audio.channels[0].iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.01, "the preview is not silent: {peak}");
    let before = r.sampler_channels().len();
    r.app().core.add_channel_from(&node.name, &node.kind);
    r.settle();
    assert_eq!(r.sampler_channels().len(), before + 1);
}

// ---- save and collect -------------------------------------------------------------------------

#[test]
fn save_as_native_and_collect_into_the_project() {
    let mut r = start();
    // a linked instrument: imported from a library folder outside the project
    r.file("Lib/samples/C3.wav", &wav(1.0));
    r.file("Lib/samples/C4.wav", &wav(2.0));
    let sfz = r.file("Lib/Piano.sfz", SFZ.as_bytes());
    let pos = r.rack_point();
    r.drop_at(&[&sfz], pos);
    let (ch, inst) = *r.sampler_channels().last().unwrap();
    let folder = r.app().core.project.instruments[&inst]
        .folder
        .clone()
        .unwrap();
    assert!(!folder.starts_with("instruments/"), "linked: {folder}");
    r.app().core.sel.channel = Some(ch);
    r.frames(4);

    // Save as Haus instrument… (the dialog the button opens)
    assert!(r.h.query_all_by_label("Save as Haus instrument…").count() >= 1);
    r.app().core.dialog = Some(Dialog::SamplerSave(ph_ui::sampler_ui::SaveForm {
        instrument: inst,
        path: "saved instruments/Piano".into(),
    }));
    r.frames(2);
    r.h.get_by_label("Save instrument").click();
    r.frames(2);
    r.settle();
    let toml = r.dir.path().join("saved instruments/Piano/instrument.toml");
    assert!(toml.exists(), "instrument.toml written");
    assert!(
        r.dir
            .path()
            .join("saved instruments/Piano/samples")
            .is_dir()
    );

    // Collect into project
    r.app().core.send(ph_ui::mapping_sampler::collect(inst));
    r.settle();
    let folder = r.app().core.project.instruments[&inst]
        .folder
        .clone()
        .unwrap();
    assert!(folder.starts_with("instruments/"), "collected: {folder}");
    assert!(r.dir.path().join(&folder).join("samples").is_dir());

    // and Load Haus instrument… puts the saved one on a new channel
    let before = r.sampler_channels().len();
    r.app().core.dialog = Some(Dialog::SamplerLoad(ph_ui::sampler_ui::LoadForm {
        channel: None,
        path: "saved instruments/Piano".into(),
    }));
    r.frames(2);
    r.h.get_by_label("Load instrument").click();
    r.frames(2);
    r.settle();
    assert_eq!(r.sampler_channels().len(), before + 1);
}

#[test]
fn the_collect_button_runs_sampler_collect() {
    let mut r = start();
    r.file("Lib/samples/C3.wav", &wav(1.0));
    r.file("Lib/samples/C4.wav", &wav(2.0));
    let sfz = r.file("Lib/Piano.sfz", SFZ.as_bytes());
    let pos = r.rack_point();
    r.drop_at(&[&sfz], pos);
    let (ch, inst) = *r.sampler_channels().last().unwrap();
    // only the new instrument's window and nothing else
    {
        let ws = &mut r.app().ws;
        for id in ws.ids().collect::<Vec<_>>() {
            if id != WindowId::Playlist {
                ws.hide(id);
            }
        }
        ws.show(WindowId::Instrument(ch));
    }
    r.frames(4);
    r.h.get_by_label("Collect into project").click();
    r.frames(2);
    r.settle();
    let folder = r.app().core.project.instruments[&inst]
        .folder
        .clone()
        .unwrap();
    assert!(folder.starts_with("instruments/"), "collected: {folder}");
}

// ---- pictures ---------------------------------------------------------------------------------

fn shots_dir() -> Option<PathBuf> {
    (std::env::var_os("PH_SCREENSHOTS").is_some_and(|v| v == "1")).then(|| {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/reports/m3-ui");
        std::fs::create_dir_all(&d).unwrap();
        d
    })
}

/// The zone map at a few window sizes in both built-in themes (and a light test theme), with
/// a selection and a keyswitch shown. Pictures only with `PH_SCREENSHOTS=1` and a GPU adapter.
#[test]
fn zone_map_snapshots() {
    let Some(dir) = shots_dir() else {
        return;
    };
    if !gpu_available() {
        eprintln!("SKIP: no GPU adapter for offscreen rendering");
        return;
    }
    let light = ph_ui::theme::Theme::from_toml(
        r##"name = "Light test"
base = "neon-slate"
dark = false
[colors]
bg0 = "#e9edf2"
bg1 = "#dfe4ea"
bg2 = "#f4f6f9"
bg3 = "#d5dbe3"
bg4 = "#c8d0da"
line = "#a9b3c0"
line_soft = "#c3cad4"
text = "#10151c"
dim = "#3e4856"
faint = "#8a94a2"
accent = "#0077c8"
accent_hot = "#005c9c"
accent_ink = "#ffffff"
"##,
    )
    .expect("light theme parses");
    for (w, hgt) in [(1280.0, 860.0), (1600.0, 1000.0), (960.0, 720.0)] {
        for theme in ["neon-slate", "red-line", "light"] {
            let mut r = start_with(w, hgt, true);
            if theme == "light" {
                let app = r.app();
                app.core.theme = Arc::new(light.clone());
                light.apply(&app.core.ctx);
            } else {
                r.app().set_theme(theme);
            }
            let ch = r.ch;
            r.app().ws.maximize_toggle(WindowId::Instrument(ch));
            r.frames(4);
            let a = r.zone_rect(r.z.low_soft).center();
            let b = r.zone_rect(r.z.rr1).center();
            r.click(a, Modifiers::NONE);
            r.click(b, Modifiers::SHIFT);
            {
                let (inst, st) = (r.inst, r.staccato);
                let v = r.app().core.sampler.get_mut(&inst).unwrap();
                v.articulation = Some(st);
            }
            r.frames(8);
            match r.h.render() {
                Ok(img) => img
                    .save(dir.join(format!("sampler-{theme}-{}x{}.png", w as u32, hgt as u32)))
                    .unwrap(),
                Err(e) => eprintln!("no picture: {e}"),
            }
        }
    }
}
