//! The agent toolset for M2 (M2/T9), end to end through `Agent::call`: tool coverage of the
//! M2 command families, the audio / automation / plugin views, musical diffs in words, a
//! rendered audio excerpt, reference-track comparison, and agent preview mode (proposals).
//! Audio is generated; no device is opened.

use super::*;
use crate::agent::Block;
use ph_commands::{Author, Grant, Scope};
use ph_formats::encode::{WavFormat, write_wav};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

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
        Project::empty("m2"),
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

fn wait_for(mut cond: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn changes(v: &Value) -> String {
    v["changes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_default()
}

/// A kick pattern over `bars` bars on the factory kick, arranged on the playlist.
fn kick_song(a: &Agent, bars: u64) {
    call(a, "set_tempo", json!({ "bpm": 120.0 }));
    let pat = call(a, "add_pattern", json!({ "name": "Beat" }))["results"][0]["result"]["pattern"]
        .clone();
    let ch = call(
        a,
        "add_channel",
        json!({ "name": "Kick", "instrument": { "kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60 } }),
    )["results"][0]["result"]["channel"]
        .clone();
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

/// A generated loop: a short decaying sine on every beat, so it has a clear tempo.
fn write_loop(path: &std::path::Path, bpm: f64) {
    let sr = 48_000.0;
    let beat = 60.0 / bpm;
    let n = (8.0 * beat * sr) as usize;
    let mut x = vec![0.0f32; n];
    for b in 0..8 {
        let s0 = (b as f64 * beat * sr) as usize;
        for i in 0..(0.2 * sr) as usize {
            if s0 + i < n {
                let t = i as f64 / sr;
                x[s0 + i] =
                    (0.8 * (std::f64::consts::TAU * 110.0 * t).sin() * (-t * 15.0).exp()) as f32;
            }
        }
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_wav(path, &[x], 48_000, WavFormat::Float32).unwrap();
}

#[test]
fn every_m2_command_family_is_reachable_and_described() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    let listed = call(&a, "list_commands", json!({}));
    let lines: Vec<&str> = listed["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    for family in [
        "audio.",
        "sample.",
        "slicer.",
        "editor.",
        "automation.",
        "plugin.",
        "record.",
        "midi.",
        "proposal.",
    ] {
        let n = lines.iter().filter(|l| l.starts_with(family)).count();
        assert!(n >= 2, "{family} commands are listed: {n}");
        // every one describes itself, with a description on every parameter
        for l in lines.iter().filter(|l| l.starts_with(family)) {
            let id = l.split(' ').next().unwrap();
            let d = call(&a, "describe_command", json!({ "id": id }));
            assert!(d["summary"].as_str().unwrap().len() > 20, "{id}");
            if let Some(props) = d["params"]["properties"].as_object() {
                for (name, p) in props {
                    assert!(
                        p["description"].is_string(),
                        "{id}.{name} has no description"
                    );
                }
            }
        }
    }
    // the convenience tools exist, are described, and map onto real commands
    let tools = a.tools();
    for (tool, cmd) in [
        ("add_audio_clip", "audio.add_clip"),
        ("fit_to_tempo", "audio.fit_to_tempo"),
        ("add_automation", "automation.add_clip"),
        ("add_plugin_instrument", "plugin.add_instrument"),
        ("add_plugin_effect", "plugin.add_effect"),
        ("propose", "proposal.submit"),
    ] {
        let t = tools
            .iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("tool {tool}"));
        assert!(t.description.len() > 80, "{tool}: {}", t.description);
        assert_eq!(
            t.input_schema,
            s.handle().registry.get(cmd).unwrap().spec().params,
            "{tool} takes {cmd}'s parameters"
        );
        assert!(!t.read_only);
    }
    let st = tools.iter().find(|t| t.name == "proposal_status").unwrap();
    assert!(st.read_only);
    // render and analyze advertise the new abilities
    let render = tools.iter().find(|t| t.name == "render").unwrap();
    assert!(
        render.description.contains("listen")
            && render.input_schema["properties"]["listen"].is_object()
    );
    let analyze = tools.iter().find(|t| t.name == "analyze").unwrap();
    assert!(analyze.description.contains("reference"));
}

#[test]
fn views_and_diffs_describe_audio_and_automation_in_words() {
    let dir = tempfile::tempdir().unwrap();
    write_loop(&dir.path().join("samples/loop.wav"), 100.0);
    let s = studio(dir.path());
    let a = agent_for(&s);
    kick_song(&a, 4);

    // an audio clip: added, then edited
    let added = call(
        &a,
        "add_audio_clip",
        json!({ "track": 1, "start": 3840, "path": "samples/loop.wav" }),
    );
    let sample = added["results"][0]["result"]["sample"].clone();
    let clip = added["results"][0]["result"]["clip"].clone();
    let text = changes(&added);
    assert!(text.contains("sample registered: loop.wav"), "{text}");
    assert!(
        text.contains("audio: added loop.wav (clip") && text.contains("on track 2: resample"),
        "{text}"
    );
    call(
        &a,
        "run",
        json!({ "id": "sample.set_bpm", "params": { "sample": sample, "bpm": 100.0 } }),
    );
    let edit = call(
        &a,
        "run_batch",
        json!({ "commands": [
            { "id": "audio.set_stretch", "params": { "clip": clip, "mode": "stretch", "tempo_sync": true } },
            { "id": "audio.set_gain", "params": { "clip": clip, "gain_db": -6.0 } },
            { "id": "audio.set_reverse", "params": { "clip": clip, "reverse": true } },
        ] }),
    );
    let text = changes(&edit);
    assert!(text.contains("mode resample -> stretch"), "{text}");
    assert!(text.contains("now follows the project tempo"), "{text}");
    assert!(text.contains("gain +0.0 -> -6.0 dB"), "{text}");
    assert!(text.contains("reversed"), "{text}");

    // an automation clip: added with points, then a point added
    let auto = call(
        &a,
        "add_automation",
        json!({ "target": "channel/0/volume", "track": 2, "start": 0, "length": 7680,
                "points": [{ "tick": 0, "value": 0.2 }, { "tick": 7680, "value": 0.9 }] }),
    );
    let aclip = auto["results"][0]["result"]["clip"].clone();
    let text = changes(&auto);
    assert!(
        text.contains("automation: added Kick volume (clip")
            && text.contains("2 point(s), 0.2 to 0.9"),
        "{text}"
    );
    let more = call(
        &a,
        "run",
        json!({ "id": "automation.add_point", "params": { "clip": aclip, "tick": 3840, "value": 1.0 } }),
    );
    let text = changes(&more);
    assert!(
        text.contains("automation of Kick volume")
            && text.contains("points +1, now 3 point(s) from 0.2 to 1"),
        "{text}"
    );

    // the views
    wait_for(|| s.project().clips.len() == 3, "clips");
    let v = call(&a, "get_state", json!({ "view": "audio" }));
    let c = &v["audio_clips"][0];
    assert_eq!(c["name"], "loop.wav");
    assert_eq!(c["file"], "samples/loop.wav");
    assert_eq!(c["mode"], "stretch");
    assert_eq!(c["bpm"], 100.0);
    assert_eq!(c["tempo_sync"], true);
    assert_eq!(c["gain_db"], -6.0);
    assert_eq!(c["reverse"], true);
    assert_eq!(c["track"], 2);
    assert!(
        c["text"]
            .as_str()
            .unwrap()
            .contains("follows tempo (100 -> 120 bpm)"),
        "{c}"
    );
    assert_eq!(v["samples"][0]["bpm_source"], "user");
    let v = call(&a, "get_state", json!({ "view": "automation" }));
    let c = &v["automation_clips"][0];
    assert_eq!(c["target"], "channel/0/volume");
    assert_eq!(c["target_words"], "Kick volume");
    assert_eq!(c["points"], 3);
    assert_eq!(
        (c["min"].as_f64(), c["max"].as_f64()),
        (Some(0.2), Some(1.0))
    );
    let v = call(&a, "get_state", json!({ "view": "plugins" }));
    assert_eq!(v["plugins"].as_array().unwrap().len(), 0);
    let o = call(&a, "get_state", json!({}));
    assert_eq!(
        (o["audio_clips"].as_u64(), o["automation_clips"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(o["samples"], 1);

    // removing them reads in words too
    let gone = call(
        &a,
        "run",
        json!({ "id": "playlist.remove_clip", "params": { "clip": clip } }),
    );
    assert!(
        changes(&gone).contains("audio: removed loop.wav"),
        "{}",
        changes(&gone)
    );
}

#[test]
fn plugin_instances_show_in_the_view_and_the_diff() {
    use ph_engine::plugins::HostConfig;
    use ph_plugin_host::testing::{GAIN_ID, SYNTH_ID, test_plugins};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("song");
    std::fs::create_dir_all(&dir).unwrap();
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let s = Studio::start(
        StudioConfig {
            project_dir: dir,
            assets_dir: assets,
            library_roots: vec![],
            exports_dir: None,
            projects_dir: None,
            engine: EngineConfig::default(),
            audio: AudioMode::Silent,
            plugins: HostConfig {
                cache_file: Some(tmp.path().join("cache.json")),
                scan_helper: Some(test_plugins().scan_helper.clone()),
                ..Default::default()
            },
        },
        Project::empty("plugins"),
    )
    .unwrap();
    let a = agent_for(&s);
    call(
        &a,
        "run",
        json!({ "id": "plugin.scan", "params": { "paths": [test_plugins().dir.to_string_lossy()] } }),
    );
    let inst = call(
        &a,
        "add_plugin_instrument",
        json!({ "format": "clap", "plugin": SYNTH_ID }),
    );
    let text = changes(&inst);
    assert!(
        text.contains("instrument is now plugin") || text.contains("added channel"),
        "{text}"
    );
    let fx = call(
        &a,
        "add_plugin_effect",
        json!({ "insert": 1, "format": "clap", "plugin": GAIN_ID }),
    );
    assert!(changes(&fx).contains("effects now ["), "{}", changes(&fx));
    wait_for(|| s.project().channels.len() == 1, "channel");
    let v = call(&a, "get_state", json!({ "view": "plugins" }));
    let list = v["plugins"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{v}");
    assert!(
        list.iter()
            .any(|p| p["role"] == "instrument" && p["plugin"] == SYNTH_ID && p["format"] == "clap")
    );
    assert!(
        list.iter()
            .any(|p| p["role"] == "effect" && p["plugin"] == GAIN_ID && p["insert"] == 1)
    );
    let o = call(&a, "get_state", json!({}));
    assert_eq!(o["plugins"], 2);
    // a parameter change reads as one
    let ch = s.project().channels.keys().next().unwrap().0;
    let set = call(
        &a,
        "run",
        json!({ "id": "plugin.set_param", "params": { "channel": ch, "param": "1", "value": 1.0 } }),
    );
    assert!(changes(&set).contains("plugin"), "{}", changes(&set));
}

// ---- rendered audio ----------------------------------------------------------------------

fn decode_wav(bytes: &[u8]) -> (hound::WavSpec, Vec<i16>) {
    let mut r = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
    let spec = r.spec();
    (spec, r.samples::<i16>().map(|s| s.unwrap()).collect())
}

#[test]
fn render_can_return_a_bounded_audio_excerpt() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    kick_song(&a, 32); // 64 s at 120 bpm
    // without listen: no audio block
    let plain = a.call(
        "claude",
        "render",
        json!({ "range": { "from_bar": 1, "to_bar": 2 } }),
    );
    assert!(!plain.is_error);
    assert!(!plain.blocks.iter().any(|b| matches!(b, Block::Audio(_))));

    let out = a.call(
        "claude",
        "render",
        json!({ "listen": true, "listen_seconds": 3, "listen_range": { "from_bar": 3, "to_bar": 8 } }),
    );
    assert!(!out.is_error, "{:?}", out.blocks);
    let info = out.value().unwrap();
    let wav = out
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Audio(w) => Some(w.clone()),
            _ => None,
        })
        .expect("an audio block");
    let (spec, samples) = decode_wav(&wav);
    assert_eq!((spec.sample_rate, spec.bits_per_sample), (24_000, 16));
    let secs = samples.len() as f64 / spec.channels as f64 / spec.sample_rate as f64;
    assert!((secs - 3.0).abs() < 0.05, "3 s asked for, got {secs}");
    assert!(
        samples.iter().any(|x| x.abs() > 2000),
        "the kicks are audible"
    );
    assert_eq!(info["listen"]["from_bar"], 3);
    assert_eq!(
        info["listen"]["truncated"], true,
        "bars 3-8 are 10 s, cut to 3 s"
    );
    assert_eq!(info["listen"]["bytes"], wav.len());
    // the MCP shape carries base64 WAV audio content
    let j = out.to_json();
    let audio = j["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "audio")
        .unwrap();
    assert_eq!(audio["mimeType"], "audio/wav");
    assert!(audio["data"].as_str().unwrap().len() > 1000);

    // the limits hold however much is asked for
    let big = a.call(
        "claude",
        "render",
        json!({ "listen": true, "listen_seconds": 999 }),
    );
    let wav = big
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Audio(w) => Some(w.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        wav.len() <= crate::agent::MAX_LISTEN_BYTES,
        "{} bytes",
        wav.len()
    );
    let (spec, samples) = decode_wav(&wav);
    let secs = samples.len() as f64 / spec.channels as f64 / spec.sample_rate as f64;
    assert!(secs <= crate::agent::MAX_LISTEN_SECONDS + 0.05, "{secs}");
    assert_eq!(
        big.value().unwrap()["listen"]["limits"]["max_seconds"],
        15.0
    );

    // an empty excerpt range is explained, not an error
    let none = a.call(
        "claude",
        "render",
        json!({ "listen": true, "range": { "from_bar": 1, "to_bar": 2 }, "listen_range": { "from_bar": 5, "to_bar": 6 } }),
    );
    assert!(none.value().unwrap()["listen"]["error"].is_string());
}

// ---- reference comparison ---------------------------------------------------------------

fn tone_file(path: &std::path::Path, freqs: &[(f64, f64)], stereo_offset: bool) {
    let sr = 44_100u32;
    let n = (sr as f64 * 4.0) as usize;
    let mk = |phase: f64| -> Vec<f32> {
        (0..n)
            .map(|i| {
                freqs
                    .iter()
                    .map(|(f, amp)| {
                        amp * (std::f64::consts::TAU * f * i as f64 / sr as f64 + phase).sin()
                    })
                    .sum::<f64>() as f32
            })
            .collect()
    };
    let (l, r) = (mk(0.0), mk(if stereo_offset { 1.3 } else { 0.0 }));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_wav(path, &[l, r], sr, WavFormat::Float32).unwrap();
}

#[test]
fn analyze_compares_the_mix_with_a_reference() {
    let dir = tempfile::tempdir().unwrap();
    // a reference with a lot of high-mid and a wide stereo image
    tone_file(
        &dir.path().join("samples/ref.wav"),
        &[(100.0, 0.05), (3000.0, 0.15)],
        true,
    );
    let s = studio(dir.path());
    let a = agent_for(&s);
    kick_song(&a, 4);
    let v = call(
        &a,
        "analyze",
        json!({ "what": ["reference"], "reference": "samples/ref.wav", "range": { "from_bar": 1, "to_bar": 4 } }),
    );
    let r = &v["reference"];
    assert_eq!(r["reference"]["name"], "ref.wav");
    assert_eq!(r["bands"].as_array().unwrap().len(), 7);
    let band = |n: &str| {
        r["bands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["band"] == n)
            .unwrap()
            .clone()
    };
    assert!(
        band("bass")["diff_db"].as_f64().unwrap() > 3.0,
        "a kick mix has more bass than the reference: {}",
        band("bass")
    );
    assert!(
        band("high_mid")["diff_db"].as_f64().unwrap() < -3.0,
        "{}",
        band("high_mid")
    );
    assert!(r["loudness"]["mix_integrated_lufs"].is_number());
    assert!(r["loudness"]["reference_integrated_lufs"].is_number());
    assert!(r["loudness"]["integrated_diff_lu"].is_number());
    assert!(r["loudness"]["short_term_max_diff_lu"].is_number());
    assert!(r["stereo"]["reference_correlation"].as_f64().unwrap() < 0.5);
    assert!(r["stereo"]["width_diff_db"].is_number());
    let tips = r["suggestions"].as_array().unwrap();
    assert!(
        tips.len() >= 2
            && tips
                .iter()
                .any(|t| t.as_str().unwrap().contains("dB vs the reference")),
        "{tips:?}"
    );

    // it works next to the other analyses, and needs its argument
    let both = call(
        &a,
        "analyze",
        json!({ "what": ["loudness", "reference"], "reference": "samples/ref.wav" }),
    );
    assert!(both["loudness"].is_object() && both["reference"].is_object());
    assert!(
        call_err(&a, "analyze", json!({ "what": ["reference"] })).contains("needs `reference`")
    );
    // reads stay confined to the folders agents may read
    let outside = std::env::temp_dir().join("ph-outside-reference.wav");
    let e = call_err(
        &a,
        "analyze",
        json!({ "what": ["reference"], "reference": outside.to_string_lossy() }),
    );
    assert!(e.contains("cannot read the reference"), "{e}");
    // the reference is never copied into the project
    let files = |d: &std::path::Path| std::fs::read_dir(d).unwrap().count();
    assert_eq!(files(&dir.path().join("samples")), 1);
    assert!(!dir.path().join("renders").exists() || files(&dir.path().join("renders")) == 0);

    // a registered sample works as a reference too (by id)
    let added = call(
        &a,
        "add_audio_clip",
        json!({ "track": 5, "start": 0, "path": "samples/ref.wav" }),
    );
    let id = added["results"][0]["result"]["sample"].clone();
    let v2 = call(
        &a,
        "analyze",
        json!({ "what": ["reference"], "reference": id }),
    );
    assert_eq!(v2["reference"]["reference"]["name"], "ref.wav");
}

// ---- agent preview mode ------------------------------------------------------------------

fn bass_proposal(pat: &Value, ch: &Value, n: u64) -> Value {
    let notes: Vec<Value> = (0..n)
        .map(|i| json!({ "key": 36 + i, "start": i * 960, "length": 480 }))
        .collect();
    json!({ "title": "Add a bass line", "commands": [
        { "id": "note.add", "params": { "pattern": pat, "channel": ch, "notes": notes } }] })
}

fn pattern_and_channel(s: &Studio) -> (Value, Value) {
    let h = Caller::human();
    let p = s.run(&h, "pattern.add", json!({})).unwrap().result["pattern"].clone();
    let c = s
        .run(
            &h,
            "channel.add",
            json!({ "name": "Bass", "instrument": { "kind": "synth", "preset": "bass/sub" } }),
        )
        .unwrap()
        .result["channel"]
        .clone();
    (p, c)
}

fn note_total(s: &Studio) -> usize {
    s.handle()
        .session
        .project()
        .unwrap()
        .patterns
        .values()
        .map(|p| p.note_count())
        .sum()
}

#[test]
fn a_proposal_previews_then_lands_as_one_undo_step_attributed_to_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    let (pat, ch) = pattern_and_channel(&s);
    let hub = s.handle().events().unwrap();
    let seq0 = hub.last_seq();

    let out = call(&a, "propose", bass_proposal(&pat, &ch, 4));
    let id = out["results"][0]["result"]["proposal"].as_u64().unwrap();
    assert_eq!(out["results"][0]["result"]["status"], "pending");
    // the musical diff of what it would do, and nothing applied
    let would = out["results"][0]["would_change"].to_string();
    assert!(would.contains("+4 notes"), "{would}");
    assert!(out.get("txn").is_none() || out["txn"].is_null());
    assert_eq!(note_total(&s), 0, "the project did not change");
    let view = call(&a, "get_state", json!({ "view": "proposals" }));
    assert_eq!(view["pending"], 1);
    assert_eq!(view["proposals"][0]["status"]["state"], "pending");
    let st = call(&a, "proposal_status", json!({ "id": id }));
    assert_eq!(st["decided"], false);

    // the person accepts
    let h = Caller::human();
    let before = s.handle().session.history().unwrap().len();
    s.run(&h, "proposal.accept", json!({ "id": id })).unwrap();
    assert_eq!(note_total(&s), 4);
    let hist = s.handle().session.history().unwrap();
    assert_eq!(hist.len(), before + 1, "one transaction");
    assert_eq!(hist.last().unwrap().author, Author::Agent("claude".into()));

    // the agent hears about it without asking
    let next = a.call("claude", "get_state", json!({ "view": "proposals" }));
    let texts: Vec<String> = next
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let dec = texts
        .iter()
        .find(|t| t.contains("proposal_decisions"))
        .expect("decision block");
    assert!(dec.contains("accepted"), "{dec}");
    // and only once
    let again = a.call("claude", "get_state", json!({}));
    assert!(
        !again
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Text(t) if t.contains("proposal_decisions")))
    );
    // proposal_status agrees, with the transaction
    let st = call(&a, "proposal_status", json!({ "id": id }));
    assert_eq!(st["decided"], true);
    assert_eq!(st["proposal"]["status"]["state"], "accepted");
    assert_eq!(st["proposal"]["status"]["txn"], hist.last().unwrap().txn);

    // one undo_mine takes the whole thing back
    call(&a, "undo_mine", json!({}));
    assert_eq!(note_total(&s), 0);

    // the event stream saw it
    let kinds: Vec<_> = hub
        .since(seq0)
        .into_iter()
        .filter(|e| e.kind == "proposal")
        .map(|e| e.summary)
        .collect();
    wait_for(
        || {
            hub.since(seq0)
                .iter()
                .filter(|e| e.kind == "proposal")
                .count()
                >= 2
        },
        "events",
    );
    assert!(kinds.len() <= 2);
}

#[test]
fn a_rejected_proposal_changes_nothing_and_the_agent_reads_why() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    let (pat, ch) = pattern_and_channel(&s);
    let id = call(&a, "propose", bass_proposal(&pat, &ch, 2))["results"][0]["result"]["proposal"]
        .as_u64()
        .unwrap();
    let project = s.handle().session.project().unwrap();
    s.run(
        &Caller::human(),
        "proposal.reject",
        json!({ "id": id, "reason": "not now" }),
    )
    .unwrap();
    assert!(Arc::ptr_eq(
        &project,
        &s.handle().session.project().unwrap()
    ));
    let st = call(
        &a,
        "proposal_status",
        json!({ "id": id, "wait_seconds": 2 }),
    );
    assert_eq!(st["proposal"]["status"]["state"], "rejected");
    assert_eq!(st["proposal"]["status"]["reason"], "not now");
    // it cannot be accepted afterwards
    assert!(
        s.run(&Caller::human(), "proposal.accept", json!({ "id": id }))
            .is_err()
    );
    assert_eq!(note_total(&s), 0);
}

#[test]
fn a_stale_proposal_is_reported_not_force_applied() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    let h = Caller::human();
    let (pat, ch) = pattern_and_channel(&s);
    s.run(
        &h,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": [{ "key": 40, "start": 0, "length": 480 }] }),
    )
    .unwrap();
    // the agent proposes to change that note; the person changes it first
    let prop = call(
        &a,
        "propose",
        json!({ "commands": [{ "id": "note.set_velocity", "params": { "pattern": pat, "channel": ch, "ids": [0], "velocity": 0.1 } }] }),
    );
    let id = prop["results"][0]["result"]["proposal"].as_u64().unwrap();
    s.run(
        &h,
        "note.set_velocity",
        json!({ "pattern": pat, "channel": ch, "ids": [0], "velocity": 0.95 }),
    )
    .unwrap();
    let view = call(&a, "get_state", json!({ "view": "proposals" }));
    assert!(
        view["proposals"][0]["stale"]
            .as_str()
            .unwrap()
            .contains("note/"),
        "{view}"
    );
    let e = s
        .run(&h, "proposal.accept", json!({ "id": id }))
        .unwrap_err()
        .to_string();
    assert!(e.contains("stale"), "{e}");
    let st = call(&a, "proposal_status", json!({ "id": id }));
    assert_eq!(st["proposal"]["status"]["state"], "stale");
    assert!(
        st["proposal"]["status"]["reason"]
            .as_str()
            .unwrap()
            .contains("changed by human")
    );
    let v = s
        .handle()
        .session
        .project()
        .unwrap()
        .pattern(ph_core::PatternId(0))
        .unwrap()
        .notes[&ph_core::ChannelId(0)][&ph_core::NoteId(0)]
        .velocity;
    assert_eq!(v, 0.95, "the person's edit stands");
}

#[test]
fn proposal_guards_for_agents() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = agent_for(&s);
    let (pat, ch) = pattern_and_channel(&s);
    // an agent cannot accept its own proposal through run
    let id = call(&a, "propose", bass_proposal(&pat, &ch, 1))["results"][0]["result"]["proposal"]
        .as_u64()
        .unwrap();
    let e = call_err(
        &a,
        "run",
        json!({ "id": "proposal.accept", "params": { "id": id } }),
    );
    assert!(e.contains("own proposal"), "{e}");
    // transport cannot be proposed
    let e = call_err(
        &a,
        "propose",
        json!({ "commands": [{ "id": "transport.play" }] }),
    );
    assert!(e.contains("cannot be previewed"), "{e}");
    // the agent can withdraw its own
    call(
        &a,
        "run",
        json!({ "id": "proposal.reject", "params": { "id": id } }),
    );
    let st = call(&a, "proposal_status", json!({}));
    assert_eq!(st["pending"], 0);
    assert_eq!(note_total(&s), 0);
}
