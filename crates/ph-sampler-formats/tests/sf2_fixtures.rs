//! SF2/SF3 fixtures F-SF2-01 … F-SF2-12, F-SF3-01, F-SF3-02 (`docs/formats/sf2.md` §12,
//! rules in `docs/formats/sampler-fixtures.md`). Every file is built here with `Sf2Builder`
//! from tagged tones; SF3 streams come from the fixture Vorbis writer
//! (`ph_sampler_formats::build::vorbis`). Each fixture is imported and its model checked,
//! then rendered against `tests/golden/sf2/<id>.json` and round-tripped through the native
//! format (`common::render_and_round_trip`).

mod common;
mod formats_sound;
mod sf2_seeds;

use common::*;
use ph_core::*;
use ph_sampler_formats::build::{Sf2Builder, Sf2Sample, Sf2Zone, tagged_tone, vorbis};
use ph_sampler_formats::sf2::{PresetChoice, import_preset, list_presets};
use ph_sampler_formats::{ImportError, ImportInput, Imported, Importer, SampleSource, SoundFont};

// generator ids (sf2.md §5)
const START: u16 = 0;
const END: u16 = 1;
const LOOP_START: u16 = 2;
const LOOP_END: u16 = 3;
const START_COARSE: u16 = 4;
const MOD_LFO_PITCH: u16 = 5;
const VIB_LFO_PITCH: u16 = 6;
const MOD_ENV_PITCH: u16 = 7;
const FC: u16 = 8;
const Q: u16 = 9;
const MOD_LFO_FC: u16 = 10;
const MOD_ENV_FC: u16 = 11;
const MOD_LFO_VOL: u16 = 13;
const PAN: u16 = 17;
const FREQ_MOD_LFO: u16 = 22;
const FREQ_VIB_LFO: u16 = 24;
const ATTACK_MOD: u16 = 26;
const DECAY_MOD: u16 = 28;
const SUSTAIN_MOD: u16 = 29;
const DELAY_VOL: u16 = 33;
const ATTACK_VOL: u16 = 34;
const HOLD_VOL: u16 = 35;
const DECAY_VOL: u16 = 36;
const SUSTAIN_VOL: u16 = 37;
const RELEASE_VOL: u16 = 38;
const KEY_TO_VOL_DECAY: u16 = 40;
const KEYNUM: u16 = 46;
const VELOCITY: u16 = 47;
const ATTEN: u16 = 48;
const END_COARSE: u16 = 12;
const COARSE: u16 = 51;
const FINE: u16 = 52;
const SAMPLE_MODES: u16 = 54;
const SCALE: u16 = 56;
const EXCLUSIVE: u16 = 57;
const ROOT_KEY: u16 = 58;

const RATE: u32 = 44_100;

fn tone(id: u32, frames: usize) -> Sf2Sample {
    let hz = 220.0 * (1.0 + id as f64 * 0.25);
    Sf2Sample::from_f32(
        &format!("tone{id}"),
        &tagged_tone(id, hz, frames, RATE),
        RATE,
        60,
    )
}

/// One preset over one instrument.
fn simple(b: &mut Sf2Builder, name: &str, bank: u16, zones: Vec<Sf2Zone>) {
    let i = b.add_instrument(name, zones);
    b.add_preset(name, bank, 0, vec![Sf2Zone::new().instrument(i)]);
}

fn import(bytes: &[u8]) -> Imported {
    SoundFont
        .import(ImportInput::Bytes {
            name: "fixture.sf2",
            bytes,
        })
        .unwrap_or_else(|e| panic!("{e}"))
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

fn routes(i: &Imported, z: &Zone) -> Vec<ModRoute> {
    i.instrument
        .modulation
        .iter()
        .filter(|r| r.group == Some(z.group))
        .cloned()
        .collect()
}

fn audio(i: &Imported, z: &Zone) -> std::sync::Arc<ph_node::SampleData> {
    let SampleRef::File(n) = &z.sample else {
        panic!()
    };
    match &i.samples.iter().find(|s| &s.name == n).unwrap().source {
        SampleSource::Audio(a) => a.clone(),
        s => panic!("{s:?}"),
    }
}

fn close(a: f32, b: f64) -> bool {
    (a as f64 - b).abs() < 1e-3 * b.abs().max(1.0)
}

struct Fixture {
    id: &'static str,
    bytes: Vec<u8>,
    phrase: Phrase,
    check: fn(&Imported),
}

fn f01() -> Fixture {
    let mut b = Sf2Builder::new();
    b.add_sample(tone(1, 12_000));
    simple(&mut b, "Minimal", 0, vec![Sf2Zone::new().sample(0)]);
    Fixture {
        id: "F-SF2-01",
        bytes: b.build(),
        phrase: vec![(60, 0, 4, 0.8), (67, 8, 4, 0.8)],
        check: |i| {
            assert_eq!(i.report.format, "sf2");
            assert_eq!(i.instrument.name, "Minimal");
            assert_eq!(i.instrument.zones.len(), 1);
            let z = zone_at(i, 60, 100);
            assert_eq!(
                (z.keys, z.velocity, z.root_key),
                (KeyRange::ALL, VelRange::ALL, 60)
            );
            assert_eq!((z.tune, z.fine, z.volume_db, z.pan), (0, 0.0, 0.0, 0.0));
            assert_eq!((z.start, z.end, z.looping.is_none()), (0, None, true));
            let a = audio(i, z);
            assert_eq!(
                (a.frames(), a.sample_rate, a.channels.len()),
                (12_000, 44_100.0, 1)
            );
            assert_eq!(i.report.presets.len(), 1);
            assert!(i.report.presets[0].imported);
            assert_eq!(i.instrument.extra["sf2.info.INAM"], "Test bank");
            assert!(group(i, z).filter.is_none(), "an open filter is off");
            assert_eq!(i.instrument.settings.bend_up, 2.0);
            assert!(i.report.unknown.is_empty(), "{:?}", i.report.unknown);
            has_note(i, "modulator:velocity-to-volume");
        },
    }
}

/// The acceptance-test kit (criterion 1): bank 128, single-key zones, scaleTuning 0, a
/// hi-hat choke.
pub fn drum_kit() -> Sf2Builder {
    let mut b = Sf2Builder::new();
    let keys = [36u8, 38, 39, 42, 44, 46, 49, 51];
    let mut zones = vec![Sf2Zone::new().set(SCALE, 0)]; // global zone
    for (n, k) in keys.iter().enumerate() {
        let s = b.add_sample(tone(n as u32 + 1, 9_000));
        let mut z = Sf2Zone::new().keys(*k, *k);
        if matches!(k, 42 | 44 | 46) {
            z = z.set(EXCLUSIVE, 1);
        }
        zones.push(z.sample(s));
    }
    let i = b.add_instrument("Haus Kit", zones);
    b.add_preset("Haus Kit", 128, 0, vec![Sf2Zone::new().instrument(i)]);
    b
}

fn f02() -> Fixture {
    Fixture {
        id: "F-SF2-02",
        bytes: drum_kit().build(),
        phrase: vec![
            (36, 0, 2, 1.0),
            (42, 2, 1, 0.7),
            (46, 4, 2, 0.7),
            (42, 5, 1, 0.7),
            (38, 8, 2, 0.9),
            (44, 10, 1, 0.6),
            (49, 12, 4, 0.8),
            (51, 16, 2, 0.8),
            (39, 20, 2, 0.8),
        ],
        check: |i| {
            assert_eq!(i.instrument.zones.len(), 8);
            assert_eq!(i.report.presets[0].bank, 128);
            for k in [36u8, 38, 39, 42, 44, 46, 49, 51] {
                let z = zone_at(i, k, 100);
                assert_eq!((z.keys.lo, z.keys.hi, z.root_key, z.tune), (k, k, 60, 0));
                // scaleTuning 0: the key does not change the pitch
                let r = routes(i, z);
                assert_eq!(r.len(), 1);
                assert_eq!(
                    (r[0].source.clone(), r[0].target, r[0].depth),
                    (ModSource::Key, ModTarget::Pitch, -6400.0)
                );
            }
            let hat = group(i, zone_at(i, 42, 100));
            assert_eq!(hat.voice_limit, Some(1));
            assert_eq!(hat.extra["sf2.exclusive_class"], "1");
            assert_eq!(zone_at(i, 46, 100).group, zone_at(i, 42, 100).group);
            let kick = group(i, zone_at(i, 36, 100));
            assert_eq!(kick.voice_limit, None);
            assert_eq!(i.instrument.groups.len(), 2);
        },
    }
}

fn f03() -> Fixture {
    let mut b = Sf2Builder::new();
    let s0 = b.add_sample(tone(1, 12_000));
    let s1 = b.add_sample(tone(2, 12_000));
    let i = b.add_instrument(
        "Layered",
        vec![
            Sf2Zone::new()
                .keys(0, 100)
                .set(ATTACK_VOL, 1200)
                .set(PAN, 100),
            Sf2Zone::new().keys(30, 59).sample(s0),
            Sf2Zone::new().set(PAN, -200).sample(s1),
        ],
    );
    b.add_preset(
        "Layered",
        0,
        0,
        vec![
            Sf2Zone::new().set(ATTACK_VOL, 2400).set(PAN, 50),
            Sf2Zone::new()
                .keys(40, 80)
                .vels(10, 127)
                .set(PAN, 100)
                .instrument(i),
            Sf2Zone::new().keys(110, 120).instrument(i),
        ],
    );
    Fixture {
        id: "F-SF2-03",
        bytes: b.build(),
        phrase: vec![(45, 0, 4, 0.8), (70, 8, 4, 0.8), (115, 12, 2, 0.8)],
        check: |i| {
            // the second preset zone meets no instrument zone
            assert_eq!(i.instrument.zones.len(), 2);
            let z1 = zone_at(i, 45, 100);
            // both instrument zones play at 45 (ranges 30–59 and 0–100, cut to 40–80)
            let at45: Vec<&Zone> = i
                .instrument
                .zones
                .values()
                .filter(|z| z.maps(45, 100))
                .collect();
            assert_eq!(at45.len(), 2);
            let low = at45.iter().find(|z| z.keys.hi == 59).unwrap();
            assert_eq!((low.keys.lo, low.velocity.lo), (40, 10));
            assert!(
                close(low.pan, 0.4),
                "instrument global 100 + preset local 100"
            );
            let other = at45.iter().find(|z| z.keys.hi == 80).unwrap();
            assert!(close(other.pan, -0.2), "local −200 + preset 100");
            assert!(
                close(group(i, z1).amp_env.attack, 8.0),
                "1200 + 2400 tc = 8 s"
            );
            assert!(
                !i.instrument
                    .zones
                    .values()
                    .any(|z| z.maps(30, 100) || z.maps(45, 5))
            );
            // zones above 100: the instrument's global key range ends there
            assert!(i.instrument.zones.values().all(|z| z.keys.hi <= 100));
        },
    }
}

fn f04() -> Fixture {
    let mut b = Sf2Builder::new();
    let s = b.add_sample(tone(1, 12_000));
    let i = b.add_instrument(
        "Layers",
        vec![
            // disables default modulator 1 (velocity → attenuation)
            Sf2Zone::new().modulator(0x0502, ATTEN, 0, 0, 0),
            Sf2Zone::new().vels(0, 40).sample(s),
            Sf2Zone::new().vels(41, 90).set(ATTEN, 240).sample(s),
            Sf2Zone::new().vels(91, 127).set(ATTEN, 960).sample(s),
        ],
    );
    b.add_preset("Layers", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-04",
        bytes: b.build(),
        phrase: vec![(60, 0, 4, 0.2), (60, 6, 4, 0.5), (60, 12, 4, 0.95)],
        check: |i| {
            assert_eq!(zone_at(i, 60, 20).volume_db, 0.0);
            assert!(close(zone_at(i, 60, 60).volume_db, -9.6), "240 cB at 0.4×");
            assert!(
                close(zone_at(i, 60, 120).volume_db, -38.4),
                "960 cB at 0.4×"
            );
            lacks_note(i, "modulator:velocity-to-volume");
            assert_eq!(i.samples.len(), 1, "one sample, shared");
        },
    }
}

fn f05() -> Fixture {
    let mut b = Sf2Builder::new();
    let mut a = tone(1, 12_000);
    a.loop_points = (2_000, 8_000);
    let a = b.add_sample(a);
    let mut long = tone(2, 70_000);
    long.loop_points = (35_000, 69_000);
    let long = b.add_sample(long);
    let mut edge = tone(3, 6_000);
    edge.loop_points = (0, 6_000);
    let edge = b.add_sample(edge);
    let i = b.add_instrument(
        "Loops",
        vec![
            Sf2Zone::new().keys(60, 60).sample(a),
            Sf2Zone::new()
                .keys(62, 62)
                .set(SAMPLE_MODES, 1)
                .set(START, 100)
                .set(END, -200)
                .set(LOOP_START, 10)
                .set(LOOP_END, -10)
                .sample(a),
            Sf2Zone::new()
                .keys(64, 64)
                .set(SAMPLE_MODES, 3)
                .set(START_COARSE, 1)
                .set(END_COARSE, -1)
                .sample(long),
            Sf2Zone::new()
                .keys(65, 65)
                .set(SAMPLE_MODES, 1)
                .sample(edge),
            Sf2Zone::new().keys(67, 67).set(SAMPLE_MODES, 2).sample(a),
        ],
    );
    b.add_preset("Loops", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-05",
        bytes: b.build(),
        phrase: vec![
            (60, 0, 3, 0.8),
            (62, 4, 6, 0.8),
            (64, 12, 6, 0.8),
            (65, 20, 4, 0.8),
            (67, 26, 3, 0.8),
        ],
        check: |i| {
            assert!(zone_at(i, 60, 100).looping.is_none(), "mode 0");
            let z = zone_at(i, 62, 100);
            assert_eq!((z.start, z.end), (100, Some(11_800)));
            let l = z.looping.as_ref().unwrap();
            assert_eq!(
                (l.start, l.end, l.on_release),
                (2_010, 7_990, LoopRelease::Continue)
            );
            // coarse offsets; the loop end is cut to the played range
            let z = zone_at(i, 64, 100);
            assert_eq!((z.start, z.end), (32_768, Some(70_000 - 32_768)));
            let l = z.looping.as_ref().unwrap();
            assert_eq!(
                (l.start, l.end, l.on_release),
                (35_000, 70_000 - 32_768, LoopRelease::Exit)
            );
            let z = zone_at(i, 65, 100);
            let l = z.looping.as_ref().unwrap();
            assert_eq!((l.start, l.end), (0, 6_000));
            assert!(zone_at(i, 67, 100).looping.is_none(), "mode 2 plays once");
        },
    }
}

fn f06() -> Fixture {
    let mut b = Sf2Builder::new();
    let plain = b.add_sample(tone(1, 12_000));
    let mut unpitched = tone(2, 12_000);
    unpitched.pitch = 255;
    let unpitched = b.add_sample(unpitched);
    let mut corrected = tone(3, 12_000);
    corrected.correction = -4;
    let corrected = b.add_sample(corrected);
    let i = b.add_instrument(
        "Pitch",
        vec![
            Sf2Zone::new().keys(48, 50).set(ROOT_KEY, 64).sample(plain),
            Sf2Zone::new().keys(52, 54).sample(unpitched),
            Sf2Zone::new().keys(55, 57).sample(corrected),
            Sf2Zone::new()
                .keys(58, 60)
                .set(COARSE, 2)
                .set(FINE, -30)
                .sample(plain),
            Sf2Zone::new().keys(62, 64).set(KEYNUM, 72).sample(plain),
            Sf2Zone::new().keys(65, 67).set(VELOCITY, 100).sample(plain),
            Sf2Zone::new().keys(69, 71).set(SCALE, 50).sample(plain),
        ],
    );
    b.add_preset("Pitch", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-06",
        bytes: b.build(),
        phrase: vec![
            (49, 0, 2, 0.8),
            (53, 3, 2, 0.8),
            (56, 6, 2, 0.8),
            (59, 9, 2, 0.8),
            (63, 12, 2, 0.8),
            (66, 15, 2, 0.8),
            (70, 18, 2, 0.8),
        ],
        check: |i| {
            assert_eq!(zone_at(i, 49, 100).root_key, 64);
            assert_eq!(zone_at(i, 53, 100).root_key, 60, "unpitched → 60");
            let z = zone_at(i, 56, 100);
            assert_eq!((z.root_key, z.tune, z.fine), (60, 0, -4.0));
            let z = zone_at(i, 59, 100);
            assert_eq!((z.tune, z.fine), (2, -30.0));
            let z = zone_at(i, 63, 100);
            assert_eq!((z.root_key, z.tune), (60, 12), "keynum 72 from root 60");
            assert_eq!(routes(i, z)[0].depth, -6400.0);
            has_note(i, "generator:velocity");
            // scaleTuning 50 from root 60: key 70 is 5 semitones up
            let z = zone_at(i, 70, 100);
            let r = routes(i, z);
            assert_eq!((r[0].source.clone(), r[0].depth), (ModSource::Key, -3200.0));
            let cents = |k: f64| {
                (k - z.root_key as f64) * 100.0
                    + z.tune as f64 * 100.0
                    + z.fine as f64
                    + r[0].depth as f64 * (k - 60.0) / 64.0
            };
            assert!((cents(70.0) - 500.0).abs() < 1e-3, "{}", cents(70.0));
            assert!(cents(60.0).abs() < 1e-3);
        },
    }
}

fn f07() -> Fixture {
    let mut b = Sf2Builder::new();
    // long enough to sound past the 0.5 s delay and through the release
    let s = b.add_sample(tone(1, 120_000));
    let i = b.add_instrument(
        "Envelopes",
        vec![
            Sf2Zone::new()
                .keys(48, 72)
                .set(DELAY_VOL, -1200)
                .set(ATTACK_VOL, -2400)
                .set(HOLD_VOL, -3600)
                .set(DECAY_VOL, 0)
                .set(SUSTAIN_VOL, 200)
                .set(RELEASE_VOL, -1200)
                .set(KEY_TO_VOL_DECAY, 100)
                .set(ATTACK_MOD, -1200)
                .set(DECAY_MOD, -1200)
                .set(SUSTAIN_MOD, 500)
                .set(MOD_ENV_PITCH, 200)
                .sample(s),
            Sf2Zone::new()
                .keys(73, 80)
                .set(ATTACK_VOL, -32768)
                .sample(s),
        ],
    );
    b.add_preset("Envelopes", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-07",
        bytes: b.build(),
        phrase: vec![(60, 0, 8, 0.8), (75, 12, 4, 0.8)],
        check: |i| {
            let z = zone_at(i, 60, 100);
            let g = group(i, z);
            let e = &g.amp_env;
            assert!(close(e.delay, 0.5) && close(e.attack, 0.25) && close(e.hold, 0.125));
            // decay 0 tc + 100 tc/key × (60 − centre 60) = 1 s
            assert!(close(e.decay, 1.0), "{}", e.decay);
            assert!(close(e.sustain, 10f64.powf(-1.0)), "200 cB = 20 dB down");
            assert!(close(e.release, 0.5));
            let m = g.filter_env.as_ref().unwrap();
            assert!(close(m.attack, 0.5) && close(m.sustain, 0.5));
            let r = routes(i, z);
            assert!(
                r.iter()
                    .any(|r| r.source == ModSource::Envelope { index: 1 }
                        && r.target == ModTarget::Pitch
                        && r.depth == 200.0)
            );
            assert!(g.filter.is_none(), "mod env to pitch only: no filter");
            has_note(i, "generator:keynumToEnvelope");
            assert_eq!(
                group(i, zone_at(i, 75, 100)).amp_env.attack,
                0.0,
                "−32768 tc = 0 s"
            );
        },
    }
}

fn f08() -> Fixture {
    let mut b = Sf2Builder::new();
    let s = b.add_sample(tone(1, 20_000));
    let i = b.add_instrument(
        "LfoFilter",
        vec![
            Sf2Zone::new()
                .keys(48, 66)
                .set(FC, 9000)
                .set(Q, 100)
                .set(MOD_LFO_PITCH, 50)
                .set(MOD_LFO_FC, 1200)
                .set(MOD_LFO_VOL, 60)
                .set(FREQ_MOD_LFO, 1200)
                .set(VIB_LFO_PITCH, 30)
                .set(FREQ_VIB_LFO, -1200)
                .set(MOD_ENV_FC, 2400)
                .sample(s),
            Sf2Zone::new().keys(67, 80).set(FC, 13500).sample(s),
        ],
    );
    b.add_preset("LfoFilter", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-08",
        bytes: b.build(),
        phrase: vec![(60, 0, 8, 0.9), (72, 10, 6, 0.9)],
        check: |i| {
            let z = zone_at(i, 60, 100);
            let g = group(i, z);
            let f = g.filter.as_ref().unwrap();
            assert_eq!(f.kind, FilterKind::LowPass12);
            // 9000 cents with default modulator 2's −2400 at velocity 0, routed back up
            assert!(
                close(f.cutoff, 8.176 * 2f64.powf(6600.0 / 1200.0)),
                "{}",
                f.cutoff
            );
            assert!(close(f.resonance, 10.0) && f.env_depth == 2400.0);
            assert_eq!(g.lfos.len(), 2);
            assert!(close(g.lfos[0].rate, 8.176 * 2.0) && close(g.lfos[1].rate, 4.088));
            assert_eq!(g.lfos[0].shape, LfoShape::Triangle);
            let r = routes(i, z);
            let has = |s: ModSource, t: ModTarget, d: f32| {
                assert!(
                    r.iter()
                        .any(|x| x.source == s && x.target == t && close(x.depth, d as f64)),
                    "{s:?} {t:?} {d}: {r:?}"
                );
            };
            has(ModSource::Lfo { index: 0 }, ModTarget::Pitch, 50.0);
            has(ModSource::Lfo { index: 0 }, ModTarget::Cutoff, 1200.0);
            has(ModSource::Lfo { index: 0 }, ModTarget::Volume, -6.0);
            has(ModSource::Lfo { index: 1 }, ModTarget::Pitch, 30.0);
            has(ModSource::Velocity, ModTarget::Cutoff, 2400.0);
            assert!(group(i, zone_at(i, 72, 100)).filter.is_none());
        },
    }
}

fn f09() -> Fixture {
    let mut b = Sf2Builder::new();
    let s = b.add_sample(tone(1, 20_000));
    let cc74 = 0x0080 | 74;
    let zone = |lo: u8, hi: u8| {
        Sf2Zone::new()
            .keys(lo, hi)
            .set(VIB_LFO_PITCH, 20)
            .set(FC, 10_000)
    };
    let i = b.add_instrument(
        "Modulators",
        vec![
            zone(48, 50).modulator(cc74, FC, 2400, 0, 0).sample(s),
            zone(51, 53)
                .modulator(cc74 | 0x0400, FC, 2400, 0, 0)
                .sample(s),
            zone(54, 56)
                .modulator(cc74 | 0x0800, FC, 2400, 0, 0)
                .sample(s),
            zone(57, 59)
                .modulator(cc74 | 0x0C00, FC, 2400, 0, 0)
                .sample(s),
            // bipolar, max → min, linear on pan
            zone(60, 62)
                .modulator(cc74 | 0x0300, PAN, 500, 0, 0)
                .sample(s),
            // amount source (skipped), absolute value on a unipolar source, a link, CC 0
            zone(63, 65)
                .modulator(cc74, ATTEN, 100, 0x0081, 0)
                .modulator(0x0002, ATTEN, -200, 0, 2)
                .modulator(0x0081, 0x8000, 100, 0, 0)
                .modulator(0x0080, FC, 100, 0, 0)
                .sample(s),
        ],
    );
    b.add_preset("Modulators", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-09",
        bytes: b.build(),
        phrase: vec![
            (49, 0, 3, 0.8),
            (52, 4, 3, 0.8),
            (55, 8, 3, 0.8),
            (58, 12, 3, 0.8),
            (61, 16, 3, 0.8),
            (64, 20, 3, 0.8),
        ],
        check: |i| {
            let cc = |key: u8| -> ModRoute {
                let z = zone_at(i, key, 100);
                routes(i, z)
                    .into_iter()
                    .find(|r| r.source == ModSource::Cc { controller: 74 })
                    .unwrap_or_else(|| panic!("no CC74 route at {key}"))
            };
            assert_eq!(
                (cc(49).curve, cc(49).depth, cc(49).target),
                (ModCurve::Linear, 2400.0, ModTarget::Cutoff)
            );
            assert_eq!(cc(52).curve, ModCurve::Concave);
            assert_eq!(cc(55).curve, ModCurve::Convex);
            assert_eq!(cc(58).curve, ModCurve::Switch);
            // bipolar max→min: 500 × (2(1 − x) − 1) = 500 − 1000x: pan +1 base, route −2
            let r = cc(61);
            assert_eq!((r.target, r.depth), (ModTarget::Pan, -2.0));
            assert!(close(zone_at(i, 61, 100).pan, 1.0));
            // |−200 × velocity| on attenuation = +200 cB → −20 dB at full velocity
            let z = zone_at(i, 64, 100);
            assert!(routes(i, z).iter().any(|r| r.source == ModSource::Velocity
                && r.target == ModTarget::Volume
                && close(r.depth, -20.0)));
            assert!(
                !routes(i, z)
                    .iter()
                    .any(|r| r.source == ModSource::Cc { controller: 74 }),
                "amount source skipped"
            );
            has_note(i, "modulator:curve");
            has_note(i, "modulator:vibrato-depth");
            assert!(
                i.report
                    .notes
                    .iter()
                    .any(|n| n.detail.contains("amount sources"))
            );
            assert!(
                i.report
                    .notes
                    .iter()
                    .any(|n| n.detail.contains("linked modulators"))
            );
            assert!(
                i.report
                    .notes
                    .iter()
                    .any(|n| n.detail.contains("illegal MIDI controller"))
            );
        },
    }
}

fn f10() -> Fixture {
    let mut b = Sf2Builder::new();
    let mut l = tone(1, 12_000);
    l.name = "Pad L".into();
    l.kind = 4;
    l.link = 1;
    let mut r = tone(2, 10_000);
    r.name = "Pad R".into();
    r.kind = 2;
    r.link = 0;
    r.loop_points = (1_000, 9_000);
    let mut lone = tone(3, 12_000);
    lone.kind = 2;
    lone.link = 0;
    b.add_sample(l);
    b.add_sample(r);
    b.add_sample(lone);
    let i = b.add_instrument(
        "Stereo",
        vec![
            Sf2Zone::new().keys(60, 70).set(PAN, -500).sample(0),
            Sf2Zone::new()
                .keys(60, 70)
                .set(PAN, 500)
                .set(SAMPLE_MODES, 1)
                .sample(1),
            Sf2Zone::new().keys(71, 80).sample(2),
        ],
    );
    b.add_preset("Stereo", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    Fixture {
        id: "F-SF2-10",
        bytes: b.build(),
        phrase: vec![(62, 0, 6, 0.8), (75, 8, 6, 0.8)],
        check: |i| {
            assert_eq!(i.instrument.zones.len(), 2);
            let z = zone_at(i, 62, 100);
            let a = audio(i, z);
            assert_eq!((a.channels.len(), a.frames()), (2, 12_000));
            assert_eq!(z.pan, 0.0);
            let l = z.looping.as_ref().unwrap();
            assert_eq!(
                (l.start, l.end),
                (1_000, 9_000),
                "loop from the right sample"
            );
            has_note(i, "stereo:join");
            assert_eq!(audio(i, zone_at(i, 75, 100)).channels.len(), 1);
            has_note(i, "sample:link");
        },
    }
}

fn tone24() -> Sf2Sample {
    let mut s = tone(1, 12_000);
    for (n, v) in s.pcm.iter_mut().enumerate() {
        *v |= (n % 251) as i32 & 0xFF;
    }
    s
}

fn f11(version: (u16, u16), damage: bool) -> Vec<u8> {
    let mut b = Sf2Builder::new();
    b.version = version;
    b.sm24 = true;
    b.add_sample(tone24());
    simple(&mut b, "Deep", 0, vec![Sf2Zone::new().sample(0)]);
    let mut c = b.chunks();
    if damage {
        c.sm24.as_mut().unwrap().truncate(100);
    }
    c.bytes()
}

fn f11_fixture() -> Fixture {
    Fixture {
        id: "F-SF2-11",
        bytes: f11((2, 4), false),
        phrase: vec![(60, 0, 4, 0.8)],
        check: |i| {
            let a = audio(i, zone_at(i, 60, 100));
            let want = tone24();
            for n in [0usize, 7, 480, 5000, 11_999] {
                assert_eq!(
                    a.channels[0][n],
                    want.pcm[n] as f32 / 8_388_608.0,
                    "frame {n}"
                );
            }
            lacks_note(i, "chunk:sm24");
            for (v, damage) in [((2, 1), false), ((2, 4), true)] {
                let i = import(&f11(v, damage));
                has_note(&i, "chunk:sm24");
                let a = audio(&i, zone_at(&i, 60, 100));
                assert_eq!(a.channels[0][5000], (want.pcm[5000] >> 8) as f32 / 32_768.0);
            }
        },
    }
}

fn f12() -> Fixture {
    Fixture {
        id: "F-SF2-12",
        bytes: sf2_seeds::accepted_oddities(),
        phrase: vec![(60, 0, 4, 0.8), (72, 6, 4, 0.8)],
        check: |i| {
            // keyRange not first: ignored, so the zone covers every key
            has_note(i, "generator:keyRange");
            has_note(i, "zone:instrument");
            assert!(i.report.unknown.iter().any(|(w, _)| w == "generator:70"));
            assert!(
                i.report
                    .notes
                    .iter()
                    .any(|n| n.detail.contains("needs the sound ROM"))
            );
            has_note(i, "preset:duplicate");
            assert_eq!(i.instrument.zones.len(), 1);
            assert_eq!(i.report.presets.len(), 2);
            assert_eq!(i.report.presets.iter().filter(|p| p.imported).count(), 1);
        },
    }
}

/// An SF3 stream with an odd total `smpl` length.
fn sf3_streams() -> (Vec<u8>, Vec<u8>) {
    let a = vorbis::tone_stream(RATE, 1, 120, 6);
    let mut blocks = 80;
    loop {
        let b = vorbis::tone_stream(RATE, 1, blocks, 9);
        let pcm_bytes = 2 * (2 * (12_000 + 46));
        if (pcm_bytes + a.len() + b.len()) % 2 == 1 {
            return (a, b);
        }
        blocks += 1;
    }
}

fn f3_01() -> Fixture {
    let mut b = Sf2Builder::new();
    b.version = (3, 1);
    b.pad_smpl = false;
    b.add_sample(tone(1, 12_000));
    b.add_sample(tone(2, 12_000));
    let (sa, sb) = sf3_streams();
    let mut va = Sf2Sample::compressed("vorbis a", sa, RATE, 60);
    va.loop_points = (2_048, 8_192);
    b.add_sample(va);
    b.add_sample(Sf2Sample::compressed("vorbis b", sb, RATE, 72));
    simple(
        &mut b,
        "Compressed",
        0,
        vec![
            Sf2Zone::new().keys(0, 47).sample(0),
            Sf2Zone::new().keys(48, 59).sample(1),
            Sf2Zone::new()
                .keys(60, 71)
                .set(SAMPLE_MODES, 1)
                .set(START, 64)
                .sample(2),
            Sf2Zone::new().keys(72, 127).sample(3),
        ],
    );
    Fixture {
        id: "F-SF3-01",
        bytes: b.build(),
        phrase: vec![
            (40, 0, 3, 0.8),
            (50, 4, 3, 0.8),
            (60, 8, 6, 0.8),
            (76, 16, 4, 0.8),
        ],
        check: |i| {
            assert_eq!(i.report.format, "sf3");
            assert_eq!(i.instrument.zones.len(), 4);
            let z = zone_at(i, 60, 100);
            let a = audio(i, z);
            assert_eq!(a.channels.len(), 1);
            assert!(
                a.frames() > 119 * 128 - 256 && a.frames() <= 120 * 128,
                "{}",
                a.frames()
            );
            let l = z.looping.as_ref().unwrap();
            assert_eq!(
                (z.start, l.start, l.end),
                (64, 2_048, 8_192),
                "loops relative to the decoded sample"
            );
            assert_eq!(zone_at(i, 76, 100).root_key, 72);
            assert_eq!(audio(i, zone_at(i, 50, 100)).frames(), 12_000);
            assert!(
                i.report.notes.iter().all(|n| n.what != "sample"),
                "{:?}",
                i.report.notes
            );
        },
    }
}

fn f3_02() -> Fixture {
    let mut b = Sf2Builder::new();
    b.version = (3, 1);
    b.add_sample(tone(1, 12_000));
    b.add_sample(Sf2Sample::compressed(
        "stereo",
        vorbis::tone_stream(RATE, 2, 20, 6),
        RATE,
        60,
    ));
    b.add_sample(Sf2Sample::compressed("garbage", vec![0x5A; 300], RATE, 60));
    b.add_sample(Sf2Sample::compressed(
        "opus",
        vorbis::opus_header_stream(),
        RATE,
        60,
    ));
    let mut past = Sf2Sample::compressed("past the end", vec![1, 2, 3], RATE, 60);
    past.loop_points = (0, 0);
    b.add_sample(past);
    simple(
        &mut b,
        "Invalid",
        0,
        vec![
            Sf2Zone::new().keys(0, 59).sample(0),
            Sf2Zone::new().keys(60, 62).sample(1),
            Sf2Zone::new().keys(63, 65).sample(2),
            Sf2Zone::new().keys(66, 68).sample(3),
        ],
    );
    Fixture {
        id: "F-SF3-02",
        bytes: b.build(),
        phrase: vec![
            (50, 0, 4, 0.8),
            (61, 6, 2, 0.8),
            (64, 9, 2, 0.8),
            (67, 12, 2, 0.8),
        ],
        check: |i| {
            assert_eq!(i.instrument.zones.len(), 1, "only the PCM zone plays");
            let why: Vec<&str> = i
                .report
                .notes
                .iter()
                .filter(|n| n.what == "sample")
                .map(|n| n.detail.as_str())
                .collect();
            assert_eq!(why.len(), 1, "one note line, counted");
            let n = i.report.notes.iter().find(|n| n.what == "sample").unwrap();
            assert_eq!(n.count, 3, "{:?}", i.report.notes);
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
        f11_fixture(),
        f12(),
        f3_01(),
        f3_02(),
    ]
}

#[test]
fn sf2_fixtures_import_render_and_round_trip() {
    let mut failures = Vec::new();
    for f in fixtures() {
        let imported = match SoundFont.import(ImportInput::Bytes {
            name: if f.id.starts_with("F-SF3") {
                "fixture.sf3"
            } else {
                "fixture.sf2"
            },
            bytes: &f.bytes,
        }) {
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
        if let Err(e) = render_and_round_trip("sf2", f.id, &imported, &f.phrase)
            .and_then(|a| formats_sound::check(f.id, &a, &imported))
        {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn presets_are_listed_and_chosen_by_bank_program_or_name() {
    let mut b = drum_kit();
    let s = b.add_sample(tone(9, 6_000));
    simple(&mut b, "Bass", 0, vec![Sf2Zone::new().sample(s)]);
    b.presets.last_mut().unwrap().program = 33;
    simple(&mut b, "Lead", 0, vec![Sf2Zone::new().sample(s)]);
    b.presets.last_mut().unwrap().program = 80;
    let bytes = b.build();
    assert_eq!(
        list_presets(&bytes).unwrap(),
        vec![
            (128, 0, "Haus Kit".into()),
            (0, 33, "Bass".into()),
            (0, 80, "Lead".into())
        ]
    );
    let input = ImportInput::Bytes {
        name: "multi.sf2",
        bytes: &bytes,
    };
    // default: first by bank, then program
    assert_eq!(
        import_preset(input, &PresetChoice::First)
            .unwrap()
            .instrument
            .name,
        "Bass"
    );
    let kit = import_preset(input, &PresetChoice::parse("128:0")).unwrap();
    assert_eq!(kit.instrument.name, "Haus Kit");
    assert_eq!(kit.report.presets.len(), 3);
    assert!(kit.report.presets[0].imported && !kit.report.presets[1].imported);
    assert!(
        kit.report
            .to_markdown()
            .contains("128:000 Haus Kit (imported)")
    );
    assert_eq!(
        import_preset(input, &PresetChoice::parse(" lead "))
            .unwrap()
            .instrument
            .name,
        "Lead"
    );
    let e = import_preset(input, &PresetChoice::parse("5:5")).unwrap_err();
    assert!(e.to_string().contains("0:33 Bass"), "{e}");
}

#[test]
fn malformed_files_are_refused_never_panic() {
    for (what, bytes) in sf2_seeds::rejected() {
        match SoundFont.import(ImportInput::Bytes {
            name: "bad.sf2",
            bytes: &bytes,
        }) {
            Err(ImportError::Malformed { .. }) => {}
            other => panic!("{what}: {:?}", other.map(|i| i.report)),
        }
    }
}

#[test]
fn a_rom_sample_without_irom_is_skipped_and_reported() {
    let i = import(&sf2_seeds::rom_without_irom());
    has_note(&i, "chunk:irom");
    assert_eq!(i.instrument.zones.len(), 1);
    assert_eq!(zone_at(&i, 100, 100).keys.lo, 64);
}
