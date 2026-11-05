//! The plugin instrument catalogue end to end (M3/T9), with the repo's test CLAP and VST3
//! plugins only (never a third-party plugin): save a channel's live state, list it, add it to
//! a new channel and hear the same sound, read the guide back through `get_state`, and probe
//! the test synth's range and keyswitches offline.

use super::*;
use ph_commands::{Grant, Scope};
use ph_engine::plugins::{HostConfig, PluginSlot};
use ph_plugin_host::testing::{SYNTH_ID, VST3_SYNTH_NAME, test_plugins};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

fn studio(dir: &Path, tmp: &Path) -> Studio {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let s = Studio::start(
        StudioConfig {
            project_dir: dir.to_path_buf(),
            assets_dir: assets,
            library_roots: vec![],
            exports_dir: None,
            projects_dir: None,
            engine: EngineConfig::default(),
            audio: AudioMode::Silent,
            plugins: HostConfig {
                cache_file: Some(tmp.join("cache.json")),
                scan_helper: Some(test_plugins().scan_helper.clone()),
                ..Default::default()
            },
        },
        Project::empty("catalogue"),
    )
    .unwrap();
    s.set_catalogue_dir(Some(tmp.join("catalogue")));
    s
}

fn agent() -> Caller {
    Caller::agent("claude", Grant(vec![Scope::Full]))
}

fn run(s: &Studio, cmd: &str, params: Value) -> Value {
    s.run(&agent(), cmd, params)
        .unwrap_or_else(|e| panic!("{cmd}: {e}"))
        .result
}

fn wait_for(mut cond: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn running(s: &Studio, ch: u64) -> bool {
    s.engine
        .plugin(PluginSlot::Channel(ph_core::ChannelId(ch as u32)))
        .is_some()
}

fn scan_all(s: &Studio) {
    let tp = test_plugins();
    let mut paths = vec![tp.dir.to_string_lossy().into_owned()];
    if let Some(v) = &tp.vst3_dir {
        paths.push(v.to_string_lossy().into_owned());
    }
    run(s, "plugin.scan", json!({ "paths": paths }));
}

/// Reads a 32-bit float WAV, de-interleaved.
fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let mut r = hound::WavReader::open(path).unwrap();
    let ch = r.spec().channels as usize;
    let all: Vec<f32> = r.samples::<f32>().map(Result::unwrap).collect();
    (0..ch)
        .map(|c| all.iter().skip(c).step_by(ch).copied().collect())
        .collect()
}

/// Saves the plugin channel `ch` (after `param` is set to `value` and reaches the running
/// plugin), adds the entry to a new channel, plays the same notes on both, and checks the
/// copy sounds exactly like the source.
fn save_add_and_compare(
    s: &Studio,
    dir: &Path,
    ch: u64,
    (param, value): (&str, f64),
    tag: &str,
    base: u64,
) {
    run(
        s,
        "plugin.set_param",
        json!({ "channel": ch, "param": param, "value": value }),
    );
    let idx: usize = param.parse().unwrap();
    wait_for(
        || {
            s.engine
                .plugin(PluginSlot::Channel(ph_core::ChannelId(ch as u32)))
                .and_then(|p| p.values())
                .is_some_and(|v| (v[idx] - value).abs() < 1e-9)
        },
        "the param reaches the running plugin",
    );
    let saved = run(
        s,
        "catalogue.save",
        json!({ "channel": ch, "name": format!("{tag} lead"), "tags": [tag, "test"],
                "guide": { "range": [36, 96], "ccs": [{ "cc": 1, "meaning": "level" }] } }),
    );
    let entry = saved["entry"].as_str().unwrap().to_string();
    let list = run(s, "catalogue.list", json!({}));
    let row = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["entry"] == entry.as_str())
        .unwrap_or_else(|| panic!("{entry} listed: {list}"))
        .clone();
    assert_eq!(row["status"], "ok", "{row}");
    assert!(row.get("warning").is_none(), "{row}");

    let added = run(s, "catalogue.add_to_channel", json!({ "entry": entry }));
    let copy = added["channel"].as_u64().unwrap();
    assert!(added.get("warnings").is_none(), "{added}");
    let state_ref = added["state"].as_str().unwrap();
    assert!(
        dir.join(state_ref).is_file(),
        "the project has its own copy"
    );
    wait_for(|| running(s, copy), "the copy is loaded");
    // the copy's running plugin has the saved value, from the state alone
    let got = run(s, "plugin.get_params", json!({ "channel": copy }));
    assert_eq!(got["live"], true);
    assert_eq!(got["params"][idx]["value"], value, "{got}");

    // the same phrase on both, four bars apart: identical audio
    let pat = run(s, "pattern.add", json!({}))["pattern"].clone();
    let phrase = |offset: u64| {
        json!([
            { "key": 60, "start": offset, "length": 480, "velocity": 0.9 },
            { "key": 64, "start": offset + 480, "length": 480, "velocity": 0.6 },
            { "key": 67, "start": offset + 960, "length": 960, "velocity": 0.8 }
        ])
    };
    let bar4 = 4 * 4 * 960;
    run(
        s,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": phrase(base) }),
    );
    run(
        s,
        "note.add",
        json!({ "pattern": pat, "channel": copy, "notes": phrase(base + bar4) }),
    );
    run(
        s,
        "playlist.add_clip",
        json!({ "track": if base == 0 { 0 } else { 1 }, "pattern": pat, "start": 0 }),
    );
    let wav = dir.join(format!("{tag}.wav"));
    run(
        s,
        "file.export_audio",
        json!({ "path": wav.to_string_lossy(), "range": [base, base + 2 * bar4], "bit_depth": 32,
                "include_tail": false, "high_quality": false }),
    );
    let audio = read_wav(&wav);
    let frames = (4.0 * 4.0 * 0.5 * 48_000.0) as usize; // 4 bars at 120 bpm
    let peak = audio[0][..frames]
        .iter()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "the source plays ({peak})");
    // the export ends with the song (the copy's phrase and release), so compare what is there
    let n = frames.min(audio[0].len() - frames);
    assert!(
        n > 3 * 48_000,
        "the copy's phrase is in the export ({n} frames)"
    );
    for c in &audio {
        let first = (0..n).find(|&i| c[i] != c[frames + i]);
        let worst = (0..n).fold(0.0f32, |m, i| m.max((c[i] - c[frames + i]).abs()));
        assert!(
            first.is_none(),
            "{tag}: the catalogue copy renders exactly like its source; first difference at frame {first:?}, largest {worst}"
        );
    }
    // get_state shows the guide of the catalogue-backed channel (and only that one)
    let a = Agent::new(s.handle(), Grants::fixed(Grant(vec![Scope::Full])));
    let v = a
        .call("claude", "get_state", json!({ "view": "plugins" }))
        .to_json()["value"]
        .clone();
    let rows = v["plugins"].as_array().unwrap();
    let mine = rows.iter().find(|r| r["channel"] == copy).unwrap();
    assert_eq!(mine["catalogue_entry"], entry.as_str());
    assert_eq!(mine["guide"]["range"], json!([36, 96]));
    assert_eq!(mine["guide_text"], "plays C2-C7 (36-96); CCs CC1 level");
    let src = rows.iter().find(|r| r["channel"] == ch).unwrap();
    assert!(src.get("guide").is_none());
}

#[test]
fn catalogue_saves_restores_and_renders_like_the_source_clap_and_vst3() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("song");
    std::fs::create_dir_all(&dir).unwrap();
    let s = studio(&dir, tmp.path());
    scan_all(&s);
    run(&s, "project.set_tempo", json!({ "bpm": 120 }));

    // CLAP: the test synth with Wave = saw
    let clap = run(
        &s,
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": SYNTH_ID }),
    )["channel"]
        .as_u64()
        .unwrap();
    wait_for(|| running(&s, clap), "the CLAP synth runs");
    save_add_and_compare(&s, &dir, clap, ("1", 1.0), "clap", 0);

    // VST3: the test synth with Octave = High
    if test_plugins().vst3_dir.is_some() {
        let list = run(&s, "plugin.list", json!({}));
        let id = list["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == VST3_SYNTH_NAME)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let v = run(
            &s,
            "plugin.add_instrument",
            json!({ "format": "vst3", "plugin": id }),
        )["channel"]
            .as_u64()
            .unwrap();
        wait_for(|| running(&s, v), "the VST3 synth runs");
        let params = run(&s, "plugin.get_params", json!({ "channel": v }));
        let max = params["params"][1]["max"].as_f64().unwrap();
        save_add_and_compare(&s, &dir, v, ("1", max), "vst3", 8 * 4 * 960);
    }

    // the MCP resource and the catalogue view list both entries
    let a = Agent::new(s.handle(), Grants::fixed(Grant(vec![Scope::Full])));
    let v = a
        .call("claude", "get_state", json!({ "view": "catalogue" }))
        .to_json()["value"]
        .clone();
    let n = if test_plugins().vst3_dir.is_some() {
        2
    } else {
        1
    };
    assert_eq!(v["entries"].as_array().unwrap().len(), n, "{v}");
    let found = run(&s, "catalogue.search", json!({ "tags": ["clap"] }));
    assert_eq!(found["entries"][0]["entry"], "clap-lead");

    // save_as takes the guide copies along with the states
    let other = dir.join("copy");
    run(
        &s,
        "file.save_as",
        json!({ "path": other.to_string_lossy() }),
    );
    assert!(other.join("plugins/clap-lead.catalogue.toml").is_file());
    assert!(other.join("plugins/clap-lead.state").is_file());

    // a plugin that is gone is reported plainly, and nothing is added
    let entry = tmp.path().join("catalogue/clap-lead/entry.toml");
    let text = std::fs::read_to_string(&entry).unwrap();
    std::fs::write(&entry, text.replace(SYNTH_ID, "dev.patternhaus.test.gone")).unwrap();
    let list = run(&s, "catalogue.list", json!({}));
    let row = &list["entries"][0];
    assert_eq!(row["status"], "missing");
    assert!(row["warning"].as_str().unwrap().contains("not installed"));
    let e = s
        .run(
            &agent(),
            "catalogue.add_to_channel",
            json!({ "entry": "clap-lead" }),
        )
        .unwrap_err();
    assert!(
        e.to_string().contains("not installed, or not scanned"),
        "{e}"
    );
}

#[test]
fn catalogue_probe_finds_the_range_and_keyswitches_of_a_limited_test_synth() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("song");
    // the test synth limited to C3-C5 (48-72), with keyswitches C1 = saw and C#1 = sine
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    let state = ph_engine::plugins::PluginState {
        format: ph_core::PluginFormat::Clap,
        plugin_id: SYNTH_ID.into(),
        version: "1.0.0".into(),
        data: b"PHTP1\n0=0.5\n1=0\n2=0.05\nrange=48,72\nks=36:1\nks=37:0\n".to_vec(),
    };
    std::fs::write(
        dir.join("plugins/ph-test-synth.state"),
        state.to_file_bytes(),
    )
    .unwrap();
    let s = studio(&dir, tmp.path());
    scan_all(&s);
    let ch = run(
        &s,
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": SYNTH_ID, "name": "Limited" }),
    )["channel"]
        .as_u64()
        .unwrap();
    wait_for(|| running(&s, ch), "the synth runs");
    run(&s, "catalogue.save", json!({ "channel": ch }));
    let stored = std::fs::read(tmp.path().join("catalogue/limited/plugin.state")).unwrap();
    assert!(
        String::from_utf8_lossy(&stored).contains("range=48,72"),
        "the saved state is the plugin's own"
    );

    let t = Instant::now();
    let out = s
        .run(
            &agent(),
            "catalogue.probe",
            json!({ "entry": "limited", "low": 30, "high": 90, "velocities": [40, 110],
                    "note_ms": 200, "gap_ms": 200, "keyswitches": [36, 37, 38], "test_key": 60 }),
        )
        .unwrap();
    let took = t.elapsed();
    let v = &out.result;
    assert_eq!(v["proposal"]["range"], json!([48, 72]), "{v}");
    assert_eq!(v["proposal"]["silent_keys"], json!([[30, 47], [73, 90]]));
    // 36 switches to saw (brighter), 37 back to sine; 38 is silent but changes nothing
    assert_eq!(
        v["proposal"]["keyswitches"],
        json!([
            { "key": 36, "articulation": "articulation 1 (name it)" },
            { "key": 37, "articulation": "articulation 2 (name it)" }
        ]),
        "{v}"
    );
    assert_eq!(
        v["proposal"]["silent_candidates_without_change"],
        json!([38])
    );
    let ks = v["report"]["keyswitches"].as_array().unwrap();
    let saw = ks[0]["after"]["centroid_hz"].as_f64().unwrap();
    let sine = ks[1]["after"]["centroid_hz"].as_f64().unwrap();
    assert!(saw > 1.5 * sine, "saw {saw} Hz vs sine {sine} Hz");
    assert_eq!(
        out.summary.as_deref(),
        Some("probed Limited: sounds on C3-C5 (48-72)")
    );
    eprintln!(
        "probe: {} notes, {:.1} s of audio in {took:?}",
        61 * 2 + 7,
        v["report"]["seconds"].as_f64().unwrap()
    );

    // the agent confirms the proposal into the guide
    run(
        &s,
        "catalogue.update_guide",
        json!({ "entry": "limited", "guide": { "range": v["proposal"]["range"],
                "keyswitches": [{ "key": 36, "articulation": "saw" }, { "key": 37, "articulation": "sine" }] } }),
    );
    let got = run(&s, "catalogue.get", json!({ "entry": "limited" }));
    assert_eq!(
        got["guide_text"],
        "plays C3-C5 (48-72); keyswitches C2 (36) saw, C#2 (37) sine"
    );
    // probing a channel works the same, with every note's measurement on request
    let v = run(
        &s,
        "catalogue.probe",
        json!({ "channel": ch, "low": 47, "high": 49, "velocities": [100], "detail": true }),
    );
    let sweep = v["report"]["sweep"].as_array().unwrap();
    assert_eq!(sweep.len(), 3);
    assert_eq!(sweep[0]["sounds"], false);
    assert_eq!(sweep[1]["sounds"], true);
    // rename and remove close the loop
    run(
        &s,
        "catalogue.rename",
        json!({ "entry": "limited", "name": "Limited Synth" }),
    );
    s.handle()
        .execute(
            &agent(),
            ph_commands::Request {
                confirm: true,
                ..ph_commands::Request::new("catalogue.remove", json!({ "entry": "limited" }))
            },
        )
        .unwrap();
    assert!(!tmp.path().join("catalogue/limited").exists());
}
