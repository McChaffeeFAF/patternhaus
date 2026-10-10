//! M1 acceptance criterion 1 (DESIGN §12, `docs/milestones/M1.md`), step by step.
//!
//! A scripted agent drives `patternhaus-cli mcp --headless` over stdio, **through MCP tools
//! only**, and the checks map onto the criterion as written:
//!
//! 1. sets 4 channels: kick, snare and hat on samplers, bass on Haus Synth;
//! 2. programs the patterns and arranges 8 bars in the playlist;
//! 3. puts EQ and compression on an insert and reverb on a send;
//! 4. exports a WAV;
//! 5. the render matches the offline oracle (the export is bit-identical to an independent
//!    offline render of the saved `project.json`, and the real-time engine on a simulated
//!    device is bit-identical to the offline render);
//! 6. the export measures between -16 and -8 LUFS integrated, with no clipping;
//! 7. the agent received an image and an analysis report.
//!
//! Runs in CI on Windows, Linux and macOS.

mod common;

use common::*;
use ph_engine::assets::FileResolver;
use ph_engine::{AudioBuffer, Engine, EngineConfig, PlayMode, RenderSettings, TransportCommand};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

const BAR: u64 = 3840; // 4/4, 960 ticks per quarter

fn resolver(project_dir: &Path) -> FileResolver {
    FileResolver::new()
        .with_assets_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
        .with_project_dir(project_dir)
}

fn first_difference(a: &[Vec<f32>], b: &[Vec<f32>]) -> Option<(usize, usize)> {
    for c in 0..2 {
        if a[c].len() != b[c].len() {
            return Some((c, usize::MAX));
        }
        if let Some(i) = (0..a[c].len()).find(|&i| a[c][i].to_bits() != b[c][i].to_bits()) {
            return Some((c, i));
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn m1_acceptance_agent_builds_a_beat_through_mcp_only() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song");
    let c = connect(&song, &[]).await;

    // ---- step 1: four channels, three samplers and a Haus Synth ---------------------------
    ok(&c, "set_tempo", json!({ "bpm": 120.0 })).await;
    let drum = |s: &str| json!({ "kind": "sampler", "sample": format!("factory:drums/haus808/{s}"), "root_key": 60 });
    let mut chans = Vec::new();
    for (name, inst, insert) in [
        ("Kick", drum("kick"), 1),
        ("Snare", drum("snare"), 2),
        ("Hat", drum("hat_closed"), 3),
        ("Bass", json!({ "kind": "synth", "preset": "bass/sub" }), 4),
    ] {
        let r = ok(
            &c,
            "add_channel",
            json!({ "name": name, "instrument": inst, "insert": insert }),
        )
        .await;
        chans.push(id(&r, "channel"));
    }
    let (kick, snare, hat, bass) = (chans[0], chans[1], chans[2], chans[3]);
    let view = ok(&c, "get_state", json!({ "view": "channels" })).await;
    let text = view.to_string();
    for name in ["Kick", "Snare", "Hat", "Bass"] {
        assert!(text.contains(name), "channel {name} missing: {view}");
    }
    assert!(text.contains("sampler") && text.contains("synth"), "{view}");

    // ---- step 2: program the patterns, arrange 8 bars ------------------------------------
    let beat = id(
        &ok(&c, "add_pattern", json!({ "name": "Beat" })).await,
        "pattern",
    );
    let line = id(
        &ok(&c, "add_pattern", json!({ "name": "Bassline" })).await,
        "pattern",
    );
    let hits = |ticks: &[u64], len: u64, vel: f64| -> Vec<Value> {
        ticks
            .iter()
            .map(|t| json!({ "key": 60, "start": t, "length": len, "velocity": vel }))
            .collect()
    };
    for (ch, notes) in [
        (kick, hits(&[0, 960, 1920, 2880], 240, 1.0)),
        (snare, hits(&[960, 2880], 240, 0.9)),
        (
            hat,
            hits(&(0..8).map(|i| i * 480).collect::<Vec<_>>(), 120, 0.6),
        ),
    ] {
        ok(
            &c,
            "add_notes",
            json!({ "pattern": beat, "channel": ch, "notes": notes }),
        )
        .await;
    }
    let mut bass_notes = Vec::new();
    for (bar, root) in [33u64, 29, 36, 31].iter().enumerate() {
        let b = bar as u64 * BAR;
        bass_notes.push(json!({ "key": root, "start": b, "length": 1700, "velocity": 0.9 }));
        bass_notes.push(json!({ "key": root, "start": b + 1920, "length": 1700, "velocity": 0.9 }));
    }
    ok(
        &c,
        "add_notes",
        json!({ "pattern": line, "channel": bass, "notes": bass_notes }),
    )
    .await;
    let mut clips: Vec<Value> = (0..8)
        .map(|b| json!({ "id": "playlist.add_clip", "params": { "track": 0, "pattern": beat, "start": b * BAR } }))
        .collect();
    for start in [0, 4 * BAR] {
        clips.push(json!({ "id": "playlist.add_clip", "params": { "track": 1, "pattern": line, "start": start } }));
    }
    ok(
        &c,
        "run_batch",
        json!({ "commands": clips, "as_one_undo": true }),
    )
    .await;
    let sections = ok(&c, "get_state", json!({ "view": "sections" })).await;
    assert_eq!(sections["song_bars"], 8, "{sections}");

    // ---- step 3: EQ and compression on an insert, reverb on a send -----------------------
    for name in ["eq", "compressor"] {
        ok(
            &c,
            "add_effect",
            json!({ "insert": 4, "effect": { "kind": "builtin", "name": name } }),
        )
        .await;
    }
    ok(
        &c,
        "add_effect",
        json!({ "insert": 5, "effect": { "kind": "builtin", "name": "reverb" }, "mix": 1.0 }),
    )
    .await;
    ok(&c, "add_send", json!({ "from": 2, "to": 5, "level": 0.4 })).await;
    let mixer = ok(&c, "get_state", json!({ "view": "mixer" })).await;
    let inserts = mixer["inserts"].as_array().unwrap();
    let fx_of = |n: u64| {
        inserts
            .iter()
            .find(|i| i["insert"] == n)
            .map(|i| i["effects"].to_string())
            .unwrap_or_default()
    };
    assert!(
        fx_of(4).contains("eq") && fx_of(4).contains("compressor"),
        "{mixer}"
    );
    assert!(fx_of(5).contains("reverb"), "{mixer}");
    assert!(mixer.to_string().contains("send"), "{mixer}");

    // the agent listens: an analysis report, then gain staging on the master
    let report = ok(&c, "analyze", json!({ "what": ["loudness", "health"] })).await;
    assert!(
        report["loudness"]["integrated_lufs"].is_number(),
        "{report}"
    );
    assert!(report["health"].is_object(), "{report}");
    let lufs0 = report["loudness"]["integrated_lufs"].as_f64().unwrap();
    ok(
        &c,
        "add_effect",
        json!({ "insert": 0, "effect": { "kind": "builtin", "name": "limiter" } }),
    )
    .await;
    ok(
        &c,
        "set_effect_param",
        json!({ "insert": 0, "index": 0, "param": "input_gain", "value": (-11.0 - lufs0).clamp(0.0, 12.0) }),
    )
    .await;

    // ---- step 4: export a WAV (32-bit float, so the oracle comparison is exact) -----------
    let rendered = ok(
        &c,
        "render",
        json!({ "name": "acceptance", "format": "wav", "bit_depth": 32 }),
    )
    .await;
    let wav = std::path::PathBuf::from(rendered["path"].as_str().unwrap());
    assert!(wav.exists(), "{rendered}");
    ok(&c, "run", json!({ "id": "file.save" })).await;

    // ---- step 7: the agent received an image and an analysis report ---------------------
    let shot = call(&c, "snapshot", json!({ "kind": "waveform" })).await;
    assert!(!shot.is_error && shot.images.len() == 1, "{:?}", shot.texts);
    assert_png(&shot.images[0], 900);
    let full = ok(&c, "analyze", json!({ "what": ["all"] })).await;
    for section in ["loudness", "health", "key"] {
        assert!(
            full[section].is_object(),
            "analysis report lacks {section}: {full}"
        );
    }
    drop(c);

    // ---- step 5: the render matches the offline oracle -----------------------------------
    let exported = ph_formats::decode::decode_file(&wav).unwrap();
    assert_eq!(exported.sample_rate as u32, 48_000);
    let project = ph_formats::load_project(&song).unwrap();
    let assets = resolver(&song);
    assert!(ph_engine::project_warnings(&project, &assets).is_empty());
    let settings = |tail: bool, block: usize| RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: 48_000,
        quality: ph_node::Quality::Live,
        include_tail: tail,
        block_frames: block,
    };
    // an independent render of the saved project, with a different block size
    let oracle = ph_engine::render_offline(&project, &settings(true, 256), &assets).unwrap();
    assert_eq!(
        first_difference(&exported.channels, &oracle.channels),
        None,
        "the exported WAV differs from the offline oracle (channel, frame)"
    );
    // real time through a simulated device, against the offline render of the song
    let song_only = ph_engine::render_offline(&project, &settings(false, 512), &assets).unwrap();
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: 48_000,
            buffer_frames: 256,
            ..EngineConfig::default()
        },
        Arc::new(resolver(&song)),
    )
    .unwrap();
    engine.set_project(Arc::new(project.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let mut rt: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
    while rt[0].len() < song_only.frames() {
        let AudioBuffer { channels, .. } = dev.pull(256);
        rt[0].extend_from_slice(&channels[0]);
        rt[1].extend_from_slice(&channels[1]);
    }
    rt[0].truncate(song_only.frames());
    rt[1].truncate(song_only.frames());
    assert_eq!(
        first_difference(&rt, &song_only.channels),
        None,
        "real-time output differs from the offline render (channel, frame)"
    );

    // ---- step 6: -16..-8 LUFS, no clipping (measured on the exported file itself) --------
    let loud = ph_analysis::loudness::analyze(&exported.channels, 48_000);
    let lufs = loud.integrated_lufs.expect("an integrated loudness");
    assert!((-16.0..=-8.0).contains(&lufs), "integrated {lufs} LUFS");
    let peak = exported
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 1.0, "sample peak {peak} clips");
    let tp = loud.true_peak_dbtp.expect("a true peak");
    assert!(tp < 0.0, "true peak {tp} dBTP");
    assert!(
        rendered["seconds"].as_f64().unwrap() >= 16.0,
        "8 bars at 120 BPM: {rendered}"
    );
}
