//! End-to-end: a real MCP client spawns `patternhaus-cli mcp --headless` over stdio and, using
//! only MCP tools, builds a four-channel beat, arranges 8 bars, mixes it, exports a WAV and
//! checks the result by analysis and snapshots. This is the agent half of M1 acceptance
//! criterion 1 (DESIGN §12).

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::PathBuf;

#[tokio::test(flavor = "multi_thread")]
async fn agent_builds_mixes_exports_and_analyses_a_beat_over_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let c = connect(&dir.path().join("song"), &[]).await;

    // -- discovery ---------------------------------------------------------------------
    let tools = c.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    for want in [
        "list_commands",
        "describe_command",
        "run",
        "run_batch",
        "get_state",
        "render",
        "analyze",
        "snapshot",
        "node_params",
        "undo_mine",
        "redo_mine",
        "history",
        "add_notes",
        "add_channel",
        "add_clip",
        "set_tempo",
        "set_mixer_volume",
        "add_effect",
        "set_effect_param",
        "play",
        "stop",
        "export_audio",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "missing tool {want}: {names:?}"
        );
    }
    assert!(tools.iter().all(|t| t.input_schema.contains_key("type")));
    let info = c.peer_info().expect("server info");
    assert_eq!(info.server_info.as_ref().unwrap().name, "patternhaus");

    let cmds = ok(&c, "list_commands", json!({ "filter": "mixer" })).await;
    assert!(cmds["count"].as_u64().unwrap() >= 10);
    let d = ok(&c, "describe_command", json!({ "id": "note.add" })).await;
    assert_eq!(d["scope"], "compose");
    assert!(d["params"]["properties"]["notes"].is_object());

    let who = ok(&c, "get_state", json!({ "view": "session" })).await;
    assert_eq!(who["you"], "agent:e2e-agent");
    assert_eq!(who["grant"], "full");

    let params = ok(&c, "node_params", json!({})).await;
    let preset = params["results"][0]["result"]["synth_presets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .find(|p| p.starts_with("bass/"))
        .expect("a bass preset")
        .to_string();
    let reverb = ok(&c, "node_params", json!({ "effect": "reverb" })).await;
    assert!(
        reverb["results"][0]["result"]["params"]
            .as_array()
            .unwrap()
            .len()
            >= 4
    );

    // -- build -------------------------------------------------------------------------
    ok(&c, "set_tempo", json!({ "bpm": 120.0 })).await;
    let beat = id(
        &ok(&c, "add_pattern", json!({ "name": "Beat" })).await,
        "pattern",
    );
    let bass_pat = id(
        &ok(&c, "add_pattern", json!({ "name": "Bass" })).await,
        "pattern",
    );

    let drum = |sample: &str| json!({ "kind": "sampler", "sample": format!("factory:drums/haus808/{sample}"), "root_key": 60 });
    let kick = id(
        &ok(
            &c,
            "add_channel",
            json!({ "name": "Kick", "instrument": drum("kick"), "insert": 1 }),
        )
        .await,
        "channel",
    );
    let snare = id(
        &ok(
            &c,
            "add_channel",
            json!({ "name": "Snare", "instrument": drum("snare"), "insert": 2 }),
        )
        .await,
        "channel",
    );
    let hat = id(
        &ok(
            &c,
            "add_channel",
            json!({ "name": "Hat", "instrument": drum("hat_closed"), "insert": 3 }),
        )
        .await,
        "channel",
    );
    let bass = id(
        &ok(
            &c,
            "add_channel",
            json!({
                "name": "Bass",
                "instrument": { "kind": "synth", "preset": preset },
                "insert": 4
            }),
        )
        .await,
        "channel",
    );

    // one bar of drums; step = 240 ticks
    let hits = |ticks: &[u64], len: u64, vel: f64| -> Vec<Value> {
        ticks
            .iter()
            .map(|t| json!({ "key": 60, "start": t, "length": len, "velocity": vel }))
            .collect()
    };
    ok(
        &c,
        "add_notes",
        json!({ "pattern": beat, "channel": kick, "notes": hits(&[0, 960, 1920, 2880], 240, 1.0) }),
    )
    .await;
    ok(
        &c,
        "add_notes",
        json!({ "pattern": beat, "channel": snare, "notes": hits(&[960, 2880], 240, 0.9) }),
    )
    .await;
    let eighths: Vec<u64> = (0..8).map(|i| i * 480).collect();
    ok(
        &c,
        "add_notes",
        json!({ "pattern": beat, "channel": hat, "notes": hits(&eighths, 120, 0.6) }),
    )
    .await;

    // Bass in A minor, Am - F - C - G, a root on beat 1 and 3 and the fifth on the and of 2
    let roots = [33u64, 29, 36, 31]; // A1 F1 C2 G1
    let fifths = [40u64, 36, 43, 38]; // E2 C2 G2 D2
    let mut bass_notes = Vec::new();
    for (bar, (r, f)) in roots.iter().zip(fifths).enumerate() {
        let b = bar as u64 * 3840;
        bass_notes.push(json!({ "key": r, "start": b, "length": 1700, "velocity": 0.9 }));
        bass_notes.push(json!({ "key": f, "start": b + 1440, "length": 400, "velocity": 0.7 }));
        bass_notes.push(json!({ "key": r, "start": b + 1920, "length": 1700, "velocity": 0.9 }));
    }
    let added = ok(
        &c,
        "add_notes",
        json!({ "pattern": bass_pat, "channel": bass, "notes": bass_notes }),
    )
    .await;
    assert!(
        added["changes"][0]
            .as_str()
            .unwrap()
            .contains("Bass in Bass: +12 notes"),
        "{added}"
    );
    assert!(
        added["changes"].as_array().unwrap().iter().any(|l| l
            .as_str()
            .unwrap()
            .contains("chords now: A5 - F5 - C5 - G5")),
        "{added}"
    );

    // -- arrange 8 bars, one undo step ---------------------------------------------------
    let mut clips: Vec<Value> = (0..8)
        .map(|b| json!({ "id": "playlist.add_clip", "params": { "track": 0, "pattern": beat, "start": b * 3840 } }))
        .collect();
    clips.push(json!({ "id": "playlist.add_clip", "params": { "track": 1, "pattern": bass_pat, "start": 0 } }));
    clips.push(json!({ "id": "playlist.add_clip", "params": { "track": 1, "pattern": bass_pat, "start": 4 * 3840 } }));
    let arranged = ok(
        &c,
        "run_batch",
        json!({ "commands": clips, "as_one_undo": true }),
    )
    .await;
    assert_eq!(arranged["results"].as_array().unwrap().len(), 10);
    let line = arranged["changes"][0].as_str().unwrap();
    assert!(line.starts_with("playlist: added"), "{arranged}");

    // -- mix: EQ and compressor on the bass insert, sidechained to the kick; reverb on a send
    ok(
        &c,
        "set_mixer_volume",
        json!({ "insert": 1, "volume": 0.9 }),
    )
    .await;
    ok(
        &c,
        "set_mixer_volume",
        json!({ "insert": 3, "volume": 0.5 }),
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
        json!({ "insert": 4, "effect": { "kind": "builtin", "name": "compressor" } }),
    )
    .await;
    ok(
        &c,
        "run",
        json!({ "id": "mixer.set_sidechain", "params": { "insert": 4, "source": 1 } }),
    )
    .await;
    ok(
        &c,
        "run",
        json!({ "id": "mixer.rename", "params": { "insert": 5, "name": "Reverb" } }),
    )
    .await;
    ok(
        &c,
        "add_effect",
        json!({ "insert": 5, "effect": { "kind": "builtin", "name": "reverb" }, "mix": 1.0 }),
    )
    .await;
    ok(&c, "add_send", json!({ "from": 2, "to": 5, "level": 0.5 })).await;
    ok(
        &c,
        "set_effect_param",
        json!({ "insert": 4, "index": 1, "param": "threshold", "value": -18.0 }),
    )
    .await;

    let mixer = ok(&c, "get_state", json!({ "view": "mixer" })).await;
    let ins = mixer["inserts"].as_array().unwrap();
    let bus = ins.iter().find(|i| i["insert"] == 4).unwrap();
    assert_eq!(bus["effects"].as_array().unwrap().len(), 2);
    assert_eq!(bus["sidechain_from"], 1);
    let rev = ins.iter().find(|i| i["insert"] == 5).unwrap();
    assert!(rev["effects"][0].as_str().unwrap().contains("reverb"));
    let snare_ins = ins.iter().find(|i| i["insert"] == 2).unwrap();
    assert!(
        snare_ins["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o.as_str().unwrap().contains("send to 5"))
    );

    // The sidechain command also flipped the compressor to its external input.
    let raw = ok(
        &c,
        "get_state",
        json!({ "view": "raw", "args": { "query": "query.mixer" } }),
    )
    .await;
    assert!(raw.to_string().contains("\"sidechain\":1.0"), "{raw}");

    // -- readable state ---------------------------------------------------------------
    let ov = ok(&c, "get_state", json!({})).await;
    assert_eq!(ov["song_bars"], 8);
    assert!(ov["key"].as_str().unwrap().starts_with("A minor"), "{ov}");
    assert!(
        ov["chords"].as_str().unwrap().contains("A5 - F5 - C5 - G5"),
        "{ov}"
    );
    assert!(ov["warnings"].as_array().unwrap().is_empty(), "{ov}");
    let notes = ok(
        &c,
        "get_state",
        json!({ "view": "notes", "args": { "pattern": "Bass", "channel": "Bass", "to_bar": 1 } }),
    )
    .await;
    assert!(
        notes["channels"]["Bass"][0]
            .as_str()
            .unwrap()
            .starts_with("A1 @1.1.0"),
        "{notes}"
    );
    let pv = ok(
        &c,
        "get_state",
        json!({ "view": "pattern", "args": { "pattern": "Bass" } }),
    )
    .await;
    assert_eq!(pv["bars"], 4);
    let secs = ok(&c, "get_state", json!({ "view": "sections" })).await;
    assert_eq!(secs["song_bars"], 8);
    assert!(secs["text"].as_str().unwrap().contains("Beat"), "{secs}");

    // -- listen, then fix what the numbers say ------------------------------------------
    // An agent reads the numbers: a peak over full scale shows up as a clipping warning, and
    // the loudness says how far the mix is from a normal level.
    let first = ok(&c, "analyze", json!({ "what": ["loudness", "health"] })).await;
    let peak = first["loudness"]["true_peak_dbtp"].as_f64().unwrap();
    if peak > -1.0 {
        assert!(
            first["health"]["warnings"].to_string().contains("clipping"),
            "{first}"
        );
    }
    // FL-style channel levels (78% = -5.2 dB, -3 dB pan law at centre) leave headroom, so a
    // quiet first mix gets the master limiter's input gain raised toward about -11 LUFS.
    let lufs0 = first["loudness"]["integrated_lufs"].as_f64().unwrap();
    let boost_db = (-11.0 - lufs0).clamp(0.0, 12.0);
    if peak > -1.0 || boost_db > 0.0 {
        ok(
            &c,
            "add_effect",
            json!({ "insert": 0, "effect": { "kind": "builtin", "name": "limiter" } }),
        )
        .await;
        ok(
            &c,
            "set_effect_param",
            json!({ "insert": 0, "index": 0, "param": "input_gain", "value": boost_db }),
        )
        .await;
        let again = ok(&c, "analyze", json!({ "what": ["loudness"] })).await;
        assert!(
            again["loudness"]["true_peak_dbtp"].as_f64().unwrap() <= -0.5,
            "{again}"
        );
    }

    // -- export + analysis -------------------------------------------------------------
    let rendered = ok(
        &c,
        "render",
        json!({ "name": "e2e-beat", "format": "wav", "bit_depth": 24 }),
    )
    .await;
    let path = PathBuf::from(rendered["path"].as_str().unwrap());
    assert!(path.exists(), "{path:?}");
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..4], b"RIFF");
    assert_eq!(&head[8..12], b"WAVE");
    assert!(rendered["seconds"].as_f64().unwrap() >= 15.9, "{rendered}");
    let lufs = rendered["analysis"]["integrated_lufs"].as_f64().unwrap();
    assert!((-16.0..=-8.0).contains(&lufs), "integrated {lufs} LUFS");

    let a = ok(&c, "analyze", json!({ "what": ["all"] })).await;
    let lufs = a["loudness"]["integrated_lufs"].as_f64().unwrap();
    assert!(
        (-16.0..=-8.0).contains(&lufs),
        "analyze: integrated {lufs} LUFS"
    );
    assert!(
        a["loudness"]["true_peak_dbtp"].as_f64().unwrap() < 0.0,
        "{}",
        a["loudness"]
    );
    assert_eq!(a["health"]["clipped_samples"], 0, "{}", a["health"]);
    assert_eq!(a["health"]["energy_per_bar"].as_array().unwrap().len(), 8);
    assert!(a["spectrum"]["bands"].as_array().unwrap().len() == 7);
    assert!(
        a["onsets"]["count"].as_u64().unwrap() > 20,
        "{}",
        a["onsets"]
    );
    // The key estimate matches the written bassline, from the notes and from the audio.
    assert_eq!(a["key"]["from_notes"]["key"], "A minor", "{}", a["key"]);
    assert_eq!(
        a["chords"]["from_notes"]["progression"], "A5 - F5 - C5 - G5 - A5 - F5 - C5 - G5",
        "{}",
        a["chords"]
    );
    // The audio estimate is reported alongside. With harmonic/percussive separation the tuned
    // kicks no longer drag it off the key signature (before T7 it read F# minor), but on this
    // short mix it can still pick the relative major; the note-based estimate is the exact one.
    let audio_key = a["key"]["from_audio"]["key"].as_str().unwrap_or("");
    assert!(
        ["A minor", "C major"].contains(&audio_key),
        "audio key {audio_key}: {}",
        a["key"]
    );
    assert!(a["masking"]["stems"].as_u64().unwrap() >= 4);
    assert_eq!(a["clashes"]["count"], 0);

    // -- snapshots ---------------------------------------------------------------------
    let shots = screenshot_dir();
    for (kind, args) in [
        ("piano_roll", json!({ "pattern": "Bass" })),
        ("playlist", json!({})),
        ("waveform", json!({})),
        ("spectrogram", json!({})),
    ] {
        let mut args = args;
        args["kind"] = json!(kind);
        let r = call(&c, "snapshot", args).await;
        assert!(!r.is_error, "{kind}: {:?}", r.texts);
        assert_eq!(r.images.len(), 1, "{kind}");
        assert_png(&r.images[0], 900);
        if let Some(d) = &shots {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(format!("agent-{kind}.png")), &r.images[0]).unwrap();
        }
    }
    // Every channel coloured in one piano roll of the drum pattern.
    let all = call(
        &c,
        "snapshot",
        json!({ "kind": "piano_roll", "pattern": "Beat" }),
    )
    .await;
    assert!(
        all.texts[0].contains("colours per channel: Kick, Snare, Hat"),
        "{:?}",
        all.texts
    );
    if let Some(d) = &shots {
        std::fs::write(d.join("agent-piano_roll-drums.png"), &all.images[0]).unwrap();
    }

    // -- per-author undo, history, transport ---------------------------------------------
    let hist = ok(&c, "history", json!({ "limit": 3 })).await;
    assert!(
        hist["recent"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["author"] == "agent:e2e-agent")
    );
    let before = ok(&c, "get_state", json!({})).await["clips"].clone();
    ok(
        &c,
        "run",
        json!({ "id": "mixer.rename", "params": { "insert": 6, "name": "Scratch" } }),
    )
    .await;
    let undone = ok(&c, "undo_mine", json!({})).await;
    assert!(
        undone["changes"][0].as_str().unwrap().contains("renamed"),
        "{undone}"
    );
    let redone = ok(&c, "redo_mine", json!({})).await;
    assert!(
        redone["changes"][0].as_str().unwrap().contains("Scratch"),
        "{redone}"
    );
    assert_eq!(ok(&c, "get_state", json!({})).await["clips"], before);

    ok(&c, "play", json!({})).await;
    // the audio thread picks the command up within a buffer or two
    let mut playing = false;
    for _ in 0..40 {
        let t = ok(&c, "get_state", json!({ "view": "transport" })).await;
        playing = t["playing"] == true;
        if playing {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(playing, "transport never started");
    ok(&c, "stop", json!({})).await;

    // errors are readable, not protocol failures
    let bad = call(
        &c,
        "run",
        json!({ "id": "note.add", "params": { "oops": 1 } }),
    )
    .await;
    assert!(bad.is_error);
    assert!(bad.texts[0].contains("describe_command"), "{:?}", bad.texts);
    let unknown = call(&c, "run", json!({ "id": "no.such" })).await;
    assert!(unknown.is_error);

    // -- save: project.json plus the per-author history log ---------------------------------
    ok(&c, "run", json!({ "id": "file.save" })).await;
    let song = dir.path().join("song");
    assert!(song.join("project.json").exists());
    let log = std::fs::read_to_string(song.join("history/log.jsonl")).unwrap();
    assert!(log.lines().count() >= 10, "{log}");
    assert!(log.contains("agent:e2e-agent"));

    c.cancel().await.ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_limit_what_a_client_may_do() {
    let dir = tempfile::tempdir().unwrap();
    let c = connect(&dir.path().join("p"), &["--grant", "compose"]).await;
    let who = ok(&c, "get_state", json!({ "view": "session" })).await;
    assert_eq!(who["grant"], "compose");
    // compose may write patterns...
    ok(&c, "add_pattern", json!({ "name": "A" })).await;
    // ...but not touch the mixer or the filesystem
    let mix = call(
        &c,
        "set_mixer_volume",
        json!({ "insert": 1, "volume": 0.5 }),
    )
    .await;
    assert!(mix.is_error);
    assert!(mix.texts[0].contains("Mix scope"), "{:?}", mix.texts);
    let save = call(&c, "run", json!({ "id": "file.save" })).await;
    assert!(
        save.is_error && save.texts[0].contains("Full scope"),
        "{:?}",
        save.texts
    );
    // commands list marks what is not granted
    let l = ok(&c, "list_commands", json!({ "filter": "mixer.set_volume" })).await;
    assert!(l["commands"][0].as_str().unwrap().contains("NOT GRANTED"));
    c.cancel().await.ok();

    let r = connect(&dir.path().join("p2"), &["--grant", "read"]).await;
    let denied = call(&r, "add_pattern", json!({})).await;
    assert!(denied.is_error);
    ok(&r, "get_state", json!({})).await;
    r.cancel().await.ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn sample_that_cannot_load_is_reported_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let c = connect(&dir.path().join("p"), &[]).await;
    let ch = ok(&c, "add_channel", json!({
        "name": "Ghost",
        "instrument": { "kind": "sampler", "sample": "factory:drums/nope/missing", "root_key": 60 }
    }))
    .await;
    assert!(
        ch["engine_warnings"][0].as_str().unwrap().contains("Ghost"),
        "{ch}"
    );
    let r = ok(&c, "render", json!({ "name": "w" })).await;
    assert!(
        r["engine_warnings"][0]
            .as_str()
            .unwrap()
            .contains("silence"),
        "{r}"
    );
    let s = ok(&c, "get_state", json!({ "view": "warnings" })).await;
    assert_eq!(s["warnings"].as_array().unwrap().len(), 1);
    c.cancel().await.ok();
}
