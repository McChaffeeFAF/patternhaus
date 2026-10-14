//! Plugin hosting end to end through commands (M2/T3), with the repo's test plugins: scan,
//! add, set and read parameters, agents' `query.node_params`, state saved to `plugins/` and
//! read back on reopen, export, the editor request, and a crash that isolates the plugin while
//! the session keeps going.

use super::*;
use ph_commands::{Grant, Scope};
use ph_engine::plugins::{HostConfig, PluginSlot};
use ph_plugin_host::testing::{CRASH_ID, GAIN_LATENCY, SYNTH_ID, test_plugins};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn studio(dir: &std::path::Path, cache: &std::path::Path) -> Studio {
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
            plugins: HostConfig {
                cache_file: Some(cache.to_path_buf()),
                scan_helper: Some(test_plugins().scan_helper.clone()),
                ..Default::default()
            },
        },
        Project::empty("plugins"),
    )
    .unwrap()
}

fn run(s: &Studio, cmd: &str, params: Value) -> Value {
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    s.run(&agent, cmd, params)
        .unwrap_or_else(|e| panic!("{cmd}: {e}"))
        .result
}

fn wait_for(mut cond: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn plugins_scan_add_play_save_reopen_and_export() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, cache) = (tmp.path().join("song"), tmp.path().join("cache.json"));
    std::fs::create_dir_all(&dir).unwrap();
    let s = studio(&dir, &cache);
    let plugins_dir = test_plugins().dir.to_string_lossy().into_owned();
    let scan = run(&s, "plugin.scan", json!({ "paths": [plugins_dir] }));
    assert_eq!(scan["plugins"].as_array().unwrap().len(), 3);
    assert!(cache.is_file(), "the scan cache is persisted");
    let list = run(&s, "plugin.list", json!({}));
    assert!(
        list["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == SYNTH_ID && p["kind"] == "instrument" && p["status"] == "ok")
    );
    // agents read any plugin's parameters by id before adding it
    let np = run(&s, "query.node_params", json!({ "plugin": SYNTH_ID }));
    assert_eq!(np["params"][1]["choices"], json!(["Sine", "Saw"]));
    assert_eq!(np["kind"], "instrument");
    let all = run(&s, "query.node_params", json!({}));
    assert_eq!(all["plugins"].as_array().unwrap().len(), 3);

    // a plugin synth playing a pattern, through the plugin gain
    let added = run(
        &s,
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": SYNTH_ID }),
    );
    let ch = added["channel"].as_u64().unwrap();
    assert_eq!(added["state"], "plugins/ph-test-synth.state");
    let pat = run(&s, "pattern.add", json!({}))["pattern"].clone();
    run(
        &s,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": [
            { "key": 60, "start": 0, "length": 960 }, { "key": 67, "start": 960, "length": 960 }
        ] }),
    );
    run(
        &s,
        "playlist.add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    );
    let fx = run(
        &s,
        "plugin.add_effect",
        json!({ "insert": 0, "format": "clap", "plugin": ph_plugin_host::testing::GAIN_ID }),
    );
    assert_eq!(fx["slot"], 0);
    run(
        &s,
        "plugin.set_param",
        json!({ "channel": ch, "param": "1", "value": 1 }),
    );
    // the running plugin has it
    wait_for(
        || {
            s.engine
                .plugin(PluginSlot::Channel(ph_core::ChannelId(ch as u32)))
                .and_then(|p| p.values())
                .is_some_and(|v| v[1] == 1.0)
        },
        "the param reaches the live synth",
    );
    let got = run(&s, "plugin.get_params", json!({ "channel": ch }));
    assert_eq!(got["live"], true);
    assert_eq!(got["params"][1]["value"], 1.0);
    assert_eq!(got["params"][1]["text"], "Saw");
    let gp = run(&s, "plugin.get_params", json!({ "insert": 0, "slot": 0 }));
    assert_eq!(gp["latency"], GAIN_LATENCY);
    assert!(
        s.engine.stats().warnings.is_empty(),
        "{:?}",
        s.engine.stats().warnings
    );

    // the editor request goes to the UI (none is attached here)
    let ed = run(&s, "plugin.open_editor", json!({ "channel": ch }));
    assert_eq!(ed["ui"], false);
    assert!(matches!(
        s.ui_requests.try_recv(),
        Ok(ph_commands::UiRequest::PluginEditor { open: true, .. })
    ));

    // save: the state goes to plugins/, next to project.json
    let saved = run(&s, "file.save", json!({}));
    assert_eq!(
        saved["plugin_states"],
        json!(["plugins/ph-test-gain.state", "plugins/ph-test-synth.state"])
    );
    let file = std::fs::read(dir.join("plugins/ph-test-synth.state")).unwrap();
    let st = ph_engine::plugins::PluginState::from_file_bytes(&file).unwrap();
    assert_eq!(st.plugin_id, SYNTH_ID);
    assert!(
        String::from_utf8_lossy(&st.data).contains("1=1"),
        "wave = saw saved"
    );
    // export renders the plugins
    let wav = tmp.path().join("song/out.wav");
    let exported = run(
        &s,
        "file.export_audio",
        json!({ "path": wav.to_string_lossy(), "include_tail": false }),
    );
    assert!(std::fs::metadata(&wav).unwrap().len() > 1000);
    let peak = exported["peak_dbfs"].as_f64().unwrap();
    assert!(
        peak > -30.0 && peak < 0.0,
        "the plugin synth is in the export ({peak} dBFS)"
    );
    drop(s);

    // reopen with the spec's own params removed: the saved state alone brings saw back
    let mut json_text = std::fs::read_to_string(dir.join("project.json")).unwrap();
    let mut v: Value = serde_json::from_str(&json_text).unwrap();
    for c in v["channels"].as_object_mut().unwrap().values_mut() {
        if let Some(i) = c.get_mut("instrument")
            && i["kind"] == "plugin"
        {
            i.as_object_mut().unwrap().remove("params");
        }
    }
    json_text = serde_json::to_string_pretty(&v).unwrap();
    std::fs::write(dir.join("project.json"), json_text).unwrap();
    let project = ph_formats::load_project(&dir).unwrap();
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let s2 = Studio::start(
        StudioConfig {
            project_dir: dir.clone(),
            assets_dir: assets,
            library_roots: vec![],
            exports_dir: None,
            projects_dir: None,
            engine: EngineConfig::default(),
            audio: AudioMode::Silent,
            plugins: HostConfig {
                cache_file: Some(cache.clone()),
                ..Default::default()
            },
        },
        project,
    )
    .unwrap();
    let got = run(&s2, "plugin.get_params", json!({ "channel": ch }));
    assert_eq!(got["live"], true);
    assert_eq!(
        got["params"][1]["value"], 1.0,
        "state survived save and reopen"
    );
}

#[test]
fn a_crashing_plugin_is_isolated_and_the_session_keeps_running() {
    let tmp = tempfile::tempdir().unwrap();
    let s = studio(tmp.path(), &tmp.path().join("cache.json"));
    let plugins_dir = test_plugins().dir.to_string_lossy().into_owned();
    run(&s, "plugin.scan", json!({ "paths": [plugins_dir] }));
    run(
        &s,
        "plugin.add_effect",
        json!({ "insert": 0, "format": "clap", "plugin": CRASH_ID }),
    );
    run(&s, "transport.play", json!({}));
    run(
        &s,
        "plugin.set_param",
        json!({ "insert": 0, "slot": 0, "param": "0", "value": 1 }),
    );
    wait_for(
        || !s.engine.plugin_faults().is_empty(),
        "the crash is caught",
    );
    let f = &s.engine.plugin_faults()[0];
    assert_eq!(f.plugin.id, CRASH_ID);
    let list = run(&s, "plugin.list", json!({}));
    let crash = list["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == CRASH_ID)
        .unwrap()
        .clone();
    assert_eq!(crash["status"], "isolate");
    assert!(crash["reason"].as_str().unwrap().contains("Process"));
    // the session keeps running: commands work, the transport plays, warnings explain
    run(&s, "project.set_tempo", json!({ "bpm": 100 }));
    assert!(s.engine.status().playing);
    assert!(
        s.engine
            .stats()
            .warnings
            .iter()
            .any(|w| w.contains("crashed"))
    );
    // an isolated plugin cannot be added again until M4 can isolate it
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    let e = s
        .run(
            &agent,
            "plugin.add_effect",
            json!({ "insert": 1, "format": "clap", "plugin": CRASH_ID }),
        )
        .unwrap_err();
    assert!(e.to_string().contains("isolate"), "{e}");
}
