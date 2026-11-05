//! M3 acceptance criterion 1 (`docs/milestones/M3.md`), step by step.
//!
//! A scripted agent drives `patternhaus-cli mcp --headless` over stdio, **through MCP only**
//! (tools and resources), and:
//!
//! 1. imports a generated multi-sample SFZ string patch (two velocity layers with two round
//!    robins each, a release-trigger group, and two keyswitched articulations, Sustain on C1
//!    and Staccato on C#1), and a generated SF2 drum kit (bank 128, `scaleTuning` 0);
//! 2. reads both through `get_state { view: zones }` (the playing guide: range, keyswitches,
//!    layers, the gap the patch ships with), sees the map with `snapshot { kind: zone_map }`
//!    and reads the import report resource;
//! 3. writes a 16-bar part in A minor (Am - F - C - G, four times) that uses both
//!    articulations: sustained chords in bars 1-8, staccato stabs with a forte top line in
//!    bars 9-16, over a drum part;
//! 4. edits the zone map: moves the staccato forte zone over the gap (its top line was
//!    silent above B4) and changes the sustain velocity split from 80/81 to 64/65 (the
//!    sustained chords now play the forte layer);
//! 5. saves the instrument as native, loads it back onto its channel, prunes the imported
//!    one, saves the project, reopens it (a new server on the same folder) and renders it.
//!
//! Checks: the export matches the offline oracle (bit for bit, and the real-time engine on a
//! simulated device matches the offline render); `analyze` finds A minor and the chords per
//! bar; both edits are audible (each section gets louder) and visible in the zones view; the
//! native instrument renders the same as the imported one; the renders before and after
//! reopening are identical.

mod common;

use common::*;
use ph_engine::assets::FileResolver;
use ph_engine::{AudioBuffer, Engine, EngineConfig, PlayMode, RenderSettings, TransportCommand};
use ph_sampler_formats::build::{Sf2Builder, Sf2Sample, Sf2Zone};
use rmcp::model::{ReadResourceRequestParams, ResourceContents};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BAR: u64 = 3840; // 4/4, 960 ticks per quarter
const BPM: f64 = 100.0;
const SR: u32 = 48_000;
const C4_HZ: f64 = 261.625_565_300_598_6;
/// SF2 generator ids (SoundFont 2.04 §8.1.2): scaleTuning and exclusiveClass.
const SCALE: u16 = 56;
const EXCLUSIVE: u16 = 57;

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

fn res(v: &Value) -> &Value {
    &v["results"][0]["result"]
}

/// A bowed-string-like tone at C4: `harmonics` partials falling as 1/n, `detune` cents off,
/// a short attack and an exponential decay (`tau` seconds; `None` holds, then fades over the
/// last 0.3 s).
fn string_tone(secs: f64, amp: f64, harmonics: usize, detune: f64, tau: Option<f64>) -> Vec<f32> {
    let n = (secs * SR as f64) as usize;
    let f = C4_HZ * 2f64.powf(detune / 1200.0);
    let norm: f64 = (1..=harmonics).map(|h| 1.0 / h as f64).sum();
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let mut v = 0.0;
            for h in 1..=harmonics {
                v += (std::f64::consts::TAU * f * h as f64 * t).sin() / h as f64;
            }
            let attack = (t / 0.02).min(1.0);
            let env = match tau {
                Some(tau) => (-t / tau).exp(),
                None => ((secs - t) / 0.3).min(1.0),
            };
            (v / norm * amp * attack * env) as f32
        })
        .collect()
}

fn write_mono(path: &Path, x: Vec<f32>) {
    ph_formats::encode::write_wav(path, &[x], SR, ph_formats::encode::WavFormat::Float32).unwrap();
}

/// The SFZ string patch, written by this test (never a recording). The staccato forte zone
/// stops at B4 (`hikey=71`): the gap the agent finds and fixes.
fn write_strings(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    for (name, amp, harm, detune) in [
        ("sus_p_rr1.wav", 0.25, 4, 0.0),
        ("sus_p_rr2.wav", 0.25, 4, 4.0),
        ("sus_f_rr1.wav", 0.5, 9, 0.0),
        ("sus_f_rr2.wav", 0.5, 9, -4.0),
    ] {
        write_mono(&dir.join(name), string_tone(3.0, amp, harm, detune, None));
    }
    write_mono(
        &dir.join("rel.wav"),
        string_tone(0.6, 0.08, 3, 0.0, Some(0.15)),
    );
    write_mono(
        &dir.join("stac_p.wav"),
        string_tone(0.4, 0.3, 5, 0.0, Some(0.09)),
    );
    write_mono(
        &dir.join("stac_f.wav"),
        string_tone(0.4, 0.55, 10, 0.0, Some(0.09)),
    );
    let sfz = dir.join("strings.sfz");
    std::fs::write(
        &sfz,
        "// generated by the M3 acceptance test\n\
         <global> sw_lokey=24 sw_hikey=25 sw_default=24 pitch_keycenter=60\n\
         <master> sw_last=24 sw_label=Sustain\n\
         <group> lokey=36 hikey=84 seq_length=2 hivel=80 ampeg_release=0.25\n\
         <region> sample=sus_p_rr1.wav seq_position=1\n\
         <region> sample=sus_p_rr2.wav seq_position=2\n\
         <group> lokey=36 hikey=84 seq_length=2 lovel=81 ampeg_release=0.25\n\
         <region> sample=sus_f_rr1.wav seq_position=1\n\
         <region> sample=sus_f_rr2.wav seq_position=2\n\
         <group> lokey=36 hikey=84 trigger=release rt_decay=3\n\
         <region> sample=rel.wav\n\
         <master> sw_last=25 sw_label=Staccato\n\
         <group> ampeg_release=0.05\n\
         <region> sample=stac_p.wav lokey=36 hikey=84 hivel=80\n\
         <region> sample=stac_f.wav lokey=36 hikey=71 lovel=81\n",
    )
    .unwrap();
    sfz
}

/// A generated drum kit: kick, snare and closed hat on 36, 38 and 42, bank 128, scaleTuning
/// 0 (the key does not change the pitch), the hat in a choke group.
fn write_kit(path: &Path) {
    let sr = SR as f64;
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut noise = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0
    };
    let kick: Vec<f32> = {
        let mut ph = 0.0f64;
        (0..(0.35 * sr) as usize)
            .map(|i| {
                let t = i as f64 / sr;
                ph += std::f64::consts::TAU * (50.0 + 90.0 * (-t * 25.0).exp()) / sr;
                (ph.sin() * (-t / 0.16).exp() * 0.55) as f32
            })
            .collect()
    };
    let snare: Vec<f32> = (0..(0.22 * sr) as usize)
        .map(|i| {
            let t = i as f64 / sr;
            let tone = (std::f64::consts::TAU * 190.0 * t).sin() * 0.2;
            ((noise() * 0.3 + tone) * (-t / 0.06).exp()) as f32
        })
        .collect();
    let mut prev = 0.0;
    let hat: Vec<f32> = (0..(0.08 * sr) as usize)
        .map(|i| {
            let t = i as f64 / sr;
            let n = noise();
            let v = (n - prev) * 0.12 * (-t / 0.025).exp();
            prev = n;
            v as f32
        })
        .collect();
    let mut b = Sf2Builder::new();
    let mut zones = vec![Sf2Zone::new().set(SCALE, 0)]; // global zone
    for (key, name, data) in [(36u8, "kick", kick), (38, "snare", snare), (42, "hat", hat)] {
        let s = b.add_sample(Sf2Sample::from_f32(name, &data, SR, 60));
        let mut z = Sf2Zone::new().keys(key, key);
        if key == 42 {
            z = z.set(EXCLUSIVE, 1);
        }
        zones.push(z.sample(s));
    }
    let i = b.add_instrument("Haus Kit", zones);
    b.add_preset("Haus Kit", 128, 0, vec![Sf2Zone::new().instrument(i)]);
    std::fs::write(path, b.build()).unwrap();
}

/// The chords, one per bar: bass note and triad (bars 1-8), plus the staccato top line above
/// B4 (bars 9-16).
const CHORDS: [(&str, u8, [u8; 3], u8); 4] = [
    ("Am", 45, [57, 60, 64], 76),
    ("F", 41, [53, 57, 60], 72),
    ("C", 48, [55, 60, 64], 79),
    ("G", 43, [55, 59, 62], 74),
];

/// A chord symbol's root and triad quality: `Am7` -> `Am`, `Fmaj7` -> `F`, `C7` -> `C`.
fn triad(s: &str) -> String {
    let n = if s[1..].starts_with(['#', 'b']) { 2 } else { 1 };
    let (root, rest) = s.split_at(n.min(s.len()));
    let minor = rest.starts_with('m') && !rest.starts_with("maj");
    format!("{root}{}", if minor { "m" } else { "" })
}

fn note(key: u8, start: u64, length: u64, velocity: f64) -> Value {
    json!({ "key": key, "start": start, "length": length, "velocity": velocity })
}

/// The strings part: the Sustain keyswitch, eight bars of held chords at velocity 0.6
/// (MIDI 76, the soft layer until the split moves), the Staccato keyswitch, eight bars of
/// eighth-note stabs with a forte top line on beats 1 and 3.
fn strings_part() -> Vec<Value> {
    let mut v = vec![note(24, 0, 120, 0.8)];
    for bar in 0..8u64 {
        let (_, bass, triad, _) = CHORDS[bar as usize % 4];
        let at = bar * BAR;
        v.push(note(bass, at, BAR - 240, 0.6));
        for k in triad {
            v.push(note(k, at, BAR - 240, 0.6));
        }
    }
    v.push(note(25, 8 * BAR - 120, 60, 0.8));
    for bar in 8..16u64 {
        let (_, bass, triad, top) = CHORDS[bar as usize % 4];
        for e in 0..8u64 {
            let at = bar * BAR + e * 480;
            if e % 4 == 0 {
                v.push(note(bass, at, 240, 0.6));
                v.push(note(top, at, 240, 0.9));
            }
            for k in triad {
                v.push(note(k, at, 240, 0.6));
            }
        }
    }
    v
}

fn drum_bar() -> Vec<Value> {
    let mut v = Vec::new();
    for beat in 0..4u64 {
        let at = beat * 960;
        v.push(note(if beat % 2 == 0 { 36 } else { 38 }, at, 240, 0.85));
        v.push(note(42, at, 120, 0.6));
        v.push(note(42, at + 480, 120, 0.45));
    }
    v
}

async fn lufs(c: &Client, from: u32, to: u32) -> f64 {
    let a = ok(
        c,
        "analyze",
        json!({ "what": ["loudness"], "range": { "from_bar": from, "to_bar": to } }),
    )
    .await;
    a["loudness"]["integrated_lufs"].as_f64().unwrap_or(-120.0)
}

async fn render(c: &Client, name: &str) -> (PathBuf, Vec<Vec<f32>>) {
    let r = ok(
        c,
        "render",
        json!({ "name": name, "format": "wav", "bit_depth": 32 }),
    )
    .await;
    let path = PathBuf::from(r["path"].as_str().unwrap());
    let audio = ph_formats::decode::decode_file(&path).unwrap();
    assert_eq!(audio.sample_rate as u32, SR);
    (path, audio.channels)
}

/// The zones of `inst` in the zones view whose text ends with `sample`.
fn zone_with<'a>(inst: &'a Value, sample: &str) -> &'a Value {
    inst["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["zones"].as_array().unwrap().iter())
        .find(|z| z["text"].as_str().unwrap().ends_with(sample))
        .unwrap_or_else(|| panic!("no zone plays {sample}: {inst}"))
}

async fn zones(c: &Client, instrument: u64) -> Value {
    let v = ok(
        c,
        "get_state",
        json!({ "view": "zones", "args": { "instrument": instrument } }),
    )
    .await;
    v["instruments"][0].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn m3_acceptance_agent_plays_a_haus_instrument_through_mcp_only() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song");
    // the generated instruments live inside the project folder, where an agent may read them
    let sfz = write_strings(&song.join("incoming/strings"));
    let kit_file = song.join("incoming/haus-kit.sf2");
    write_kit(&kit_file);
    let c = connect(&song, &[]).await;

    // ---- 1. import both ------------------------------------------------------------------
    ok(&c, "set_tempo", json!({ "bpm": BPM })).await;
    let strings = ok(
        &c,
        "run",
        json!({ "id": "sampler.import", "params": { "path": sfz.to_string_lossy(), "collect": true } }),
    )
    .await;
    let (str_inst, str_ch) = (id(&strings, "instrument"), id(&strings, "channel"));
    assert_eq!(res(&strings)["zones"], 7, "{strings}");
    assert_eq!(res(&strings)["report"]["format"], "sfz", "{strings}");
    assert!(
        strings["changes"]
            .to_string()
            .contains("added instrument strings"),
        "{strings}"
    );
    let kit = ok(
        &c,
        "run",
        json!({ "id": "sampler.import", "params": { "path": kit_file.to_string_lossy(), "preset": "128:0" } }),
    )
    .await;
    let (kit_inst, kit_ch) = (id(&kit, "instrument"), id(&kit, "channel"));
    assert_eq!(res(&kit)["zones"], 3, "{kit}");

    // ---- 2. read, see and look up the reports --------------------------------------------
    let s = zones(&c, str_inst).await;
    let guide = s["guide_text"].as_str().unwrap().to_string();
    println!("strings guide: {guide}");
    assert!(guide.contains("plays C2-C6 (36-84)"), "{guide}");
    assert!(
        guide.contains("keyswitches C1 (24) Sustain, C#1 (25) Staccato"),
        "{guide}"
    );
    assert!(guide.contains("starts as Sustain"), "{guide}");
    assert!(guide.contains("round robins: up to 2 takes"), "{guide}");
    assert!(
        guide.contains("release samples sound on note-off"),
        "{guide}"
    );
    assert!(
        guide.contains("vel 81-127: no zone on C5-C6"),
        "the gap is reported: {guide}"
    );
    assert_eq!(s["kit"], false);
    let k = zones(&c, kit_inst).await;
    assert_eq!(k["kit"], true, "{k}");
    let kit_guide = k["guide_text"].as_str().unwrap();
    println!("kit guide: {kit_guide}");
    assert!(
        kit_guide.contains("a kit: each key plays its own sound"),
        "{kit_guide}"
    );

    let shot = call(
        &c,
        "snapshot",
        json!({ "kind": "zone_map", "instrument": str_inst }),
    )
    .await;
    assert!(!shot.is_error && shot.images.len() == 1, "{:?}", shot.texts);
    assert_png(&shot.images[0], 1000);
    assert!(
        shot.texts[0].contains("keyswitches C1 Sustain, C#1 Staccato"),
        "{:?}",
        shot.texts
    );
    if std::env::var_os("PH_SAVE_SCREENSHOTS").is_some() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/M3");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("agent-zone-map.png"), &shot.images[0]).unwrap();
    }

    let listed = c.list_resources(None).await.unwrap();
    let uri = format!("patternhaus://instrument/{str_inst}/report");
    assert!(
        listed.resources.iter().any(|r| r.uri == uri),
        "{:?}",
        listed.resources.iter().map(|r| &r.uri).collect::<Vec<_>>()
    );
    let templates = c.list_resource_templates(None).await.unwrap();
    assert_eq!(
        templates.resource_templates[0].uri_template,
        "patternhaus://instrument/{id}/report"
    );
    let read = c
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    let ResourceContents::TextResourceContents { text, .. } = &read.contents[0] else {
        panic!("a text resource");
    };
    let report: Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["source"]["format"], "sfz", "{report}");
    assert!(
        report["import_report"]
            .as_str()
            .is_some_and(|m| m.contains("strings.sfz")),
        "{report}"
    );

    // ---- 3. write the part ---------------------------------------------------------------
    let pat = ok(&c, "add_pattern", json!({ "name": "Strings" })).await;
    let str_pat = id(&pat, "pattern");
    let pat = ok(&c, "add_pattern", json!({ "name": "Beat" })).await;
    let beat = id(&pat, "pattern");
    let mut batch = vec![
        json!({ "id": "note.add", "params": { "pattern": str_pat, "channel": str_ch, "notes": strings_part() } }),
        json!({ "id": "note.add", "params": { "pattern": beat, "channel": kit_ch, "notes": drum_bar() } }),
        json!({ "id": "playlist.add_clip", "params": { "track": 0, "pattern": str_pat, "start": 0 } }),
    ];
    for b in 0..16u64 {
        batch.push(json!({ "id": "playlist.add_clip", "params": { "track": 1, "pattern": beat, "start": b * BAR } }));
    }
    batch.push(
        json!({ "id": "channel.set_volume", "params": { "channel": kit_ch, "volume": 0.6 } }),
    );
    let wrote = ok(&c, "run_batch", json!({ "commands": batch })).await;
    assert!(
        wrote["changes"].to_string().contains("Strings chords now"),
        "{wrote}"
    );
    let warnings = ok(&c, "get_state", json!({ "view": "warnings" })).await;
    assert_eq!(warnings["warnings"], json!([]), "{warnings}");
    let sections = ok(&c, "get_state", json!({ "view": "sections" })).await;
    assert_eq!(sections["song_bars"], 16, "{sections}");

    // the key and the chords, from the notes (keyswitches and the kit left out) and the audio
    let a = ok(
        &c,
        "analyze",
        json!({ "what": ["key", "chords", "loudness"] }),
    )
    .await;
    let key_notes = a["key"]["from_notes"]["key"].as_str().unwrap().to_string();
    let key_audio = a["key"]["from_audio"]["key"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    let by_bar: Vec<String> = a["chords"]["from_notes"]["by_bar"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().split(' ').nth(1).unwrap().to_string())
        .collect();
    let audio_prog = a["chords"]["from_audio"]["progression"]
        .as_str()
        .unwrap_or("")
        .to_string();
    println!(
        "key from notes {key_notes}, from audio {key_audio}; chords by bar {by_bar:?}; from audio {audio_prog}"
    );
    assert_eq!(key_notes, "A minor", "{a}");
    assert!(
        key_audio == "A minor" || key_audio == "C major",
        "the audio reads A minor or its relative major: {a}"
    );
    let want: Vec<String> = (0..16).map(|b| CHORDS[b % 4].0.to_string()).collect();
    assert_eq!(by_bar, want, "{a}");
    let audio_bars: Vec<String> = a["chords"]["from_audio"]["by_bar"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().split(' ').nth(1).unwrap().to_string())
        .collect();
    // from the audio, the harmonics of the samples may add a seventh: compare root and
    // triad quality
    let audio_hits = audio_bars
        .iter()
        .zip(&want)
        .filter(|(a, b)| triad(a) == triad(b))
        .count();
    assert!(
        audio_hits >= 14,
        "the audio chords follow the part: {audio_bars:?}"
    );

    // ---- 4. the edits, audible --------------------------------------------------------------
    let (sus0, stac0) = (lufs(&c, 1, 8).await, lufs(&c, 9, 16).await);
    let forte = zone_with(&s, "stac_f.wav").clone();
    assert_eq!(forte["keys"], json!([36, 71]), "{forte}");
    let moved = ok(
        &c,
        "run",
        json!({ "id": "sampler.zone_move", "params": {
            "instrument": str_inst, "zones": [forte["id"]], "keys": [36, 84], "root_follows": false } }),
    )
    .await;
    assert_eq!(
        moved["changes"],
        json!(["strings: moved zone stac_f.wav to C2-C6 vel 81-127"]),
        "{moved}"
    );
    // the velocity split: soft layer 0-64, forte layer 65-127, one undo step
    let soft: Vec<Value> = ["sus_p_rr1.wav", "sus_p_rr2.wav"]
        .iter()
        .map(|n| zone_with(&s, n)["id"].clone())
        .collect();
    let loud: Vec<Value> = ["sus_f_rr1.wav", "sus_f_rr2.wav"]
        .iter()
        .map(|n| zone_with(&s, n)["id"].clone())
        .collect();
    let lo = zone_with(&s, "sus_p_rr1.wav")["velocity"][0]
        .as_u64()
        .unwrap();
    let split = ok(
        &c,
        "run_batch",
        json!({ "commands": [
            { "id": "sampler.zone_move", "params": { "instrument": str_inst, "zones": soft, "velocity": [lo, 64] } },
            { "id": "sampler.zone_move", "params": { "instrument": str_inst, "zones": loud, "velocity": [65, 127] } },
        ] }),
    )
    .await;
    let lines = split["changes"].to_string();
    assert!(
        lines.contains("moved zone sus_p_rr1.wav to C2-C6 vel")
            && lines.contains("-64")
            && lines.contains("moved zone sus_f_rr2.wav to C2-C6 vel 65-127"),
        "{split}"
    );
    // visible in the zones view: no gap left, the new layers
    let s2 = zones(&c, str_inst).await;
    let guide2 = s2["guide_text"].as_str().unwrap();
    assert!(!guide2.contains("no zone on"), "{guide2}");
    assert_eq!(zone_with(&s2, "stac_f.wav")["keys"], json!([36, 84]));
    assert_eq!(
        zone_with(&s2, "sus_f_rr1.wav")["velocity"],
        json!([65, 127])
    );
    // and audible: the held chords now play the forte layer, the top line sounds
    let (sus1, stac1) = (lufs(&c, 1, 8).await, lufs(&c, 9, 16).await);
    println!(
        "edits: bars 1-8 {sus0:.2} -> {sus1:.2} LUFS (velocity split), bars 9-16 {stac0:.2} -> {stac1:.2} LUFS (zone move)"
    );
    assert!(sus1 > sus0 + 2.0, "the split is audible: {sus0} -> {sus1}");
    assert!(
        stac1 > stac0 + 0.5,
        "the move is audible: {stac0} -> {stac1}"
    );

    // ---- 5. native instrument, prune, save, reopen, render -------------------------------
    let (_, imported) = render(&c, "imported").await;
    let saved = ok(
        &c,
        "run",
        json!({ "id": "sampler.save_native", "params": { "instrument": str_inst, "path": "instruments/Strings Native" } }),
    )
    .await;
    assert_eq!(res(&saved)["zones"], 7, "{saved}");
    assert!(
        song.join("instruments/Strings Native/instrument.toml")
            .exists()
    );
    let loaded = ok(
        &c,
        "run",
        json!({ "id": "sampler.load", "params": { "path": "instruments/Strings Native", "channel": str_ch } }),
    )
    .await;
    let native = id(&loaded, "instrument");
    assert_ne!(native, str_inst);
    let pruned = ok(&c, "run", json!({ "id": "sampler.prune" })).await;
    assert_eq!(res(&pruned)["removed"], json!([str_inst]), "{pruned}");
    let (_, from_native) = render(&c, "native").await;
    assert_eq!(
        first_difference(&from_native, &imported),
        None,
        "the native instrument renders like the imported one (channel, frame)"
    );
    ok(&c, "run", json!({ "id": "file.save" })).await;
    let (_, before_reopen) = render(&c, "before-reopen").await;
    drop(c);

    let c = connect(&song, &[]).await;
    let s3 = zones(&c, native).await;
    assert_eq!(s3["name"], "strings");
    assert_eq!(s3["folder"], "instruments/Strings Native", "{s3}");
    assert_eq!(zone_with(&s3, "stac_f.wav")["keys"], json!([36, 84]));
    let (wav, after_reopen) = render(&c, "acceptance-m3").await;
    assert_eq!(
        first_difference(&after_reopen, &before_reopen),
        None,
        "renders before and after reopening differ (channel, frame)"
    );
    let shot = call(&c, "snapshot", json!({ "kind": "zone_map" })).await;
    assert!(!shot.is_error, "{:?}", shot.texts);
    let a = ok(&c, "analyze", json!({ "what": ["key", "loudness"] })).await;
    assert_eq!(a["key"]["from_notes"]["key"], "A minor", "{a}");
    drop(c);

    // ---- the export matches the offline oracle -------------------------------------------
    let project = ph_formats::load_project(&song).unwrap();
    assert_eq!(project.instruments.len(), 2, "strings (native) and the kit");
    let assets = resolver(&song);
    assert!(
        ph_engine::project_warnings(&project, &assets).is_empty(),
        "{:?}",
        ph_engine::project_warnings(&project, &assets)
    );
    let settings = |tail: bool, block: usize| RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: SR,
        quality: ph_node::Quality::Live,
        include_tail: tail,
        block_frames: block,
    };
    let oracle = ph_engine::render_offline(&project, &settings(true, 256), &assets).unwrap();
    let exported = ph_formats::decode::decode_file(&wav).unwrap();
    assert_eq!(
        first_difference(&exported.channels, &oracle.channels),
        None,
        "the exported WAV differs from the offline oracle (channel, frame)"
    );
    // real time through a simulated device, against the offline render of the song
    let song_only = ph_engine::render_offline(&project, &settings(false, 512), &assets).unwrap();
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
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
    assert!(engine.stats().warnings.is_empty());
    assert_eq!(engine.stats().underruns, 0);

    // ---- the numbers ------------------------------------------------------------------------
    let loud = ph_analysis::loudness::analyze(&exported.channels, SR);
    let lufs = loud.integrated_lufs.expect("an integrated loudness");
    let peak = exported
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    println!(
        "M3 acceptance export: {lufs:.2} LUFS integrated, true peak {:.2} dBTP, sample peak {peak:.3}; key {key_notes} (audio {key_audio}); chords {}",
        loud.true_peak_dbtp.unwrap_or(f64::NAN),
        by_bar.join(" ")
    );
    assert!(peak < 1.0, "sample peak {peak} clips");
    assert!((-24.0..=-12.0).contains(&lufs), "integrated {lufs} LUFS");
    // 16 bars at 100 BPM is 38.4 s, plus the release tails
    let secs = exported.channels[0].len() as f64 / SR as f64;
    assert!(secs >= 38.4, "{secs}");
}
