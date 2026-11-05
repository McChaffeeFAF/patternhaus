//! DecentSampler fixtures F-DS-01 … F-DS-14 (`docs/formats/dspreset.md` §8, rules in
//! `docs/formats/sampler-fixtures.md`). Each preset is a string written beside tagged-tone
//! WAV samples in a temp folder, imported, its model checked, rendered against
//! `tests/golden/dspreset/<id>.json` and round-tripped through the native format.

mod common;
mod ds_seeds;

use common::*;
use ph_core::*;
use ph_sampler_formats::build::tagged_tone;
use ph_sampler_formats::{DecentSampler, ImportError, ImportInput, Imported, Importer};
use std::path::{Path, PathBuf};

/// (path relative to the preset's folder, frames, `smpl` root and inclusive loop).
type SampleSpec = (&'static str, usize, Option<(u32, u32, u32)>);

struct Fixture {
    id: &'static str,
    preset: &'static str,
    samples: Vec<SampleSpec>,
    phrase: Phrase,
    check: fn(&Imported),
}

fn s(p: &'static str) -> SampleSpec {
    (p, 12_000, None)
}

fn write_samples(dir: &Path, samples: &[SampleSpec]) {
    for (n, (p, frames, smpl)) in samples.iter().enumerate() {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let hz = 220.0 * (1.0 + n as f64 * 0.25);
        std::fs::write(
            path,
            wav(&tagged_tone(n as u32 + 1, hz, *frames, SR), SR, *smpl),
        )
        .unwrap();
    }
}

fn zone_at(i: &Imported, key: u8, vel: u8) -> &Zone {
    i.instrument
        .zones
        .values()
        .find(|z| z.maps(key, vel))
        .unwrap_or_else(|| panic!("no zone at {key}/{vel}"))
}

fn zone_of<'a>(i: &'a Imported, sample: &str) -> &'a Zone {
    i.instrument
        .zones
        .values()
        .find(|z| matches!(&z.sample, SampleRef::File(f) if f.ends_with(sample)))
        .unwrap_or_else(|| panic!("no zone plays {sample}"))
}

fn group(i: &Imported, z: &Zone) -> Group {
    i.instrument.groups[&z.group].clone()
}

fn routes(i: &Imported, z: &Zone) -> Vec<ModRoute> {
    i.instrument
        .modulation
        .iter()
        .filter(|r| r.group == Some(z.group) || r.group.is_none())
        .cloned()
        .collect()
}

fn close(a: f32, b: f64) -> bool {
    (a as f64 - b).abs() < 1e-3 * b.abs().max(1.0)
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            id: "F-DS-01",
            preset: r#"<?xml version="1.0" encoding="UTF-8"?>
<DecentSampler>
  <groups>
    <group name="Keys">
      <sample path="Samples/low.wav" rootNote="48" loNote="36" hiNote="59"/>
      <sample path="Samples/mid.wav" rootNote="60" loNote="60" hiNote="71" loVel="0" hiVel="80" tuning="0.5" pan="-50" start="100" end="9999"/>
      <sample path="Samples/mid_loud.wav" rootNote="60" loNote="60" hiNote="71" loVel="81" hiVel="127" pitchKeyTrack="0"/>
    </group>
  </groups>
</DecentSampler>"#,
            samples: vec![
                s("Samples/low.wav"),
                s("Samples/mid.wav"),
                s("Samples/mid_loud.wav"),
            ],
            phrase: vec![
                (48, 0, 2, 0.8),
                (55, 3, 2, 0.8),
                (62, 6, 2, 0.4),
                (65, 9, 2, 0.9),
            ],
            check: |i| {
                assert_eq!(i.report.format, "dspreset");
                assert_eq!(i.instrument.zones.len(), 3);
                let z = zone_at(i, 50, 100);
                assert_eq!((z.keys.lo, z.keys.hi, z.root_key), (36, 59, 48));
                assert_eq!(z.sample, SampleRef::File("low.wav".into()));
                let m = zone_at(i, 62, 40);
                assert_eq!(
                    (m.velocity.lo, m.velocity.hi, m.tune, m.fine, m.pan),
                    (0, 80, 1, -50.0, -0.5)
                );
                assert_eq!(
                    (m.start, m.end),
                    (100, Some(10_000)),
                    "end is the last frame played"
                );
                let l = zone_at(i, 62, 100);
                assert_eq!(l.root_key, 60);
                assert_eq!(group(i, l).name, "Keys (2)", "fixed pitch splits the group");
                assert!(i.report.unknown.is_empty(), "{:?}", i.report.unknown);
            },
        },
        Fixture {
            id: "F-DS-02",
            preset: r#"<DecentSampler>
  <groups volume="0.5" globalPan="20" globalTuning="1" release="0.5">
    <group name="A" groupVolume="-6dB" groupPan="-10" groupTuning="-0.25" pan="30" release="0.25">
      <sample path="a.wav" rootNote="60" volume="2.0"/>
      <sample path="b.wav" rootNote="60" loNote="72" hiNote="72" volume="-3dB" pan="-100" release="1"/>
    </group>
  </groups>
</DecentSampler>"#,
            samples: vec![s("a.wav"), s("b.wav")],
            phrase: vec![(60, 0, 4, 0.8), (72, 6, 4, 0.8)],
            check: |i| {
                let a = zone_of(i, "a.wav");
                // 0.5 × −6 dB × 2.0 = −6.02 + −6 + 6.02
                assert!(close(a.volume_db, -6.0), "{}", a.volume_db);
                // pan inherits from the group (30) plus offsets −10 and 20
                assert!(close(a.pan, 0.4), "{}", a.pan);
                // tuning offsets add: 1 − 0.25 semitones
                assert_eq!((a.tune, a.fine), (1, -25.0));
                assert!(
                    close(group(i, a).amp_env.release, 0.25),
                    "the group's release wins"
                );
                let b = zone_of(i, "b.wav");
                assert!(close(b.volume_db, -6.02 - 6.0 - 3.0), "{}", b.volume_db);
                assert_eq!(b.pan, -0.9);
                assert!(close(group(i, b).amp_env.release, 1.0));
            },
        },
        Fixture {
            id: "F-DS-03",
            preset: r#"<DecentSampler>
  <groups>
    <group name="Seq" seqMode="round_robin" loNote="60" hiNote="60" rootNote="60">
      <sample path="r1.wav" seqPosition="1"/><sample path="r2.wav" seqPosition="2"/><sample path="r3.wav" seqPosition="3"/>
    </group>
    <group name="Rand" seqMode="random" seqLength="2" loNote="62" hiNote="62" rootNote="62">
      <sample path="x1.wav" seqPosition="1"/><sample path="x2.wav" seqPosition="2"/>
    </group>
    <group name="TrueRand" seqMode="true_random" loNote="64" hiNote="64" rootNote="64">
      <sample path="t1.wav" seqPosition="1"/><sample path="t2.wav" seqPosition="2"/>
    </group>
    <group name="Always" seqMode="always" loNote="65" hiNote="65" rootNote="65"><sample path="w.wav"/></group>
  </groups>
</DecentSampler>"#,
            samples: vec![
                s("r1.wav"),
                s("r2.wav"),
                s("r3.wav"),
                s("x1.wav"),
                s("x2.wav"),
                s("t1.wav"),
                s("t2.wav"),
                s("w.wav"),
            ],
            phrase: vec![
                (60, 0, 1, 0.8),
                (60, 2, 1, 0.8),
                (60, 4, 1, 0.8),
                (60, 6, 1, 0.8),
                (62, 8, 1, 0.8),
                (62, 10, 1, 0.8),
                (64, 12, 1, 0.8),
                (65, 14, 1, 0.8),
            ],
            check: |i| {
                assert_eq!(zone_of(i, "r3.wav").round_robin, Some(2));
                assert_eq!(
                    group(i, zone_of(i, "r1.wav")).round_robin,
                    RoundRobinMode::Sequential
                );
                let x = group(i, zone_of(i, "x2.wav"));
                assert_eq!(
                    (x.round_robin, x.extra["dspreset.seqLength"].as_str()),
                    (RoundRobinMode::Random, "2")
                );
                assert_eq!(
                    group(i, zone_of(i, "t1.wav")).round_robin,
                    RoundRobinMode::Random
                );
                has_note(i, "sample:seqMode");
                assert_eq!(zone_of(i, "w.wav").round_robin, None);
            },
        },
        Fixture {
            id: "F-DS-04",
            preset: r#"<DecentSampler>
  <groups>
    <group name="Notes"><sample path="a.wav" rootNote="60" loNote="48" hiNote="72"/></group>
    <group name="Release" trigger="release" releaseTriggerDecay="3dB"><sample path="rel.wav" rootNote="60" loNote="48" hiNote="72"/></group>
    <group name="Lin" trigger="release" releaseTriggerDecay="0.5"><sample path="rel2.wav" rootNote="60" loNote="73" hiNote="80"/></group>
    <group name="Other">
      <sample path="first.wav" rootNote="60" loNote="81" hiNote="81" trigger="first"/>
      <sample path="leg.wav" rootNote="60" loNote="82" hiNote="82" trigger="legato" previousNotes="60,62" legatoInterval="2"/>
      <sample path="cont.wav" rootNote="60" loNote="83" hiNote="83" trigger="continuous"/>
    </group>
  </groups>
</DecentSampler>"#,
            samples: vec![
                s("a.wav"),
                s("rel.wav"),
                s("rel2.wav"),
                s("first.wav"),
                s("leg.wav"),
                s("cont.wav"),
            ],
            phrase: vec![
                (60, 0, 4, 0.8),
                (75, 6, 2, 0.8),
                (81, 10, 2, 0.8),
                (82, 12, 2, 0.8),
                (83, 14, 2, 0.8),
            ],
            check: |i| {
                let r = zone_of(i, "rel.wav");
                assert_eq!((r.trigger, r.release_decay_db), (ZoneTrigger::Release, 3.0));
                let r2 = zone_of(i, "rel2.wav");
                assert!(
                    close(r2.release_decay_db, 6.0206),
                    "{}",
                    r2.release_decay_db
                );
                assert_eq!(zone_of(i, "cont.wav").trigger, ZoneTrigger::Attack);
                has_note(i, "trigger:first");
                has_note(i, "trigger:legato");
                has_note(i, "sample:previousNotes");
                has_note(i, "sample:legatoInterval");
            },
        },
        Fixture {
            id: "F-DS-05",
            preset: r#"<DecentSampler>
  <groups>
    <group name="Layers" rootNote="60" loNote="48" hiNote="72">
      <sample path="soft.wav" loCC1="0" hiCC1="63"/>
      <sample path="hard.wav" loCC1="64" hiCC1="127"/>
      <sample path="half.wav" loCC2="10"/>
    </group>
    <group name="Pedal"><sample path="pedal.wav" rootNote="60" loNote="100" hiNote="100" onLoCC64="64" onHiCC64="127"/></group>
  </groups>
</DecentSampler>"#,
            samples: vec![s("soft.wav"), s("hard.wav"), s("half.wav"), s("pedal.wav")],
            phrase: vec![(60, 0, 4, 0.8), (100, 6, 2, 0.8)],
            check: |i| {
                let c = &zone_of(i, "soft.wav").cc_conditions;
                assert_eq!((c.len(), c[0].controller, c[0].lo), (1, 1, 0.0));
                assert!(close(c[0].hi, 63.0 / 127.0));
                assert!(close(
                    zone_of(i, "hard.wav").cc_conditions[0].lo,
                    64.0 / 127.0
                ));
                assert!(
                    zone_of(i, "half.wav").cc_conditions.is_empty(),
                    "both ends must be set"
                );
                has_note(i, "sample:onLoCC");
            },
        },
        Fixture {
            id: "F-DS-06",
            preset: r#"<DecentSampler>
  <groups>
    <group name="Loops" rootNote="60">
      <sample path="marked.wav" loNote="60" hiNote="60"/>
      <sample path="explicit.wav" loNote="62" hiNote="62" loopStart="1000" loopEnd="8999" loopCrossfade="200" loopCrossfadeMode="equal_power"/>
      <sample path="linear.wav" loNote="64" hiNote="64" loopStart="1000" loopEnd="8999" loopCrossfade="100" loopCrossfadeMode="linear" loopDuringRelease="false"/>
      <sample path="off.wav" loNote="65" hiNote="65" loopEnabled="false" loopStart="10" loopEnd="20"/>
      <sample path="plain.wav" loNote="67" hiNote="67"/>
    </group>
    <group name="OneShot" ampEnvEnabled="false"><sample path="hit.wav" rootNote="60" loNote="69" hiNote="69"/></group>
  </groups>
</DecentSampler>"#,
            samples: vec![
                (("marked.wav"), 12_000, Some((60, 2000, 9999))),
                s("explicit.wav"),
                s("linear.wav"),
                s("off.wav"),
                s("plain.wav"),
                s("hit.wav"),
            ],
            phrase: vec![
                (60, 0, 4, 0.8),
                (62, 5, 4, 0.8),
                (64, 10, 4, 0.8),
                (65, 15, 2, 0.8),
                (67, 18, 2, 0.8),
                (69, 21, 1, 0.8),
            ],
            check: |i| {
                let l = zone_of(i, "marked.wav").looping.clone().unwrap();
                assert_eq!((l.start, l.end), (2_000, 10_000), "the file's loop marker");
                let l = zone_of(i, "explicit.wav").looping.clone().unwrap();
                assert_eq!(
                    (l.start, l.end, l.crossfade, l.on_release),
                    (1_000, 9_000, 200, LoopRelease::Continue)
                );
                assert_eq!(
                    zone_of(i, "linear.wav")
                        .looping
                        .as_ref()
                        .unwrap()
                        .on_release,
                    LoopRelease::Exit
                );
                has_note(i, "sample:loopCrossfadeMode");
                assert!(zone_of(i, "off.wav").looping.is_none());
                assert!(
                    zone_of(i, "plain.wav").looping.is_none(),
                    "no marker, no loop attributes"
                );
                assert!(zone_of(i, "hit.wav").one_shot);
            },
        },
        Fixture {
            id: "F-DS-07",
            preset: r#"<DecentSampler>
  <groups attack="0.5" decay="0.4" sustain="0.3" release="0.2">
    <group name="Short"><sample path="a.wav" rootNote="60" loNote="48" hiNote="60"/></group>
    <group name="Long" ampEnvAttack="0.01" attack="2" ampEnvHold="0.1" ampEnvDecay="1" ampEnvSustain="0.8" ampEnvRelease="1.5" ampEnvDelay="0.05" attackCurve="0" decayCurve="-50">
      <sample path="b.wav" rootNote="60" loNote="61" hiNote="72"/>
    </group>
  </groups>
</DecentSampler>"#,
            samples: vec![(("a.wav"), 24_000, None), (("b.wav"), 24_000, None)],
            phrase: vec![(55, 0, 6, 0.8), (65, 8, 6, 0.8)],
            check: |i| {
                let e = group(i, zone_of(i, "a.wav")).amp_env;
                assert!(
                    close(e.attack, 0.5)
                        && close(e.decay, 0.4)
                        && close(e.sustain, 0.3)
                        && close(e.release, 0.2)
                );
                let e = group(i, zone_of(i, "b.wav")).amp_env;
                assert!(close(e.attack, 0.01), "the ampEnv spelling wins");
                assert!(close(e.hold, 0.1) && close(e.decay, 1.0) && close(e.sustain, 0.8));
                assert!(close(e.release, 1.5) && close(e.delay, 0.05));
                has_note(i, "sample:attackCurve");
                has_note(i, "sample:decayCurve");
            },
        },
        Fixture {
            id: "F-DS-08",
            preset: r#"<DecentSampler>
  <groups>
    <group name="Hats" rootNote="60">
      <sample path="closed.wav" loNote="42" hiNote="42" tags="hat"/>
      <sample path="pedal.wav" loNote="44" hiNote="44" tags="hat" silencingMode="fast"/>
      <sample path="open.wav" loNote="46" hiNote="46" tags="open" silencedByTags="hat" silencingMode="normal" silencingDecay="0.1"/>
    </group>
    <group name="Kick"><sample path="kick.wav" rootNote="36" loNote="36" hiNote="36"/></group>
  </groups>
</DecentSampler>"#,
            samples: vec![
                s("closed.wav"),
                s("pedal.wav"),
                s("open.wav"),
                s("kick.wav"),
            ],
            phrase: vec![
                (36, 0, 2, 0.9),
                (46, 2, 4, 0.8),
                (42, 4, 1, 0.8),
                (46, 6, 2, 0.8),
                (44, 8, 1, 0.8),
            ],
            check: |i| {
                let open = zone_of(i, "open.wav");
                let closed = zone_of(i, "closed.wav");
                assert_eq!(open.group, closed.group, "a silencing set shares a group");
                assert_eq!(zone_of(i, "pedal.wav").group, open.group);
                let g = group(i, open);
                assert_eq!(
                    (g.voice_limit, g.extra["dspreset.choke"].as_str()),
                    (Some(1), "hat")
                );
                assert_ne!(zone_of(i, "kick.wav").group, open.group);
                has_note(i, "sample:silencedByTags");
                has_note(i, "sample:silencingMode");
            },
        },
        Fixture {
            id: "F-DS-09",
            preset: r#"<DecentSampler>
  <ui><keyboard><color loNote="24" hiNote="25" color="FF3070FF"/></keyboard></ui>
  <groups rootNote="60" loNote="48" hiNote="72">
    <group name="Sustain" enabled="true"><sample path="sus.wav"/></group>
    <group name="Staccato" enabled="false"><sample path="stac.wav"/></group>
    <group name="Always"><sample path="noise.wav" loNote="90" hiNote="90"/></group>
  </groups>
  <midi>
    <note note="24" swallowNotes="true">
      <binding type="amp" level="group" position="0" parameter="ENABLED" translation="fixed_value" translationValue="true"/>
      <binding type="amp" level="group" position="1" parameter="ENABLED" translation="fixed_value" translationValue="false"/>
    </note>
    <note note="25" swallowNotes="true">
      <binding type="amp" level="group" position="0" parameter="ENABLED" translation="fixed_value" translationValue="false"/>
      <binding type="amp" level="group" position="1" parameter="ENABLED" translation="fixed_value" translationValue="true"/>
    </note>
    <note note="30"><binding type="general" level="instrument" parameter="LO_NOTE"/></note>
  </midi>
</DecentSampler>"#,
            samples: vec![s("sus.wav"), s("stac.wav"), s("noise.wav")],
            phrase: vec![
                (60, 0, 3, 0.8),
                (25, 4, 1, 0.8),
                (60, 5, 2, 0.8),
                (24, 8, 1, 0.8),
                (64, 9, 3, 0.8),
                (90, 13, 2, 0.8),
            ],
            check: |i| {
                let a = &i.instrument.articulations;
                assert_eq!(a.len(), 2);
                assert_eq!(
                    (a[0].name.as_str(), &a[0].trigger),
                    ("Sustain", &ArticulationTrigger::Key { key: 24 })
                );
                assert_eq!(a[1].name, "Staccato");
                assert_eq!(i.instrument.settings.default_articulation, Some(a[0].id));
                assert_eq!(group(i, zone_of(i, "sus.wav")).articulations, vec![a[0].id]);
                assert_eq!(
                    group(i, zone_of(i, "stac.wav")).articulations,
                    vec![a[1].id]
                );
                assert!(
                    group(i, zone_of(i, "noise.wav")).articulations.is_empty(),
                    "plays always"
                );
                has_note(i, "midi:note");
                has_note(i, "element:ui");
                assert!(i.instrument.extra["dspreset.ui"].contains("FF3070FF"));
            },
        },
        Fixture {
            id: "F-DS-10",
            preset: r#"<DecentSampler>
  <groups>
    <group name="Dry" rootNote="60" loNote="48" hiNote="59">
      <effects><effect type="lowpass" frequency="1200" resonance="2"/><effect type="gain" level="-6"/></effects>
      <sample path="a.wav"/>
    </group>
    <group name="Wet" rootNote="60" loNote="60" hiNote="72" output1Target="BUS_1"><sample path="b.wav"/></group>
  </groups>
  <effects>
    <effect type="highpass" frequency="40"/><effect type="bandpass"/><effect type="lowpass_1pl"/><effect type="notch" Q="2"/>
    <effect type="peak" frequency="1000" gain="6" gainUnit="decibels"/><effect type="low_shelf" gain="0.5"/><effect type="high_shelf"/>
    <effect type="compressor"/><effect type="reverb" wetLevel="0.2"/><effect type="delay" delayTimeFormat="musical_time" delayTime="0.5"/>
    <effect type="chorus"/><effect type="phaser"/><effect type="convolution" irFile="ir/room.wav"/><effect type="stereo_simulator"/>
    <effect type="bit_crusher" bitDepth="12"/><effect type="pitch_shift"/><effect type="wave_folder"/><effect type="wave_shaper"/><effect type="stutter"/>
    <effect type="gain" level="-2" enabled="true"/><effect type="mystery"/>
  </effects>
  <buses><bus busVolume="-6dB"><effects><effect type="reverb" wetLevel="0.5"/></effects></bus></buses>
</DecentSampler>"#,
            samples: vec![s("a.wav"), s("b.wav")],
            phrase: vec![(55, 0, 4, 0.8), (65, 6, 4, 0.8)],
            check: |i| {
                let dry = group(i, zone_of(i, "a.wav"));
                assert_eq!(dry.effects[0].name, "filter");
                assert_eq!(dry.effects[0].params["cutoff"], 1200.0);
                assert!(
                    close(zone_of(i, "a.wav").volume_db, -6.0),
                    "group gain → level"
                );
                assert_eq!(
                    i.instrument.settings.volume_db, -2.0,
                    "instrument gain → level"
                );
                let names: Vec<&str> = dry.effects.iter().map(|e| e.name.as_str()).collect();
                assert_eq!(
                    names,
                    [
                        "filter",
                        "filter",
                        "filter",
                        "filter",
                        "filter",
                        "eq",
                        "eq",
                        "eq",
                        "compressor",
                        "reverb",
                        "delay",
                        "chorus",
                        "phaser",
                        "convolution",
                        "stereo",
                        "bitcrush"
                    ]
                );
                let wet = group(i, zone_of(i, "b.wav"));
                assert_eq!(wet.effects[0].name, "reverb", "the bus's effects first");
                assert!(close(zone_of(i, "b.wav").volume_db, -6.0));
                for e in [
                    "effect:pitch_shift",
                    "effect:wave_folder",
                    "effect:wave_shaper",
                    "effect:stutter",
                ] {
                    has_note(i, e);
                }
                assert!(i.report.unknown.iter().any(|(w, _)| w == "effect:mystery"));
                assert!(i.instrument.extra["dspreset.effects"].contains("ir/room.wav"));
            },
        },
        Fixture {
            id: "F-DS-11",
            preset: r#"<DecentSampler>
  <ui width="600" height="300"><tab name="main">
    <labeled-knob x="10" y="10" label="Volume" minValue="0" maxValue="1" value="0.8"><binding type="amp" level="instrument" position="0" parameter="AMP_VOLUME" translation="linear" translationOutputMin="0" translationOutputMax="1"/></labeled-knob>
    <labeled-knob x="90" y="10" label="Tone" minValue="0" maxValue="1" value="0.5"><binding type="effect" level="instrument" position="0" parameter="FX_FILTER_FREQUENCY" translation="table" translationTable="0,60;0.5,1000;1,22000"/></labeled-knob>
    <menu x="10" y="100" value="1"><option name="A"><binding type="amp" level="group" position="0" parameter="AMP_VOLUME" translation="fixed_value" translationValue="1"/></option></menu>
    <button x="10" y="150" value="0"><state name="Off"/><state name="On"><binding type="general" level="instrument" parameter="SAMPLE_START" translation="fixed_value" translationValue="100"/></state></button>
    <control x="200" y="10" parameterName="Mystery"><binding type="control" level="ui" parameter="TEXT"/></control>
  </tab></ui>
  <groups><group name="G"><sample path="a.wav" rootNote="60"/></group></groups>
  <effects><effect type="lowpass" frequency="22000"/></effects>
  <midi><cc number="74"><binding type="effect" level="instrument" position="0" parameter="FX_FILTER_FREQUENCY" translationOutputMin="200" translationOutputMax="3200"/></cc>
        <cc number="7"><binding type="general" level="instrument" parameter="SAMPLE_START"/></cc></midi>
</DecentSampler>"#,
            samples: vec![s("a.wav")],
            phrase: vec![(60, 0, 4, 0.8), (67, 6, 4, 0.8)],
            check: |i| {
                has_note(i, "element:ui");
                let ui = &i.instrument.extra["dspreset.ui"];
                assert!(ui.contains("translationTable") && ui.contains("Mystery"));
                let r = routes(i, zone_of(i, "a.wav"));
                let cc = r
                    .iter()
                    .find(|r| r.source == ModSource::Cc { controller: 74 })
                    .unwrap();
                assert_eq!(cc.target, ModTarget::Cutoff);
                assert!(close(cc.depth, 4800.0), "200 → 3200 Hz = 4 octaves");
                has_note(i, "midi:cc");
                has_note(i, "binding:SAMPLE_START");
            },
        },
        Fixture {
            id: "F-DS-12",
            preset: r#"<DecentSampler>
  <groups>
    <group name="A"><sample path="a.wav" rootNote="60" loNote="48" hiNote="60"/></group>
    <group name="B"><sample path="b.wav" rootNote="60" loNote="61" hiNote="72"/></group>
  </groups>
  <effects><effect type="lowpass" frequency="5000"/></effects>
  <modulators>
    <lfo shape="triangle" frequency="6" modAmount="0.5" scope="voice"><binding type="amp" level="group" groupIndex="0" parameter="GROUP_TUNING" translationOutputMin="-1" translationOutputMax="1"/></lfo>
    <lfo shape="sine" frequency="0.5" scope="global"><binding type="effect" level="instrument" effectIndex="0" parameter="FX_FILTER_FREQUENCY" translationOutputMin="500" translationOutputMax="2000"/></lfo>
    <envelope attack="0.2" decay="0.3" sustain="0.5" release="0.4"><binding type="amp" level="group" groupIndex="1" parameter="PAN" translationOutputMin="-50" translationOutputMax="50"/></envelope>
    <midiCC number="11"><binding type="amp" level="instrument" parameter="AMP_VOLUME" translationOutputMin="0.5" translationOutputMax="1"/></midiCC>
    <midiVelocity><binding type="amp" level="group" groupIndex="1" parameter="GROUP_TUNING" translationOutputMin="0" translationOutputMax="0.5"/></midiVelocity>
    <random mode="note_on"><binding type="amp" level="group" groupIndex="0" parameter="PAN" translationOutputMin="-10" translationOutputMax="10"/></random>
    <mpePressure/>
  </modulators>
</DecentSampler>"#,
            samples: vec![(("a.wav"), 24_000, None), (("b.wav"), 24_000, None)],
            phrase: vec![(55, 0, 6, 0.8), (65, 8, 6, 0.6)],
            check: |i| {
                let a = zone_of(i, "a.wav");
                let ga = group(i, a);
                assert_eq!(ga.lfos.len(), 2);
                assert_eq!(
                    (ga.lfos[0].shape, ga.lfos[0].rate),
                    (LfoShape::Triangle, 6.0)
                );
                let ra = routes(i, a);
                let has = |r: &[ModRoute], s: ModSource, t: ModTarget, d: f64| {
                    assert!(
                        r.iter()
                            .any(|x| x.source == s && x.target == t && close(x.depth, d)),
                        "{s:?} {t:?} {d}: {r:?}"
                    );
                };
                has(&ra, ModSource::Lfo { index: 0 }, ModTarget::Pitch, 100.0);
                has(&ra, ModSource::Lfo { index: 1 }, ModTarget::Cutoff, 2400.0);
                has(&ra, ModSource::Random, ModTarget::Pan, 0.2);
                has(
                    &ra,
                    ModSource::Cc { controller: 11 },
                    ModTarget::Volume,
                    6.0206,
                );
                let b = zone_of(i, "b.wav");
                let rb = routes(i, b);
                has(&rb, ModSource::Envelope { index: 1 }, ModTarget::Pan, 1.0);
                has(&rb, ModSource::Velocity, ModTarget::Pitch, 50.0);
                assert!(group(i, b).filter_env.is_some());
                has_note(i, "modulator:mpePressure");
                has_note(i, "modulator:lfo");
            },
        },
        Fixture {
            id: "F-DS-13",
            preset: r#"<DecentSampler>
  <tags><tag name="hat" polyphony="1" volume="0.5"/><tag name="muted" enabled="false"/></tags>
  <groups>
    <group name="Hat" tags="hat"><sample path="h.wav" rootNote="60" loNote="42" hiNote="42"/></group>
    <group name="Aux" output1Target="AUX_STEREO_OUTPUT_2"><sample path="Aux/A.WAV" rootNote="60" loNote="60" hiNote="72"/>
      <oscillator waveform="saw"/></group>
    <group name="Gone" tags="muted"><sample path="h.wav" rootNote="60" loNote="80" hiNote="80"/></group>
  </groups>
  <noteSequences><sequence name="s" length="4" rate="1"><note position="0" velocity="100" note="60" length="1"/></sequence></noteSequences>
  <arpeggiator enabled="false"/>
  <mystery/>
</DecentSampler>"#,
            samples: vec![s("h.wav"), s("aux/a.wav")],
            phrase: vec![(42, 0, 2, 0.8), (42, 2, 2, 0.8), (64, 5, 4, 0.8)],
            check: |i| {
                let h = zone_of(i, "h.wav");
                assert_eq!(group(i, h).voice_limit, Some(1));
                assert!(close(h.volume_db, -6.0206));
                assert_eq!(
                    i.instrument.zones.len(),
                    2,
                    "the disabled tag's sample is skipped"
                );
                let a = zone_at(i, 64, 100);
                assert_eq!(group(i, a).output, 2);
                has_note(i, "sample:path-case");
                has_note(i, "element:oscillator");
                has_note(i, "element:noteSequences");
                has_note(i, "tag:enabled");
                assert!(i.instrument.extra.contains_key("dspreset.noteSequences"));
                assert!(
                    i.instrument
                        .extra
                        .contains_key("dspreset.group1.oscillators")
                );
                assert!(i.report.unknown.iter().any(|(w, _)| w == "element:mystery"));
            },
        },
    ]
}

fn run(f: &Fixture) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_samples(&lib, &f.samples);
    let path = lib.join(format!("{}.dspreset", f.id));
    std::fs::write(&path, f.preset).unwrap();
    let imported = DecentSampler
        .import(ImportInput::Path(&path))
        .map_err(|e| format!("{}: {e}", f.id))?;
    let check = f.check;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&imported))).map_err(|p| {
        let msg = p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        format!("{}: check failed: {msg}", f.id)
    })?;
    render_and_round_trip("dspreset", f.id, &imported, &f.phrase)
}

#[test]
fn ds_fixtures_import_render_and_round_trip() {
    let failures: Vec<String> = fixtures().iter().filter_map(|f| run(f).err()).collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// F-DS-14: a `.dsbundle` folder with `DSLibraryInfo.xml` and two presets.
fn bundle(root: &Path) -> PathBuf {
    let b = root.join("Haus Keys.dsbundle");
    write_samples(&b, &[s("Samples/k.wav")]);
    std::fs::write(
        b.join("DSLibraryInfo.xml"),
        r#"<DecentSamplerLibraryInfo name="Haus Keys" productId="haus.keys" version="1.0">
  <presetMenu><menu name="Keys"><preset file="Presets/Soft.dspreset"/><preset file="Presets/Hard.dspreset"/></menu></presetMenu>
</DecentSamplerLibraryInfo>"#,
    )
    .unwrap();
    std::fs::create_dir_all(b.join("Presets")).unwrap();
    for (n, v) in [("Soft", "0.5"), ("Hard", "1.0")] {
        std::fs::write(
            b.join(format!("Presets/{n}.dspreset")),
            format!(r#"<DecentSampler><groups volume="{v}"><group name="{n}"><sample path="../Samples/k.wav" rootNote="60"/></group></groups></DecentSampler>"#),
        )
        .unwrap();
    }
    b
}

#[test]
fn ds_bundle_imports_its_first_preset_and_lists_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let b = bundle(tmp.path());
    let i = DecentSampler.import(ImportInput::Path(&b)).unwrap();
    assert_eq!(i.instrument.name, "Soft");
    assert_eq!(i.report.presets.len(), 2);
    assert_eq!(
        (
            i.report.presets[1].name.as_str(),
            i.report.presets[0].imported
        ),
        ("Presets/Hard.dspreset", true)
    );
    assert!(i.instrument.extra["dspreset.library"].contains("haus.keys"));
    has_note(&i, "file:DSLibraryInfo.xml");
    render_and_round_trip("dspreset", "F-DS-14", &i, &[(60, 0, 4, 0.8)]).unwrap();
    // without the library file: the first preset at the root by name
    let plain = tmp.path().join("Plain.dsbundle");
    write_samples(&plain, &[s("x.wav")]);
    for n in ["b", "a"] {
        std::fs::write(plain.join(format!("{n}.dspreset")), r#"<DecentSampler><groups><group><sample path="x.wav" rootNote="60"/></group></groups></DecentSampler>"#).unwrap();
    }
    let j = DecentSampler.import(ImportInput::Path(&plain)).unwrap();
    assert_eq!(j.report.presets[0].name, "a.dspreset");
    // .dslibrary zips are refused with a message
    let z = tmp.path().join("x.dslibrary");
    std::fs::write(&z, b"PK\x03\x04").unwrap();
    match DecentSampler.import(ImportInput::Path(&z)) {
        Err(ImportError::Unsupported { reason, .. }) => assert!(reason.contains("unzip")),
        other => panic!("{:?}", other.map(|i| i.report)),
    }
}

#[test]
fn malformed_presets_are_refused_never_panic() {
    for m in ds_seeds::MALFORMED {
        match DecentSampler.import(ImportInput::Bytes {
            name: "bad.dspreset",
            bytes: m.as_bytes(),
        }) {
            Err(ImportError::Malformed { .. }) => {}
            other => panic!("{m:?}: {:?}", other.map(|i| i.report)),
        }
    }
    for v in ds_seeds::VALID {
        let tmp = tempfile::tempdir().unwrap();
        write_samples(tmp.path(), &[s("a.wav")]);
        let p = tmp.path().join("v.dspreset");
        std::fs::write(&p, v).unwrap();
        let i = DecentSampler.import(ImportInput::Path(&p)).unwrap();
        validate(&i).unwrap();
    }
}
