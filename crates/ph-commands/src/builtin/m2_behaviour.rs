//! Behaviour tests for the M2 commands: `sample.*`, `audio.*`, `automation.*`, and the
//! stubs that name their track until it lands.

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// A host that "imports" any path to `samples/<file name>` and knows the format of every
/// `samples/*` and `factory:*` reference: 2 s of stereo at 48 kHz.
#[derive(Default)]
struct AudioHost {
    inner: NullHost,
    imported: Vec<String>,
}

impl Host for AudioHost {
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
        self.inner.ui(r)
    }
    fn import_file(&mut self, source: &Path, _: ImportKind, _: &Author) -> CmdResult<String> {
        let name = source.file_name().unwrap().to_string_lossy();
        if name.ends_with(".txt") {
            return Err(CmdError::Host(format!("cannot use {name} as a sample")));
        }
        self.imported.push(source.to_string_lossy().into());
        Ok(format!("samples/{name}"))
    }
    fn sample_info(&self, reference: &str) -> CmdResult<SampleInfo> {
        if reference.starts_with("samples/") || reference.starts_with("factory:") {
            Ok(SampleInfo {
                frames: 96_000,
                sample_rate: 48_000,
                channels: 2,
                root_key: None,
            })
        } else {
            Err(CmdError::Host(format!("cannot read {reference}")))
        }
    }
    /// Files with `loop` in their name are a 4-beat loop at 120 BPM with a transient per
    /// beat (more with a high sensitivity); anything else has no clear tempo.
    fn analyze_audio(&self, reference: &str, sensitivity: f32) -> CmdResult<AudioAnalysis> {
        self.sample_info(reference)?;
        let looped = reference.contains("loop");
        let mut onsets = vec![0, 24_000, 48_000, 72_000];
        if sensitivity > 0.8 {
            onsets.extend([12_000, 36_000]);
            onsets.sort();
        }
        Ok(AudioAnalysis {
            bpm: looped.then_some(120.0),
            confidence: if looped { 0.8 } else { 0.1 },
            loop_beats: looped.then_some(4),
            alternatives: vec![(60.0, 0.4), (240.0, 0.2)],
            onsets,
            sample_rate: 48_000,
            key: "test/onsets/v1".into(),
        })
    }
}

struct Rig {
    s: Session,
    h: AudioHost,
    who: Caller,
}

impl Rig {
    fn new() -> Self {
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("M2")),
            h: AudioHost::default(),
            who: Caller::human(),
        }
    }

    fn p(&self) -> &Project {
        self.s.project()
    }

    fn try_run(&mut self, cmd: &str, params: Value) -> CmdResult<Executed> {
        let out = self
            .s
            .execute(&self.who, Request::new(cmd, params), &mut self.h)?;
        Ok(out)
    }

    fn run(&mut self, cmd: &str, params: Value) -> Value {
        match self.try_run(cmd, params) {
            Ok(o) => o.result,
            Err(e) => panic!("{cmd}: {e}"),
        }
    }

    fn err(&mut self, cmd: &str, params: Value) -> CmdError {
        match self.try_run(cmd, params) {
            Ok(o) => panic!("{cmd} should fail, got {:?}", o.result),
            Err(e) => e,
        }
    }

    /// A 120 BPM loop clip on track 0 at bar 2; returns (sample, clip).
    fn loop_clip(&mut self) -> (u32, u32) {
        let out = self.run(
            "audio.add_clip",
            json!({ "track": 0, "start": 3840, "path": "C:/loops/beat.wav" }),
        );
        (
            out["sample"].as_u64().unwrap() as u32,
            out["clip"].as_u64().unwrap() as u32,
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
}

#[test]
fn sample_import_registers_samples() {
    let mut r = Rig::new();
    let out = r.run(
        "sample.import",
        json!({ "paths": ["C:/kit/kick.wav", "C:/kit/notes.txt"] }),
    );
    assert_eq!(out["samples"], json!([0]));
    assert_eq!(out["channels"], json!([0]));
    assert_eq!(out["skipped"][0]["path"], "C:/kit/notes.txt");
    let e = r.p().sample(SampleId(0)).unwrap();
    assert_eq!(
        (e.path.as_str(), e.name.as_str()),
        ("samples/kick.wav", "kick.wav")
    );
    assert_eq!((e.frames, e.sample_rate, e.channels), (96_000, 48_000, 2));
    assert!(matches!(&e.provenance.origin, Origin::Imported { from } if from == "C:/kit/kick.wav"));
    assert!(e.provenance.at.is_some());
    // importing the same file again reuses its registry entry
    let out = r.run("sample.import", json!({ "paths": ["C:/kit/kick.wav"] }));
    assert_eq!(out["samples"], json!([0]));
    assert_eq!(r.p().samples.len(), 1);
}

#[test]
fn sample_registry_commands() {
    let mut r = Rig::new();
    let (s, clip) = r.loop_clip();
    let list = r.run("sample.list", json!({}));
    assert_eq!(list["samples"][0]["name"], "beat.wav");
    assert_eq!(list["samples"][0]["seconds"], 2.0);
    assert_eq!(list["samples"][0]["clips"], 1);
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": 96.5 }));
    let e = r.p().sample(SampleId(s)).unwrap();
    assert_eq!(
        e.bpm,
        Some(SampleBpm {
            value: 96.5,
            source: BpmSource::User
        })
    );
    assert!(matches!(
        r.err("sample.set_bpm", json!({ "sample": s, "bpm": 5000 })),
        CmdError::Model(_)
    ));
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": null }));
    assert_eq!(r.p().sample(SampleId(s)).unwrap().bpm, None);
    r.run("sample.set_root_key", json!({ "sample": s, "key": 57 }));
    assert_eq!(r.p().sample(SampleId(s)).unwrap().root_key, Some(57));
    // in use: cannot be removed
    assert!(matches!(
        r.err("sample.remove", json!({ "sample": s })),
        CmdError::Model(_)
    ));
    r.run("playlist.remove_clip", json!({ "clip": clip }));
    r.run("sample.remove", json!({ "sample": s }));
    assert!(r.p().samples.is_empty());
    assert!(matches!(
        r.err("sample.detect_bpm", json!({ "sample": 9 })),
        CmdError::Model(ModelError::NoSample(_))
    ));
}

#[test]
fn audio_add_clip_from_a_file_a_factory_sample_or_the_registry() {
    let mut r = Rig::new();
    let (s, clip) = r.loop_clip();
    assert_eq!(r.h.imported, vec!["C:/loops/beat.wav".to_string()]);
    let c = r.p().clip(ClipId(clip)).unwrap();
    // 2 s at the project's 130 BPM
    assert_eq!((c.track, c.start, c.length, c.offset), (0, 3840, 4160, 0));
    assert_eq!(c.audio().unwrap(), &AudioClip::new(SampleId(s)));
    // a registered sample by id, synced, with an explicit length
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": 120 }));
    let out = r.run(
        "audio.add_clip",
        json!({ "track": 1, "start": 0, "sample": s, "tempo_sync": true, "stretch": "stretch" }),
    );
    // synced: the loop's own 4 beats, whatever the project tempo
    assert_eq!(out["length"], 3840);
    assert_eq!(out["warnings"], json!([]));
    // a factory reference is registered without importing; the same path is reused
    let a = r.run(
        "audio.add_clip",
        json!({ "track": 2, "start": 0, "path": "factory:drums/haus808/kick", "length": 960 }),
    );
    let b = r.run(
        "audio.add_clip",
        json!({ "track": 2, "start": 960, "path": "factory:drums/haus808/kick", "length": 960 }),
    );
    assert_eq!(a["sample"], b["sample"]);
    assert_eq!(r.p().samples.len(), 2);
    assert_eq!(r.h.imported.len(), 1);
    // a synced clip of a sample without tempo warns
    let w = r.run(
        "audio.add_clip",
        json!({ "track": 3, "start": 0, "sample": 1, "tempo_sync": true }),
    );
    assert!(w["warnings"][0].as_str().unwrap().contains("no tempo"));
    for bad in [
        json!({ "track": 0, "start": 0 }),
        json!({ "track": 0, "start": 0, "sample": s, "path": "x.wav" }),
        json!({ "track": 99, "start": 0, "sample": s }),
        json!({ "track": 0, "start": 0, "sample": 42 }),
        json!({ "track": 0, "start": 0, "sample": s, "length": 0 }),
        json!({ "track": 0, "start": 0, "sample": s, "source_offset": 96_000 }),
    ] {
        r.err("audio.add_clip", bad);
    }
}

#[test]
fn audio_clip_settings() {
    let mut r = Rig::new();
    let (_, clip) = r.loop_clip();
    r.run(
        "audio.set_fade",
        json!({ "clip": clip, "fade_in": { "length": 240, "curve": "s_curve" }, "fade_out": { "length": 480 } }),
    );
    let a = r.audio(clip);
    assert_eq!(
        a.fade_in,
        Fade {
            length: 240,
            curve: FadeCurve::SCurve
        }
    );
    assert_eq!(
        a.fade_out,
        Fade {
            length: 480,
            curve: FadeCurve::Linear
        }
    );
    assert!(matches!(
        r.err(
            "audio.set_fade",
            json!({ "clip": clip, "fade_out": { "length": 5000 } })
        ),
        CmdError::Model(_)
    ));
    r.run("audio.set_gain", json!({ "clip": clip, "gain_db": -3.5 }));
    assert_eq!(r.audio(clip).gain_db, -3.5);
    r.err("audio.set_gain", json!({ "clip": clip, "gain_db": 40 }));
    let out = r.run("audio.set_pitch", json!({ "clip": clip, "semitones": -5 }));
    assert_eq!(out, json!({ "clip": clip, "semitones": -5, "cents": 0.0 }));
    r.run("audio.set_pitch", json!({ "clip": clip, "cents": 12.5 }));
    assert_eq!(
        (r.audio(clip).pitch_semitones, r.audio(clip).fine_cents),
        (-5, 12.5)
    );
    r.run(
        "audio.set_reverse",
        json!({ "clip": clip, "reverse": true }),
    );
    assert!(r.audio(clip).reverse);
    r.run(
        "audio.set_stretch",
        json!({ "clip": clip, "mode": "slice", "tempo_sync": true }),
    );
    assert_eq!(
        (r.audio(clip).stretch, r.audio(clip).tempo_sync),
        (StretchMode::Slice, true)
    );
    r.run(
        "audio.set_range",
        json!({ "clip": clip, "start": 0, "length": 1920, "source_offset": 4800 }),
    );
    let c = r.p().clip(ClipId(clip)).unwrap();
    assert_eq!(
        (c.start, c.length, c.audio().unwrap().source_offset),
        (0, 1920, 4800)
    );
    r.err(
        "audio.set_range",
        json!({ "clip": clip, "source_offset": 96_000 }),
    );
    // audio commands refuse other clip kinds
    let pat = r.run("pattern.add", json!({ "name": "p" }))["pattern"].clone();
    let pc = r.run(
        "playlist.add_clip",
        json!({ "track": 5, "pattern": pat, "start": 0 }),
    )["clip"]
        .clone();
    assert!(matches!(
        r.err("audio.set_gain", json!({ "clip": pc, "gain_db": 0 })),
        CmdError::Model(ModelError::Invalid(m)) if m.contains("pattern clip")
    ));
    // one undo per command
    let before = r.p().clone();
    r.run("audio.set_gain", json!({ "clip": clip, "gain_db": 6 }));
    r.s.undo(&Author::Human).unwrap();
    assert_eq!(r.p(), &before);
}

#[test]
fn audio_split_and_fit_to_tempo() {
    let mut r = Rig::new();
    let (s, clip) = r.loop_clip();
    r.run("project.set_tempo", json!({ "bpm": 120 }));
    r.run("audio.set_range", json!({ "clip": clip, "length": 3840 }));
    r.run(
        "audio.set_fade",
        json!({ "clip": clip, "fade_in": { "length": 100 }, "fade_out": { "length": 200 } }),
    );
    // split one beat in: 0.5 s at 120 BPM = 24000 frames
    let out = r.run("audio.split", json!({ "clip": clip, "at": 3840 + 960 }));
    let right = out["clips"][1].as_u64().unwrap() as u32;
    let (l, rc) = (
        r.p().clip(ClipId(clip)).unwrap().clone(),
        r.p().clip(ClipId(right)).unwrap().clone(),
    );
    assert_eq!((l.start, l.length), (3840, 960));
    assert_eq!((rc.start, rc.length), (4800, 2880));
    let (la, ra) = (l.audio().unwrap(), rc.audio().unwrap());
    assert_eq!((la.source_offset, ra.source_offset), (0, 24_000));
    assert_eq!((la.fade_in.length, la.fade_out.length), (100, 0));
    assert_eq!((ra.fade_in.length, ra.fade_out.length), (0, 200));
    r.err("audio.split", json!({ "clip": clip, "at": 3840 }));
    // fit to tempo needs the sample's BPM
    assert!(matches!(
        r.err("audio.fit_to_tempo", json!({ "clip": right })),
        CmdError::Model(ModelError::Invalid(m)) if m.contains("sample.set_bpm")
    ));
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": 90 }));
    let out = r.run("audio.fit_to_tempo", json!({ "clip": right }));
    // 72000 frames left = 1.5 s = 2.25 beats at the loop's 90 BPM
    assert_eq!(
        out,
        json!({ "clip": right, "length": 2160, "mode": "stretch" })
    );
    let a = r.audio(right);
    assert!(a.tempo_sync && a.stretch == StretchMode::Stretch);
}

#[test]
fn sample_detect_bpm_stores_a_detected_tempo_but_keeps_the_users() {
    let mut r = Rig::new();
    let out = r.run(
        "audio.add_clip",
        json!({ "track": 0, "start": 0, "path": "C:/loops/drumloop.wav" }),
    );
    let s = out["sample"].as_u64().unwrap() as u32;
    let out = r.run("sample.detect_bpm", json!({ "sample": s }));
    assert_eq!(out["bpm"], 120.0);
    assert_eq!(out["loop_beats"], 4);
    assert_eq!(out["onsets"], 4);
    assert_eq!(out["kept_user_bpm"], Value::Null);
    assert_eq!(out["alternatives"][0], json!({ "bpm": 60.0, "score": 0.4 }));
    let e = r.p().sample(SampleId(s)).unwrap();
    assert_eq!(
        e.bpm,
        Some(SampleBpm {
            value: 120.0,
            source: BpmSource::Detected
        })
    );
    assert_eq!(e.analysis.onsets.as_deref(), Some("test/onsets/v1"));
    // a tempo the user set is never overwritten
    r.run("sample.set_bpm", json!({ "sample": s, "bpm": 95 }));
    let out = r.run("sample.detect_bpm", json!({ "sample": s }));
    assert_eq!(out["kept_user_bpm"], 95.0);
    assert_eq!(r.p().sample(SampleId(s)).unwrap().bpm.unwrap().value, 95.0);
    // no clear beat: nothing stored
    let (other, _) = r.loop_clip();
    assert!(matches!(
        r.err("sample.detect_bpm", json!({ "sample": other })),
        CmdError::Model(ModelError::Invalid(m)) if m.contains("no clear tempo")
    ));
    assert_eq!(r.p().sample(SampleId(other)).unwrap().bpm, None);
}

#[test]
fn fit_to_tempo_detects_a_missing_tempo() {
    let mut r = Rig::new();
    r.run("project.set_tempo", json!({ "bpm": 100 }));
    let out = r.run(
        "audio.add_clip",
        json!({ "track": 1, "start": 0, "path": "C:/loops/drumloop.wav" }),
    );
    let (s, clip) = (
        out["sample"].as_u64().unwrap() as u32,
        out["clip"].as_u64().unwrap() as u32,
    );
    let out = r.run("audio.fit_to_tempo", json!({ "clip": clip }));
    // 2 s at the detected 120 BPM = 4 beats = 3840 ticks
    assert_eq!(
        out,
        json!({ "clip": clip, "length": 3840, "mode": "stretch", "detected_bpm": 120.0 })
    );
    assert_eq!(
        r.p().sample(SampleId(s)).unwrap().bpm.unwrap().source,
        BpmSource::Detected
    );
    // one undo step takes back the tempo and the fit together
    r.run("history.undo", json!({}));
    assert_eq!(r.p().sample(SampleId(s)).unwrap().bpm, None);
    assert!(!r.audio(clip).tempo_sync);
}

#[test]
fn slicer_detect_returns_slice_points() {
    let mut r = Rig::new();
    let out = r.run(
        "audio.add_clip",
        json!({ "track": 0, "start": 0, "path": "C:/loops/drumloop.wav" }),
    );
    let s = out["sample"].as_u64().unwrap();
    let out = r.run("slicer.detect", json!({ "sample": s }));
    assert_eq!(out["slices"], json!([0, 24_000, 48_000, 72_000]));
    assert_eq!(out["seconds"], json!([0.0, 0.5, 1.0, 1.5]));
    assert_eq!(out["sensitivity"], 0.5);
    let out = r.run("slicer.detect", json!({ "sample": s, "sensitivity": 0.9 }));
    assert_eq!(out["slices"].as_array().unwrap().len(), 6);
    assert!(matches!(
        r.err("slicer.detect", json!({ "sample": s, "sensitivity": 2.0 })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("slicer.detect", json!({ "sample": 77 })),
        CmdError::Model(ModelError::NoSample(_))
    ));
    // read-only
    assert_eq!(r.s.history().len(), 1);
}

#[test]
fn automation_clips_and_points() {
    let mut r = Rig::new();
    // no points given: one point at the current value (like "create automation clip")
    let out = r.run(
        "automation.add_clip",
        json!({ "target": "insert/0/volume", "track": 4, "start": 0 }),
    );
    let auto = out["clip"].as_u64().unwrap() as u32;
    let p0 = out["points"][0].as_u64().unwrap() as u32;
    let c = r.p().clip(ClipId(auto)).unwrap();
    assert_eq!(
        (
            c.length,
            c.automation().unwrap().target.to_string().as_str()
        ),
        (3840, "insert/0/volume")
    );
    assert_eq!(r.p().point(ClipId(auto), PointId(p0)).unwrap().value, 1.0);
    let out = r.run(
        "automation.add_point",
        json!({ "clip": auto, "tick": 1920, "value": 0.25, "curve": "s_curve", "tension": 0.5 }),
    );
    let p1 = out["point"].as_u64().unwrap() as u32;
    let pt = r.p().point(ClipId(auto), PointId(p1)).unwrap().clone();
    assert_eq!(
        (pt.tick, pt.value, pt.curve, pt.tension),
        (1920, 0.25, CurveKind::SCurve, 0.5)
    );
    let out = r.run(
        "automation.move_point",
        json!({ "clip": auto, "point": p1, "tick": 2880 }),
    );
    assert_eq!(
        out,
        json!({ "clip": auto, "point": p1, "tick": 2880, "value": 0.25 })
    );
    r.run(
        "automation.set_curve",
        json!({ "clip": auto, "point": p0, "curve": "hold" }),
    );
    assert_eq!(
        r.p().point(ClipId(auto), PointId(p0)).unwrap().curve,
        CurveKind::Hold
    );
    // values are checked against the target's range
    r.err(
        "automation.add_point",
        json!({ "clip": auto, "tick": 0, "value": 3.0 }),
    );
    r.err(
        "automation.move_point",
        json!({ "clip": auto, "point": p1, "value": -1 }),
    );
    r.run(
        "automation.remove_points",
        json!({ "clip": auto, "points": [p0] }),
    );
    assert_eq!(
        r.p()
            .clip(ClipId(auto))
            .unwrap()
            .automation()
            .unwrap()
            .points
            .len(),
        1
    );
    // retarget; out-of-range points refuse a target that cannot hold them
    r.run(
        "automation.set_target",
        json!({ "clip": auto, "target": "channel/9/pan" }),
    );
    assert_eq!(
        r.p()
            .clip(ClipId(auto))
            .unwrap()
            .automation()
            .unwrap()
            .target,
        ParamPath::ChannelPan(ChannelId(9))
    );
    r.run(
        "automation.add_point",
        json!({ "clip": auto, "tick": 0, "value": -0.5 }),
    );
    r.err(
        "automation.set_target",
        json!({ "clip": auto, "target": "channel/9/volume" }),
    );
    // tempo automation with explicit points; bad paths are bad params
    let out = r.run(
        "automation.add_clip",
        json!({ "target": "tempo", "track": 5, "start": 0, "length": 7680,
                "points": [{ "tick": 0, "value": 120 }, { "tick": 7680, "value": 140, "curve": "smooth" }] }),
    );
    assert_eq!(out["points"].as_array().unwrap().len(), 2);
    r.err(
        "automation.add_clip",
        json!({ "target": "tempo", "track": 5, "start": 0, "points": [{ "tick": 0, "value": 5 }] }),
    );
    assert!(matches!(
        r.err(
            "automation.add_clip",
            json!({ "target": "mixer/1/volume", "track": 0, "start": 0 })
        ),
        CmdError::BadParams { .. }
    ));
    // removing the clip removes its points, and undo brings both back
    let before = r.p().clone();
    r.run("playlist.remove_clip", json!({ "clip": auto }));
    assert!(r.p().clips.get(&ClipId(auto)).is_none());
    r.s.undo(&Author::Human).unwrap();
    assert_eq!(r.p(), &before);
    // automation commands refuse other clip kinds
    let (_, audio) = r.loop_clip();
    r.err(
        "automation.add_point",
        json!({ "clip": audio, "tick": 0, "value": 0 }),
    );
}

#[test]
fn automation_add_clip_without_a_track_takes_the_first_free_one() {
    let mut r = Rig::new();
    let first = r.run(
        "automation.add_clip",
        json!({ "target": "insert/0/volume", "start": 0, "length": 3840 }),
    );
    assert_eq!(first["track"], 0);
    // overlapping the first: the next track; clear of it: the first track again
    let second = r.run(
        "automation.add_clip",
        json!({ "target": "insert/0/pan", "start": 1920, "length": 3840 }),
    );
    assert_eq!(second["track"], 1);
    let third = r.run(
        "automation.add_clip",
        json!({ "target": "insert/0/pan", "start": 3840, "length": 960 }),
    );
    assert_eq!(third["track"], 0);
    // a send level is a target, with its own range
    r.run(
        "mixer.add_send",
        json!({ "from": 1, "to": 2, "level": 0.5 }),
    );
    let out = r.run(
        "automation.add_clip",
        json!({ "target": "insert/1/send/2", "start": 0 }),
    );
    let pt = r
        .p()
        .point(
            ClipId(out["clip"].as_u64().unwrap() as u32),
            PointId(out["points"][0].as_u64().unwrap() as u32),
        )
        .unwrap()
        .clone();
    assert_eq!(pt.value, 0.5, "starts at the send's level");
    assert!(matches!(
        r.err(
            "automation.add_clip",
            json!({ "target": "insert/1/send/2", "start": 0, "points": [{ "tick": 0, "value": 2.5 }] })
        ),
        CmdError::Model(ModelError::Invalid(_))
    ));
}

#[test]
fn automation_record_thins_samples_and_holds_across_pauses() {
    let mut r = Rig::new();
    // a drag: a straight ramp 0 -> 1 over one beat sampled every 10 ticks, a pause of 2 beats,
    // then a second short move; 200 samples become a handful of points
    let mut samples: Vec<Value> = (0..=96)
        .map(|k| json!({ "tick": 1000 + k * 10, "value": k as f64 / 96.0 }))
        .collect();
    samples.extend(
        (0..=20).map(|k| json!({ "tick": 3880 + k * 10, "value": 0.5 + 0.001 * k as f64 })),
    );
    let n = samples.len();
    // shuffled input is fine: positions decide
    samples.reverse();
    let out = r.run(
        "automation.record",
        json!({ "target": "insert/0/volume", "samples": samples.clone(), "tolerance": 0.002 }),
    );
    assert_eq!(out["samples"], n);
    assert_eq!(out["start"], 1000);
    assert_eq!(out["length"], 3080);
    assert_eq!(out["track"], 0);
    let clip = ClipId(out["clip"].as_u64().unwrap() as u32);
    let a = r.p().clip(clip).unwrap().automation().unwrap().clone();
    let pts = a.points_in_order();
    assert!(pts.len() >= 4 && pts.len() < 14, "{} points", pts.len());
    assert_eq!((pts[0].tick, pts[0].value), (0, 0.0));
    // the straight ramp keeps only its two ends; the first run ends at the pause
    let end_of_ramp = pts.iter().find(|p| p.tick == 960).unwrap();
    assert!((end_of_ramp.value - 1.0).abs() < 1e-9);
    assert_eq!(end_of_ramp.curve, CurveKind::Hold, "held across the pause");
    let second_start = pts.iter().find(|p| p.tick == 2880).unwrap();
    assert_eq!(second_start.value, 0.5);
    // the thinned curve stays within tolerance (0.2% of the 0..2 range) of every sample
    let start = out["start"].as_u64().unwrap() as f64;
    for s in &samples {
        let (t, v) = (s["tick"].as_f64().unwrap(), s["value"].as_f64().unwrap());
        let got = a.value_at(t - start).unwrap();
        assert!((got - v).abs() <= 0.004 + 1e-9, "tick {t}: {got} vs {v}");
    }
    assert_eq!(r.p().clip(clip).unwrap().length, 3080);
    // one undo step removes the clip with all its points
    r.s.undo(&Author::Human).unwrap();
    assert!(r.p().clips.get(&clip).is_none());
    // errors: no samples, values outside the target's range, a bad track
    assert!(matches!(
        r.err(
            "automation.record",
            json!({ "target": "tempo", "samples": [] })
        ),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err(
            "automation.record",
            json!({ "target": "insert/0/volume", "samples": [{ "tick": 0, "value": 3.0 }] })
        ),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err(
            "automation.record",
            json!({ "target": "tempo", "samples": [{ "tick": 0, "value": 120.0 }], "track": 9999 })
        ),
        CmdError::Model(ModelError::NoTrack(_))
    ));
    // a single sample is a one-step clip with one point
    let one = r.run(
        "automation.record",
        json!({ "target": "tempo", "samples": [{ "tick": 480, "value": 150.0 }] }),
    );
    assert_eq!(one["points"].as_array().unwrap().len(), 1);
}

#[test]
fn m2_stubs_name_their_track() {
    let mut r = Rig::new();
    let cases: [(&str, Value, &str); 14] = [
        ("record.list_inputs", json!({}), "T8"),
        ("record.arm", json!({ "input": 0, "track": 2 }), "T8"),
        ("record.disarm", json!({ "input": 0 }), "T8"),
        (
            "record.set_monitor",
            json!({ "input": 0, "on": true }),
            "T8",
        ),
        (
            "record.start",
            json!({ "count_in_bars": 1, "punch": [0, 3840] }),
            "T8",
        ),
        ("record.stop", json!({}), "T8"),
        ("midi.list_inputs", json!({}), "T8"),
        (
            "midi.learn",
            json!({ "target": "channel/0/param/cutoff" }),
            "T8",
        ),
        (
            "midi.map",
            json!({ "controller": 74, "target": "insert/1/volume" }),
            "T8",
        ),
        ("midi.unmap", json!({ "controller": 74 }), "T8"),
        ("midi.list_mappings", json!({}), "T8"),
        (
            "slicer.make_instrument",
            json!({ "sample": 0, "write_pattern": true }),
            "T6",
        ),
        ("editor.open", json!({ "sample": 0 }), "T6"),
        (
            "editor.apply",
            json!({ "sample": 0, "operation": "normalize" }),
            "T6",
        ),
    ];
    for (cmd, params, track) in cases {
        match r.err(cmd, params) {
            CmdError::Unavailable {
                command, track: t, ..
            } => {
                assert_eq!((command.as_str(), t.as_str()), (cmd, track));
            }
            e => panic!("{cmd}: {e}"),
        }
    }
    // params are still checked first, so agents learn the schema now
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0, "trak": 2 })),
        CmdError::BadParams { .. }
    ));
}
