//! Recording and MIDI end to end (M2/T8): commands through a `Studio` on the silent engine,
//! with simulated audio inputs and a fake MIDI source. Nothing here opens a real device.
//!
//! The silent engine runs in real time, so these tests wait for effects with a timeout rather
//! than for fixed frame counts, and assert structure (what was placed where) over exact
//! sample positions, which the engine's own tests pin down.

use super::*;
use ph_commands::{Grant, Scope};
use ph_engine::midi::{FakeMidi, MappingStore};
use ph_engine::record::SimulatedInputs;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn studio(dir: &std::path::Path) -> Studio {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let config = StudioConfig {
        project_dir: dir.to_path_buf(),
        assets_dir: assets,
        library_roots: vec![],
        exports_dir: None,
        projects_dir: None,
        engine: EngineConfig::default(),
        audio: AudioMode::Silent,
        plugins: Default::default(),
    };
    let mut p = Project::empty("rec");
    p.settings.bpm = 120.0;
    Studio::start(config, p).unwrap()
}

fn agent() -> Caller {
    Caller::agent("claude", Grant(vec![Scope::Full]))
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn run(s: &Studio, cmd: &str, params: Value) -> Value {
    match s.run(&agent(), cmd, params) {
        Ok(o) => o.result,
        Err(e) => panic!("{cmd}: {e}"),
    }
}

/// A stereo ramp, 8 s long, so a take of any realistic length stays inside it.
fn inputs() -> Box<SimulatedInputs> {
    let l: Vec<f32> = (0..8 * 48_000).map(|i| (i % 1000) as f32 / 1000.0).collect();
    let r = l.iter().map(|v| -v).collect();
    Box::new(SimulatedInputs::new().with("Scripted: In 1-2", 48_000, vec![l, r], 240))
}

#[test]
fn an_agent_records_a_take_into_the_project_in_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    s.engine.set_input_provider(Some(inputs()));
    let v = run(&s, "record.list_inputs", json!({}));
    assert_eq!(v["inputs"][0]["name"], "Scripted: In 1-2");
    run(&s, "record.arm", json!({ "input": 0, "track": 0, "monitor": false }));
    let v = run(&s, "record.list_inputs", json!({}));
    assert_eq!(v["inputs"][0]["armed"], true);
    assert_eq!(v["latency"]["input_frames"], 240);
    run(&s, "record.start", json!({}));
    assert!(run(&s, "transport.get_state", json!({}))["recording"] == true);
    wait_for("the transport to move", || s.engine.status().position > 1920);
    let before_clips = s.project().clips.len();
    let out = run(&s, "record.stop", json!({}));
    let clip = &out["takes"][0];
    assert_eq!(clip["track"], 0);
    assert_eq!(clip["latency_frames"], 240);
    let file = clip["file"].as_str().unwrap();
    assert!(file.starts_with("samples/Take 1"), "{file}");
    // the file is a real WAV in the project folder
    let path = dir.path().join(file);
    let wav = hound::WavReader::open(&path).unwrap();
    assert_eq!(wav.spec().channels, 2);
    assert!(wav.duration() > 4_000, "{} frames", wav.duration());
    // and the project holds the sample and the clip
    wait_for("the snapshot", || s.project().clips.len() == before_clips + 1);
    let p = s.project();
    let entry = p.sample_by_path(file).expect("registered");
    assert!(matches!(
        entry.provenance.origin,
        ph_core::Origin::Recorded(_)
    ));
    let c = p
        .clips
        .values()
        .find_map(|c| c.audio().map(|a| (c, a)))
        .expect("an audio clip");
    assert_eq!(c.1.source_offset, 240);
    assert_eq!(c.1.sample, entry.id);
    // the transport stopped, and nothing records any more
    assert!(!s.engine.status().playing);
    assert!(run(&s, "transport.get_state", json!({}))["recording"] == false);
    // one undo step takes the clip and the registry entry away; the file stays on disk
    run(&s, "history.undo", json!({}));
    wait_for("the snapshot", || s.project().clips.len() == before_clips);
    assert!(s.project().sample_by_path(file).is_none());
    assert!(path.exists(), "undo never deletes a recording");
    // recording again makes the next numbered file
    run(&s, "record.start", json!({ "midi_notes": false }));
    wait_for("the transport to move", || s.engine.status().position > 480);
    let out = run(&s, "record.stop", json!({}));
    assert!(
        out["takes"][0]["file"].as_str().unwrap().starts_with("samples/Take 2"),
        "{out}"
    );
}

#[test]
fn recording_without_an_armed_input_or_a_pattern_explains_itself() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let e = s.run(&agent(), "record.start", json!({})).unwrap_err();
    assert!(e.to_string().contains("nothing to record"), "{e}");
    let e = s.run(&agent(), "record.stop", json!({})).unwrap_err();
    assert!(e.to_string().contains("not recording"), "{e}");
    // no device in a silent studio
    let e = s
        .run(&agent(), "record.arm", json!({ "input": 0, "track": 0 }))
        .unwrap_err();
    assert!(e.to_string().contains("no audio input"), "{e}");
}

#[test]
fn midi_notes_played_while_recording_land_in_the_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let fake = FakeMidi::new();
    fake.add_port("Scripted Keys");
    s.midi.set_backend(Box::new(fake.clone()));
    wait_for("the port", || fake.connected() == ["Scripted Keys"]);
    let pat = run(&s, "pattern.add", json!({}))["pattern"].clone();
    let ch = run(
        &s,
        "channel.add",
        json!({ "name": "Keys", "instrument": { "kind": "synth" } }),
    )["channel"]
        .clone();
    let chan = ph_core::ChannelId(ch.as_u64().unwrap() as u32);
    wait_for("the snapshot", || s.project().channels.contains_key(&chan));
    run(&s, "transport.set_mode", json!({ "mode": "pattern", "pattern": pat }));
    // the selected channel gets the keyboard
    s.midi.set_target_channel(Some(chan));
    let out = run(&s, "record.start", json!({}));
    assert_eq!(out["notes"], true);
    wait_for("the transport to move", || s.engine.status().position > 0);
    assert!(fake.send("Scripted Keys", &[0x90, 64, 100]));
    wait_for("the note on", || {
        s.midi
            .drain_events()
            .iter()
            .any(|e| matches!(e, ph_engine::midi::MidiEvent::Note { on: true, key: 64 }))
    });
    std::thread::sleep(Duration::from_millis(120));
    fake.send("Scripted Keys", &[0x80, 64, 0]);
    wait_for("the note off", || {
        s.midi
            .drain_events()
            .iter()
            .any(|e| matches!(e, ph_engine::midi::MidiEvent::Note { on: false, key: 64 }))
    });
    let out = run(&s, "record.stop", json!({ "quantize": 240 }));
    assert_eq!(out["notes"].as_array().unwrap().len(), 1, "{out}");
    wait_for("the snapshot", || {
        s.project()
            .pattern(ph_core::PatternId(pat.as_u64().unwrap() as u32))
            .is_ok_and(|p| p.notes.get(&chan).is_some_and(|n| n.len() == 1))
    });
    let p = s.project();
    let pattern = p
        .pattern(ph_core::PatternId(pat.as_u64().unwrap() as u32))
        .unwrap();
    let n = pattern.notes[&chan].values().next().unwrap();
    assert_eq!(n.key, 64);
    assert_eq!(n.start % 240, 0, "quantised to a step");
    assert!(n.length >= 1);
    // one undo removes the notes again
    run(&s, "history.undo", json!({}));
    wait_for("the snapshot", || {
        s.project()
            .pattern(ph_core::PatternId(pat.as_u64().unwrap() as u32))
            .is_ok_and(|p| p.notes.get(&chan).is_none_or(|n| n.is_empty()))
    });
}

#[test]
fn midi_learn_and_mappings_work_through_commands_and_persist_per_user() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    s.midi.set_store(MappingStore::open(cfg.path()));
    let fake = FakeMidi::new();
    fake.add_port("Scripted Keys");
    s.midi.set_backend(Box::new(fake.clone()));
    wait_for("the port", || fake.connected() == ["Scripted Keys"]);
    let l = run(&s, "midi.list_inputs", json!({}));
    assert_eq!(l["inputs"][0]["name"], "Scripted Keys");
    // learn from the keyboard
    let out = run(&s, "midi.learn", json!({ "target": "tempo" }));
    assert_eq!(out["learning"], "tempo");
    fake.send("Scripted Keys", &[0xb0, 74, 64]);
    wait_for("the mapping", || {
        run(&s, "midi.list_mappings", json!({}))["mappings"]
            .as_array()
            .is_some_and(|m| m.len() == 1)
    });
    let m = run(&s, "midi.list_mappings", json!({}));
    assert_eq!(m["mappings"][0]["controller"], 74);
    assert_eq!(m["mappings"][0]["device"], "Scripted Keys");
    assert_eq!(m["mappings"][0]["target"], "tempo");
    // by hand
    run(&s, "midi.map", json!({ "controller": 1, "target": "tempo" }));
    // the file is in the config folder, not the project folder
    let text = std::fs::read_to_string(cfg.path().join("midi-mappings.json")).unwrap();
    assert!(text.contains("\"controller\": 74") && text.contains("\"controller\": 1"), "{text}");
    assert!(!dir.path().join("midi-mappings.json").exists());
    // a new session in the same config folder finds them
    assert_eq!(MappingStore::open(cfg.path()).list().len(), 2);
    run(&s, "midi.unmap", json!({ "controller": 74, "device": "Scripted Keys" }));
    run(&s, "midi.unmap", json!({ "controller": 1 }));
    assert_eq!(MappingStore::open(cfg.path()).list().len(), 0);
}
