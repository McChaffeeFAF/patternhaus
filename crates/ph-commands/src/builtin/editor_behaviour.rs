//! Behaviour tests for `editor.*`, `slicer.make_instrument` and `audio.duplicate` (M2/T6)
//! over an in-memory "file system": every `samples/*` reference is a buffer in a map, so the
//! tests can see exactly which files an edit wrote and that the original never changed.

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

const SR: u32 = 1000;

/// Files by reference; `import_file` makes a 1000-frame stereo file at 1 kHz whose left
/// channel is `i / 1000` and whose right channel is `-i / 1000`.
#[derive(Default)]
struct FsHost {
    inner: NullHost,
    files: BTreeMap<String, AudioBuffer>,
    ui: Vec<UiRequest>,
    writes: Vec<String>,
}

fn ramp(n: usize) -> AudioBuffer {
    AudioBuffer {
        channels: vec![
            (0..n).map(|i| i as f32 / 1000.0).collect(),
            (0..n).map(|i| -(i as f32) / 1000.0).collect(),
        ],
        sample_rate: SR,
        root_key: None,
    }
}

impl Host for FsHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        self.inner.transport(cmd)
    }
    fn transport_state(&self) -> TransportState {
        self.inner.transport_state()
    }
    fn render(&mut self, p: &Project, r: &RenderRequest) -> CmdResult<RenderReport> {
        self.inner.render(p, r)
    }
    fn resolve_path(&self, p: &str, a: FileAccess) -> CmdResult<std::path::PathBuf> {
        self.inner.resolve_path(p, a)
    }
    fn ui(&mut self, r: UiRequest) -> CmdResult<()> {
        self.ui.push(r);
        Ok(())
    }
    fn import_file(&mut self, source: &Path, _: ImportKind, _: &Author) -> CmdResult<String> {
        let name = source.file_name().unwrap().to_string_lossy().into_owned();
        let r = format!("samples/{name}");
        self.files.entry(r.clone()).or_insert_with(|| {
            let mut b = ramp(1000);
            if name.contains("44k") {
                b.sample_rate = 44_100;
            }
            b
        });
        Ok(r)
    }
    fn sample_info(&self, reference: &str) -> CmdResult<SampleInfo> {
        let b = self
            .files
            .get(reference)
            .ok_or_else(|| CmdError::Host(format!("cannot read {reference}")))?;
        Ok(SampleInfo {
            frames: b.frames() as u64,
            sample_rate: b.sample_rate,
            channels: b.channels.len() as u16,
            root_key: None,
        })
    }
    fn read_audio(&self, reference: &str) -> CmdResult<AudioBuffer> {
        self.files
            .get(reference)
            .cloned()
            .ok_or_else(|| CmdError::Host(format!("cannot read {reference}")))
    }
    fn write_audio(&mut self, name: &str, audio: &AudioBuffer) -> CmdResult<String> {
        let mut n = 1;
        let r = loop {
            let r = if n == 1 {
                format!("samples/{name}.wav")
            } else {
                format!("samples/{name} ({n}).wav")
            };
            if !self.files.contains_key(&r) {
                break r;
            }
            n += 1;
        };
        self.files.insert(r.clone(), audio.clone());
        self.writes.push(r.clone());
        Ok(r)
    }
    fn analyze_audio(&self, reference: &str, sensitivity: f32) -> CmdResult<AudioAnalysis> {
        self.sample_info(reference)?;
        let mut onsets = vec![0, 250, 500, 750];
        if sensitivity > 0.8 {
            onsets.push(125);
            onsets.sort();
        }
        Ok(AudioAnalysis {
            bpm: Some(120.0),
            confidence: 0.9,
            loop_beats: Some(2),
            alternatives: vec![],
            onsets,
            sample_rate: SR,
            key: "t".into(),
        })
    }
}

struct Rig {
    s: Session,
    h: FsHost,
}

impl Rig {
    fn new() -> Self {
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("T6")),
            h: FsHost::default(),
        }
    }
    fn p(&self) -> &Project {
        self.s.project()
    }
    fn try_run(&mut self, cmd: &str, params: Value) -> CmdResult<Executed> {
        self.s
            .execute(&Caller::human(), Request::new(cmd, params), &mut self.h)
    }
    fn run(&mut self, cmd: &str, params: Value) -> Value {
        self.try_run(cmd, params)
            .unwrap_or_else(|e| panic!("{cmd}: {e}"))
            .result
    }
    fn err(&mut self, cmd: &str, params: Value) -> CmdError {
        match self.try_run(cmd, params) {
            Ok(o) => panic!("{cmd} should fail, got {:?}", o.result),
            Err(e) => e,
        }
    }
    /// A clip of the 1000-frame ramp at tick 0 on track 0; returns (sample, clip).
    fn clip(&mut self) -> (u32, u32) {
        let o = self.run(
            "audio.add_clip",
            json!({ "track": 0, "start": 0, "path": "C:/x/ramp.wav", "length": 1920 }),
        );
        (
            o["sample"].as_u64().unwrap() as u32,
            o["clip"].as_u64().unwrap() as u32,
        )
    }
    fn audio(&self, clip: u32) -> AudioClip {
        self.p()
            .clip(ClipId(clip))
            .unwrap()
            .audio()
            .unwrap()
            .clone()
    }
    fn file_of(&self, sample: u64) -> AudioBuffer {
        let e = self.p().sample(SampleId(sample as u32)).unwrap();
        self.h.files[&e.path].clone()
    }
}

#[test]
fn editor_open_asks_the_ui_for_the_editor_window() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    let out = r.run("editor.open", json!({ "sample": s }));
    assert_eq!(out["frames"], 1000);
    assert_eq!(out["sample_rate"], 1000);
    assert_eq!(
        r.h.ui,
        vec![UiRequest::Open {
            panel: format!("audio_editor:{s}")
        }]
    );
    // the panel name is accepted by ui.open_panel too, and a missing sample is refused
    r.run(
        "ui.open_panel",
        json!({ "panel": format!("audio_editor:{s}") }),
    );
    assert!(matches!(
        r.err("ui.open_panel", json!({ "panel": "audio_editor:77" })),
        CmdError::BadParams { .. }
    ));
    assert!(matches!(
        r.err("editor.open", json!({ "sample": 77 })),
        CmdError::Model(ModelError::NoSample(_))
    ));
}

#[test]
fn every_edit_writes_a_new_file_and_never_touches_the_original() {
    let mut r = Rig::new();
    let (s, clip) = r.clip();
    let original = r.file_of(s as u64);
    let orig_path = r.p().sample(SampleId(s)).unwrap().path.clone();
    let ops = [
        json!({ "operation": "trim", "start": 100, "end": 300 }),
        json!({ "operation": "cut", "start": 100, "end": 300 }),
        json!({ "operation": "silence", "start": 100, "end": 300 }),
        json!({ "operation": "normalize", "amount": -6.0 }),
        json!({ "operation": "fade_in", "end": 500, "curve": "s_curve" }),
        json!({ "operation": "fade_out", "start": 500 }),
        json!({ "operation": "reverse" }),
        json!({ "operation": "gain", "amount": 6.0 }),
        json!({ "operation": "paste", "from_start": 0, "from_end": 10, "at": 5 }),
    ];
    for (i, mut op) in ops.into_iter().enumerate() {
        op["sample"] = json!(s);
        op["repoint"] = json!(false);
        let o = r.run("editor.apply", op);
        assert_ne!(o["path"], json!(orig_path), "op {i} wrote a new file");
        assert_eq!(
            r.file_of(s as u64),
            original,
            "op {i}: the original is untouched"
        );
        assert_eq!(
            r.audio(clip).sample,
            SampleId(s),
            "repoint false leaves clips alone"
        );
    }
    assert_eq!(r.h.writes.len(), 9);
    let unique: std::collections::BTreeSet<_> = r.h.writes.iter().collect();
    assert_eq!(unique.len(), 9, "no file was written twice");
    // each one is a registry entry that says where it came from
    let edited = r
        .p()
        .samples
        .values()
        .filter(
            |e| matches!(&e.provenance.origin, Origin::Edited { from, .. } if *from == SampleId(s)),
        )
        .count();
    assert_eq!(edited, 9);
}

#[test]
fn edits_have_the_exact_result() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    let apply = |r: &mut Rig, mut p: Value| {
        p["sample"] = json!(s);
        p["repoint"] = json!(false);
        let o = r.run("editor.apply", p);
        r.file_of(o["sample"].as_u64().unwrap())
    };
    let t = apply(
        &mut r,
        json!({ "operation": "trim", "start": 100, "end": 300 }),
    );
    assert_eq!(t.frames(), 200);
    assert_eq!(t.channels[0][0], 0.1);
    assert_eq!(t.channels[1][0], -0.1);
    let c = apply(
        &mut r,
        json!({ "operation": "cut", "start": 100, "end": 300 }),
    );
    assert_eq!(c.frames(), 800);
    assert_eq!(c.channels[0][99], 0.099);
    assert_eq!(c.channels[0][100], 0.3);
    let z = apply(
        &mut r,
        json!({ "operation": "silence", "start": 10, "end": 20 }),
    );
    assert_eq!(z.channels[0][9], 0.009);
    assert_eq!(z.channels[0][10..20], [0.0; 10]);
    assert_eq!(z.channels[0][20], 0.02);
    let n = apply(&mut r, json!({ "operation": "normalize", "amount": -6.0 }));
    let peak = n.channels[0].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.501_187).abs() < 1e-4, "{peak}");
    let g = apply(&mut r, json!({ "operation": "gain", "amount": -20.0 }));
    assert!((g.channels[0][500] - 0.05).abs() < 1e-6);
    let rv = apply(
        &mut r,
        json!({ "operation": "reverse", "start": 0, "end": 4 }),
    );
    assert_eq!(rv.channels[0][..4], [0.003, 0.002, 0.001, 0.0]);
    let fi = apply(
        &mut r,
        json!({ "operation": "fade_in", "start": 0, "end": 100 }),
    );
    assert!(fi.channels[0][1] < 0.0001 && (fi.channels[0][99] - 0.099).abs() < 0.001);
    let fo = apply(&mut r, json!({ "operation": "fade_out", "start": 900 }));
    assert!(fo.channels[0][999] < 0.01 * 0.999 + 0.0001);
    let p = apply(
        &mut r,
        json!({ "operation": "paste", "from_start": 0, "from_end": 10, "at": 500 }),
    );
    assert_eq!(p.frames(), 1010);
    assert_eq!(p.channels[0][500], 0.0);
    assert_eq!(p.channels[0][510], 0.5);
}

#[test]
fn an_edit_is_one_undo_step_that_puts_the_pointers_back() {
    let mut r = Rig::new();
    let (s, clip) = r.clip();
    r.run(
        "channel.add",
        json!({ "name": "S", "instrument": { "kind": "sampler", "sample": "samples/ramp.wav", "root_key": 60 } }),
    );
    let before_history = r.s.history().len();
    let o = r.run(
        "editor.apply",
        json!({ "sample": s, "operation": "trim", "start": 200, "end": 800 }),
    );
    let new = SampleId(o["sample"].as_u64().unwrap() as u32);
    assert_eq!(o["clips_repointed"], 1);
    assert_eq!(o["channels_repointed"], 1);
    assert_eq!(r.s.history().len(), before_history + 1, "one step");
    assert_eq!(r.audio(clip).sample, new);
    assert!(matches!(
        &r.p().channels.values().next().unwrap().instrument,
        InstrumentSpec::Sampler { sample: Some(p), .. } if p == o["path"].as_str().unwrap()
    ));
    // the original is still registered and untouched; the new one has no tempo (new length)
    assert!(r.p().sample(SampleId(s)).is_ok());
    r.run("history.undo", json!({}));
    assert_eq!(r.audio(clip).sample, SampleId(s));
    assert!(
        r.p().sample(new).is_err(),
        "the new entry goes with the undo"
    );
    assert!(matches!(
        &r.p().channels.values().next().unwrap().instrument,
        InstrumentSpec::Sampler { sample: Some(p), .. } if p == "samples/ramp.wav"
    ));
    // the file stays on disk (nothing is ever deleted)
    assert!(r.h.files.contains_key(o["path"].as_str().unwrap()));
}

#[test]
fn trim_cut_and_paste_keep_clips_on_the_same_sound() {
    let mut r = Rig::new();
    let (s, clip) = r.clip();
    r.run(
        "audio.set_range",
        json!({ "clip": clip, "source_offset": 500 }),
    );
    // trim to 200..800: the clip's 500 becomes 300
    let o = r.run(
        "editor.apply",
        json!({ "sample": s, "operation": "trim", "start": 200, "end": 800 }),
    );
    assert_eq!(r.audio(clip).source_offset, 300);
    // cut 100..200 out of the new file: 300 stays, then cut 0..100: 300 -> 200
    let s2 = o["sample"].as_u64().unwrap();
    r.run(
        "editor.apply",
        json!({ "sample": s2, "operation": "cut", "start": 0, "end": 100 }),
    );
    assert_eq!(r.audio(clip).source_offset, 200);
    // paste 50 frames before it: pushed later by 50
    let s3 = r.audio(clip).sample.0;
    r.run(
        "editor.apply",
        json!({ "sample": s3, "operation": "paste", "from": s, "from_start": 0, "from_end": 50, "at": 10 }),
    );
    assert_eq!(r.audio(clip).source_offset, 250);
}

#[test]
fn a_length_preserving_edit_keeps_the_tempo_and_others_drop_it() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": 90.0 }));
    let o = r.run(
        "editor.apply",
        json!({ "sample": s, "operation": "reverse" }),
    );
    let e = r
        .p()
        .sample(SampleId(o["sample"].as_u64().unwrap() as u32))
        .unwrap();
    assert_eq!(e.bpm().unwrap(), 90.0);
    let o = r.run(
        "editor.apply",
        json!({ "sample": s, "operation": "trim", "start": 0, "end": 500 }),
    );
    let e = r
        .p()
        .sample(SampleId(o["sample"].as_u64().unwrap() as u32))
        .unwrap();
    assert_eq!(e.bpm(), None);
    assert_eq!(e.frames, 500);
}

#[test]
fn editor_apply_refuses_what_it_cannot_do() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    let bad = |r: &mut Rig, p: Value| {
        let mut p = p;
        p["sample"] = json!(s);
        assert!(
            matches!(
                r.err("editor.apply", p.clone()),
                CmdError::Model(ModelError::Invalid(_))
            ),
            "{p}"
        );
    };
    bad(&mut r, json!({ "operation": "trim", "start": 5, "end": 5 }));
    bad(&mut r, json!({ "operation": "cut" }));
    bad(&mut r, json!({ "operation": "gain", "amount": 99.0 }));
    bad(&mut r, json!({ "operation": "normalize", "amount": 3.0 }));
    bad(&mut r, json!({ "operation": "declick" })); // a ramp has no clicks
    bad(
        &mut r,
        json!({ "operation": "paste", "from_start": 9, "from_end": 9 }),
    );
    assert!(matches!(
        r.err(
            "editor.apply",
            json!({ "sample": s, "operation": "wobble" })
        ),
        CmdError::BadParams { .. }
    ));
    // silence cannot be normalised
    let o = r.run(
        "editor.apply",
        json!({ "sample": s, "operation": "silence" }),
    );
    let silent = o["sample"].as_u64().unwrap();
    bad_on(&mut r, silent, json!({ "operation": "normalize" }));
    // a different sample rate cannot be pasted
    let o = r.run(
        "audio.add_clip",
        json!({ "track": 1, "start": 0, "path": "C:/x/44k.wav" }),
    );
    let id = o["sample"].as_u64().unwrap();
    bad(&mut r, json!({ "operation": "paste", "from": id }));
}

fn bad_on(r: &mut Rig, sample: u64, p: Value) {
    let mut p = p;
    p["sample"] = json!(sample);
    assert!(matches!(
        r.err("editor.apply", p),
        CmdError::Model(ModelError::Invalid(_))
    ));
}

#[test]
fn declick_removes_clicks() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    // a smooth tone with two clicks, as a file the editor can read
    let tone: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
    let mut clicked = tone.clone();
    clicked[300] = 0.95;
    clicked[700] -= 0.5;
    let path = r.p().sample(SampleId(s)).unwrap().path.clone();
    r.h.files.get_mut(&path).unwrap().channels = vec![clicked.clone(), clicked];
    let o = r.run(
        "editor.apply",
        json!({ "sample": s, "operation": "declick" }),
    );
    assert_eq!(o["note"], "4 clicks removed");
    let f = r.file_of(o["sample"].as_u64().unwrap());
    for (i, (a, b)) in f.channels[0].iter().zip(&tone).enumerate() {
        assert!((a - b).abs() < 0.06, "frame {i}");
    }
}

#[test]
fn slicer_make_instrument_builds_a_sliced_sampler() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    let o = r.run(
        "slicer.make_instrument",
        json!({ "sample": s, "slices": [0, 250, 500, 750], "root_key": 48, "name": "Loop kit" }),
    );
    assert_eq!(o["slices"], 4);
    assert_eq!(
        (o["first_key"].as_u64(), o["last_key"].as_u64()),
        (Some(48), Some(51))
    );
    let ch = r
        .p()
        .channel(ChannelId(o["channel"].as_u64().unwrap() as u32))
        .unwrap();
    assert_eq!(ch.name, "Loop kit");
    assert!(matches!(
        &ch.instrument,
        InstrumentSpec::Sampler { sample: Some(p), root_key: 48, slices, .. }
            if p == "samples/ramp.wav" && slices == &[0, 250, 500, 750]
    ));
    assert!(o.get("pattern").is_none());
    // detection: sensitivity decides how many
    let o = r.run("slicer.make_instrument", json!({ "sample": s }));
    assert_eq!(o["slices"], 4);
    let o = r.run(
        "slicer.make_instrument",
        json!({ "sample": s, "sensitivity": 0.9 }),
    );
    assert_eq!(o["slices"], 5);
    // one undo step takes the channel back
    let n = r.p().channels.len();
    r.run("history.undo", json!({}));
    assert_eq!(r.p().channels.len(), n - 1);
}

#[test]
fn slicer_pattern_replays_the_loop() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    // 1 s of audio at 120 BPM is two beats = 1920 ticks; slices every quarter second
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": 120.0 }));
    let o = r.run(
        "slicer.make_instrument",
        json!({ "sample": s, "slices": [0, 250, 500, 750], "write_pattern": true }),
    );
    let pat = PatternId(o["pattern"].as_u64().unwrap() as u32);
    let ch = ChannelId(o["channel"].as_u64().unwrap() as u32);
    let pattern = r.p().pattern(pat).unwrap();
    let notes: Vec<(u8, Tick, Tick)> = pattern.notes[&ch]
        .values()
        .map(|n| (n.key, n.start, n.length))
        .collect();
    assert_eq!(
        notes,
        vec![
            (36, 0, 480),
            (37, 480, 480),
            (38, 960, 480),
            (39, 1440, 480)
        ]
    );
    assert_eq!(pattern.length, Some(3840), "rounded up to a whole bar");
    // it starts at the same place every time: keys follow the slice order
    assert_eq!(r.p().pattern_length(pat).unwrap(), 3840);
}

#[test]
fn slicer_make_instrument_checks_its_slices() {
    let mut r = Rig::new();
    let (s, _) = r.clip();
    for p in [
        json!({ "sample": s, "slices": [] }),
        json!({ "sample": s, "slices": [0, 500, 500] }),
        json!({ "sample": s, "slices": [500, 100] }),
        json!({ "sample": s, "slices": [0, 1000] }),
        json!({ "sample": s, "slices": [0, 1], "root_key": 127 }),
        json!({ "sample": s, "sensitivity": 3.0 }),
    ] {
        assert!(
            matches!(
                r.err("slicer.make_instrument", p.clone()),
                CmdError::Model(ModelError::Invalid(_))
            ),
            "{p}"
        );
    }
    assert!(matches!(
        r.err("slicer.make_instrument", json!({ "sample": 99 })),
        CmdError::Model(ModelError::NoSample(_))
    ));
}

#[test]
fn audio_duplicate_copies_every_setting() {
    let mut r = Rig::new();
    let (_, clip) = r.clip();
    r.run("audio.set_gain", json!({ "clip": clip, "gain_db": -3.0 }));
    r.run("audio.set_pitch", json!({ "clip": clip, "semitones": 2 }));
    r.run(
        "audio.set_reverse",
        json!({ "clip": clip, "reverse": true }),
    );
    r.run(
        "audio.set_fade",
        json!({ "clip": clip, "fade_in": { "length": 100, "curve": "exp" } }),
    );
    let o = r.run("audio.duplicate", json!({ "clip": clip }));
    let copy = o["clip"].as_u64().unwrap() as u32;
    assert_eq!(o["start"], 1920, "right after the original");
    assert_eq!(r.audio(copy), r.audio(clip));
    let c = r.p().clip(ClipId(copy)).unwrap();
    assert_eq!((c.track, c.start, c.length), (0, 1920, 1920));
    let o = r.run(
        "audio.duplicate",
        json!({ "clip": clip, "start": 100, "track": 3 }),
    );
    let c = r
        .p()
        .clip(ClipId(o["clip"].as_u64().unwrap() as u32))
        .unwrap();
    assert_eq!((c.track, c.start), (3, 100));
    assert!(matches!(
        r.err("audio.duplicate", json!({ "clip": clip, "track": 9999 })),
        CmdError::Model(ModelError::NoTrack(_))
    ));
    let pat = r.run("pattern.add", json!({}))["pattern"].as_u64().unwrap();
    let pc = r.run(
        "playlist.add_clip",
        json!({ "track": 1, "pattern": pat, "start": 0, "length": 3840 }),
    )["clip"]
        .as_u64()
        .unwrap();
    assert!(matches!(
        r.err("audio.duplicate", json!({ "clip": pc })),
        CmdError::Model(ModelError::Invalid(_))
    ));
}
