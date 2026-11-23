//! M3/T12 through `Agent::call`: the tempo and beat-grid section of `analyze` (project
//! render, registered sample, audio clip), chord `resolution`, and the Ogg Vorbis render.
//! Audio is generated; no device is opened.

use super::*;
use crate::agent::Block;
use ph_commands::{Grant, Scope};
use ph_formats::encode::{WavFormat, write_wav};
use serde_json::{Value, json};

fn studio(dir: &std::path::Path) -> Studio {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    Studio::start(
        StudioConfig {
            project_dir: dir.to_path_buf(),
            assets_dir: assets,
            library_roots: vec![],
            exports_dir: None,
            projects_dir: None,
            engine: EngineConfig::default(),
            audio: AudioMode::Silent,
            plugins: Default::default(),
        },
        Project::empty("t12"),
    )
    .unwrap()
}

fn agent_for(s: &Studio) -> Arc<Agent> {
    Agent::new(s.handle(), Grants::fixed(Grant(vec![Scope::Full])))
}

fn call(a: &Agent, tool: &str, args: Value) -> Value {
    let o = a.call("claude", tool, args);
    assert!(!o.is_error, "{tool}: {:?}", o.blocks);
    o.value().expect("a JSON reply")
}

fn call_err(a: &Agent, tool: &str, args: Value) -> String {
    let o = a.call("claude", tool, args);
    assert!(o.is_error, "{tool} should fail: {:?}", o.blocks);
    match &o.blocks[0] {
        Block::Text(t) => t.clone(),
        b => panic!("{b:?}"),
    }
}

fn result_of(v: &Value, key: &str) -> Value {
    v["results"][0]["result"][key].clone()
}

/// The factory kick on every beat of `bars` bars at `bpm`.
fn kick_song(a: &Agent, bpm: f64, bars: u64) {
    call(a, "set_tempo", json!({ "bpm": bpm }));
    let pat = result_of(
        &call(a, "add_pattern", json!({ "name": "Beat" })),
        "pattern",
    );
    let ch = result_of(
        &call(
            a,
            "add_channel",
            json!({ "name": "Kick", "instrument": { "kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60 } }),
        ),
        "channel",
    );
    let notes: Vec<Value> = (0..bars * 4)
        .map(|i| json!({ "key": 60, "start": i * 960, "length": 240, "velocity": 1.0 }))
        .collect();
    call(
        a,
        "add_notes",
        json!({ "pattern": pat, "channel": ch, "notes": notes }),
    );
    call(
        a,
        "add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    );
}

/// A generated loop of eight beats at `bpm`: a decaying 110 Hz thump on each beat, the first
/// one `lead` seconds into the file, every fourth one harder.
fn write_loop(path: &std::path::Path, bpm: f64, lead: f64) {
    let sr = 48_000.0;
    let beat = 60.0 / bpm;
    let n = ((8.0 * beat + lead) * sr) as usize;
    let mut x = vec![0.0f32; n];
    for b in 0..8 {
        let s0 = ((lead + b as f64 * beat) * sr) as usize;
        let amp = if b % 4 == 0 { 0.9 } else { 0.5 };
        for i in 0..(0.2 * sr) as usize {
            if s0 + i < n {
                let t = i as f64 / sr;
                x[s0 + i] +=
                    (amp * (std::f64::consts::TAU * 110.0 * t).sin() * (-t * 15.0).exp()) as f32;
            }
        }
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_wav(path, &[x], 48_000, WavFormat::Float32).unwrap();
}

#[test]
fn analyze_reports_the_tempo_of_a_project_render_against_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    kick_song(&a, 120.0, 8);
    let v = call(&a, "analyze", json!({ "what": ["tempo"] }));
    let t = &v["tempo"];
    if std::env::var_os("PH_T12_SHOW").is_some() {
        eprintln!("{}", serde_json::to_string_pretty(&v).unwrap());
    }
    let bpm = t["bpm"].as_f64().unwrap();
    assert!((bpm - 120.0).abs() < 0.3, "{t}");
    assert!(t["confidence"].as_f64().unwrap() > 0.3, "{t}");
    assert!(t["alternatives"].is_array());
    assert_eq!(t["project"]["relation"], "matches", "{t}");
    assert!(
        t["project"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("matches the project tempo"),
        "{t}"
    );
    assert!(
        t["project"]["grid_offset_ms"].as_f64().unwrap().abs() < 25.0,
        "{t}"
    );
    assert!(t["first_beat_s"].as_f64().unwrap() < 0.5);
    assert_eq!(t["downbeat"]["beats_per_bar"], 4);
    assert_eq!(t["drift"]["windows"].as_array().unwrap().len(), 1);
    assert_eq!(t["drift"]["verdict"], "steady");
    // tempo is part of the default set
    let d = call(&a, "analyze", json!({}));
    assert!(d["tempo"]["bpm"].is_number(), "{d}");
}

/// A drum loop from factory one-shots (there are no factory loops): kick on 1 and 3, snare on
/// 2 and 4, closed hats on the eighths.
fn drum_loop_song(a: &Agent, bpm: f64, bars: u64) {
    call(a, "set_tempo", json!({ "bpm": bpm }));
    let pat = result_of(
        &call(a, "add_pattern", json!({ "name": "Loop" })),
        "pattern",
    );
    let lane = |name: &str, sample: &str, steps: Vec<u64>| {
        let ch = result_of(
            &call(
                a,
                "add_channel",
                json!({ "name": name, "instrument": { "kind": "sampler", "sample": sample, "root_key": 60 } }),
            ),
            "channel",
        );
        let notes: Vec<Value> = (0..bars)
            .flat_map(|b| {
                steps.iter().map(move |s| {
                    json!({ "key": 60, "start": b * 3840 + s * 240, "length": 120, "velocity": 1.0 })
                })
            })
            .collect();
        call(
            a,
            "add_notes",
            json!({ "pattern": pat, "channel": ch, "notes": notes }),
        );
    };
    lane("Kick", "factory:drums/haus808/kick", vec![0, 8]);
    lane("Snare", "factory:drums/haus808/snare", vec![4, 12]);
    lane(
        "Hat",
        "factory:drums/haus808/hat_closed",
        (0..16).step_by(2).collect(),
    );
    call(
        a,
        "add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    );
}

#[test]
fn analyze_finds_the_known_tempo_of_a_factory_drum_loop() {
    for bpm in [100.0, 128.0, 140.0] {
        let dir = tempfile::tempdir().unwrap();
        let s = studio(dir.path());
        let a = agent_for(&s);
        drum_loop_song(&a, bpm, 8);
        let v = call(&a, "analyze", json!({ "what": ["tempo"] }));
        let t = &v["tempo"];
        assert!((t["bpm"].as_f64().unwrap() - bpm).abs() < 0.5, "{bpm}: {t}");
        assert_eq!(t["project"]["relation"], "matches", "{bpm}: {t}");
        assert_eq!(t["drift"]["verdict"], "steady", "{bpm}: {t}");
        assert!(
            t["project"]["grid_offset_ms"].as_f64().unwrap().abs() < 25.0,
            "{bpm}: {t}"
        );
        // the same loop as a file, read as a sample, with no project in the way
        call(
            &a,
            "run",
            json!({ "id": "file.export_audio", "params": { "path": "samples/drums.wav", "bit_depth": 16 } }),
        );
        let f = call(
            &a,
            "analyze",
            json!({ "what": ["tempo"], "sample": "samples/drums.wav" }),
        );
        assert!(
            (f["tempo"]["bpm"].as_f64().unwrap() - bpm).abs() < 0.5,
            "{bpm}: {}",
            f["tempo"]
        );
    }
}

#[test]
fn analyze_moves_an_octave_the_estimator_took_too_fast_and_says_so() {
    // eighth-note hats make the pulse rule take 186 BPM for a 93 BPM loop; its own comb
    // score prefers 93, so that is reported, with a note naming both
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    drum_loop_song(&a, 93.0, 8);
    let v = call(&a, "analyze", json!({ "what": ["tempo"] }));
    let t = &v["tempo"];
    assert!((t["bpm"].as_f64().unwrap() - 93.0).abs() < 0.5, "{t}");
    assert_eq!(t["project"]["relation"], "matches", "{t}");
    assert!(
        t["octave_note"].as_str().unwrap().contains("pulse rule"),
        "{t}"
    );
    assert!(
        t["alternatives"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| (a["bpm"].as_f64().unwrap() - 186.0).abs() < 1.0),
        "{t}"
    );
}

#[test]
fn analyze_reads_the_beat_grid_of_a_sample_and_a_clip() {
    let dir = tempfile::tempdir().unwrap();
    write_loop(&dir.path().join("samples/loop.wav"), 100.0, 0.012);
    let s = studio(dir.path());
    let a = agent_for(&s);
    let v = call(
        &a,
        "analyze",
        json!({ "what": ["tempo"], "sample": "samples/loop.wav" }),
    );
    let t = &v["tempo"];
    if std::env::var_os("PH_T12_SHOW").is_some() {
        eprintln!("{}", serde_json::to_string_pretty(&v).unwrap());
    }
    assert!((t["bpm"].as_f64().unwrap() - 100.0).abs() < 1.0, "{t}");
    assert!(
        (t["first_beat_s"].as_f64().unwrap() - 0.012).abs() < 0.01,
        "{t}"
    );
    assert_eq!(v["source"]["kind"], "sample");
    // the loop starts on its accented downbeat, 12 ms in
    assert_eq!(t["downbeat"]["first_beat_is_beat"], 1, "{t}");
    assert!(
        (v["source"]["bars"]["bar_1_at_s"].as_f64().unwrap() - 0.012).abs() < 0.01,
        "{v}"
    );
    assert!(
        t.get("project").is_none(),
        "no project comparison for a sample"
    );
    // the same audio through a clip
    let added = call(
        &a,
        "add_audio_clip",
        json!({ "track": 0, "start": 0, "path": "samples/loop.wav" }),
    );
    let clip = result_of(&added, "clip");
    let c = call(
        &a,
        "analyze",
        json!({ "what": ["tempo", "chords"], "clip": clip }),
    );
    assert_eq!(c["source"]["kind"], "clip");
    assert!(
        (c["tempo"]["bpm"].as_f64().unwrap() - 100.0).abs() < 1.0,
        "{c}"
    );
    assert!(c["chords"]["from_notes"].is_null());
    assert!(c["chords"]["from_audio"]["by_bar"].is_array());
    assert!(c["source"]["bars"]["bar_1_at_s"].is_number());
    // the arguments are checked
    assert!(
        call_err(
            &a,
            "analyze",
            json!({ "sample": "samples/loop.wav", "clip": clip })
        )
        .contains("either")
    );
    assert!(
        call_err(
            &a,
            "analyze",
            json!({ "what": ["clashes"], "sample": "samples/loop.wav" })
        )
        .contains("project")
    );
    assert!(
        call_err(
            &a,
            "analyze",
            json!({ "sample": "samples/loop.wav", "range": { "from_bar": 1, "to_bar": 2 } })
        )
        .contains("range")
    );
    assert!(
        call_err(&a, "analyze", json!({ "sample": "samples/missing.wav" })).contains("cannot read")
    );
    assert!(call_err(&a, "analyze", json!({ "resolution": "third" })).contains("resolution"));
}

#[test]
fn analyze_chords_mark_a_change_inside_a_bar_at_half_bar_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    call(&a, "set_tempo", json!({ "bpm": 120.0 }));
    let pat = result_of(
        &call(&a, "add_pattern", json!({ "name": "Prog" })),
        "pattern",
    );
    let ch = result_of(
        &call(
            &a,
            "add_channel",
            json!({ "name": "Keys", "instrument": { "kind": "synth" } }),
        ),
        "channel",
    );
    // bars 1-3: C, F, Am; bar 4: Dm for two beats, then G
    let bar = 3840u64;
    let voice = |start: u64, len: u64, keys: &[u8]| -> Vec<Value> {
        keys.iter()
            .map(|k| json!({ "key": k, "start": start, "length": len, "velocity": 0.8 }))
            .collect()
    };
    let mut notes = voice(0, bar, &[48, 60, 64, 67]);
    notes.extend(voice(bar, bar, &[41, 60, 65, 69]));
    notes.extend(voice(2 * bar, bar, &[45, 60, 64, 69]));
    notes.extend(voice(3 * bar, bar / 2, &[38, 62, 65, 69]));
    notes.extend(voice(3 * bar + bar / 2, bar / 2, &[43, 59, 62, 67]));
    call(
        &a,
        "add_notes",
        json!({ "pattern": pat, "channel": ch, "notes": notes }),
    );
    call(
        &a,
        "add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    );

    let half = call(
        &a,
        "analyze",
        json!({ "what": ["chords"], "resolution": "half_bar", "range": { "from_bar": 1, "to_bar": 4 } }),
    );
    let c = &half["chords"];
    if std::env::var_os("PH_T12_SHOW").is_some() {
        eprintln!("{}", serde_json::to_string_pretty(c).unwrap());
    }
    assert_eq!(c["resolution"], "half_bar");
    let by_bar: Vec<&str> = c["from_notes"]["by_bar"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(
        by_bar,
        ["1.1 C", "2.1 F", "3.1 Am", "4.1 Dm", "4.3 G"],
        "{c}"
    );
    assert_eq!(c["from_notes"]["mid_bar_changes"], json!(["4.3 G"]));
    assert!(c["from_audio"]["by_bar"].is_array());
    // the default resolution is unchanged
    let bar_res = call(
        &a,
        "analyze",
        json!({ "what": ["chords"], "range": { "from_bar": 1, "to_bar": 4 } }),
    );
    assert_eq!(bar_res["chords"]["resolution"], "bar");
    assert!(bar_res["chords"]["from_notes"]["progression"].is_string());
    // beat resolution agrees with the half-bar reading on this material
    let beat = call(
        &a,
        "analyze",
        json!({ "what": ["chords"], "resolution": "beat", "range": { "from_bar": 1, "to_bar": 4 } }),
    );
    assert_eq!(
        beat["chords"]["from_notes"]["mid_bar_changes"],
        json!(["4.3 G"])
    );
}

#[test]
fn render_writes_ogg_vorbis_with_a_quality_option() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    kick_song(&a, 120.0, 4);
    let v = call(
        &a,
        "render",
        json!({ "format": "ogg", "name": "preview", "quality": 0.3 }),
    );
    let path = std::path::PathBuf::from(v["path"].as_str().unwrap());
    assert_eq!(path.extension().unwrap(), "ogg");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"OggS");
    let wav = call(
        &a,
        "render",
        json!({ "format": "wav", "bit_depth": 16, "name": "full" }),
    );
    let wav_len = std::fs::metadata(wav["path"].as_str().unwrap())
        .unwrap()
        .len();
    assert!(
        (bytes.len() as u64) * 4 < wav_len,
        "{} vs {wav_len}",
        bytes.len()
    );
    let d = ph_formats::decode::decode_bytes(&bytes, Some("ogg")).unwrap();
    let secs = d.frames() as f64 / d.sample_rate;
    assert!(
        (secs - v["seconds"].as_f64().unwrap()).abs() < 0.1,
        "{secs}"
    );
    assert!(call_err(&a, "render", json!({ "format": "ogg", "quality": 7 })).contains("quality"));
    assert!(call_err(&a, "render", json!({ "format": "mp3" })).contains("unsupported"));
}
