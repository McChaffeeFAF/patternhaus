//! Factory Haus Synth presets: every file loads, names real parameters with in-range
//! values, renders a C3 at full velocity without clipping, and overrides work.

#[path = "../../ph-effects/tests/common/mod.rs"]
mod common;
mod util;

use common::*;
use ph_core::InstrumentSpec;
use ph_dsp::analysis::{peak, rms};
use ph_instruments::presets::{synth_preset, synth_preset_names};
use ph_node::{EventKind, Processor, Quality};
use std::collections::BTreeMap;

fn make(preset: Option<&str>, params: &[(&str, f64)]) -> Result<Box<dyn Processor>, String> {
    let spec = InstrumentSpec::Synth {
        preset: preset.map(String::from),
        params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    };
    ph_instruments::create(&spec, &util::MapResolver(Default::default()))
}

fn c3_hold(p: &mut dyn Processor, q: Quality) -> Vec<Vec<f32>> {
    activate(p, 512, q, 1);
    let events = [
        (
            0,
            EventKind::NoteOn {
                note_id: 1,
                key: 48,
                velocity: 1.0,
                pan: 0.0,
                fine_pitch: 0.0,
            },
        ),
        (
            (2.5 * SR) as usize,
            EventKind::NoteOff {
                note_id: 1,
                key: 48,
                velocity: 0.0,
            },
        ),
    ];
    render(p, (4.0 * SR) as usize, 256, &events, None, None)
}

#[test]
fn there_are_about_thirty_presets_in_every_category() {
    let names = synth_preset_names();
    assert!(names.len() >= 30, "{} presets", names.len());
    for cat in ["bass", "lead", "pad", "pluck", "keys", "fx"] {
        let n = names
            .iter()
            .filter(|n| n.starts_with(&format!("{cat}/")))
            .count();
        assert!(n >= 4, "category {cat} has {n} presets");
    }
    for required in [
        "bass/sub",
        "bass/reese",
        "bass/acid",
        "bass/pluck",
        "lead/supersaw",
        "lead/square",
        "pad/warm",
        "pad/glass",
        "pluck/bell",
        "keys/epiano-ish",
        "fx/riser",
    ] {
        assert!(names.contains(&required), "missing {required}");
    }
}

#[test]
fn embedded_table_matches_the_preset_folder() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/synth");
    let mut on_disk = Vec::new();
    for cat in std::fs::read_dir(&root).unwrap().flatten() {
        for f in std::fs::read_dir(cat.path()).unwrap().flatten() {
            let p = f.path();
            assert_eq!(
                p.extension().and_then(|e| e.to_str()),
                Some("json"),
                "{p:?}"
            );
            on_disk.push(format!(
                "{}/{}",
                cat.file_name().to_string_lossy(),
                p.file_stem().unwrap().to_string_lossy()
            ));
        }
    }
    on_disk.sort();
    assert_eq!(on_disk, synth_preset_names());
}

#[test]
fn every_preset_names_real_parameters_with_in_range_values() {
    let desc = ph_instruments::synth::descriptor();
    for name in synth_preset_names() {
        let values = synth_preset(name).unwrap();
        assert!(!values.is_empty(), "{name} is empty");
        for (id, v) in values {
            let i = desc
                .param_id(&id)
                .unwrap_or_else(|| panic!("{name}: no parameter `{id}`"));
            let info = &desc.params[i.0 as usize];
            assert!(
                (info.min..=info.max).contains(&v),
                "{name}: {id} = {v} outside {}..={}",
                info.min,
                info.max
            );
            if info.stepped {
                assert_eq!(v.fract(), 0.0, "{name}: stepped {id} = {v}");
            }
        }
    }
}

#[test]
fn every_preset_loads_and_renders_c3_without_clipping() {
    for name in synth_preset_names() {
        for q in [Quality::Live, Quality::High] {
            let mut p = make(Some(name), &[]).unwrap_or_else(|e| panic!("{name}: {e}"));
            let out = c3_hold(p.as_mut(), q);
            let pk = out.iter().map(|c| peak(c)).fold(0.0f32, f32::max);
            let level = out.iter().map(|c| rms(c)).fold(0.0f64, f64::max);
            let finite = out.iter().all(|c| c.iter().all(|s| s.is_finite()));
            if q == Quality::Live {
                println!(
                    "{name:<18} peak {:+.1} dBFS rms {:+.1} dBFS",
                    20.0 * pk.log10(),
                    20.0 * level.log10()
                );
            }
            assert!(finite, "{name}: non-finite output");
            assert!(pk <= 1.0, "{name}: clips ({pk}) at {q:?}");
            assert!(pk >= 0.1, "{name}: too quiet ({pk}) at {q:?}");
        }
    }
}

#[test]
fn presets_sound_different_from_each_other() {
    let mut renders: Vec<(&str, Vec<f32>)> = Vec::new();
    for name in synth_preset_names() {
        let mut p = make(Some(name), &[]).unwrap();
        renders.push((name, c3_hold(p.as_mut(), Quality::Live).remove(0)));
    }
    for (i, (a, x)) in renders.iter().enumerate() {
        for (b, y) in &renders[i + 1..] {
            assert_ne!(x, y, "{a} and {b} render identically");
        }
    }
}

#[test]
fn params_override_the_preset() {
    let a = synth_preset("bass/sub").unwrap();
    let vol = a.iter().find(|(k, _)| k == "volume").unwrap().1;
    let render_with = |params: &[(&str, f64)]| {
        let mut p = make(Some("bass/sub"), params).unwrap();
        peak(&c3_hold(p.as_mut(), Quality::Live)[0])
    };
    let base = render_with(&[]);
    let quieter = render_with(&[("volume", vol - 12.0)]);
    let ratio = base / quieter;
    assert!((ratio - 3.98).abs() < 0.2, "ratio {ratio}");
}

#[test]
fn preset_alone_equals_its_params_spelled_out() {
    let from_preset = {
        let mut p = make(Some("pad/warm"), &[]).unwrap();
        c3_hold(p.as_mut(), Quality::Live)
    };
    let map: BTreeMap<String, f64> = synth_preset("pad/warm").unwrap().into_iter().collect();
    let pairs: Vec<(&str, f64)> = map.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let mut p = make(None, &pairs).unwrap();
    assert_eq!(from_preset, c3_hold(p.as_mut(), Quality::Live));
}

#[test]
fn unknown_preset_is_an_error_listing_the_available_names() {
    let e = make(Some("bass/nope"), &[]).err().expect("must fail");
    assert!(e.contains("bass/nope"), "{e}");
    for name in ["bass/sub", "lead/supersaw", "fx/riser"] {
        assert!(e.contains(name), "error should list {name}: {e}");
    }
}

#[test]
fn no_preset_is_unchanged() {
    assert!(make(None, &[]).is_ok());
}
