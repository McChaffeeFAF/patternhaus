//! Controller lanes through the agent core (M3/T10): the musical diff, the `pattern` view, the
//! piano-roll lane strip and the server instructions.

use super::*;
use crate::agent::Block;
use ph_commands::{Grant, Scope};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

fn studio(dir: &Path) -> Studio {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
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
        Project::empty("cc"),
    )
    .unwrap()
}

fn call(a: &Agent, tool: &str, args: Value) -> ToolOutput {
    let out = a.call("claude", tool, args.clone());
    assert!(!out.is_error, "{tool} {args}: {:?}", out.blocks);
    out
}

fn texts(o: &ToolOutput) -> String {
    o.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn image(o: &ToolOutput) -> Vec<u8> {
    o.blocks
        .iter()
        .find_map(|b| match b {
            Block::Image(p) => Some(p.clone()),
            _ => None,
        })
        .expect("an image")
}

/// Image size from a PNG header.
fn size(png: &[u8]) -> (u32, u32) {
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
    (w, h)
}

fn setup(a: &Agent) {
    call(
        a,
        "run",
        json!({ "id": "pattern.add", "params": { "name": "Verse" } }),
    );
    call(
        a,
        "run",
        json!({ "id": "channel.add", "params": { "name": "Strings", "instrument": { "kind": "synth" } } }),
    );
    call(
        a,
        "run",
        json!({ "id": "note.add", "params": { "pattern": 0, "channel": 0,
            "notes": [{ "key": 60, "start": 0, "length": 7680 }] } }),
    );
}

#[test]
fn the_diff_the_views_and_the_snapshot_show_controller_lanes() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = Agent::new(s.handle(), Grants::fixed(Grant(vec![Scope::Full])));
    setup(&a);
    // the musical diff names the controller, channel, bars and shape
    let out = call(
        &a,
        "run",
        json!({ "id": "cc.shape", "params": { "pattern": 0, "channel": 0, "controller": "mod wheel",
            "start": 3840, "end": 7679, "from": 0.3, "peak": 0.9, "to": 0.4 } }),
    )
    .value()
    .unwrap();
    assert_eq!(
        out["changes"],
        json!(["CC1 Mod Wheel on Strings in Verse, bar 2: swell 0.3→0.9→0.4"]),
        "{out}"
    );
    // a rewrite and a clear read as such
    let out = call(
        &a,
        "run",
        json!({ "id": "cc.set", "params": { "pattern": 0, "channel": 0, "controller": 11,
            "points": [{ "tick": 0, "value": 0.2 }, { "tick": 3840, "value": 0.8 }] } }),
    )
    .value()
    .unwrap();
    assert_eq!(
        out["changes"],
        json!(["CC11 Expression on Strings in Verse, bar 1: ramp 0.2→0.8"]),
        "{out}"
    );
    let out = call(
        &a,
        "run",
        json!({ "id": "cc.clear", "params": { "pattern": 0, "channel": 0, "controller": 11 } }),
    )
    .value()
    .unwrap();
    assert!(
        out["changes"][0].as_str().unwrap().contains("cleared"),
        "{out}"
    );

    // get_state: the pattern view and the overview mention the lanes compactly
    let v = call(
        &a,
        "get_state",
        json!({ "view": "pattern", "args": { "pattern": "Verse" } }),
    )
    .value()
    .unwrap();
    let lanes = &v["channels"][0]["lanes"];
    assert_eq!(
        lanes,
        &json!(["CC1 Mod Wheel: swell 0.3→0.9→0.4 (bar 2)"]),
        "{v}"
    );
    let o = call(&a, "get_state", json!({})).value().unwrap();
    assert!(
        o["patterns"][0]
            .as_str()
            .unwrap()
            .contains("3 controller points"),
        "{o}"
    );

    // the piano roll draws a strip under the notes, and says so
    let with = call(
        &a,
        "snapshot",
        json!({ "kind": "piano_roll", "pattern": "Verse" }),
    );
    let caption = texts(&with);
    assert!(
        caption.contains("controller lanes below: CC1 Mod Wheel on Strings (swell 0.3→0.9→0.4)"),
        "{caption}"
    );
    let without = call(
        &a,
        "snapshot",
        json!({ "kind": "piano_roll", "pattern": "Verse", "lanes": false }),
    );
    assert!(!texts(&without).contains("controller lanes"));
    let (w1, h1) = size(&image(&with));
    let (w2, h2) = size(&image(&without));
    assert_eq!(w1, w2);
    assert!(h1 > h2 + 40, "strip adds height: {h2} -> {h1}");
    // picking a controller that is not there draws nothing extra
    let none = call(
        &a,
        "snapshot",
        json!({ "kind": "piano_roll", "pattern": "Verse", "lanes": ["pitch bend"] }),
    );
    assert_eq!(size(&image(&none)).1, h2);

    // a person edits a lane: the agent hears about it in while_you_worked
    s.run(
        &Caller::human(),
        "cc.shape",
        json!({ "pattern": 0, "channel": 0, "controller": 7, "start": 0, "end": 1920, "from": 1.0, "to": 0.5 }),
    )
    .unwrap();
    let t = Instant::now();
    loop {
        let seen = texts(&a.call("claude", "get_state", json!({ "view": "transport" })));
        if seen.contains("CC7 Volume on Strings in Verse, bar 1: ramp 1→0.5") {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(20), "{seen}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_server_instructions_point_at_cc1_and_cc_shape() {
    let text = include_str!("mcp.rs");
    let at = text.find("const INSTRUCTIONS").unwrap();
    let block = &text[at..text[at..].find("pub struct PatternhausServer").unwrap() + at];
    assert!(
        block.contains("CC1 (mod wheel) or CC11 (expression)") && block.contains("cc.shape"),
        "the instructions name the controllers and the command"
    );
}
