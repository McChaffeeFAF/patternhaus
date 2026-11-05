//! Haus Sampler agent features (M3/T7) through the agent core: the completed `zones` view and
//! its playing guide, the `zone_map` snapshot, the `report` view behind
//! `patternhaus://instrument/<id>/report`, sampler lines in the musical diff and in
//! `while_you_worked`, and keyswitches and kits left out of the key and chord analysis.

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
        Project::empty("sampler"),
    )
    .unwrap()
}

fn wait_for(mut cond: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn tone(path: &Path, hz: f32, secs: f32) {
    let n = (48_000.0 * secs) as usize;
    let x: Vec<f32> = (0..n)
        .map(|i| (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.3)
        .collect();
    ph_formats::encode::write_wav(path, &[x], 48_000, ph_formats::encode::WavFormat::Float32)
        .unwrap();
}

/// A small strings patch: two keyswitched articulations, two velocity layers, a release
/// group, and a staccato forte zone that stops at B4.
fn write_strings(dir: &Path) -> std::path::PathBuf {
    let lib = dir.join("incoming/strings");
    std::fs::create_dir_all(&lib).unwrap();
    for (n, s) in [
        ("sus_p.wav", 1.0),
        ("sus_f.wav", 1.0),
        ("rel.wav", 0.2),
        ("stac_p.wav", 0.3),
        ("stac_f.wav", 0.3),
    ] {
        tone(&lib.join(n), 261.63, s);
    }
    let sfz = lib.join("strings.sfz");
    std::fs::write(
        &sfz,
        "<global> sw_lokey=24 sw_hikey=25 sw_default=24\n\
         <master> sw_last=24 sw_label=Sustain\n\
         <group> lokey=36 hikey=84 pitch_keycenter=60 hivel=80\n<region> sample=sus_p.wav\n\
         <group> lokey=36 hikey=84 pitch_keycenter=60 lovel=81\n<region> sample=sus_f.wav\n\
         <group> lokey=36 hikey=84 pitch_keycenter=60 trigger=release\n<region> sample=rel.wav\n\
         <master> sw_last=25 sw_label=Staccato\n\
         <group> pitch_keycenter=60\n\
         <region> sample=stac_p.wav lokey=36 hikey=84 hivel=80\n\
         <region> sample=stac_f.wav lokey=36 hikey=71 lovel=81\n",
    )
    .unwrap();
    sfz
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

#[test]
fn an_agent_reads_sees_and_edits_a_haus_instrument() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let a = Agent::new(s.handle(), Grants::fixed(Grant(vec![Scope::Full])));
    let sfz = write_strings(dir.path());
    let out = call(
        &a,
        "run",
        json!({ "id": "sampler.import", "params": { "path": sfz.to_string_lossy(), "collect": true } }),
    );
    let v = out.value().unwrap();
    assert_eq!(v["results"][0]["result"]["zones"], 5, "{v}");
    let changes = v["changes"].to_string();
    assert!(
        changes.contains("added instrument strings (id 0): 5 zones in 4 groups, C2-C6"),
        "{changes}"
    );

    // the zones view: a guide in the catalogue's shape, and the gap the import left
    let z = call(
        &a,
        "get_state",
        json!({ "view": "zones", "args": { "instrument": 0 } }),
    )
    .value()
    .unwrap();
    let inst = &z["instruments"][0];
    let guide = inst["guide_text"].as_str().unwrap();
    assert!(
        guide.contains("plays C2-C6 (36-84)")
            && guide.contains("keyswitches C1 (24) Sustain, C#1 (25) Staccato"),
        "{guide}"
    );
    assert!(guide.contains("no zone on C5-C6"), "{guide}");
    let stac = inst["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["articulation_names"] == json!(["Staccato"]))
        .expect("a staccato group")
        .clone();
    let forte = stac["zones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|z| z["text"].as_str().unwrap().ends_with("stac_f.wav"))
        .unwrap()
        .clone();
    assert_eq!(forte["keys"], json!([36, 71]));

    // the zone map, as an image
    let shot = call(
        &a,
        "snapshot",
        json!({ "kind": "zone_map", "instrument": 0 }),
    );
    let caption = texts(&shot);
    assert!(
        caption.contains("zone map of strings: 5 zones in 4 groups over C2-C6")
            && caption.contains("keyswitches C1 Sustain, C#1 Staccato"),
        "{caption}"
    );
    let png = shot
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Image(p) => Some(p.clone()),
            _ => None,
        })
        .expect("an image");
    assert_eq!(&png[1..4], b"PNG");
    assert!(
        a.call(
            "claude",
            "snapshot",
            json!({ "kind": "zone_map", "instrument": 9 })
        )
        .is_error
    );

    // the report view (the instrument report resource reads it)
    let r = call(
        &a,
        "get_state",
        json!({ "view": "report", "args": { "instrument": 0 } }),
    )
    .value()
    .unwrap();
    assert_eq!(r["source"]["format"], "sfz", "{r}");
    assert!(
        r["import_report"]
            .as_str()
            .is_some_and(|m| m.contains("strings.sfz")),
        "{r}"
    );
    assert!(r["guide_text"].as_str().unwrap().contains("Staccato"));
    assert!(
        a.call(
            "claude",
            "get_state",
            json!({ "view": "report", "args": {} })
        )
        .is_error
    );

    // the agent fixes the gap: the diff names the zone and its new place
    let moved = call(
        &a,
        "run",
        json!({ "id": "sampler.zone_move", "params": { "instrument": 0, "zones": [forte["id"]], "keys": [36, 84] } }),
    )
    .value()
    .unwrap();
    assert_eq!(
        moved["changes"],
        json!(["strings: moved zone stac_f.wav to C2-C6 vel 81-127"]),
        "{moved}"
    );

    // a person changes a velocity split: the agent hears about it in while_you_worked
    let sus_p = inst["groups"][0]["zones"][0]["id"].as_u64().unwrap();
    s.run(
        &Caller::human(),
        "sampler.zone_move",
        json!({ "instrument": 0, "zones": [sus_p], "velocity": [0, 64] }),
    )
    .unwrap();
    let mut seen = String::new();
    wait_for(
        || {
            let o = a.call("claude", "get_state", json!({ "view": "transport" }));
            seen = texts(&o);
            seen.contains("while_you_worked")
        },
        "the person's edit reaches the agent",
    );
    assert!(
        seen.contains("strings: moved zone sus_p.wav to C2-C6 vel 0-64"),
        "{seen}"
    );
}

#[test]
fn keyswitches_and_kits_stay_out_of_the_key_and_chords() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let who = Caller::agent("claude", Grant(vec![Scope::Full]));
    let run = |cmd: &str, p: Value| s.run(&who, cmd, p).unwrap().result;
    // a strings instrument with a keyswitch on C#1, and a kit on scattered keys
    run("sampler.new", json!({ "name": "Strings" }));
    run(
        "sampler.zone_add",
        json!({ "instrument": 0, "zones": [{ "group": 0, "sample": "s.wav", "root_key": 60, "keys": [36, 84] }] }),
    );
    run(
        "sampler.keyswitch_set",
        json!({ "instrument": 0, "name": "Staccato", "trigger": { "kind": "key", "key": 25 } }),
    );
    run("sampler.new", json!({ "name": "Kit" }));
    run(
        "sampler.zone_add",
        json!({ "instrument": 1, "zones": [
            { "group": 1, "sample": "kick.wav", "root_key": 36, "keys": [36, 36] },
            { "group": 1, "sample": "snare.wav", "root_key": 38, "keys": [38, 38] },
            { "group": 1, "sample": "hat.wav", "root_key": 42, "keys": [42, 42] }] }),
    );
    let pat = run("pattern.add", json!({ "name": "A" }))["pattern"].clone();
    // A minor triads, with a C#1 keyswitch on the strings and kit hits on C2, D2 and F#2
    let mut notes = vec![json!({ "key": 25, "start": 0, "length": 120 })];
    for k in [57, 60, 64] {
        notes.push(json!({ "key": k, "start": 0, "length": 3840 }));
    }
    run(
        "note.add",
        json!({ "pattern": pat, "channel": 0, "notes": notes }),
    );
    let hits: Vec<Value> = [36, 38, 42, 36, 42]
        .iter()
        .enumerate()
        .map(|(i, k)| json!({ "key": k, "start": i * 480, "length": 240 }))
        .collect();
    run(
        "note.add",
        json!({ "pattern": pat, "channel": 1, "notes": hits }),
    );
    let p = s.session.project().unwrap();
    let notes =
        ph_analysis::music::pattern_notes(&p, ph_core::PatternId(pat.as_u64().unwrap() as u32));
    let pitched = views::pitched_only(&p, notes);
    let keys: Vec<u8> = pitched.iter().map(|n| n.key).collect();
    assert_eq!(keys, vec![57, 60, 64]);
    let v = views::pattern_view(&p, &json!({})).unwrap().to_string();
    assert!(v.contains("Am"), "{v}");
}

#[test]
fn instrument_report_uris_name_an_instrument() {
    use crate::mcp::{INSTRUMENT_REPORT_TEMPLATE, instrument_report_id};
    assert_eq!(
        instrument_report_id("patternhaus://instrument/12/report"),
        Some(12)
    );
    assert_eq!(
        instrument_report_id("patternhaus://instrument/x/report"),
        None
    );
    assert_eq!(instrument_report_id("patternhaus://instrument/3"), None);
    assert_eq!(
        instrument_report_id(&INSTRUMENT_REPORT_TEMPLATE.replace("{id}", "7")),
        Some(7)
    );
}
