use super::*;
use ph_commands::{Grant, Scope};
use serde_json::json;
use std::time::{Duration, Instant};

fn studio(dir: &std::path::Path) -> Studio {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let config = StudioConfig {
        project_dir: dir.to_path_buf(),
        assets_dir: assets,
        library_roots: vec![],
        exports_dir: None,
        engine: EngineConfig::default(),
        audio: AudioMode::Silent,
    };
    Studio::start(config, Project::empty("t")).unwrap()
}

fn wait_for(mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn edits_reach_the_engine_and_export_renders_audio() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    let pat = s.run(&agent, "pattern.add", json!({})).unwrap().result["pattern"].clone();
    let ch = s
        .run(&agent, "channel.add", json!({
            "name": "Kick",
            "instrument": { "kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60 }
        }))
        .unwrap()
        .result["channel"]
        .clone();
    let notes: Vec<_> = (0..4)
        .map(|i| json!({ "key": 60, "start": i * 960, "length": 240, "velocity": 1.0 }))
        .collect();
    s.run(
        &agent,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": notes }),
    )
    .unwrap();
    s.run(
        &agent,
        "playlist.add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    )
    .unwrap();

    // the bridge delivers the snapshot
    wait_for(|| s.project().clips.len() == 1);

    let out = s
        .run(
            &agent,
            "file.export_audio",
            json!({ "path": "kick.wav", "bit_depth": 16 }),
        )
        .unwrap();
    assert!(dir.path().join("kick.wav").exists(), "{out:?}");
    let peak = out.result["peak_dbfs"].as_f64().unwrap();
    assert!(
        peak > -12.0 && peak <= 0.0,
        "factory kick audible, not clipping: {peak} dBFS"
    );
}

#[test]
fn file_access_is_confined() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    let err = s.run(
        &agent,
        "file.export_audio",
        json!({ "path": "../escape.wav" }),
    );
    assert!(
        err.is_err(),
        "climbing out of the project folder is refused"
    );
    let abs = std::env::temp_dir().join("ph-outside.wav");
    assert!(
        s.run(
            &agent,
            "file.export_midi",
            json!({ "path": abs.to_string_lossy() })
        )
        .is_err()
    );
}

#[test]
fn transport_goes_through_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let h = Caller::human();
    s.run(&h, "pattern.add", json!({})).unwrap();
    let st = s
        .run(&h, "transport.set_mode", json!({ "mode": "pattern" }))
        .unwrap();
    assert_eq!(st.result["mode"], "pattern");
    let st = s.run(&h, "transport.play", json!({})).unwrap();
    assert_eq!(st.result["playing"], true);
    // the silent pump drives the audio thread, so the engine really plays and advances
    wait_for(|| s.engine.status().playing && s.engine.status().position > 0);
    let st = s.run(&h, "transport.toggle", json!({})).unwrap();
    assert_eq!(st.result["playing"], false);
}

#[test]
fn ui_requests_are_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    s.run(
        &Caller::human(),
        "ui.open_panel",
        json!({ "panel": "mixer" }),
    )
    .unwrap();
    assert!(matches!(
        s.ui_requests.recv_timeout(Duration::from_secs(2)).unwrap(),
        UiRequest::Open { .. }
    ));
}
