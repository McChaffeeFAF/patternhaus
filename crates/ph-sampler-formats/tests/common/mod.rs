//! The fixture harness shared by the SF2/SF3, DecentSampler and EXS fixture tests (the SFZ
//! test keeps its own copy): it collects an import's samples into a folder the way
//! `sampler.import` does, renders a phrase through the engine twice (bit-identical),
//! compares the render with its reference `tests/golden/<format>/<id>.json`, saves the
//! instrument as a native folder, reopens it and renders again (bit-identical).
//!
//! `PH_GOLDEN_UPDATE=1` rewrites the references (say why in the PR). Fixtures whose sound
//! needs M3/T2's DSP are marked *pending T2* in `docs/parity/haus-sampler.md`.

#![allow(dead_code)]

use ph_core::*;
use ph_engine::assets::FileResolver;
use ph_engine::{AudioBuffer, PlayMode, RenderSettings, render_offline};
use ph_formats::instrument::{load_instrument, save_instrument};
use ph_node::Quality;
use ph_sampler_formats::{Generated, Imported, SampleSource, audio_wav};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const SR: u32 = 48_000;
const STAT_TOL_DB: f64 = 0.1;
const SEG_TOL_DB: f64 = 0.5;
const FLOOR_DB: f64 = -70.0;

/// (key, start step, length in steps, velocity 0..1); 120 BPM, two bars rendered.
pub type Phrase = Vec<(u8, u64, u64, f32)>;

/// A 16-bit mono WAV, optionally with a `smpl` chunk (root key, one loop `[start, end]`
/// inclusive as the chunk stores it).
pub fn wav(data: &[f32], rate: u32, smpl: Option<(u32, u32, u32)>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&rate.to_le_bytes());
    body.extend_from_slice(&(rate * 2).to_le_bytes());
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32 * 2).to_le_bytes());
    for s in data {
        body.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    if let Some((root, ls, le)) = smpl {
        let mut c = vec![0u8; 36];
        c[12..16].copy_from_slice(&root.to_le_bytes());
        c[28..32].copy_from_slice(&1u32.to_le_bytes());
        let mut l = vec![0u8; 24];
        l[8..12].copy_from_slice(&ls.to_le_bytes());
        l[12..16].copy_from_slice(&le.to_le_bytes());
        c.extend(l);
        body.extend_from_slice(b"smpl");
        body.extend_from_slice(&(c.len() as u32).to_le_bytes());
        body.extend(c);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

pub fn notes(i: &Imported) -> Vec<String> {
    i.report.notes.iter().map(|n| n.what.clone()).collect()
}

pub fn has_note(i: &Imported, what: &str) {
    assert!(
        notes(i).iter().any(|n| n == what),
        "{what} not in the report: {:?}",
        i.report.notes
    );
}

pub fn lacks_note(i: &Imported, what: &str) {
    assert!(
        !notes(i).iter().any(|n| n == what),
        "{what} unexpectedly in the report"
    );
}

/// The model's checks, and every zone plays a listed sample.
pub fn validate(imported: &Imported) -> Result<(), String> {
    let i = &imported.instrument;
    validate_instrument_header(i).map_err(|e| e.to_string())?;
    for g in i.groups.values() {
        validate_group(g).map_err(|e| format!("group {}: {e}", g.name))?;
    }
    for z in i.zones.values() {
        validate_zone(z).map_err(|e| format!("zone {}: {e}", z.id))?;
        if !i.groups.contains_key(&z.group) {
            return Err(format!("zone {} in a missing group", z.id));
        }
        let SampleRef::File(n) = &z.sample else {
            return Err("registry sample".into());
        };
        if !imported.samples.iter().any(|s| &s.name == n) {
            return Err(format!("{n} unlisted"));
        }
    }
    Ok(())
}

/// Writes every sample of an import under `dir` by its zone name (collected).
pub fn collect(imported: &Imported, dir: &Path) {
    for s in &imported.samples {
        let path = dir.join(&s.name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        match &s.source {
            SampleSource::File(p) => {
                std::fs::copy(p, &path).unwrap();
            }
            SampleSource::Generated(g) => {
                ph_formats::encode::write_wav(
                    &path,
                    &[g.render()],
                    Generated::RATE,
                    ph_formats::encode::WavFormat::Float32,
                )
                .unwrap();
            }
            SampleSource::Audio(a) => std::fs::write(&path, audio_wav(a).unwrap()).unwrap(),
            SampleSource::Chunk { .. } => panic!("chunk sources are not used by these importers"),
        }
    }
}

/// A project playing `inst` with the phrase.
pub fn project(inst: &HausInstrument, phrase: &[(u8, u64, u64, f32)]) -> Project {
    let mut e = Edit::new(&Project::empty("fixture"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let i = e.add_instrument(inst.clone()).unwrap();
    let ch = e
        .add_channel(
            "Fixture",
            InstrumentSpec::MultiSampler {
                library: None,
                instrument: Some(i),
                params: Default::default(),
            },
        )
        .unwrap();
    let pat = e.add_pattern("Phrase").unwrap();
    for (k, start, len, v) in phrase {
        let mut n = NoteSpec::new(*k, start * STEP, len * STEP);
        n.velocity = *v;
        e.add_note(pat, ch, n).unwrap();
    }
    e.add_clip(0, pat, 0).unwrap();
    e.finish().0
}

pub fn render(p: &Project, dir: &Path) -> AudioBuffer {
    let settings = RenderSettings {
        mode: PlayMode::Song,
        range: Some((0, 8 * PPQ)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    let assets = FileResolver::new().with_project_dir(dir);
    render_offline(p, &settings, &assets).expect("render")
}

fn db(x: f64) -> f64 {
    if x <= 1e-9 { -180.0 } else { 20.0 * x.log10() }
}

pub fn fingerprint(a: &AudioBuffer) -> Value {
    let frames = a.frames();
    let mono: Vec<f64> = (0..frames)
        .map(|i| a.channels.iter().map(|c| c[i] as f64).sum::<f64>() / a.channels.len() as f64)
        .collect();
    let peak = mono.iter().fold(0f64, |m, x| m.max(x.abs()));
    let rms = |s: &[f64]| (s.iter().map(|x| x * x).sum::<f64>() / s.len().max(1) as f64).sqrt();
    let segs: Vec<f64> = mono
        .chunks(SR as usize / 4)
        .map(|c| (db(rms(c)) * 100.0).round() / 100.0)
        .collect();
    json!({
        "frames": frames,
        "peak_db": (db(peak) * 100.0).round() / 100.0,
        "rms_db": (db(rms(&mono)) * 100.0).round() / 100.0,
        "segments_db": segs,
    })
}

fn compare(dir: &Path, id: &str, got: &Value) -> Result<(), String> {
    let path = dir.join(format!("{id}.json"));
    if std::env::var_os("PH_GOLDEN_UPDATE").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(got).unwrap() + "\n").unwrap();
        return Ok(());
    }
    let want: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .map_err(|e| format!("{id}: no reference ({e}); run with PH_GOLDEN_UPDATE=1"))?,
    )
    .map_err(|e| e.to_string())?;
    if want["frames"] != got["frames"] {
        return Err(format!(
            "{id}: length {} != {}",
            got["frames"], want["frames"]
        ));
    }
    for k in ["peak_db", "rms_db"] {
        let (a, b) = (got[k].as_f64().unwrap(), want[k].as_f64().unwrap());
        if (a - b).abs() > STAT_TOL_DB && a.max(b) > FLOOR_DB {
            return Err(format!("{id}: {k} {a} != {b}"));
        }
    }
    let (a, b) = (
        got["segments_db"].as_array().unwrap(),
        want["segments_db"].as_array().unwrap(),
    );
    for (n, (x, y)) in a.iter().zip(b).enumerate() {
        let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
        if x.max(y) > FLOOR_DB && (x - y).abs() > SEG_TOL_DB {
            return Err(format!("{id}: segment {n}: {x} dB != {y} dB"));
        }
    }
    Ok(())
}

/// The native round trip: `instrument.toml` + copies of every sample, reopened.
fn save_and_reopen(inst: &HausInstrument, from: &Path, project_dir: &Path) -> HausInstrument {
    let out = project_dir.join("native");
    let mut file = inst.clone();
    file.folder = None;
    file.id = InstrumentId(0);
    for z in file.zones.values_mut() {
        let SampleRef::File(name) = &z.sample else {
            panic!()
        };
        let target = format!("samples/{}", name.replace('/', "_"));
        std::fs::create_dir_all(out.join("samples")).unwrap();
        std::fs::copy(from.join(name), out.join(&target)).unwrap();
        z.sample = SampleRef::File(target);
    }
    save_instrument(&file, &out).unwrap();
    let mut back = load_instrument(&out).unwrap();
    assert_eq!(
        ph_formats::instrument::instrument_to_toml(&back).unwrap(),
        std::fs::read_to_string(out.join("instrument.toml")).unwrap(),
        "byte-stable"
    );
    back.folder = Some("native".into());
    back
}

pub fn golden_dir(format: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden")
        .join(format)
}

/// Validates, collects, renders (twice), compares with the reference, round-trips through
/// the native format and renders again.
pub fn render_and_round_trip(
    format: &str,
    id: &str,
    imported: &Imported,
    phrase: &[(u8, u64, u64, f32)],
) -> Result<(), String> {
    validate(imported).map_err(|e| format!("{id}: {e}"))?;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("inst");
    collect(imported, &folder);
    let mut inst = imported.instrument.clone();
    inst.folder = Some("inst".into());
    let p = project(&inst, phrase);
    let a = render(&p, tmp.path());
    let b = render(&p, tmp.path());
    if a.channels != b.channels {
        return Err(format!("{id}: render is not deterministic"));
    }
    let peak = a
        .channels
        .iter()
        .flatten()
        .fold(0f32, |m, x| m.max(x.abs()));
    if peak <= 0.01 {
        return Err(format!("{id}: the render is silent"));
    }
    compare(&golden_dir(format), id, &fingerprint(&a))?;
    let back = save_and_reopen(&inst, &folder, tmp.path());
    let c = render(&project(&back, phrase), tmp.path());
    if a.channels != c.channels {
        return Err(format!("{id}: the native round trip renders differently"));
    }
    Ok(())
}
