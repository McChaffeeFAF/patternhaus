//! M2 acceptance criterion 1 (`docs/milestones/M2.md`), step by step.
//!
//! A scripted agent drives `patternhaus-cli mcp --headless` over stdio, **through MCP tools
//! only** (`run`, `run_batch`, `get_state`, `analyze`, `render`, `snapshot` and the M1
//! convenience tools, all on `main` before M2/T9), and builds a 32-bar song at 124 BPM:
//!
//! 1. intro (bars 1-8), build (9-16) and drop (17-32) sections, arranged from patterns named
//!    after them;
//! 2. a filter sweep (an automation clip on a filter's cutoff, bars 1-16) and a volume
//!    automation clip (an insert volume ride over the build);
//! 3. a generated drum loop (100 BPM, written by this test, never a recording) imported as
//!    audio (`audio.add_clip { path }`) and fitted to the project tempo (`audio.fit_to_tempo`,
//!    which detects the loop's tempo), then repeated through the build and the drop;
//! 4. a slice instrument made from that loop (`slicer.make_instrument { write_pattern }`),
//!    whose pattern plays in the drop;
//! 5. one instance of the repo's test CLAP plugin (the synth, `plugin.scan` +
//!    `plugin.add_instrument`), playing an arpeggio all through;
//! 6. M2 effects: a multiband compressor on the master and a convolution reverb on a send.
//!
//! Then it asserts what the criterion asks:
//! - the export matches the offline oracle (the exported 32-bit WAV is bit-identical to an
//!   independent offline render of the saved project with its own plugin host, and the
//!   real-time engine on a simulated device is bit-identical to the offline render);
//! - loudness sits between -14 and -8 LUFS integrated;
//! - no clipping (sample peak below full scale, true peak below 0 dBTP);
//! - the analysis reports the section structure (`get_state { view: sections }` reads intro,
//!   build and drop at the right bars, and `analyze` per section shows the drop louder than
//!   the intro).
//!
//! The server runs with a private plugin cache (its cache folder points into a temp dir), so
//! the test never reads or writes the user's own plugin cache.

mod common;

use common::*;
use ph_engine::assets::FileResolver;
use ph_engine::plugins::{HostConfig, PluginEnv, PluginHost};
use ph_engine::{AudioBuffer, Engine, EngineConfig, PlayMode, RenderSettings, TransportCommand};
use ph_plugin_host::testing::{SYNTH_ID, test_plugins};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

const BAR: u64 = 3840; // 4/4, 960 ticks per quarter
const BPM: f64 = 124.0;
const LOOP_BPM: f64 = 100.0;
const SR: u32 = 48_000;

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

/// The first command result of a `run` / `run_batch` / convenience tool reply.
fn res(v: &Value) -> &Value {
    &v["results"][0]["result"]
}

/// A generated two-bar drum loop at [`LOOP_BPM`]: a pitched-down sine kick on every beat, a
/// noise snare on 2 and 4, closed hats on the off-beat eighths. Deterministic (an LCG for the
/// noise), stereo (the hats sit slightly apart), peaking around -2 dBFS.
fn drum_loop() -> Vec<Vec<f32>> {
    let beat = (60.0 / LOOP_BPM * SR as f64).round() as usize;
    let frames = 8 * beat;
    let mut l = vec![0.0f32; frames];
    let mut r = vec![0.0f32; frames];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut noise = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0) as f32
    };
    let sr = SR as f64;
    for b in 0..8 {
        let at = b * beat;
        // kick: 150 Hz falling to 50 Hz, 180 ms decay
        let mut phase = 0.0f64;
        for i in 0..(0.35 * sr) as usize {
            let t = i as f64 / sr;
            let f = 50.0 + 100.0 * (-t * 25.0).exp();
            phase += std::f64::consts::TAU * f / sr;
            let v = (phase.sin() * (-t / 0.18).exp() * 0.75) as f32;
            l[at + i] += v;
            r[at + i] += v;
        }
        // snare on beats 2 and 4 of each bar
        if b % 2 == 1 {
            for i in 0..(0.2 * sr) as usize {
                let t = i as f64 / sr;
                let env = (-t / 0.06).exp();
                let tone = (std::f64::consts::TAU * 190.0 * t).sin() * 0.25;
                let v = ((noise() as f64 * 0.45 + tone) * env) as f32;
                l[at + i] += v;
                r[at + i] += v;
            }
        }
        // closed hat on the off-beat eighth: differentiated noise, 25 ms decay
        let off = at + beat / 2;
        let mut prev = 0.0f32;
        for i in 0..(0.08 * sr) as usize {
            let t = i as f64 / sr;
            let n = noise();
            let v = (n - prev) * 0.18 * (-t / 0.025).exp() as f32;
            prev = n;
            l[off + i] += v * 0.9;
            r[off + i] += v * 1.1;
        }
    }
    vec![l, r]
}

#[tokio::test(flavor = "multi_thread")]
async fn m2_acceptance_agent_builds_a_song_through_mcp_only() {
    let plugins = test_plugins();
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song");
    // the generated loop lives inside the project folder, where an agent may read it
    let incoming = song.join("incoming");
    std::fs::create_dir_all(&incoming).unwrap();
    let loop_file = incoming.join("groove100.wav");
    ph_formats::encode::write_wav(
        &loop_file,
        &drum_loop(),
        SR,
        ph_formats::encode::WavFormat::Float32,
    )
    .unwrap();
    // a private plugin cache and Documents for the server process
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let env: Vec<(&str, &Path)> = vec![
        ("LOCALAPPDATA", &home),
        ("USERPROFILE", &home),
        ("HOME", &home),
        ("XDG_CACHE_HOME", &home),
        ("PATTERNHAUS_PLUGIN_SCAN", &plugins.scan_helper),
    ];
    let c = connect_with_env(&song, &[], &env).await;

    // ---- channels, the plugin and the mixer ----------------------------------------------
    ok(&c, "set_tempo", json!({ "bpm": BPM })).await;
    let drum = |s: &str| json!({ "kind": "sampler", "sample": format!("factory:drums/haus808/{s}"), "root_key": 60 });
    let mut chans = Vec::new();
    for (name, inst, insert) in [
        ("Pad", json!({ "kind": "synth", "preset": "pad/warm" }), 1),
        ("Kick", drum("kick"), 3),
        ("Snare", drum("snare"), 3),
        ("Hat", drum("hat_closed"), 3),
        ("Bass", json!({ "kind": "synth", "preset": "bass/sub" }), 4),
        (
            "Lead",
            json!({ "kind": "synth", "preset": "lead/supersaw" }),
            6,
        ),
    ] {
        let r = ok(
            &c,
            "add_channel",
            json!({ "name": name, "instrument": inst, "insert": insert }),
        )
        .await;
        chans.push(id(&r, "channel"));
    }
    let (pad, kick, snare, hat, bass, lead) =
        (chans[0], chans[1], chans[2], chans[3], chans[4], chans[5]);

    // the repo's test CLAP plugin: scan its folder, add the synth, route it to insert 2
    let scan = ok(
        &c,
        "run",
        json!({ "id": "plugin.scan", "params": { "paths": [plugins.dir.to_string_lossy()] } }),
    )
    .await;
    assert_eq!(
        res(&scan)["plugins"].as_array().map(Vec::len),
        Some(3),
        "{scan}"
    );
    let added = ok(
        &c,
        "run",
        json!({ "id": "plugin.add_instrument",
                "params": { "format": "clap", "plugin": SYNTH_ID, "name": "Plug Arp" } }),
    )
    .await;
    let arp = res(&added)["channel"].as_u64().expect("plugin channel");
    ok(&c, "route_channel", json!({ "channel": arp, "insert": 2 })).await;
    ok(
        &c,
        "run",
        json!({ "id": "plugin.set_param", "params": { "channel": arp, "param": "1", "value": 1 } }),
    )
    .await; // saw
    ok(
        &c,
        "run",
        json!({ "id": "plugin.set_param", "params": { "channel": arp, "param": "0", "value": 0.35 } }),
    )
    .await;

    // effects: a filter on the pad (its cutoff gets the sweep), EQ on the bass, the
    // convolution reverb on a send, the multiband compressor and a limiter on the master
    ok(
        &c,
        "add_effect",
        json!({ "insert": 1, "effect": { "kind": "builtin", "name": "filter" } }),
    )
    .await;
    ok(
        &c,
        "set_effect_param",
        json!({ "insert": 1, "index": 0, "param": "cutoff", "value": 12000.0 }),
    )
    .await;
    ok(
        &c,
        "add_effect",
        json!({ "insert": 4, "effect": { "kind": "builtin", "name": "eq" } }),
    )
    .await;
    ok(
        &c,
        "add_effect",
        json!({ "insert": 5, "effect": { "kind": "builtin", "name": "convolution" } }),
    )
    .await;
    ok(
        &c,
        "set_effect_param",
        json!({ "insert": 5, "index": 0, "param": "mix", "value": 1.0 }),
    )
    .await;
    for from in [1, 2] {
        ok(
            &c,
            "add_send",
            json!({ "from": from, "to": 5, "level": 0.35 }),
        )
        .await;
    }
    ok(
        &c,
        "add_effect",
        json!({ "insert": 0, "effect": { "kind": "builtin", "name": "multiband" } }),
    )
    .await;
    ok(
        &c,
        "add_effect",
        json!({ "insert": 0, "effect": { "kind": "builtin", "name": "limiter" } }),
    )
    .await;

    // ---- patterns: one per section (named after it) plus the shared ones -----------------
    let pattern = |name: &'static str| {
        let c = &c;
        async move {
            id(
                &ok(c, "add_pattern", json!({ "name": name })).await,
                "pattern",
            )
        }
    };
    let intro = pattern("Intro").await;
    let build = pattern("Build").await;
    let drop_pat = pattern("Drop").await;
    let chords = pattern("Chords").await;
    let note = |key: u64, start: u64, length: u64, velocity: f64| json!({ "key": key, "start": start, "length": length, "velocity": velocity });
    let add_notes = |pattern: u64, channel: u64, notes: Vec<Value>| {
        let c = &c;
        async move {
            ok(
                c,
                "add_notes",
                json!({ "pattern": pattern, "channel": channel, "notes": notes }),
            )
            .await;
        }
    };
    // chords: four bars, one chord per bar (Am F C G)
    let progression: [[u64; 3]; 4] = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]];
    let mut pad_notes = Vec::new();
    for (bar, chord) in progression.iter().enumerate() {
        for k in chord {
            pad_notes.push(note(*k, bar as u64 * BAR, BAR - 120, 0.7));
        }
    }
    add_notes(chords, pad, pad_notes).await;
    // the plugin arpeggio, all through
    let arp_notes: Vec<Value> = [69u64, 72, 76, 79, 76, 72, 69, 64]
        .iter()
        .enumerate()
        .map(|(i, k)| note(*k, i as u64 * 480, 360, 0.8))
        .collect();
    add_notes(intro, arp, arp_notes.clone()).await;
    add_notes(build, arp, arp_notes.clone()).await;
    add_notes(drop_pat, arp, arp_notes).await;
    // the build: a snare roll in sixteenths, louder towards the bar end
    let roll: Vec<Value> = (0..16)
        .map(|i| note(60, i * 240, 120, 0.25 + 0.015 * i as f64))
        .collect();
    add_notes(build, snare, roll).await;
    // the drop: four on the floor, backbeat, eighth hats, a bass line
    add_notes(
        drop_pat,
        kick,
        (0..4).map(|i| note(60, i * 960, 240, 1.0)).collect(),
    )
    .await;
    add_notes(
        drop_pat,
        snare,
        [960u64, 2880]
            .iter()
            .map(|t| note(60, *t, 240, 0.9))
            .collect(),
    )
    .await;
    add_notes(
        drop_pat,
        hat,
        (0..8).map(|i| note(60, i * 480, 120, 0.6)).collect(),
    )
    .await;
    add_notes(
        drop_pat,
        bass,
        [33u64, 33, 45, 33]
            .iter()
            .enumerate()
            .map(|(i, k)| note(*k, i as u64 * 960, 840, 0.9))
            .collect(),
    )
    .await;
    // and a lead hook on the offbeats
    add_notes(
        drop_pat,
        lead,
        [76u64, 79, 81, 79]
            .iter()
            .enumerate()
            .map(|(i, k)| note(*k, i as u64 * 960 + 480, 400, 0.8))
            .collect(),
    )
    .await;

    // ---- the arrangement -----------------------------------------------------------------
    let mut clips: Vec<Value> = Vec::new();
    for b in 0..32u64 {
        let p = match b {
            0..=7 => intro,
            8..=15 => build,
            _ => drop_pat,
        };
        clips.push(json!({ "id": "playlist.add_clip", "params": { "track": 0, "pattern": p, "start": b * BAR } }));
    }
    for k in 0..8u64 {
        clips.push(json!({ "id": "playlist.add_clip", "params": { "track": 1, "pattern": chords, "start": k * 4 * BAR } }));
    }
    ok(
        &c,
        "run_batch",
        json!({ "commands": clips, "as_one_undo": true }),
    )
    .await;

    // ---- the drum loop as audio, fitted to the project tempo -----------------------------
    let placed = ok(
        &c,
        "run",
        json!({ "id": "audio.add_clip",
                "params": { "track": 2, "start": 8 * BAR, "path": "incoming/groove100.wav" } }),
    )
    .await;
    let loop_clip = res(&placed)["clip"].as_u64().expect("audio clip");
    let sample = res(&placed)["sample"].as_u64().expect("sample id");
    let fitted = ok(
        &c,
        "run",
        json!({ "id": "audio.fit_to_tempo", "params": { "clip": loop_clip } }),
    )
    .await;
    let detected = res(&fitted)["detected_bpm"]
        .as_f64()
        .expect("the loop's tempo was detected");
    assert!(
        (detected - LOOP_BPM).abs() < 0.01 * LOOP_BPM,
        "detected {detected} BPM for a {LOOP_BPM} BPM loop: {fitted}"
    );
    // two bars of loop are two bars of song once fitted
    assert_eq!(res(&fitted)["length"], 2 * BAR, "{fitted}");
    assert_eq!(res(&fitted)["mode"], "stretch", "{fitted}");
    // repeat it through the build and the drop
    let repeats: Vec<Value> = (1..12u64)
        .map(|k| {
            json!({ "id": "audio.add_clip", "params": {
                "track": 2, "start": 8 * BAR + k * 2 * BAR, "sample": sample,
                "tempo_sync": true, "stretch": "stretch", "length": 2 * BAR } })
        })
        .collect();
    let placed = ok(
        &c,
        "run_batch",
        json!({ "commands": repeats, "as_one_undo": true }),
    )
    .await;
    // the loop sits back in the build (its first four clips), full level in the drop
    let mut build_clips = vec![loop_clip];
    build_clips.extend(
        placed["results"].as_array().unwrap()[..3]
            .iter()
            .map(|r| r["result"]["clip"].as_u64().unwrap()),
    );
    let quieter: Vec<Value> = build_clips
        .iter()
        .map(|clip| json!({ "id": "audio.set_gain", "params": { "clip": clip, "gain_db": -7.0 } }))
        .collect();
    ok(
        &c,
        "run_batch",
        json!({ "commands": quieter, "as_one_undo": true }),
    )
    .await;

    // ---- the slice instrument from that loop, playing the drop ---------------------------
    let sliced = ok(
        &c,
        "run",
        json!({ "id": "slicer.make_instrument",
                "params": { "sample": sample, "write_pattern": true, "name": "Loop Slices" } }),
    )
    .await;
    let slices = res(&sliced)["slices"].as_u64().unwrap();
    assert!(
        slices >= 8,
        "a two-bar loop with kicks on every beat slices at least 8 times: {sliced}"
    );
    let slice_ch = res(&sliced)["channel"].as_u64().unwrap();
    let slice_pat = res(&sliced)["pattern"].as_u64().unwrap();
    ok(
        &c,
        "route_channel",
        json!({ "channel": slice_ch, "insert": 3 }),
    )
    .await;
    ok(
        &c,
        "set_channel_volume",
        json!({ "channel": slice_ch, "volume": 0.45 }),
    )
    .await;
    let slice_clips: Vec<Value> = (0..8u64)
        .map(|k| json!({ "id": "playlist.add_clip", "params": { "track": 3, "pattern": slice_pat, "start": 16 * BAR + k * 2 * BAR } }))
        .collect();
    ok(
        &c,
        "run_batch",
        json!({ "commands": slice_clips, "as_one_undo": true }),
    )
    .await;

    // ---- automation: the filter sweep and the volume ride --------------------------------
    let sweep = ok(
        &c,
        "run",
        json!({ "id": "automation.add_clip", "params": {
            "target": "insert/1/slot/0/param/cutoff", "track": 4, "start": 0, "length": 16 * BAR,
            "points": [
                { "tick": 0, "value": 250.0, "curve": "smooth" },
                { "tick": 8 * BAR, "value": 1800.0, "curve": "smooth" },
                { "tick": 16 * BAR, "value": 12000.0 }
            ] } }),
    )
    .await;
    assert!(res(&sweep)["clip"].is_u64(), "{sweep}");
    let ride = ok(
        &c,
        "run",
        json!({ "id": "automation.add_clip", "params": {
            "target": "insert/2/volume", "track": 5, "start": 8 * BAR, "length": 8 * BAR,
            "points": [
                { "tick": 0, "value": 0.35 },
                { "tick": 8 * BAR, "value": 0.8 }
            ] } }),
    )
    .await;
    assert!(res(&ride)["clip"].is_u64(), "{ride}");

    // ---- what the agent sees: no warnings, the plugin and the M2 effects in place --------
    let warnings = ok(&c, "get_state", json!({ "view": "warnings" })).await;
    assert_eq!(warnings["warnings"], json!([]), "{warnings}");
    let mixer = ok(&c, "get_state", json!({ "view": "mixer" })).await;
    let text = mixer.to_string();
    for fx in ["multiband", "convolution", "limiter", "filter"] {
        assert!(text.contains(fx), "{fx} missing from {mixer}");
    }
    let channels = ok(&c, "get_state", json!({ "view": "channels" })).await;
    assert!(
        channels.to_string().contains(SYNTH_ID) || channels.to_string().contains("plugin"),
        "{channels}"
    );

    // ---- the analysis reports the section structure --------------------------------------
    let sections = ok(&c, "get_state", json!({ "view": "sections" })).await;
    assert_eq!(sections["song_bars"], 32, "{sections}");
    let read: Vec<(String, u64, u64)> = sections["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["label"].as_str().unwrap().to_string(),
                s["from_bar"].as_u64().unwrap(),
                s["to_bar"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            ("intro".to_string(), 1, 8),
            ("build".to_string(), 9, 16),
            ("drop".to_string(), 17, 32)
        ],
        "{sections}"
    );
    // ... and the sections sound like it: each louder than the one before
    let mut section_lufs = Vec::new();
    for (from, to) in [(1, 8), (9, 16), (17, 32)] {
        let a = ok(
            &c,
            "analyze",
            json!({ "what": ["loudness"], "range": { "from_bar": from, "to_bar": to } }),
        )
        .await;
        section_lufs.push(a["loudness"]["integrated_lufs"].as_f64().unwrap_or(-120.0));
    }
    assert!(
        section_lufs[0] < section_lufs[1] && section_lufs[1] < section_lufs[2],
        "intro < build < drop in loudness: {section_lufs:?}"
    );

    // ---- gain staging on the master limiter, from the agent's own measurement ------------
    let report = ok(&c, "analyze", json!({ "what": ["loudness", "health"] })).await;
    let lufs0 = report["loudness"]["integrated_lufs"].as_f64().unwrap();
    ok(
        &c,
        "set_effect_param",
        json!({ "insert": 0, "index": 1, "param": "input_gain", "value": (-10.5 - lufs0).clamp(0.0, 12.0) }),
    )
    .await;

    // ---- export (32-bit float, so the oracle comparison is exact) and save ---------------
    let rendered = ok(
        &c,
        "render",
        json!({ "name": "acceptance-m2", "format": "wav", "bit_depth": 32 }),
    )
    .await;
    let wav = std::path::PathBuf::from(rendered["path"].as_str().unwrap());
    assert!(wav.exists(), "{rendered}");
    let saved = ok(&c, "run", json!({ "id": "file.save" })).await;
    assert_eq!(
        res(&saved)["plugin_states"].as_array().map(Vec::len),
        Some(1),
        "the plugin's state is saved next to project.json: {saved}"
    );
    let shot = call(&c, "snapshot", json!({ "kind": "playlist" })).await;
    assert!(!shot.is_error && shot.images.len() == 1, "{:?}", shot.texts);
    assert_png(&shot.images[0], 600);
    drop(c);

    // ---- the export matches the offline oracle -------------------------------------------
    let exported = ph_formats::decode::decode_file(&wav).unwrap();
    assert_eq!(exported.sample_rate as u32, SR);
    let project = ph_formats::load_project(&song).unwrap();
    assert_eq!(project.samples.len(), 1, "one registered sample, the loop");
    let assets = resolver(&song);
    // an independent plugin host, scanning the test plugins itself
    let host = PluginHost::new(HostConfig {
        cache_file: None,
        extra_paths: vec![],
        scan_helper: Some(plugins.scan_helper.clone()),
        timeout: None,
    });
    host.scan(std::slice::from_ref(&plugins.dir), |_| {})
        .unwrap();
    let env = PluginEnv::new(Arc::new(host), &song);
    assert!(
        ph_engine::project_warnings_with(&project, &assets, Some(&env)).is_empty(),
        "{:?}",
        ph_engine::project_warnings_with(&project, &assets, Some(&env))
    );
    let settings = |tail: bool, block: usize| RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: SR,
        quality: ph_node::Quality::Live,
        include_tail: tail,
        block_frames: block,
    };
    // an independent render of the saved project (plugin state read from plugins/), with a
    // different block size than the export
    let oracle =
        ph_engine::render_offline_with(&project, &settings(true, 256), &assets, Some(&env))
            .unwrap();
    assert_eq!(
        first_difference(&exported.channels, &oracle.channels),
        None,
        "the exported WAV differs from the offline oracle (channel, frame)"
    );
    // the test plugin is in the export: without plugin hosting the render differs
    let unhosted = ph_engine::render_offline(&project, &settings(true, 256), &assets).unwrap();
    assert!(
        first_difference(&unhosted.channels, &oracle.channels).is_some(),
        "the plugin synth is audible in the export"
    );
    // real time through a simulated device, against the offline render of the song
    let song_only =
        ph_engine::render_offline_with(&project, &settings(false, 512), &assets, Some(&env))
            .unwrap();
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: 256,
            ..EngineConfig::default()
        },
        Arc::new(resolver(&song)),
    )
    .unwrap();
    engine.set_plugins(Some(env.clone())).unwrap();
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
    assert!(engine.stats().warnings.is_empty());
    assert_eq!(engine.stats().underruns, 0);

    // ---- -14..-8 LUFS, no clipping (measured on the exported file itself) ----------------
    let loud = ph_analysis::loudness::analyze(&exported.channels, SR);
    let lufs = loud.integrated_lufs.expect("an integrated loudness");
    let peak = exported
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    let tp = loud.true_peak_dbtp.expect("a true peak");
    println!(
        "M2 acceptance export: {lufs:.2} LUFS integrated, true peak {tp:.2} dBTP, sample peak {peak:.3} ({:.2} dBFS); sections {section_lufs:?} LUFS",
        20.0 * peak.log10()
    );
    assert!((-14.0..=-8.0).contains(&lufs), "integrated {lufs} LUFS");
    assert!(peak < 1.0, "sample peak {peak} clips");
    assert!(tp < 0.0, "true peak {tp} dBTP");
    // 32 bars at 124 BPM is 61.9 s, plus the reverb tail
    let seconds = rendered["seconds"].as_f64().unwrap();
    assert!(seconds >= 61.9, "32 bars at 124 BPM: {rendered}");
}
