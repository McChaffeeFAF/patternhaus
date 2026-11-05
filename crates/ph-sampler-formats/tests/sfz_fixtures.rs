//! SFZ fixtures F-SFZ-01 … F-SFZ-17 (`docs/formats/sfz.md` §18, rules in
//! `docs/formats/sampler-fixtures.md`). Every fixture is generated here: the `.sfz` text is a
//! string, the samples are tagged tones written beside it in a temp folder (a short DC "ID
//! level" then a sine whose pitch names the sample).
//!
//! For each fixture the test:
//! 1. imports it and checks the model (zones, groups, articulations, routes, report notes);
//! 2. renders a fixed phrase through the engine (`ph.haus_sampler`), twice: bit-identical;
//! 3. compares the render with its reference `tests/golden/sfz/<id>.json` (length, peak, RMS
//!    and the RMS of every quarter second, within the golden tolerances);
//! 4. saves the instrument as a native folder (`instrument.toml` + `samples/`), reopens it
//!    and renders again: bit-identical to step 2.
//!
//! `PH_GOLDEN_UPDATE=1` rewrites the references (say why in the PR). Fixtures whose features
//! need M3/T2's DSP (envelopes, loops, filters, LFOs, crossfades, modulation) are marked
//! *pending T2* in `docs/parity/haus-sampler.md`: their references hold the T0 engine's
//! sound until T2 lands and regenerates them; their models are checked now.

use ph_core::*;
use ph_engine::assets::FileResolver;
use ph_engine::{AudioBuffer, PlayMode, RenderSettings, render_offline};
use ph_formats::instrument::{load_instrument, save_instrument};
use ph_node::Quality;
use ph_sampler_formats::{Generated, ImportInput, Imported, Importer, SampleSource, Sfz};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

mod sfz_seeds;
use sfz_seeds::MALFORMED;

const SR: u32 = 48_000;
const STAT_TOL_DB: f64 = 0.1;
const SEG_TOL_DB: f64 = 0.5;
const FLOOR_DB: f64 = -70.0;

// ------------------------------------------------------------------------- samples

/// A tagged tone: `id` × 0.05 DC for 10 ms, then a sine at `hz`, 0.25 s, mono.
fn tagged(id: u32, hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            if i < 480 {
                0.05 * id as f32
            } else {
                (0.4 * (std::f64::consts::TAU * hz * i as f64 / SR as f64).sin()) as f32
            }
        })
        .collect()
}

/// A 16-bit mono WAV, optionally with a `smpl` chunk (root key, one loop `[start, end]`
/// inclusive as the chunk stores it).
fn wav(data: &[f32], smpl: Option<(u32, u32, u32)>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&SR.to_le_bytes());
    body.extend_from_slice(&(SR * 2).to_le_bytes());
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

// ------------------------------------------------------------------------- fixtures

/// (path relative to `lib/`, frames, `smpl` root and inclusive loop).
type SampleSpec = (&'static str, usize, Option<(u32, u32, u32)>);

struct Fixture {
    id: &'static str,
    /// The main file, `lib/<id>.sfz`.
    sfz: &'static str,
    /// Other text files (includes), relative to `lib/`.
    texts: Vec<(&'static str, &'static str)>,
    /// Samples relative to `lib/`: (path, frames, smpl).
    samples: Vec<SampleSpec>,
    /// (key, start step, length in steps, velocity 0..1); 120 BPM, two bars rendered.
    phrase: Vec<(u8, u64, u64, f32)>,
    check: fn(&Imported),
}

fn s(path: &'static str) -> SampleSpec {
    (path, 12_000, None)
}

fn notes(i: &Imported) -> Vec<String> {
    i.report.notes.iter().map(|n| n.what.clone()).collect()
}

fn has_note(i: &Imported, what: &str) {
    assert!(
        notes(i).iter().any(|n| n == what),
        "{what} not in the report: {:?}",
        notes(i)
    );
}

fn zones(i: &Imported) -> Vec<&Zone> {
    i.instrument.zones.values().collect()
}

fn zone<'a>(i: &'a Imported, sample: &str) -> &'a Zone {
    i.instrument
        .zones
        .values()
        .find(|z| matches!(&z.sample, SampleRef::File(f) if f.ends_with(sample)))
        .unwrap_or_else(|| panic!("no zone plays {sample}"))
}

fn group_of<'a>(i: &'a Imported, sample: &str) -> &'a Group {
    &i.instrument.groups[&zone(i, sample).group]
}

fn routes_of(i: &Imported, g: GroupId) -> Vec<&ModRoute> {
    i.instrument
        .modulation
        .iter()
        .filter(|r| r.group == Some(g))
        .collect()
}

fn keys(z: &Zone) -> (u8, u8, u8) {
    (z.keys.lo, z.keys.hi, z.root_key)
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            id: "F-SFZ-01",
            sfz: "<region> sample=low.wav lokey=36 hikey=59 pitch_keycenter=48\n\
                  <region> sample=mid.wav lokey=60 hikey=71 pitch_keycenter=c5 tune=-30 transpose=1\n\
                  <region> sample=hi.wav key=84 pan=-50 volume=-6\n",
            texts: vec![],
            samples: vec![s("low.wav"), s("mid.wav"), s("hi.wav")],
            phrase: vec![
                (48, 0, 2, 0.8),
                (55, 4, 2, 0.8),
                (60, 8, 2, 0.8),
                (72, 12, 2, 0.8),
                (84, 16, 2, 0.8),
            ],
            check: |i| {
                assert_eq!(zones(i).len(), 3);
                assert_eq!(keys(zone(i, "low.wav")), (36, 59, 48));
                let m = zone(i, "mid.wav");
                assert_eq!((keys(m), m.tune, m.fine), ((60, 71, 72), 1, -30.0));
                let h = zone(i, "hi.wav");
                assert_eq!((keys(h), h.pan, h.volume_db), ((84, 84, 84), -0.5, -6.0));
                assert!(i.report.unknown.is_empty() && i.report.notes.is_empty());
            },
        },
        Fixture {
            id: "F-SFZ-02",
            sfz: "<global> volume=-6 pan=10 group_label=G\n\
                  <master> transpose=12 master_label=M1\n\
                  <group> lokey=0 hikey=63 group_label=Lo\n\
                  <region> sample=a.wav pitch_keycenter=48\n\
                  <region> sample=b.wav pitch_keycenter=50 volume=-3\n\
                  <master> master_label=M2\n\
                  <group> lokey=64 hikey=127\n\
                  <region> sample=c.wav pitch_keycenter=72\n\
                  <global>\n\
                  <region> sample=d.wav key=100\n",
            texts: vec![],
            samples: vec![s("a.wav"), s("b.wav"), s("c.wav"), s("d.wav")],
            phrase: vec![
                (40, 0, 2, 0.8),
                (50, 4, 2, 0.8),
                (72, 8, 2, 0.8),
                (100, 12, 2, 0.8),
            ],
            check: |i| {
                let a = zone(i, "a.wav");
                assert_eq!((a.volume_db, a.pan, a.tune, a.keys.hi), (-6.0, 0.1, 12, 63));
                assert_eq!(group_of(i, "a.wav").name, "Lo");
                let b = zone(i, "b.wav");
                assert_eq!((b.volume_db, b.tune), (-3.0, 12));
                let c = zone(i, "c.wav");
                assert_eq!(
                    (c.volume_db, c.tune, c.keys.lo),
                    (-6.0, 0, 64),
                    "master resets group"
                );
                assert_eq!(group_of(i, "c.wav").name, "G");
                let d = zone(i, "d.wav");
                assert_eq!(
                    (d.volume_db, d.pan, keys(d)),
                    (0.0, 0.0, (100, 100, 100)),
                    "global resets all"
                );
                assert_eq!(i.instrument.groups.len(), 3);
            },
        },
        Fixture {
            id: "F-SFZ-03",
            sfz: "<group> lokey=60 hikey=72 pitch_keycenter=60 amp_veltrack=80 amp_velcurve_64=0.3\n\
                  <region> sample=p.wav hivel=50 xfout_lovel=40 xfout_hivel=50\n\
                  <region> sample=m.wav lovel=40 hivel=90 xfin_lovel=40 xfin_hivel=50 xfout_lovel=80 xfout_hivel=90 xf_velcurve=gain\n\
                  <region> sample=f.wav lovel=80 xfin_lovel=80 xfin_hivel=90\n",
            texts: vec![],
            samples: vec![s("p.wav"), s("m.wav"), s("f.wav")],
            phrase: vec![
                (60, 0, 2, 0.2),
                (62, 4, 2, 0.35),
                (64, 8, 2, 0.55),
                (65, 12, 2, 0.68),
                (67, 16, 2, 0.9),
            ],
            check: |i| {
                let v = |n: &str| {
                    let z = zone(i, n);
                    (z.velocity.lo, z.velocity.hi)
                };
                assert_eq!(
                    (v("p.wav"), v("m.wav"), v("f.wav")),
                    ((0, 50), (40, 90), (80, 127))
                );
                let m = &zone(i, "m.wav").crossfades[0];
                assert_eq!(m.source, CrossfadeSource::Velocity);
                assert_eq!(m.curve, CrossfadeCurve::Gain);
                assert!(
                    (m.in_lo - 40.0 / 127.0).abs() < 1e-6 && (m.out_hi - 90.0 / 127.0).abs() < 1e-6
                );
                let p = &zone(i, "p.wav").crossfades[0];
                assert_eq!(
                    (p.in_lo, p.in_hi, p.curve),
                    (0.0, 0.0, CrossfadeCurve::Power)
                );
                has_note(i, "opcode:amp_veltrack");
            },
        },
        Fixture {
            id: "F-SFZ-04",
            sfz: "<group> key=60 seq_length=3\n\
                  <region> sample=r1.wav seq_position=1\n\
                  <region> sample=r2.wav seq_position=2\n\
                  <region> sample=r3.wav seq_position=3\n\
                  <group> key=62\n\
                  <region> sample=x1.wav lorand=0 hirand=0.5\n\
                  <region> sample=x2.wav lorand=0.5 hirand=1\n",
            texts: vec![],
            samples: vec![
                s("r1.wav"),
                s("r2.wav"),
                s("r3.wav"),
                s("x1.wav"),
                s("x2.wav"),
            ],
            phrase: (0..4u64)
                .map(|n| (60, n * 4, 2, 0.8))
                .chain((0..4u64).map(|n| (62, 16 + n * 4, 2, 0.8)))
                .collect(),
            check: |i| {
                let rr: Vec<_> = ["r1.wav", "r2.wav", "r3.wav", "x1.wav", "x2.wav"]
                    .iter()
                    .map(|n| zone(i, n).round_robin)
                    .collect();
                assert_eq!(rr, vec![Some(0), Some(1), Some(2), Some(0), Some(1)]);
                assert_eq!(
                    group_of(i, "r1.wav").round_robin,
                    RoundRobinMode::Sequential
                );
                assert_eq!(group_of(i, "x1.wav").round_robin, RoundRobinMode::Random);
                has_note(i, "opcode:lorand");
            },
        },
        Fixture {
            id: "F-SFZ-05",
            sfz: "<group> key=60\n\
                  <region> sample=att.wav\n\
                  <region> sample=rel.wav trigger=release rt_decay=10\n\
                  <region> sample=first.wav trigger=first\n\
                  <region> sample=leg.wav trigger=legato\n",
            texts: vec![],
            samples: vec![s("att.wav"), s("rel.wav"), s("first.wav"), s("leg.wav")],
            phrase: vec![(60, 0, 4, 0.8), (60, 8, 1, 0.5), (60, 16, 8, 1.0)],
            check: |i| {
                let r = zone(i, "rel.wav");
                assert_eq!(
                    (r.trigger, r.release_decay_db, r.one_shot),
                    (ZoneTrigger::Release, 10.0, true)
                );
                assert_eq!(zone(i, "att.wav").trigger, ZoneTrigger::Attack);
                has_note(i, "opcode:trigger=first");
                has_note(i, "opcode:trigger=legato");
            },
        },
        Fixture {
            id: "F-SFZ-06",
            sfz: "<global> sw_lokey=24 sw_hikey=26 sw_default=24\n\
                  <group> sw_last=24 sw_label=Sustain lokey=48 hikey=72 pitch_keycenter=60\n\
                  <region> sample=sus.wav\n\
                  <group> sw_last=25 sw_label=Staccato lokey=48 hikey=72 pitch_keycenter=60\n\
                  <region> sample=stac.wav\n\
                  <group> sw_down=26 key=80\n\
                  <region> sample=down.wav\n\
                  <group> sw_previous=48 key=81\n\
                  <region> sample=prev.wav\n",
            texts: vec![],
            samples: vec![s("sus.wav"), s("stac.wav"), s("down.wav"), s("prev.wav")],
            phrase: vec![
                (60, 0, 2, 0.8),
                (25, 4, 1, 0.8),
                (62, 6, 2, 0.8),
                (24, 10, 1, 0.8),
                (64, 12, 2, 0.8),
                (80, 16, 2, 0.8),
            ],
            check: |i| {
                let a: Vec<_> = i
                    .instrument
                    .articulations
                    .iter()
                    .map(|a| (a.id.0, a.name.as_str(), a.trigger.clone()))
                    .collect();
                assert_eq!(
                    a,
                    vec![
                        (0, "Sustain", ArticulationTrigger::Key { key: 24 }),
                        (1, "Staccato", ArticulationTrigger::Key { key: 25 }),
                    ]
                );
                assert_eq!(
                    i.instrument.settings.default_articulation,
                    Some(ArticulationId(0))
                );
                assert_eq!(
                    group_of(i, "sus.wav").articulations,
                    vec![ArticulationId(0)]
                );
                assert_eq!(
                    group_of(i, "stac.wav").articulations,
                    vec![ArticulationId(1)]
                );
                assert!(group_of(i, "down.wav").articulations.is_empty());
                has_note(i, "opcode:sw_down");
                has_note(i, "opcode:sw_previous");
            },
        },
        Fixture {
            id: "F-SFZ-07",
            sfz: "<control> set_cc1=64\n\
                  <group> key=60\n\
                  <region> sample=lo.wav locc1=0 hicc1=63\n\
                  <region> sample=hi.wav locc1=64 hicc1=127\n\
                  <group> key=62 xf_cccurve=gain\n\
                  <region> sample=soft.wav xfout_locc1=32 xfout_hicc1=96\n\
                  <region> sample=loud.wav xfin_locc1=32 xfin_hicc1=96\n\
                  <group> key=64\n\
                  <region> sample=pw.wav xfin_locc2=0 xfin_hicc2=127 lohdcc3=0.25\n",
            texts: vec![],
            samples: vec![
                s("lo.wav"),
                s("hi.wav"),
                s("soft.wav"),
                s("loud.wav"),
                s("pw.wav"),
            ],
            phrase: vec![(60, 0, 2, 0.8), (62, 4, 2, 0.8), (64, 8, 2, 0.8)],
            check: |i| {
                let c = &zone(i, "hi.wav").cc_conditions[0];
                assert_eq!((c.controller, c.hi), (1, 1.0));
                assert!((c.lo - 64.0 / 127.0).abs() < 1e-6);
                let x = &zone(i, "soft.wav").crossfades[0];
                assert_eq!(
                    (x.source, x.curve),
                    (CrossfadeSource::Cc { controller: 1 }, CrossfadeCurve::Gain)
                );
                let pw = zone(i, "pw.wav");
                assert_eq!(pw.crossfades[0].curve, CrossfadeCurve::Power);
                assert_eq!(pw.cc_conditions[0].controller, 3);
                assert_eq!(i.instrument.extra["sfz.control"], "set_cc1=64");
                has_note(i, "opcode:set_ccN");
            },
        },
        Fixture {
            id: "F-SFZ-08",
            sfz: "<group> key=60\n<region> sample=loop.wav\n\
                  <group> key=61\n<region> sample=plain.wav loop_mode=loop_sustain loop_start=100 loop_end=1999\n\
                  <group> key=62\n<region> sample=plain.wav loop_mode=loop_continuous loop_type=alternate loop_crossfade=0.01\n\
                  <group> key=63\n<region> sample=loop.wav loop_mode=one_shot\n\
                  <group> key=64\n<region> sample=plain.wav direction=reverse offset=100 end=3999\n\
                  <group> key=65\n<region> sample=plain.wav loop_mode=no_loop\n",
            texts: vec![],
            samples: vec![("loop.wav", 12_000, Some((60, 1000, 2999))), s("plain.wav")],
            phrase: vec![
                (60, 0, 4, 0.8),
                (61, 4, 4, 0.8),
                (62, 8, 4, 0.8),
                (63, 12, 1, 0.8),
                (64, 16, 2, 0.8),
                (65, 20, 2, 0.8),
            ],
            check: |i| {
                let by_key = |k: u8| {
                    i.instrument
                        .zones
                        .values()
                        .find(|z| z.keys.lo == k)
                        .unwrap()
                };
                let l = by_key(60).looping.clone().unwrap();
                assert_eq!(
                    (l.start, l.end, l.on_release),
                    (1000, 3000, LoopRelease::Continue)
                );
                let l = by_key(61).looping.clone().unwrap();
                assert_eq!(
                    (l.start, l.end, l.on_release),
                    (100, 2000, LoopRelease::Exit)
                );
                let l = by_key(62).looping.clone().unwrap();
                assert_eq!(
                    (l.mode, l.start, l.end, l.crossfade),
                    (LoopMode::PingPong, 0, 12_000, 480)
                );
                assert!(by_key(63).one_shot && by_key(63).looping.is_none());
                let r = by_key(64);
                assert_eq!((r.reverse, r.start, r.end), (true, 100, Some(4000)));
                assert!(by_key(65).looping.is_none());
            },
        },
        Fixture {
            id: "F-SFZ-09",
            sfz: "<group> key=60 ampeg_delay=0.01 ampeg_attack=0.02 ampeg_hold=0.03 ampeg_decay=0.1 ampeg_sustain=50 ampeg_release=0.2 ampeg_vel2attack=0.1 \
                  cutoff=2000 fileg_attack=0.05 fileg_depth=1200 fileg_sustain=20\n\
                  <region> sample=a.wav\n\
                  <group> key=62 pitcheg_attack=0.1 pitcheg_depth=-200\n\
                  <region> sample=b.wav\n",
            texts: vec![],
            samples: vec![s("a.wav"), s("b.wav")],
            phrase: vec![(60, 0, 4, 0.8), (62, 8, 4, 0.8)],
            check: |i| {
                let g = group_of(i, "a.wav");
                let e = &g.amp_env;
                assert_eq!(
                    (e.delay, e.attack, e.hold, e.decay, e.sustain, e.release),
                    (0.01, 0.02, 0.03, 0.1, 0.5, 0.2)
                );
                let fe = g.filter_env.as_ref().unwrap();
                assert_eq!((fe.attack, fe.sustain), (0.05, 0.2));
                assert_eq!(g.filter.as_ref().unwrap().env_depth, 1200.0);
                let pg = group_of(i, "b.wav");
                assert_eq!(pg.filter_env.as_ref().unwrap().attack, 0.1);
                let r = routes_of(i, pg.id);
                assert_eq!(
                    (r[0].source.clone(), r[0].target, r[0].depth),
                    (ModSource::Envelope { index: 1 }, ModTarget::Pitch, -200.0)
                );
                has_note(i, "opcode:ampeg_vel2attack");
                has_note(i, "opcode:pitcheg_depth");
            },
        },
        Fixture {
            id: "F-SFZ-10",
            sfz: "<group> key=60 amplfo_freq=5 amplfo_depth=3 pitchlfo_freq=6 pitchlfo_depth=20 pitchlfo_delay=0.1 pitchlfo_fade=0.2\n\
                  <region> sample=a.wav\n\
                  <group> key=61 lfo1_freq=4 lfo1_pitch=30 lfo1_pitch_oncc1=50\n<region> sample=a.wav\n\
                  <group> key=62 lfo1_freq=3 lfo1_wave=0 lfo1_pitch=10\n<region> sample=a.wav\n\
                  <group> key=63 lfo1_freq=3 lfo1_wave=2 lfo1_volume=2\n<region> sample=a.wav\n\
                  <group> key=64 lfo1_freq=3 lfo1_wave=3 lfo1_cutoff=100 lfo1_pan=50\n<region> sample=a.wav\n\
                  <group> key=65 lfo1_freq=3 lfo1_wave=4 lfo1_pitch=10\n<region> sample=a.wav\n\
                  <group> key=66 lfo1_freq=3 lfo1_wave=5 lfo1_pitch=10\n<region> sample=a.wav\n\
                  <group> key=67 lfo1_freq=3 lfo1_wave=6 lfo1_pitch=10\n<region> sample=a.wav\n\
                  <group> key=68 lfo1_freq=3 lfo1_wave=7 lfo1_resonance=3\n<region> sample=a.wav\n",
            texts: vec![],
            samples: vec![s("a.wav")],
            phrase: (60..69u8)
                .map(|k| (k, (k as u64 - 60) * 3, 2, 0.8))
                .collect(),
            check: |i| {
                let g = |k: u8| {
                    let z = i
                        .instrument
                        .zones
                        .values()
                        .find(|z| z.keys.lo == k)
                        .unwrap();
                    &i.instrument.groups[&z.group]
                };
                let g60 = g(60);
                assert_eq!(g60.lfos.len(), 2);
                assert_eq!(
                    (g60.lfos[1].rate, g60.lfos[1].delay, g60.lfos[1].fade),
                    (6.0, 0.1, 0.2)
                );
                let r = routes_of(i, g60.id);
                assert_eq!(
                    (r[0].target, r[0].depth, r[1].target, r[1].depth),
                    (ModTarget::Volume, 3.0, ModTarget::Pitch, 20.0)
                );
                let shapes: Vec<_> = (61..69u8).map(|k| g(k).lfos[0].shape).collect();
                use LfoShape::*;
                assert_eq!(
                    shapes,
                    vec![
                        Sine, Triangle, Square, Square, Square, Square, SawUp, SawDown
                    ]
                );
                let r64 = routes_of(i, g(64).id);
                assert_eq!(
                    (r64[0].target, r64[1].target, r64[1].depth),
                    (ModTarget::Pan, ModTarget::Cutoff, 100.0)
                );
                has_note(i, "opcode:lfo1_pitch_onccN");
                has_note(i, "opcode:lfo1_wave=2");
            },
        },
        Fixture {
            id: "F-SFZ-11",
            sfz: "<group> cutoff=1000 resonance=6 fil_keytrack=100 fil_veltrack=1200 cutoff_oncc74=2400\n\
                  <region> sample=a.wav key=60 fil_type=lpf_1p\n\
                  <region> sample=a.wav key=61 fil_type=hpf_1p\n\
                  <region> sample=a.wav key=62 fil_type=lpf_2p\n\
                  <region> sample=a.wav key=63 fil_type=hpf_2p\n\
                  <region> sample=a.wav key=64 fil_type=bpf_2p\n\
                  <region> sample=a.wav key=65 fil_type=brf_2p\n\
                  <region> sample=a.wav key=66 fil_type=lpf_4p fil_keycenter=72\n\
                  <region> sample=a.wav key=67 fil_type=pkf_2p\n",
            texts: vec![],
            samples: vec![s("a.wav")],
            phrase: (60..68u8)
                .map(|k| (k, (k as u64 - 60) * 3, 2, 0.8))
                .collect(),
            check: |i| {
                let f = |k: u8| {
                    let z = i
                        .instrument
                        .zones
                        .values()
                        .find(|z| z.keys.lo == k)
                        .unwrap();
                    i.instrument.groups[&z.group].filter.clone()
                };
                use FilterKind::*;
                let kinds: Vec<_> = (60..67u8).map(|k| f(k).unwrap().kind).collect();
                assert_eq!(
                    kinds,
                    vec![
                        LowPass12, HighPass12, LowPass12, HighPass12, BandPass, Notch, LowPass24
                    ]
                );
                let lp = f(62).unwrap();
                assert_eq!(
                    (lp.cutoff, lp.resonance, lp.key_track),
                    (1000.0, 6.0, 100.0)
                );
                assert_eq!(
                    f(66).unwrap().cutoff,
                    500.0,
                    "keycenter 72 shifts the cutoff an octave down"
                );
                assert!(f(67).is_none(), "peaking filter bypassed");
                let z = i
                    .instrument
                    .zones
                    .values()
                    .find(|z| z.keys.lo == 62)
                    .unwrap();
                let r = routes_of(i, z.group);
                assert_eq!(
                    (r[0].source.clone(), r[0].target, r[0].depth),
                    (ModSource::Velocity, ModTarget::Cutoff, 1200.0)
                );
                assert_eq!(
                    (r[1].source.clone(), r[1].depth),
                    (ModSource::Cc { controller: 74 }, 2400.0)
                );
                has_note(i, "opcode:fil_type=lpf_1p");
                has_note(i, "opcode:fil_type=pkf_2p");
            },
        },
        Fixture {
            id: "F-SFZ-12",
            sfz: "#define $VEL 40\n#define $VELHI 100\n#include \"inc/defs.sfzh\"\n\
                  <control> default_path=Samples/ octave_offset=1 note_offset=-2\n\
                  <region> sample=a.wav key=c#4 lovel=$VEL hivel=$VELHI\n\
                  <region> sample=b.wav lokey=Eb-1 hikey=G9 pitch_keycenter=$ROOT\n",
            texts: vec![
                (
                    "inc/defs.sfzh",
                    "#define $ROOT 60\n#include \"inc/more.sfzh\"\n",
                ),
                ("inc/more.sfzh", "<region> sample=c.wav key=40\n"),
            ],
            samples: vec![s("Samples/a.wav"), s("Samples/b.wav"), s("c.wav")],
            phrase: vec![(40, 0, 2, 0.8), (71, 4, 2, 0.5), (20, 8, 2, 0.8)],
            check: |i| {
                let a = zone(i, "a.wav");
                assert_eq!(
                    (keys(a), a.velocity.lo, a.velocity.hi),
                    ((71, 71, 71), 40, 100)
                );
                assert_eq!(keys(zone(i, "b.wav")), (13, 127, 70));
                assert_eq!(
                    keys(zone(i, "c.wav")),
                    (40, 40, 40),
                    "before <control>: no offsets"
                );
                let names: Vec<_> = i.samples.iter().map(|s| s.name.as_str()).collect();
                assert_eq!(names, vec!["c.wav", "Samples/a.wav", "Samples/b.wav"]);
                has_note(i, "opcode:hikey");
            },
        },
        Fixture {
            id: "F-SFZ-13",
            sfz: "<group>\n\
                  <region> sample=closed.wav key=42 group=1\n\
                  <region> sample=open.wav key=46 group=2 off_by=1 off_mode=fast\n\
                  <group> key=60 polyphony=2 note_polyphony=1\n\
                  <region> sample=a.wav\n",
            texts: vec![],
            samples: vec![s("closed.wav"), s("open.wav"), s("a.wav")],
            phrase: vec![
                (46, 0, 4, 0.8),
                (42, 2, 1, 0.8),
                (60, 8, 1, 0.8),
                (60, 9, 1, 0.8),
                (60, 10, 1, 0.8),
            ],
            check: |i| {
                assert_eq!(zone(i, "closed.wav").group, zone(i, "open.wav").group);
                assert_eq!(group_of(i, "open.wav").voice_limit, Some(1));
                assert_eq!(group_of(i, "a.wav").voice_limit, Some(2));
                has_note(i, "opcode:off_by");
                has_note(i, "opcode:note_polyphony");
            },
        },
        Fixture {
            id: "F-SFZ-14",
            sfz: "<curve> curve_index=7 v000=0 v063=0.1 v127=1\n\
                  <group> key=60 gain_oncc1=-12 gain_curvecc1=7 amplitude_oncc1=50 amplitude_curvecc1=7\n\
                  <region> sample=a.wav\n\
                  <group> key=62 volume_oncc2=6 volume_curvecc2=5 pan_oncc10=100\n\
                  <region> sample=a.wav\n",
            texts: vec![],
            samples: vec![s("a.wav")],
            phrase: vec![(60, 0, 2, 0.8), (62, 4, 2, 0.8)],
            check: |i| {
                let z = |k: u8| {
                    i.instrument
                        .zones
                        .values()
                        .find(|z| z.keys.lo == k)
                        .unwrap()
                };
                let r = routes_of(i, z(60).group);
                assert_eq!((r[0].depth, r[0].curve), (-12.0, ModCurve::Convex));
                let r = routes_of(i, z(62).group);
                assert_eq!(
                    (r[0].target, r[0].depth, r[0].curve),
                    (ModTarget::Volume, 6.0, ModCurve::Concave)
                );
                assert_eq!((r[1].target, r[1].depth), (ModTarget::Pan, 1.0));
                has_note(i, "opcode:gain_curvecc1");
                has_note(i, "opcode:amplitude_oncc1");
            },
        },
        Fixture {
            id: "F-SFZ-15",
            sfz: "// header comment\r\n<group>\tloopmode=one_shot\tfiltype=hpf_2p cutoff=500 /* inline\r\n block */ polyphony_group=3\r\n\
                  <region> sample=Sub Dir\\My Sample.WAV key=60 volume=99 mystery=1 tune=5.5\r\n\
                  <group>\r\n<region> sample=*sine key=72\r\n<region> sample=*silence key=73\r\n<region> sample=*saw key=74\r\n\
                  <midi> foo=1\r\n<sample> name=x.wav\r\n<region> key=75\r\n<Region> lokey=1\r\n",
            texts: vec![],
            samples: vec![s("sub dir/my sample.wav")],
            phrase: vec![(60, 0, 2, 0.8), (72, 4, 4, 0.8), (73, 8, 2, 0.8)],
            check: |i| {
                assert_eq!(zones(i).len(), 3);
                let m = zone(i, "my sample.wav");
                assert_eq!((m.one_shot, m.volume_db, m.fine), (true, 24.0, 5.0));
                let g = group_of(i, "my sample.wav");
                assert_eq!(g.filter.as_ref().unwrap().kind, FilterKind::HighPass12);
                let sine = zone(i, "generated/sine.wav");
                assert!(sine.looping.is_some());
                assert!(
                    i.samples
                        .iter()
                        .any(|s| s.source == SampleSource::Generated(Generated::Sine))
                );
                has_note(i, "sample:Sub Dir/My Sample.WAV");
                has_note(i, "opcode:volume");
                has_note(i, "sample:*saw");
                has_note(i, "region:no-sample");
                has_note(i, "header:<midi>");
                has_note(i, "header:<sample>");
                has_note(i, "opcode:tune");
                let unknown: Vec<_> = i.report.unknown.iter().map(|u| u.0.as_str()).collect();
                assert_eq!(unknown, vec!["header:<Region>", "opcode:mystery"]);
            },
        },
        Fixture {
            id: "F-SFZ-16",
            sfz: "<effect> type=reverb bus=fx1 reverb_size=50\n\
                  <effect> type=delay\n\
                  <effect> type=lofi bus=main\n\
                  <group> key=60 effect1=30\n<region> sample=a.wav\n\
                  <group> key=62\n<region> sample=a.wav\n",
            texts: vec![],
            samples: vec![s("a.wav")],
            phrase: vec![(60, 0, 2, 0.8), (62, 4, 2, 0.8)],
            check: |i| {
                let fx = &i.instrument.extra["sfz.effects"];
                assert_eq!(fx.lines().count(), 3);
                assert!(fx.contains("reverb_size=50"));
                let z = |k: u8| {
                    i.instrument
                        .zones
                        .values()
                        .find(|z| z.keys.lo == k)
                        .unwrap()
                };
                let e: Vec<_> = i.instrument.groups[&z(60).group]
                    .effects
                    .iter()
                    .map(|e| (e.name.as_str(), e.mix))
                    .collect();
                assert_eq!(e, vec![("reverb", 0.3), ("delay", 1.0)]);
                assert_eq!(i.instrument.groups[&z(62).group].effects.len(), 1);
                has_note(i, "effect:lofi");
                has_note(i, "effect:reverb");
            },
        },
        Fixture {
            id: "F-SFZ-17",
            sfz: "<global> sw_lokey=24 sw_hikey=25 sw_default=24\n\
                  <master> sw_last=24 sw_label=Sustain\n\
                  <group> lokey=48 hikey=72 pitch_keycenter=60 seq_length=2 hivel=80\n\
                  <region> sample=sus_p_rr1.wav seq_position=1\n\
                  <region> sample=sus_p_rr2.wav seq_position=2\n\
                  <group> lokey=48 hikey=72 pitch_keycenter=60 seq_length=2 lovel=81\n\
                  <region> sample=sus_f_rr1.wav seq_position=1\n\
                  <region> sample=sus_f_rr2.wav seq_position=2\n\
                  <group> lokey=48 hikey=72 pitch_keycenter=60 trigger=release rt_decay=3\n\
                  <region> sample=rel.wav\n\
                  <master> sw_last=25 sw_label=Staccato\n\
                  <group> lokey=48 hikey=72 pitch_keycenter=60\n\
                  <region> sample=stac_p.wav hivel=80\n\
                  <region> sample=stac_f.wav lovel=81\n",
            texts: vec![],
            samples: vec![
                s("sus_p_rr1.wav"),
                s("sus_p_rr2.wav"),
                s("sus_f_rr1.wav"),
                s("sus_f_rr2.wav"),
                s("rel.wav"),
                s("stac_p.wav"),
                s("stac_f.wav"),
            ],
            phrase: vec![
                (60, 0, 2, 0.5),
                (64, 2, 2, 0.5),
                (67, 4, 2, 0.9),
                (60, 6, 2, 0.9),
                (25, 8, 1, 0.8),
                (60, 9, 1, 0.5),
                (64, 10, 1, 0.9),
                (24, 12, 1, 0.8),
                (67, 13, 3, 0.7),
            ],
            check: |i| {
                assert_eq!(i.instrument.articulations.len(), 2);
                assert_eq!(
                    i.instrument.settings.default_articulation,
                    Some(ArticulationId(0))
                );
                assert_eq!(zone(i, "sus_p_rr2.wav").round_robin, Some(1));
                assert_eq!(zone(i, "rel.wav").trigger, ZoneTrigger::Release);
                assert_eq!(
                    group_of(i, "rel.wav").articulations,
                    vec![ArticulationId(0)]
                );
                assert_eq!(
                    group_of(i, "stac_f.wav").articulations,
                    vec![ArticulationId(1)]
                );
                assert_eq!(zones(i).len(), 7);
                assert!(i.report.unknown.is_empty(), "{:?}", i.report.unknown);
            },
        },
    ]
}

// ------------------------------------------------------------------------- rendering

fn write_fixture(f: &Fixture, root: &Path) -> PathBuf {
    let lib = root.join("lib");
    for (i, (p, frames, smpl)) in f.samples.iter().enumerate() {
        let path = lib.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let hz = 220.0 * (1.0 + i as f64 * 0.25);
        std::fs::write(&path, wav(&tagged(i as u32 + 1, hz, *frames), *smpl)).unwrap();
    }
    for (p, text) in &f.texts {
        let path = lib.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let main = lib.join(format!("{}.sfz", f.id));
    std::fs::write(&main, f.sfz).unwrap();
    main
}

/// Places the samples of an import under `dir` the way `sampler.import` links them: file
/// samples stay where they are when `dir` is their folder, generated ones are written.
fn folder_of(imported: &Imported) -> PathBuf {
    for s in &imported.samples {
        if let SampleSource::File(p) = &s.source {
            let mut f = p.clone();
            for _ in s.name.split('/') {
                f.pop();
            }
            return f;
        }
    }
    panic!("no file samples")
}

fn write_generated(imported: &Imported, dir: &Path) {
    for s in &imported.samples {
        if let SampleSource::Generated(g) = s.source {
            let path = dir.join(&s.name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            ph_formats::encode::write_wav(
                &path,
                &[g.render()],
                Generated::RATE,
                ph_formats::encode::WavFormat::Float32,
            )
            .unwrap();
        }
    }
}

/// A project playing `inst` (folder relative to `project_dir`) with the phrase.
fn project(inst: &HausInstrument, phrase: &[(u8, u64, u64, f32)]) -> Project {
    let mut e = Edit::new(&Project::empty("sfz"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let i = e.add_instrument(inst.clone()).unwrap();
    let ch = e
        .add_channel(
            "SFZ",
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

fn render(p: &Project, dir: &Path) -> AudioBuffer {
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

fn fingerprint(a: &AudioBuffer) -> Value {
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

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/sfz")
}

fn compare(id: &str, got: &Value) -> Result<(), String> {
    let path = reference_dir().join(format!("{id}.json"));
    if std::env::var_os("PH_GOLDEN_UPDATE").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(reference_dir()).unwrap();
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

fn run_fixture(f: &Fixture) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    let main = write_fixture(f, tmp.path());
    let imported = Sfz
        .import(ImportInput::Path(&main))
        .map_err(|e| e.to_string())?;
    (f.check)(&imported);
    // every zone plays a listed sample, and the instrument passes the model's checks
    for z in imported.instrument.zones.values() {
        validate_zone(z).map_err(|e| e.to_string())?;
        let SampleRef::File(n) = &z.sample else {
            panic!()
        };
        assert!(
            imported.samples.iter().any(|s| &s.name == n),
            "{n} unlisted"
        );
    }
    for g in imported.instrument.groups.values() {
        validate_group(g).map_err(|e| e.to_string())?;
    }
    validate_instrument_header(&imported.instrument).map_err(|e| e.to_string())?;

    let folder = folder_of(&imported);
    write_generated(&imported, &folder);
    let mut inst = imported.instrument.clone();
    let rel = folder
        .strip_prefix(tmp.path())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    inst.folder = Some(rel);
    let p = project(&inst, &f.phrase);
    let a = render(&p, tmp.path());
    let b = render(&p, tmp.path());
    assert!(
        a.channels == b.channels,
        "{}: render is not deterministic",
        f.id
    );
    let peak = a
        .channels
        .iter()
        .flatten()
        .fold(0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "{}: the render is silent", f.id);
    compare(f.id, &fingerprint(&a))?;

    let back = save_and_reopen(&inst, &folder, tmp.path());
    let p2 = project(&back, &f.phrase);
    let c = render(&p2, tmp.path());
    if a.channels != c.channels {
        return Err(format!(
            "{}: the native round trip renders differently",
            f.id
        ));
    }
    Ok(())
}

#[test]
fn sfz_fixtures_import_render_and_round_trip() {
    let mut failures = Vec::new();
    for f in fixtures() {
        if let Err(e) = run_fixture(&f) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn malformed_fixtures_never_panic() {
    for m in MALFORMED {
        let _ = Sfz.import(ImportInput::Bytes {
            name: "bad.sfz",
            bytes: m.as_bytes(),
        });
    }
    // a malformed file with a real folder (samples exist): still a valid instrument
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.wav"), wav(&tagged(1, 220.0, 4000), None)).unwrap();
    for m in MALFORMED {
        let p = tmp.path().join("bad.sfz");
        std::fs::write(&p, m).unwrap();
        if let Ok(out) = Sfz.import(ImportInput::Path(&p)) {
            for z in out.instrument.zones.values() {
                validate_zone(z).unwrap();
            }
        }
    }
}
