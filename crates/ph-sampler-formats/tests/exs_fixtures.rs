//! EXS24 fixtures F-EXS-01 … F-EXS-10 (`docs/formats/exs.md` §10, rules in
//! `docs/formats/sampler-fixtures.md`). Every file is built here with `ExsBuilder` (both byte
//! orders) beside tagged-tone WAV samples in a temp folder. The layout is our spec's and is
//! unconfirmed on Logic's own files, so these fixtures prove the reader agrees with the spec.

mod common;
mod exs_seeds;

use common::*;
use ph_core::*;
use ph_sampler_formats::build::{
    BeSignature, ExsBuilder, ExsGroup, ExsSample, ExsZone, tagged_tone,
};
use ph_sampler_formats::{Exs, ImportError, ImportInput, Imported, Importer};
use std::path::Path;

fn sample_file(dir: &Path, rel: &str, id: u32, frames: usize) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let hz = 220.0 * (1.0 + id as f64 * 0.25);
    std::fs::write(p, wav(&tagged_tone(id, hz, frames, SR), SR, None)).unwrap();
}

struct Fixture {
    id: &'static str,
    exs: Vec<u8>,
    /// Sample files, relative to the temp root (the `.exs` is written to `lib/<id>.exs`).
    samples: Vec<(&'static str, usize)>,
    phrase: Phrase,
    check: fn(&Imported),
}

fn import_at(root: &Path, id: &str, exs: &[u8]) -> Result<Imported, ImportError> {
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let path = lib.join(format!("{id}.exs"));
    std::fs::write(&path, exs).unwrap();
    Exs.import(ImportInput::Path(&path))
}

fn zone_at(i: &Imported, key: u8, vel: u8) -> &Zone {
    i.instrument
        .zones
        .values()
        .find(|z| z.maps(key, vel))
        .unwrap_or_else(|| panic!("no zone at {key}/{vel}"))
}

fn group(i: &Imported, z: &Zone) -> Group {
    i.instrument.groups[&z.group].clone()
}

fn shape(i: &Imported) -> Vec<(u8, u8, u8, u8, u8, i32, String)> {
    i.instrument
        .zones
        .values()
        .map(|z| {
            (
                z.keys.lo,
                z.keys.hi,
                z.velocity.lo,
                z.velocity.hi,
                z.root_key,
                z.tune,
                format!("{:?}", z.sample),
            )
        })
        .collect()
}

fn f01() -> Fixture {
    Fixture {
        id: "F-EXS-01",
        exs: exs_seeds::minimal(false).build(),
        samples: vec![("lib/a.wav", 12_000)],
        phrase: vec![(60, 0, 4, 0.8), (67, 8, 4, 0.8)],
        check: |i| {
            assert_eq!(i.report.format, "exs");
            assert_eq!(i.instrument.name, "Minimal");
            assert_eq!(i.instrument.zones.len(), 1);
            let z = zone_at(i, 60, 100);
            assert_eq!((z.keys.lo, z.keys.hi, z.root_key), (48, 72, 60));
            assert_eq!(z.sample, SampleRef::File("a.wav".into()));
            assert_eq!(group(i, z).name, "Main");
            assert_eq!(i.instrument.settings.voice_limit, 16);
            lacks_note(i, "exs:counts");
            lacks_note(i, "exs:sample-search");
            assert!(i.report.unknown.is_empty(), "{:?}", i.report.unknown);
        },
    }
}

fn f02() -> Fixture {
    let mut b = exs_seeds::minimal(true);
    b.size_flag = true;
    Fixture {
        id: "F-EXS-02",
        exs: b.build(),
        samples: vec![("lib/a.wav", 12_000)],
        phrase: vec![(60, 0, 4, 0.8), (67, 8, 4, 0.8)],
        check: |i| {
            has_note(i, "exs:size-flag");
            has_note(i, "exs:big-endian");
            let tmp = tempfile::tempdir().unwrap();
            sample_file(tmp.path(), "lib/a.wav", 1, 12_000);
            let le = import_at(tmp.path(), "le", &exs_seeds::minimal(false).build()).unwrap();
            assert_eq!(shape(i), shape(&le), "both byte orders read the same");
            assert_eq!(i.instrument.settings.voice_limit, 16);
            for sig in [BeSignature::Tonv, BeSignature::Reversed] {
                let mut b = exs_seeds::minimal(true);
                b.be_signature = sig;
                let other = import_at(tmp.path(), "be", &b.build()).unwrap();
                assert_eq!(shape(&other), shape(&le), "{sig:?}");
            }
        },
    }
}

fn f03() -> Fixture {
    let mut b = ExsBuilder::new("Short forms");
    let mut g = ExsGroup::new("Short");
    g.size = 76;
    b.groups.push(g);
    let mut s = ExsSample::new("short.wav", "");
    s.size = 336;
    b.samples.push(s);
    let mut z = ExsZone::new(0, 0, (0, 127), 60);
    z.size = 96;
    b.zones.push(z);
    Fixture {
        id: "F-EXS-03",
        exs: b.build(),
        samples: vec![("lib/short.wav", 12_000)],
        phrase: vec![(60, 0, 4, 0.8)],
        check: |i| {
            let z = zone_at(i, 60, 100);
            assert_eq!(
                z.sample,
                SampleRef::File("short.wav".into()),
                "name from the header"
            );
            assert_eq!(group(i, z).name, "Short");
            assert!(
                i.report
                    .notes
                    .iter()
                    .all(|n| !n.what.starts_with("exs:block"))
            );
        },
    }
}

fn f04() -> Fixture {
    let mut b = ExsBuilder::new("Mapping");
    b.groups.push(ExsGroup::new("All"));
    for n in 0..12 {
        b.samples.push(ExsSample::new(&format!("m{n}.wav"), ""));
    }
    let z = |s: i32, keys: (u8, u8)| ExsZone::new(s, 0, keys, keys.0);
    let mut zs = Vec::new();
    let mut a = z(0, (36, 40));
    a.vels = (10, 20); // bit 3 off: ignored
    zs.push(a);
    let mut a = z(1, (41, 43));
    a.options = 0x08;
    a.vels = (0, 63);
    zs.push(a);
    let mut a = z(2, (41, 43));
    a.options = 0x08;
    a.vels = (64, 127);
    zs.push(a);
    let mut a = z(3, (44, 44));
    a.fine = 25;
    a.coarse = 2;
    zs.push(a);
    let mut a = z(4, (45, 45));
    a.pan = -50;
    a.volume = -6;
    zs.push(a);
    let mut a = z(5, (46, 46));
    a.options = 0x02;
    zs.push(a);
    let mut a = z(6, (47, 47));
    a.options = 0x04;
    zs.push(a);
    let mut a = z(7, (48, 48));
    a.options = 0x01;
    zs.push(a);
    let mut a = z(8, (49, 49));
    a.volume = 6;
    zs.push(a);
    let mut a = z(9, (50, 50));
    a.options = 0x40;
    a.output = 3;
    zs.push(a);
    let mut a = z(10, (51, 51));
    a.coarse = -12;
    zs.push(a);
    let mut a = z(11, (52, 52));
    a.start = 100;
    a.end = 5_000;
    zs.push(a);
    b.zones = zs;
    let samples: Vec<(&'static str, usize)> = vec![
        ("lib/m0.wav", 12_000),
        ("lib/m1.wav", 12_000),
        ("lib/m2.wav", 12_000),
        ("lib/m3.wav", 12_000),
        ("lib/m4.wav", 12_000),
        ("lib/m5.wav", 12_000),
        ("lib/m6.wav", 12_000),
        ("lib/m7.wav", 12_000),
        ("lib/m8.wav", 12_000),
        ("lib/m9.wav", 12_000),
        ("lib/m10.wav", 12_000),
        ("lib/m11.wav", 12_000),
    ];
    Fixture {
        id: "F-EXS-04",
        exs: b.build(),
        samples,
        phrase: (0..12u8)
            .map(|n| {
                (
                    [38, 42, 42, 44, 45, 46, 47, 48, 49, 50, 51, 52][n as usize],
                    n as u64 * 2,
                    2,
                    if n == 2 { 0.9 } else { 0.4 },
                )
            })
            .collect(),
        check: |i| {
            assert_eq!(i.instrument.zones.len(), 12);
            assert_eq!(
                zone_at(i, 38, 100).velocity,
                VelRange::ALL,
                "velocity bit off"
            );
            assert_eq!(zone_at(i, 42, 30).velocity, VelRange { lo: 0, hi: 63 });
            assert_eq!(zone_at(i, 42, 90).velocity, VelRange { lo: 64, hi: 127 });
            let z = zone_at(i, 44, 100);
            assert_eq!((z.tune, z.fine), (2, 25.0));
            let z = zone_at(i, 45, 100);
            assert_eq!((z.pan, z.volume_db), (-1.0, -6.0));
            let z = zone_at(i, 46, 100);
            assert_eq!(z.root_key, 60, "pitch tracking off");
            let r: Vec<_> = i
                .instrument
                .modulation
                .iter()
                .filter(|r| r.group == Some(z.group))
                .collect();
            assert_eq!((r.len(), r[0].depth), (1, -6400.0));
            assert!(zone_at(i, 47, 100).reverse);
            assert!(zone_at(i, 48, 100).one_shot);
            assert_eq!(zone_at(i, 49, 100).volume_db, 6.0);
            assert_eq!(group(i, zone_at(i, 50, 100)).output, 3);
            assert_eq!(zone_at(i, 51, 100).tune, -12);
            let z = zone_at(i, 52, 100);
            assert_eq!((z.start, z.end), (100, Some(5_000)));
            has_note(i, "exs:pan-scale");
        },
    }
}

fn f05() -> Fixture {
    let mut b = ExsBuilder::new("Loops");
    b.groups.push(ExsGroup::new("Loops"));
    b.samples.push(ExsSample::new("loop.wav", ""));
    let mut zs = Vec::new();
    for (n, (opts, dir)) in [
        (0x03u8, 0u8),
        (0x01, 1),
        (0x01, 2),
        (0x05, 0),
        (0x00, 0),
        (0x01, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let k = 60 + 2 * n as u8;
        let mut z = ExsZone::new(0, 0, (k, k), k);
        z.loop_options = opts;
        z.loop_dir = dir;
        z.loop_start = 2_000;
        z.loop_end = 10_000;
        if n == 0 {
            z.loop_xfade_ms = 10;
        }
        if n == 5 {
            z.loop_xfade_ms = 5; // linear crossfade
        }
        zs.push(z);
    }
    b.zones = zs;
    Fixture {
        id: "F-EXS-05",
        exs: b.build(),
        samples: vec![("lib/loop.wav", 24_000)],
        phrase: vec![
            (60, 0, 6, 0.8),
            (62, 6, 4, 0.8),
            (64, 10, 4, 0.8),
            (66, 14, 4, 0.8),
            (68, 18, 4, 0.8),
            (70, 22, 4, 0.8),
        ],
        check: |i| {
            let l = zone_at(i, 60, 100).looping.clone().unwrap();
            assert_eq!(
                (l.start, l.end, l.crossfade, l.mode),
                (2_000, 10_000, 480, LoopMode::Forward)
            );
            assert_eq!(
                zone_at(i, 62, 100).looping.as_ref().unwrap().mode,
                LoopMode::Forward
            );
            has_note(i, "exs:loop-backward");
            assert_eq!(
                zone_at(i, 64, 100).looping.as_ref().unwrap().mode,
                LoopMode::PingPong
            );
            assert_eq!(
                zone_at(i, 66, 100).looping.as_ref().unwrap().on_release,
                LoopRelease::Exit
            );
            assert!(
                zone_at(i, 68, 100).looping.is_none(),
                "loop off with points present"
            );
            has_note(i, "exs:loop-crossfade");
        },
    }
}

fn f06() -> Fixture {
    let mut b = ExsBuilder::new("Groups");
    for n in 0..8 {
        b.samples.push(ExsSample::new(&format!("g{n}.wav"), ""));
    }
    let mut g = ExsGroup::new("Body");
    g.volume = -6;
    g.pan = 25;
    b.groups.push(g);
    let mut g = ExsGroup::new("Closed hat");
    g.exclusive = 1;
    b.groups.push(g);
    let mut g = ExsGroup::new("Open hat");
    g.exclusive = 1;
    b.groups.push(g);
    let mut g = ExsGroup::new("Soft only");
    g.vels = (0, 63);
    b.groups.push(g);
    let mut g = ExsGroup::new("Release");
    g.release_trigger = 1;
    g.options = 0x40;
    b.groups.push(g);
    let mut g = ExsGroup::new("Muted");
    g.options = 0x10;
    b.groups.push(g);
    let mut g = ExsGroup::new("Four voices");
    g.polyphony = 4;
    b.groups.push(g);
    let mut zs = vec![
        ExsZone::new(0, 0, (36, 36), 36),
        ExsZone::new(1, 1, (42, 42), 42),
        ExsZone::new(2, 2, (46, 46), 46),
        ExsZone::new(3, 3, (48, 50), 48),
    ];
    let mut loud = ExsZone::new(4, 3, (48, 50), 48);
    loud.options = 0x08;
    loud.vels = (80, 127); // outside the group's 0–63: dropped
    zs.push(loud);
    zs.push(ExsZone::new(5, 4, (36, 36), 36));
    zs.push(ExsZone::new(6, 5, (52, 52), 52));
    zs.push(ExsZone::new(7, 6, (55, 60), 55));
    b.zones = zs;
    Fixture {
        id: "F-EXS-06",
        exs: b.build(),
        samples: (0..8)
            .map(|n| {
                (
                    [
                        "lib/g0.wav",
                        "lib/g1.wav",
                        "lib/g2.wav",
                        "lib/g3.wav",
                        "lib/g4.wav",
                        "lib/g5.wav",
                        "lib/g6.wav",
                        "lib/g7.wav",
                    ][n],
                    12_000,
                )
            })
            .collect(),
        phrase: vec![
            (36, 0, 2, 0.8),
            (42, 3, 1, 0.8),
            (46, 4, 2, 0.8),
            (42, 5, 1, 0.8),
            (49, 8, 2, 0.3),
            (52, 11, 2, 0.8),
            (57, 14, 2, 0.8),
            (58, 14, 2, 0.8),
        ],
        check: |i| {
            let z = zone_at(i, 36, 100);
            assert_eq!(z.trigger, ZoneTrigger::Attack);
            assert_eq!((z.volume_db, z.pan), (-6.0, 0.5));
            let hat = zone_at(i, 42, 100);
            assert_eq!(hat.group, zone_at(i, 46, 100).group, "one choke group");
            let g = group(i, hat);
            assert_eq!((g.name.as_str(), g.voice_limit), ("Closed hat", Some(1)));
            assert_eq!(zone_at(i, 49, 30).velocity, VelRange { lo: 0, hi: 63 });
            assert!(!i.instrument.zones.values().any(|z| z.maps(49, 100)));
            has_note(i, "exs:group-limits");
            let rel = i
                .instrument
                .zones
                .values()
                .find(|z| z.trigger == ZoneTrigger::Release)
                .unwrap();
            assert_eq!(rel.release_decay_db, 3.0);
            assert_eq!(zone_at(i, 52, 100).volume_db, -144.0);
            assert_eq!(group(i, zone_at(i, 57, 100)).voice_limit, Some(4));
        },
    }
}

fn f07() -> Fixture {
    let mut b = ExsBuilder::new("Enable by");
    for n in 0..6 {
        b.samples.push(ExsSample::new(&format!("e{n}.wav"), ""));
    }
    for (n, pos) in [5, 3, 4].into_iter().enumerate() {
        let mut g = ExsGroup::new(&format!("RR {}", n + 1));
        g.enable_by = 2;
        g.rr_position = pos;
        b.groups.push(g);
        b.zones.push(ExsZone::new(n as i32, n as i32, (60, 60), 60));
    }
    for (n, key) in [(3, 24u8), (4, 26)] {
        let mut g = ExsGroup::new(if key == 24 { "Sustain" } else { "Staccato" });
        g.enable_by = 1;
        g.enable_note = key;
        b.groups.push(g);
        b.zones.push(ExsZone::new(n, n, (62, 72), 62));
    }
    let mut g = ExsGroup::new("Wheel up");
    g.enable_by = 3;
    g.enable_cc = (1, 64, 127);
    b.groups.push(g);
    b.zones.push(ExsZone::new(5, 5, (74, 74), 74));
    Fixture {
        id: "F-EXS-07",
        exs: b.build(),
        samples: (0..6)
            .map(|n| {
                (
                    [
                        "lib/e0.wav",
                        "lib/e1.wav",
                        "lib/e2.wav",
                        "lib/e3.wav",
                        "lib/e4.wav",
                        "lib/e5.wav",
                    ][n],
                    12_000,
                )
            })
            .collect(),
        phrase: vec![
            (60, 0, 1, 0.8),
            (60, 2, 1, 0.8),
            (60, 4, 1, 0.8),
            (64, 6, 2, 0.8),
            (26, 9, 1, 0.8),
            (64, 10, 2, 0.8),
            (74, 13, 2, 0.8),
        ],
        check: |i| {
            let rr: Vec<_> = i
                .instrument
                .zones
                .values()
                .filter(|z| z.maps(60, 100))
                .collect();
            assert_eq!(rr.len(), 3);
            assert!(
                rr.iter().all(|z| z.group == rr[0].group),
                "one cycle, one group"
            );
            let pos = |s: &str| {
                rr.iter()
                    .find(|z| z.sample == SampleRef::File(s.into()))
                    .unwrap()
                    .round_robin
            };
            // stored 5, 3, 4: sorted 3 (e1), 4 (e2), 5 (e0)
            assert_eq!(
                (pos("e1.wav"), pos("e2.wav"), pos("e0.wav")),
                (Some(0), Some(1), Some(2))
            );
            let arts = &i.instrument.articulations;
            assert_eq!(arts.len(), 2);
            assert_eq!(arts[0].trigger, ArticulationTrigger::Key { key: 24 });
            assert_eq!(arts[1].name, "Staccato");
            let stac = i
                .instrument
                .zones
                .values()
                .find(|z| z.sample == SampleRef::File("e4.wav".into()))
                .unwrap();
            assert_eq!(group(i, stac).articulations, vec![arts[1].id]);
            let cc = &zone_at(i, 74, 100).cc_conditions[0];
            assert_eq!(cc.controller, 1);
            assert!((cc.lo - 64.0 / 127.0).abs() < 1e-6 && cc.hi == 1.0);
        },
    }
}

fn f08() -> Fixture {
    let mut b = exs_seeds::minimal(false);
    b.params = vec![
        (0x07, -3),
        (0x03, 12),
        (0x04, -1),
        (0x05, 24),
        (0x0A, 1),
        (0x2D, 2),
        (0x0E, 1),
        (0x0F, 10),
        (0x2C, 1),
        (0xF3, 4),
        (0x1E, 500),
        (0x1D, 250),
        (0x2E, 1000),
        (0x6B, 2),
        (0x52, 64),
        (0x53, 0),
        (0x54, 127),
        (0x51, 100),
        (0x55, 32),
        (0x4C, 10),
        (0x4E, 40),
        (0x14, 5),
        (0x99, 7),
    ];
    b.params2 = vec![(0x160, 25), (0x16D, 20), (0x1FF, 3)];
    Fixture {
        id: "F-EXS-08",
        exs: b.build(),
        samples: vec![("lib/a.wav", 20_000)],
        phrase: vec![(60, 0, 8, 0.8), (64, 10, 4, 0.8)],
        check: |i| {
            let s = &i.instrument.settings;
            assert_eq!(
                (s.volume_db, s.pan, s.bend_up, s.bend_down),
                (-3.0, 0.5, 12.0, 12.0)
            );
            assert_eq!((s.voice_limit, s.transpose, s.tune_cents), (24, 3, 10.0));
            let g = group(i, zone_at(i, 60, 100));
            assert_eq!(g.polyphony, Polyphony::Legato);
            let f = g.filter.as_ref().unwrap();
            assert_eq!(f.kind, FilterKind::HighPass12);
            assert!(
                (f.cutoff - 20.0 * 1000f32.powf(0.5)).abs() < 0.1,
                "{}",
                f.cutoff
            );
            assert!((f.resonance - 6.0).abs() < 1e-4 && f.key_track == 100.0);
            let e = &g.amp_env;
            assert_eq!(e.delay, 0.0, "ADSR: no delay stage");
            assert!((e.attack - 10.0 * (64f32 / 127.0).powi(4)).abs() < 1e-4);
            assert!((e.decay - 10.0).abs() < 1e-4 && (e.sustain - 100.0 / 127.0).abs() < 1e-6);
            assert!(g.filter_env.is_some());
            has_note(i, "exs:param:filter envelope depth");
            has_note(i, "exs:param:glide");
            assert!(i.report.unknown.iter().any(|(w, _)| w == "exs:param:0x099"));
            assert!(i.report.unknown.iter().any(|(w, _)| w == "exs:param:0x1ff"));
            assert!(i.instrument.extra["exs.params"].contains("0x160=25"));
        },
    }
}

fn f09() -> Fixture {
    let mut b = ExsBuilder::new("Lookup");
    b.groups.push(ExsGroup::new("Lookup"));
    b.samples
        .push(ExsSample::new("near.wav", "/Volumes/Library/Samples"));
    b.samples
        .push(ExsSample::new("sibling.wav", "/Users/x/Samples"));
    b.samples.push(ExsSample::new("CASE.WAV", ""));
    b.samples.push(ExsSample::new("missing.wav", ""));
    for n in 0..4 {
        let k = 60 + 2 * n as u8;
        b.zones.push(ExsZone::new(n, 0, (k, k), k));
    }
    Fixture {
        id: "F-EXS-09",
        exs: b.build(),
        samples: vec![
            ("lib/near.wav", 12_000),
            ("Other/sibling.wav", 12_000),
            ("lib/sub/case.wav", 12_000),
        ],
        phrase: vec![(60, 0, 2, 0.8), (62, 3, 2, 0.8), (64, 6, 2, 0.8)],
        check: |i| {
            assert_eq!(
                i.instrument.zones.len(),
                3,
                "the missing sample's zone is skipped"
            );
            let names: Vec<_> = i.samples.iter().map(|s| s.name.clone()).collect();
            assert_eq!(
                names,
                ["lib/near.wav", "Other/sibling.wav", "lib/sub/case.wav"]
            );
            has_note(i, "exs:sample-search");
            assert!(
                i.report
                    .notes
                    .iter()
                    .any(|n| n.what == "exs:sample" && n.detail.contains("missing.wav"))
            );
        },
    }
}

fn f10() -> Fixture {
    let (_, bytes) = exs_seeds::accepted_oddities().remove(0);
    Fixture {
        id: "F-EXS-10",
        exs: bytes,
        samples: vec![("lib/a.wav", 12_000)],
        phrase: vec![(60, 0, 4, 0.8)],
        check: |i| {
            assert_eq!(i.instrument.zones.len(), 1);
            has_note(i, "exs:block:5");
            has_note(i, "exs:block:7");
            has_note(i, "exs:block:0x0a");
            has_note(i, "exs:zone");
            assert!(i.report.unknown.iter().any(|(w, _)| w == "exs:block:0x0c"));
            assert!(
                i.instrument
                    .extra
                    .keys()
                    .any(|k| k.starts_with("exs.block.0x0b"))
            );
            // every index −1: zone ids become sample indices
            let (_, b) = exs_seeds::accepted_oddities().remove(1);
            let tmp = tempfile::tempdir().unwrap();
            sample_file(tmp.path(), "lib/a.wav", 1, 4_000);
            let j = import_at(tmp.path(), "ids", &b).unwrap();
            has_note(&j, "exs:zone-sample-ids");
            assert_eq!(j.instrument.zones.len(), 1);
            assert_eq!(
                j.instrument.groups.values().next().unwrap().name,
                "Ungrouped"
            );
        },
    }
}

fn fixtures() -> Vec<Fixture> {
    vec![
        f01(),
        f02(),
        f03(),
        f04(),
        f05(),
        f06(),
        f07(),
        f08(),
        f09(),
        f10(),
    ]
}

#[test]
fn exs_fixtures_import_render_and_round_trip() {
    let mut failures = Vec::new();
    for f in fixtures() {
        let tmp = tempfile::tempdir().unwrap();
        for (n, (rel, frames)) in f.samples.iter().enumerate() {
            sample_file(tmp.path(), rel, n as u32 + 1, *frames);
        }
        let imported = match import_at(tmp.path(), f.id, &f.exs) {
            Ok(i) => i,
            Err(e) => {
                failures.push(format!("{}: {e}", f.id));
                continue;
            }
        };
        let check = f.check;
        if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&imported)))
        {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failures.push(format!("{}: check failed: {msg}", f.id));
            continue;
        }
        if let Err(e) = render_and_round_trip("exs", f.id, &imported, &f.phrase) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn malformed_files_are_refused_never_panic() {
    for (what, bytes) in exs_seeds::rejected() {
        match Exs.import(ImportInput::Bytes {
            name: "bad.exs",
            bytes: &bytes,
        }) {
            Err(ImportError::Malformed { .. }) => {}
            other => panic!("{what}: {:?}", other.map(|i| i.report)),
        }
    }
    // from bytes alone (no folder) the samples cannot be found: zones are reported, not lost
    let i = Exs
        .import(ImportInput::Bytes {
            name: "a.exs",
            bytes: &exs_seeds::minimal(false).build(),
        })
        .unwrap();
    assert!(i.instrument.zones.is_empty());
    assert!(i.report.notes.iter().any(|n| n.what == "exs:sample"));
}
