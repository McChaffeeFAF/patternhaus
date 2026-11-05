//! Format migrations: M1 (format 1) and M2 (format 2) projects load unchanged into format 3
//! (M3).
//!
//! Fixtures (committed, never regenerated):
//! - `fixtures/m2/<name>.json`: the six golden projects exactly as M2 saved them (format 2);
//! - `fixtures/m1/<name>.json`: the four golden projects exactly as M1 saved them;
//! - `fixtures/m1-folder/`: an M1 project folder (Sampler channel on `samples/hit.wav`, two
//!   pattern clips, and the `history/log.jsonl` M1 wrote while making it).

use ph_core::*;
use ph_formats::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// An older project's text with only the version number moved to the current one.
fn as_current(old: &str) -> String {
    let old = lf(old);
    let from = ["\"format_version\": 1,", "\"format_version\": 2,"]
        .into_iter()
        .find(|v| old.contains(v))
        .expect("fixture is version 1 or 2");
    let now = old.replacen(from, &format!("\"format_version\": {FORMAT_VERSION},"), 1);
    assert_ne!(now, old);
    now
}

const GOLDEN: [&str; 4] = [
    "arrangement-waltz",
    "drums-swing-choke",
    "mix-sidechain-sends",
    "synth-chords",
];

#[test]
fn m1_golden_projects_load_unchanged() {
    for name in GOLDEN {
        let v1 = std::fs::read_to_string(fixtures().join(format!("m1/{name}.json"))).unwrap();
        let p = project_from_json(&v1).unwrap();
        assert_eq!(p.format_version, FORMAT_VERSION);
        assert!(p.samples.is_empty());
        assert!(p.clips.values().all(|c| c.pattern().is_some()));
        // the model is the same as the committed (version 2) golden project ...
        let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/projects")
            .join(name);
        assert_eq!(p, load_project(&golden).unwrap(), "{name}");
        // ... and saving it writes the M1 bytes with only the version changed
        assert_eq!(project_to_json(&p), as_current(&v1), "{name}");
    }
}

#[test]
fn m1_project_folder_loads_unchanged_with_its_history() {
    let dir = fixtures().join("m1-folder");
    let v1 = std::fs::read_to_string(dir.join(PROJECT_FILE)).unwrap();
    let p = load_project(&dir).unwrap();
    assert_eq!(p.format_version, FORMAT_VERSION);
    assert_eq!(project_to_json(&p), as_current(&v1));
    let hit = p.channels_in_order().next().unwrap();
    assert!(matches!(
        &hit.instrument,
        InstrumentSpec::Sampler { sample: Some(s), .. } if s == "samples/hit.wav"
    ));
    assert!(dir.join("samples/hit.wav").exists());
    // the registry starts empty: M1 samples stay plain Sampler references
    assert!(p.samples.is_empty() && p.next_ids.sample == 0);

    // save and reload in a fresh folder: identical
    let tmp = tempfile::tempdir().unwrap();
    save_project(&p, tmp.path()).unwrap();
    assert_eq!(load_project(tmp.path()).unwrap(), p);

    // the M1 history log still replays onto an empty project to exactly this project
    let log = read_history_log(&dir).unwrap();
    assert_eq!(log.len(), 6);
    let mut replay = Project::empty("Test");
    for entry in &log {
        let set: PatchSet = serde_json::from_value(entry["patches"].clone()).unwrap();
        for patch in set.iter() {
            if let (ObjectPath::Clip(_), Some(v)) = (patch.path, &patch.after) {
                let c: Clip = serde_json::from_value(v.clone()).unwrap();
                assert!(c.pattern().is_some(), "M1 clip is a pattern clip");
            }
        }
        replay = apply(&replay, &set).unwrap();
    }
    assert_eq!(replay, p);
}

#[test]
fn migration_refuses_m2_objects_in_a_version_1_file() {
    let v1 = std::fs::read_to_string(fixtures().join("m1-folder").join(PROJECT_FILE)).unwrap();
    let mut value: Value = serde_json::from_str(&v1).unwrap();
    value["clips"]["0"]["kind"] = "audio".into();
    assert!(matches!(migrate(&mut value), Err(FormatError::Invalid(_))));
    let mut value: Value = serde_json::from_str(&v1).unwrap();
    value["samples"] = serde_json::json!({});
    assert!(migrate(&mut value).is_err());
}

#[test]
fn version_2_projects_with_m2_objects_round_trip() {
    let mut ed = Edit::new(&Project::empty("m2"));
    let s = ed
        .add_sample(SampleEntry {
            id: SampleId(0),
            path: "samples/loop.wav".into(),
            name: "loop.wav".into(),
            frames: 48_000,
            sample_rate: 48_000,
            channels: 2,
            bpm: Some(SampleBpm {
                value: 120.0,
                source: BpmSource::User,
            }),
            root_key: Some(57),
            analysis: SampleAnalysis {
                onsets: Some("onsets-v1-abc".into()),
                key: None,
            },
            provenance: Provenance {
                origin: Origin::Recorded(Take {
                    input: "In 1-2".into(),
                    start: 0,
                    length: 3840,
                }),
                at: Some(1),
            },
        })
        .unwrap();
    ed.add_audio_clip(0, 0, 3840, AudioClip::new(s)).unwrap();
    let a = ed
        .add_automation_clip(1, 0, 3840, ParamPath::Tempo)
        .unwrap();
    ed.add_point(a, 0, 120.0, CurveKind::SCurve, 0.5).unwrap();
    ed.add_point(a, 1920, 140.0, CurveKind::Hold, 0.0).unwrap();
    let (p, _) = ed.finish();
    let tmp = tempfile::tempdir().unwrap();
    save_project(&p, tmp.path()).unwrap();
    let back = load_project(tmp.path()).unwrap();
    assert_eq!(back, p);
    let text = std::fs::read_to_string(tmp.path().join(PROJECT_FILE)).unwrap();
    assert_eq!(text, project_to_json(&back), "byte-stable");
    // a broken registry reference is caught on load
    let broken = text.replace("\"sample\": 0,", "\"sample\": 9,");
    assert!(project_from_json(&broken).is_err());
}

/// Every project the M2 build saved (the six golden projects at format 2).
const M2_GOLDEN: [&str; 6] = [
    "arrangement-waltz",
    "audio-clips",
    "automation",
    "drums-swing-choke",
    "mix-sidechain-sends",
    "synth-chords",
];

#[test]
fn m2_golden_projects_load_unchanged() {
    for name in M2_GOLDEN {
        let v2 = std::fs::read_to_string(fixtures().join(format!("m2/{name}.json"))).unwrap();
        assert!(
            v2.contains("\"format_version\": 2,"),
            "{name} is a version 2 file"
        );
        let p = project_from_json(&v2).unwrap();
        assert_eq!(p.format_version, FORMAT_VERSION);
        assert!(p.instruments.is_empty());
        let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/projects")
            .join(name);
        assert_eq!(p, load_project(&golden).unwrap(), "{name}");
        assert_eq!(project_to_json(&p), as_current(&v2), "{name}");
    }
}

#[test]
fn migration_refuses_m3_objects_in_a_version_2_file() {
    let v2 = std::fs::read_to_string(fixtures().join("m2/audio-clips.json")).unwrap();
    let mut value: Value = serde_json::from_str(&v2).unwrap();
    value["instruments"] = serde_json::json!({});
    assert!(matches!(migrate(&mut value), Err(FormatError::Invalid(_))));
    let mut value: Value = serde_json::from_str(&v2).unwrap();
    value["next_ids"]["zone"] = 3.into();
    assert!(migrate(&mut value).is_err());
}

#[test]
fn version_3_projects_with_instruments_round_trip_and_are_checked() {
    let mut ed = Edit::new(&Project::empty("m3"));
    let mut inst = HausInstrument::new("Piano");
    inst.folder = Some("instruments/Piano".into());
    let i = ed.add_instrument(inst).unwrap();
    let g = ed.add_group(i, Group::new("Main")).unwrap();
    let mut z = Zone::new(
        g,
        SampleRef::File("samples/C4.wav".into()),
        KeyRange { lo: 55, hi: 66 },
        60,
    );
    z.looping = Some(ZoneLoop {
        mode: LoopMode::PingPong,
        start: 1000,
        end: 9000,
        crossfade: 256,
        on_release: LoopRelease::Continue,
    });
    ed.add_zone(i, z).unwrap();
    ed.add_channel(
        "Piano",
        InstrumentSpec::MultiSampler {
            library: None,
            instrument: Some(i),
            params: Default::default(),
        },
    )
    .unwrap();
    let (p, _) = ed.finish();
    let tmp = tempfile::tempdir().unwrap();
    save_project(&p, tmp.path()).unwrap();
    assert_eq!(load_project(tmp.path()).unwrap(), p);
    let text = std::fs::read_to_string(tmp.path().join(PROJECT_FILE)).unwrap();
    assert!(text.contains("\"instruments\""));
    // a zone in a missing group is caught on load
    let broken = text.replace("\"group\": 0,", "\"group\": 5,");
    assert_ne!(broken, text);
    assert!(project_from_json(&broken).is_err());
    // a channel playing a missing instrument is caught on load
    let broken = text.replace("\"instrument\": 0", "\"instrument\": 4");
    assert_ne!(broken, text);
    assert!(project_from_json(&broken).is_err());
}
