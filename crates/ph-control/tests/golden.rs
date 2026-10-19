//! Golden renders (DESIGN §12; M1 criteria 4, 6 and 8).
//!
//! Each folder in `tests/projects/` is a small project saved as a readable `project.json`.
//! Between them they use both M1 instruments (Sampler one-shot and loop, Haus Synth presets),
//! all seven M1 effects, sends, a sidechain, swing, a choke group, muted notes, a muted
//! playlist track, a loop region of the playlist and a 3/4 bar with a tempo other than 120.
//! `audio-clips` (M2/T1) plays a generated drum loop (`samples/loop.wav`, written by the
//! builder) as tempo-synced audio clips in all three stretch modes, with fades, reverse,
//! pitch and gain, through a tempo ramp, next to a pattern clip.
//! `automation` (M2/T7) rides a synth cutoff, a sampler's volume, an effect parameter, channel
//! volume and pan, insert volume, pan and mute, and a send level, with every curve type,
//! overlapping clips and a clip offset.
//!
//! For every project this test checks:
//! - **determinism:** the offline render is bit-identical run to run, across block sizes
//!   (64, 256, 1000 frames) and across thread counts (serial and the worker pool);
//! - **oracle (criterion 6):** the real-time engine on a simulated device (256-frame
//!   buffer) is bit-identical to the offline render;
//! - **round trip (criterion 4):** load, save, save again gives a byte-identical
//!   `project.json`, equal to the committed file;
//! - **reference:** the render matches `tests/golden/<name>.json` (length, peak, RMS,
//!   integrated LUFS, and a band-energy fingerprint per quarter second) within a small
//!   tolerance, so a different libm on another OS does not fail it but a DSP change does.
//!
//! `cargo xtask golden --update` rebuilds the projects from [`build_projects`] and rewrites
//! the references. A PR that does so must say why the sound changed.

use ph_analysis::spectrum::{band_powers, mean_power_bins};
use ph_core::*;
use ph_engine::assets::FileResolver;
use ph_engine::{
    AudioBuffer, Engine, EngineConfig, PlayMode, RenderSettings, TransportCommand, render_offline,
};
use ph_node::Quality;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SR: u32 = 48_000;
/// Fingerprint segment: a quarter second.
const SEGMENT: usize = SR as usize / 4;
/// Tolerances against the committed reference (cross-platform libm differences).
const STAT_TOL_DB: f64 = 0.1;
const BAND_TOL_DB: f64 = 0.5;
/// Bands quieter than this are not compared (noise floor of the fingerprint).
const BAND_FLOOR_DB: f64 = -70.0;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn projects_dir() -> PathBuf {
    root().join("tests/projects")
}

fn golden_dir() -> PathBuf {
    root().join("tests/golden")
}

fn updating() -> bool {
    std::env::var_os("PH_GOLDEN_UPDATE").is_some_and(|v| v == "1")
}

fn resolver(project_dir: &Path) -> FileResolver {
    FileResolver::new()
        .with_assets_dir(root().join("assets"))
        .with_project_dir(project_dir)
}

// ------------------------------------------------------------------------------ projects

fn params(p: &[(&str, f64)]) -> BTreeMap<String, f64> {
    p.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn drum(name: &str, p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Sampler {
        sample: Some(format!("factory:drums/haus808/{name}")),
        root_key: 60,
        params: params(p),
    }
}

fn synth(preset: &str, p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Synth {
        preset: Some(preset.into()),
        params: params(p),
    }
}

fn fx(name: &str, p: &[(&str, f64)]) -> EffectSlot {
    EffectSlot {
        effect: EffectSpec::Builtin {
            name: name.into(),
            params: params(p),
        },
        bypass: false,
        mix: 1.0,
    }
}

fn steps(e: &mut Edit, pat: PatternId, ch: ChannelId, on: &[u32], key: u8, vel: f32) {
    for s in on {
        let mut n = NoteSpec::new(key, *s as Tick * STEP, STEP);
        n.velocity = vel;
        e.add_note(pat, ch, n).unwrap();
    }
}

fn note(e: &mut Edit, pat: PatternId, ch: ChannelId, key: u8, start: Tick, len: Tick) {
    e.add_note(pat, ch, NoteSpec::new(key, start, len)).unwrap();
}

/// Drums only: all one-shot samplers, swing, an open/closed hat choke group, a muted note,
/// velocity and pan, the filter and saturator on inserts.
fn drums_swing_choke() -> Project {
    let mut e = Edit::new(&Project::empty("drums-swing-choke"));
    e.update_settings(|s| {
        s.bpm = 128.0;
        s.swing = 0.6;
    })
    .unwrap();
    let pat = e.add_pattern("Beat").unwrap();
    let kick = e.add_channel("Kick", drum("kick", &[])).unwrap();
    let snare = e
        .add_channel("Snare", drum("snare", &[("tune", 1.0)]))
        .unwrap();
    let hat_c = e
        .add_channel("Hat closed", drum("hat_closed", &[("choke_group", 1.0)]))
        .unwrap();
    let hat_o = e
        .add_channel("Hat open", drum("hat_open", &[("choke_group", 1.0)]))
        .unwrap();
    let clap = e
        .add_channel("Clap", drum("clap", &[("reverse", 0.0)]))
        .unwrap();
    for (i, c) in [kick, snare, hat_c, hat_o, clap].into_iter().enumerate() {
        e.update_channel(c, |ch| ch.insert = InsertId(i as u32 + 1))
            .unwrap();
    }
    e.update_channel(hat_o, |ch| ch.pan = 0.4).unwrap();
    e.update_channel(clap, |ch| {
        ch.pan = -0.3;
        ch.volume = 0.65;
    })
    .unwrap();
    steps(&mut e, pat, kick, &[0, 4, 8, 10, 12], 60, 0.9);
    steps(&mut e, pat, snare, &[4, 12], 60, 0.85);
    steps(
        &mut e,
        pat,
        hat_c,
        &[0, 1, 2, 3, 5, 6, 7, 9, 10, 11, 13, 14, 15],
        60,
        0.6,
    );
    // the open hat rings until the next closed hat chokes it
    steps(&mut e, pat, hat_o, &[4, 8], 60, 0.7);
    steps(&mut e, pat, clap, &[12, 15], 62, 0.8);
    // a muted note stays in the pattern and is not played
    let mut m = NoteSpec::new(60, 6 * STEP, STEP);
    m.muted = true;
    e.add_note(pat, snare, m).unwrap();
    e.update_insert(InsertId(1), |i| {
        i.effects
            .push(fx("saturator", &[("drive", 6.0), ("mix", 0.7)]))
    })
    .unwrap();
    e.update_insert(InsertId(3), |i| {
        i.effects
            .push(fx("filter", &[("cutoff", 6000.0), ("resonance", 0.3)]))
    })
    .unwrap();
    for t in 0..2 {
        e.add_clip(0, pat, t * 4 * PPQ).unwrap();
    }
    e.finish().0
}

/// Haus Synth only: preset chords, a mono bass and a pluck lead, delay and EQ.
fn synth_chords() -> Project {
    let mut e = Edit::new(&Project::empty("synth-chords"));
    e.update_settings(|s| s.bpm = 100.0).unwrap();
    let pat = e.add_pattern("Chords").unwrap();
    let pad = e.add_channel("Pad", synth("pad/warm", &[])).unwrap();
    let bass = e.add_channel("Bass", synth("bass/acid", &[])).unwrap();
    let lead = e.add_channel("Lead", synth("pluck/bell", &[])).unwrap();
    e.update_channel(pad, |c| c.insert = InsertId(1)).unwrap();
    e.update_channel(bass, |c| c.insert = InsertId(2)).unwrap();
    e.update_channel(lead, |c| {
        c.insert = InsertId(3);
        c.pan = 0.25;
    })
    .unwrap();
    let bar = 4 * PPQ;
    // Dm - Bb - F - C, 8-voice-ish voicings
    let chords: [&[u8]; 4] = [
        &[50, 57, 62, 65, 69],
        &[46, 53, 58, 62, 65],
        &[41, 53, 57, 60, 65],
        &[48, 55, 60, 64, 67],
    ];
    for (k, ch) in chords.iter().enumerate() {
        for key in *ch {
            note(&mut e, pat, pad, *key, k as Tick * bar, bar - STEP);
        }
        let root = ch[0] - 12;
        for s in [0, 3, 6, 10, 12] {
            note(
                &mut e,
                pat,
                bass,
                root,
                k as Tick * bar + s * STEP,
                STEP * 2,
            );
        }
    }
    for (i, key) in [74u8, 77, 81, 79, 77, 74, 72, 69].iter().enumerate() {
        note(&mut e, pat, lead, *key, i as Tick * 2 * PPQ, PPQ);
    }
    e.update_insert(InsertId(1), |i| {
        i.effects
            .push(fx("eq", &[("b2_gain", -3.0), ("b2_freq", 400.0)]))
    })
    .unwrap();
    e.update_insert(InsertId(3), |i| {
        i.effects.push(fx(
            "delay",
            &[
                ("time_l", 450.0),
                ("time_r", 300.0),
                ("feedback", 0.4),
                ("mix", 0.3),
            ],
        ))
    })
    .unwrap();
    e.add_clip(0, pat, 0).unwrap();
    e.finish().0
}

/// Mixing: kick sidechains a compressor on the bass bus, reverb on a send, EQ, a master
/// limiter, a muted insert and insert pan.
fn mix_sidechain_sends() -> Project {
    let mut e = Edit::new(&Project::empty("mix-sidechain-sends"));
    let pat = e.add_pattern("Groove").unwrap();
    let kick = e.add_channel("Kick", drum("kick", &[])).unwrap();
    let snare = e.add_channel("Snare", drum("snare", &[])).unwrap();
    let bass = e.add_channel("Bass", synth("bass/sub", &[])).unwrap();
    let keys = e
        .add_channel("Keys", synth("keys/epiano-ish", &[]))
        .unwrap();
    let ghost = e.add_channel("Ghost", drum("rim", &[])).unwrap();
    for (c, i) in [(kick, 1), (snare, 2), (bass, 3), (keys, 4), (ghost, 6)] {
        e.update_channel(c, |ch| ch.insert = InsertId(i)).unwrap();
    }
    steps(&mut e, pat, kick, &[0, 4, 8, 12], 60, 1.0);
    steps(&mut e, pat, snare, &[4, 12], 60, 0.9);
    steps(&mut e, pat, ghost, &[2, 6, 10, 14], 60, 0.9);
    for s in [0u32, 8] {
        note(&mut e, pat, bass, 33, s as Tick * STEP, 6 * STEP);
    }
    for key in [57u8, 60, 64] {
        note(&mut e, pat, keys, key, 0, 2 * PPQ);
        note(&mut e, pat, keys, key + 2, 2 * PPQ, 2 * PPQ);
    }
    e.update_insert(InsertId(3), |i| {
        i.effects.push(fx("eq", &[("b1_gain", 2.0)]));
        i.effects.push(fx(
            "compressor",
            &[("threshold", -24.0), ("ratio", 8.0), ("sidechain", 1.0)],
        ));
        i.sidechain_from = Some(InsertId(1));
    })
    .unwrap();
    // reverb send bus
    e.update_insert(InsertId(5), |i| {
        i.effects.push(fx(
            "reverb",
            &[("decay", 2.5), ("mix", 1.0), ("predelay", 20.0)],
        ))
    })
    .unwrap();
    for from in [2u32, 4] {
        e.update_insert(InsertId(from), |i| {
            i.sends.push(Send {
                to: InsertId(5),
                level: 0.4,
            })
        })
        .unwrap();
    }
    e.update_insert(InsertId(4), |i| i.pan = -0.35).unwrap();
    e.update_insert(InsertId(6), |i| i.muted = true).unwrap();
    e.update_insert(InsertId(0), |i| {
        i.effects
            .push(fx("limiter", &[("input_gain", 3.0), ("ceiling", -1.0)]))
    })
    .unwrap();
    for t in 0..2 {
        e.add_clip(0, pat, t * 4 * PPQ).unwrap();
    }
    e.finish().0
}

/// Arrangement: several patterns on several tracks, a muted track, a clip that starts part
/// way into its pattern, a looped sample, 3/4 time at 90 BPM.
fn arrangement_waltz() -> Project {
    let mut e = Edit::new(&Project::empty("arrangement-waltz"));
    e.update_settings(|s| {
        s.bpm = 90.0;
        s.time_signature = TimeSignature {
            numerator: 3,
            denominator: 4,
        };
    })
    .unwrap();
    let bar = 3 * PPQ;
    let drums = e.add_pattern("Drums").unwrap();
    let music = e.add_pattern("Music").unwrap();
    let extra = e.add_pattern("Extra").unwrap();
    let kick = e.add_channel("Kick", drum("kick", &[])).unwrap();
    let tom = e.add_channel("Tom", drum("tom_mid", &[])).unwrap();
    // a looped sampler voice: the sub sample held with loop mode on
    let sub = e
        .add_channel("Sub loop", drum("sub_c2", &[("loop_mode", 1.0)]))
        .unwrap();
    let organ = e.add_channel("Organ", synth("keys/organ", &[])).unwrap();
    steps(&mut e, drums, kick, &[0, 12], 60, 0.9);
    steps(&mut e, drums, tom, &[4, 8, 16, 20], 60, 0.7);
    note(&mut e, music, sub, 60, 0, 2 * bar);
    for (k, chord) in [[60u8, 64, 67], [59, 62, 67]].iter().enumerate() {
        for key in chord {
            note(&mut e, music, organ, *key, k as Tick * bar, bar);
        }
    }
    steps(&mut e, extra, tom, &[0, 2, 4, 6, 8, 10], 64, 1.0);
    e.update_insert(InsertId(0), |i| {
        i.effects.push(fx("compressor", &[("threshold", -12.0)]))
    })
    .unwrap();
    for t in 0..4 {
        e.add_clip(0, drums, t * 2 * bar).unwrap();
    }
    e.add_clip(1, music, 0).unwrap();
    let c = e.add_clip(1, music, 4 * bar).unwrap();
    e.update_clip(c, |c| {
        c.offset = bar;
        c.length = bar;
    })
    .unwrap();
    e.add_clip(2, extra, 2 * bar).unwrap();
    e.update_track(2, |t| t.muted = true).unwrap();
    e.finish().0
}

/// The audio-clips project's loop: one bar of drums at 100 BPM (2.4 s), 48 kHz mono,
/// synthesised (deterministic noise), written as 16-bit `samples/loop.wav`.
fn golden_loop() -> Vec<f32> {
    let sr = SR as f64;
    let n = (2.4 * sr) as usize;
    let mut x = vec![0.0f32; n];
    let mut seed = 0x2545_f491u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f64 / u32::MAX as f64 * 2.0 - 1.0
    };
    let step = n / 16;
    for k in 0..16 {
        let s0 = k * step;
        for i in 0..step.min(n - s0) {
            let t = i as f64 / sr;
            let mut v = 0.0;
            if k % 4 == 0 {
                v += 0.7
                    * (std::f64::consts::TAU * (48.0 + 110.0 * (-t * 35.0).exp()) * t).sin()
                    * (-t * 9.0).exp();
            }
            if k == 4 || k == 12 {
                v += 0.35 * noise() * (-t * 22.0).exp()
                    + 0.2 * (std::f64::consts::TAU * 185.0 * t).sin() * (-t * 30.0).exp();
            }
            if k % 2 == 1 {
                v += 0.12 * noise() * (-t * 90.0).exp();
            }
            x[s0 + i] += v as f32;
        }
    }
    x
}

/// Audio clips (M2/T1): the loop in Stretch, Resample and Slice, tempo-synced at 120 BPM
/// with a ramp to 132, plus fades, reverse, pitch and gain; a kick pattern alongside.
fn audio_clips() -> Project {
    let mut e = Edit::new(&Project::empty("audio-clips"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let bar = 4 * PPQ;
    let pat = e.add_pattern("Kick").unwrap();
    let kick = e.add_channel("Kick", drum("kick", &[])).unwrap();
    e.update_channel(kick, |c| c.volume = 0.6).unwrap();
    steps(&mut e, pat, kick, &[0, 4, 8, 12], 60, 0.8);
    for b in 0..4 {
        e.add_clip(0, pat, b * bar).unwrap();
    }
    let s = e
        .add_sample(SampleEntry {
            id: SampleId(0),
            path: "samples/loop.wav".into(),
            name: "loop.wav".into(),
            frames: golden_loop().len() as u64,
            sample_rate: SR,
            channels: 1,
            bpm: Some(SampleBpm {
                value: 100.0,
                source: BpmSource::User,
            }),
            root_key: None,
            analysis: Default::default(),
            provenance: Default::default(),
        })
        .unwrap();
    let synced = |mode: StretchMode| {
        let mut a = AudioClip::new(s);
        a.tempo_sync = true;
        a.stretch = mode;
        a
    };
    e.add_audio_clip(1, 0, bar, synced(StretchMode::Stretch))
        .unwrap();
    let mut b = synced(StretchMode::Resample);
    b.pitch_semitones = 2;
    b.gain_db = -2.0;
    e.add_audio_clip(1, bar, bar, b).unwrap();
    e.add_audio_clip(2, 2 * bar, bar, synced(StretchMode::Slice))
        .unwrap();
    let mut d = synced(StretchMode::Stretch);
    d.reverse = true;
    d.pitch_semitones = -2;
    d.gain_db = -3.0;
    d.fade_in = Fade {
        length: PPQ,
        curve: FadeCurve::Exp,
    };
    d.fade_out = Fade {
        length: PPQ,
        curve: FadeCurve::SCurve,
    };
    e.add_audio_clip(2, 3 * bar, bar, d).unwrap();
    // tempo: 120, a ramp to 132 over bar 3, then 132
    let t = e
        .add_automation_clip(3, 0, 4 * bar, ParamPath::Tempo)
        .unwrap();
    e.add_point(t, 0, 120.0, CurveKind::Hold, 0.0).unwrap();
    e.add_point(t, 2 * bar, 120.0, CurveKind::Linear, 0.0)
        .unwrap();
    e.add_point(t, 3 * bar, 132.0, CurveKind::Hold, 0.0)
        .unwrap();
    e.finish().0
}

/// Automation (M2/T7): every kind of target, every curve type, two clips overlapping on one
/// target, a clip played from an offset, a jump (two points on one tick).
fn automation() -> Project {
    let mut e = Edit::new(&Project::empty("automation"));
    e.update_settings(|s| s.bpm = 110.0).unwrap();
    let bar = 4 * PPQ;
    let pat = e.add_pattern("Groove").unwrap();
    let kick = e.add_channel("Kick", drum("kick", &[])).unwrap();
    let snare = e.add_channel("Snare", drum("snare", &[])).unwrap();
    let bass = e.add_channel("Bass", synth("bass/acid", &[])).unwrap();
    let pad = e.add_channel("Pad", synth("pad/warm", &[])).unwrap();
    for (c, i) in [(kick, 1), (snare, 1), (bass, 2), (pad, 3)] {
        e.update_channel(c, |ch| ch.insert = InsertId(i)).unwrap();
    }
    steps(&mut e, pat, kick, &[0, 4, 8, 12], 60, 0.9);
    steps(&mut e, pat, snare, &[4, 12], 60, 0.85);
    for (i, k) in [33u8, 33, 36, 31].iter().enumerate() {
        note(&mut e, pat, bass, *k, i as Tick * PPQ, PPQ - STEP);
    }
    for key in [57u8, 60, 64, 67] {
        note(&mut e, pat, pad, key, 0, bar - STEP);
    }
    e.update_insert(InsertId(2), |i| {
        i.effects
            .push(fx("saturator", &[("drive", 3.0), ("mix", 1.0)]));
    })
    .unwrap();
    e.update_insert(InsertId(4), |i| {
        i.effects
            .push(fx("reverb", &[("decay", 2.0), ("mix", 1.0)]));
    })
    .unwrap();
    e.update_insert(InsertId(1), |i| {
        i.sends.push(Send {
            to: InsertId(4),
            level: 0.3,
        })
    })
    .unwrap();
    for b in 0..4 {
        e.add_clip(0, pat, b * bar).unwrap();
    }
    fn clip(
        e: &mut Edit,
        track: u32,
        start: Tick,
        len: Tick,
        target: ParamPath,
        pts: &[(Tick, f64, CurveKind)],
    ) -> ClipId {
        let c = e.add_automation_clip(track, start, len, target).unwrap();
        for (t, v, k) in pts {
            e.add_point(c, *t, *v, *k, 0.0).unwrap();
        }
        c
    }
    use CurveKind::*;
    // the bass cutoff: a smooth sweep, a jump, an S-curve back; a later clip wins on bar 3
    clip(
        &mut e,
        1,
        0,
        4 * bar,
        ParamPath::ChannelParam(bass, "f1_cutoff".into()),
        &[
            (0, 300.0, Smooth),
            (bar, 5000.0, SCurve),
            (2 * bar, 600.0, Hold),
            (2 * bar, 3500.0, Linear),
            (4 * bar, 400.0, Hold),
        ],
    );
    clip(
        &mut e,
        2,
        bar * 5 / 2,
        bar / 2,
        ParamPath::ChannelParam(bass, "f1_cutoff".into()),
        &[(0, 1200.0, Linear), (bar / 2, 200.0, Hold)],
    );
    // a sampler parameter, played from the middle of its clip
    let c = clip(
        &mut e,
        3,
        0,
        2 * bar,
        ParamPath::ChannelParam(snare, "tune".into()),
        &[(0, -5.0, Linear), (4 * bar, 7.0, Hold)],
    );
    e.update_clip(c, |k| k.offset = bar).unwrap();
    // an effect parameter
    clip(
        &mut e,
        4,
        bar,
        3 * bar,
        ParamPath::EffectParam {
            insert: InsertId(2),
            slot: 0,
            param: "drive".into(),
        },
        &[
            (0, 1.0, SCurve),
            (2 * bar, 12.0, Linear),
            (3 * bar, 2.0, Hold),
        ],
    );
    // strips
    clip(
        &mut e,
        5,
        0,
        4 * bar,
        ParamPath::ChannelVolume(pad),
        &[
            (0, 0.2, SCurve),
            (2 * bar, 0.9, Smooth),
            (4 * bar, 0.3, Hold),
        ],
    );
    clip(
        &mut e,
        6,
        0,
        4 * bar,
        ParamPath::ChannelPan(kick),
        &[(0, -0.7, Linear), (4 * bar, 0.7, Hold)],
    );
    clip(
        &mut e,
        7,
        bar,
        2 * bar,
        ParamPath::InsertVolume(InsertId(2)),
        &[(0, 1.0, Linear), (bar, 0.3, Hold), (2 * bar, 1.0, Hold)],
    );
    clip(
        &mut e,
        8,
        0,
        4 * bar,
        ParamPath::InsertPan(InsertId(3)),
        &[(0, 0.8, SCurve), (4 * bar, -0.8, Hold)],
    );
    clip(
        &mut e,
        9,
        3 * bar,
        PPQ,
        ParamPath::InsertMute(InsertId(3)),
        &[(0, 1.0, Hold)],
    );
    clip(
        &mut e,
        10,
        0,
        4 * bar,
        ParamPath::InsertSend {
            insert: InsertId(1),
            to: InsertId(4),
        },
        &[
            (0, 0.0, Linear),
            (3 * bar, 1.2, Linear),
            (4 * bar, 0.1, Hold),
        ],
    );
    e.finish().0
}

/// Every project, by folder name. Used only by `cargo xtask golden --update`.
fn build_projects() -> Vec<(&'static str, Project)> {
    vec![
        ("automation", automation()),
        ("audio-clips", audio_clips()),
        ("drums-swing-choke", drums_swing_choke()),
        ("synth-chords", synth_chords()),
        ("mix-sidechain-sends", mix_sidechain_sends()),
        ("arrangement-waltz", arrangement_waltz()),
    ]
}

// ------------------------------------------------------------------------------ rendering

fn song_settings(block: usize) -> RenderSettings {
    RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: block,
    }
}

fn render(p: &Project, dir: &Path, block: usize) -> AudioBuffer {
    render_offline(p, &song_settings(block), &resolver(dir)).unwrap()
}

/// The real-time engine on a simulated device, pulled for `frames` frames.
fn render_simulated(p: &Project, dir: &Path, frames: usize, threads: usize) -> AudioBuffer {
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: 256,
            threads,
            ..EngineConfig::default()
        },
        Arc::new(resolver(dir)),
    )
    .unwrap();
    engine.set_project(Arc::new(p.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    while out[0].len() < frames {
        let b = dev.pull(256);
        out[0].extend_from_slice(&b.channels[0]);
        out[1].extend_from_slice(&b.channels[1]);
    }
    out[0].truncate(frames);
    out[1].truncate(frames);
    AudioBuffer {
        sample_rate: SR,
        channels: out.into(),
    }
}

fn first_difference(a: &AudioBuffer, b: &AudioBuffer) -> Option<(usize, usize)> {
    if a.frames() != b.frames() {
        return Some((usize::MAX, a.frames().min(b.frames())));
    }
    for c in 0..2 {
        for i in 0..a.frames() {
            if a.channels[c][i].to_bits() != b.channels[c][i].to_bits() {
                return Some((c, i));
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------ reference

fn db(x: f64) -> f64 {
    if x <= 1e-10 { -200.0 } else { 20.0 * x.log10() }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Stats and a fingerprint: per quarter second of the mono sum, the power in the seven
/// `ph-analysis` bands (sub to air), in dB.
fn reference_of(a: &AudioBuffer) -> Value {
    let ch = &a.channels;
    let peak = |c: &[f32]| c.iter().fold(0.0f32, |m, v| m.max(v.abs())) as f64;
    let rms = |c: &[f32]| {
        (c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64).sqrt()
    };
    let lufs = ph_analysis::loudness::integrated_lufs(ch, SR).unwrap_or(-200.0);
    let mono: Vec<f32> = ch[0]
        .iter()
        .zip(&ch[1])
        .map(|(l, r)| 0.5 * (l + r))
        .collect();
    let bands: Vec<Vec<f64>> = mono
        .chunks(SEGMENT)
        .filter(|s| s.len() == SEGMENT)
        .map(|s| {
            band_powers(&mean_power_bins(s), SR)
                .iter()
                .map(|p| round2((10.0 * p.max(1e-20).log10()).max(-120.0)))
                .collect()
        })
        .collect();
    json!({
        "frames": a.frames(),
        "sample_rate": SR,
        "peak_db": [round2(db(peak(&ch[0]))), round2(db(peak(&ch[1])))],
        "rms_db": [round2(db(rms(&ch[0]))), round2(db(rms(&ch[1])))],
        "integrated_lufs": round2(lufs),
        "segment_frames": SEGMENT,
        "bands_db": bands,
    })
}

fn compare_reference(name: &str, got: &Value, want: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    if got["frames"] != want["frames"] {
        errs.push(format!(
            "{name}: frames {} vs {}",
            got["frames"], want["frames"]
        ));
        return errs;
    }
    let near = |key: &str, a: &Value, b: &Value, errs: &mut Vec<String>| {
        let (x, y) = (a.as_f64().unwrap(), b.as_f64().unwrap());
        if (x - y).abs() > STAT_TOL_DB && !(x < -100.0 && y < -100.0) {
            errs.push(format!("{name}: {key} {x} vs reference {y}"));
        }
    };
    for key in ["peak_db", "rms_db"] {
        for c in 0..2 {
            near(key, &got[key][c], &want[key][c], &mut errs);
        }
    }
    near(
        "integrated_lufs",
        &got["integrated_lufs"],
        &want["integrated_lufs"],
        &mut errs,
    );
    let (g, w) = (
        got["bands_db"].as_array().unwrap(),
        want["bands_db"].as_array().unwrap(),
    );
    for (s, (gs, ws)) in g.iter().zip(w).enumerate() {
        for (b, (x, y)) in gs
            .as_array()
            .unwrap()
            .iter()
            .zip(ws.as_array().unwrap())
            .enumerate()
        {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x > BAND_FLOOR_DB || y > BAND_FLOOR_DB) && (x - y).abs() > BAND_TOL_DB {
                errs.push(format!(
                    "{name}: segment {s} band {b}: {x} dB vs reference {y} dB"
                ));
            }
        }
    }
    errs
}

// ------------------------------------------------------------------------------ the test

fn project_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(projects_dir())
        .expect("tests/projects exists")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("project.json").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn write_projects() {
    for (name, p) in build_projects() {
        let dir = projects_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        ph_formats::save_project(&p, &dir).unwrap();
    }
    // generated material for audio-clips
    let samples = projects_dir().join("audio-clips/samples");
    std::fs::create_dir_all(&samples).unwrap();
    ph_formats::encode::write_wav(
        &samples.join("loop.wav"),
        &[golden_loop()],
        SR,
        ph_formats::encode::WavFormat::Pcm16,
    )
    .unwrap();
}

#[test]
fn golden_projects_render_deterministically_and_match_their_references() {
    if updating() {
        write_projects();
        std::fs::create_dir_all(golden_dir()).unwrap();
    }
    let names = project_names();
    assert!(names.len() >= 5, "golden projects missing: {names:?}");
    let mut errors = Vec::new();
    for name in &names {
        let dir = projects_dir().join(name);
        let p = ph_formats::load_project(&dir).unwrap();
        assert!(
            ph_engine::project_warnings(&p, &resolver(&dir)).is_empty(),
            "{name}: {:?}",
            ph_engine::project_warnings(&p, &resolver(&dir))
        );

        // determinism: run to run, block sizes, threads (render_offline uses the pool)
        let a = render(&p, &dir, 256);
        assert!(a.frames() > SR as usize, "{name}: render too short");
        for block in [64, 1000, 256] {
            let b = render(&p, &dir, block);
            assert_eq!(
                first_difference(&a, &b),
                None,
                "{name}: block {block} differs (channel, frame)"
            );
        }

        // criterion 6: real-time through the simulated device == offline, serial and
        // multi-threaded
        for threads in [1, 4] {
            let rt = render_simulated(&p, &dir, a.frames(), threads);
            assert_eq!(
                first_difference(&a, &rt),
                None,
                "{name}: simulated device ({threads} threads) differs from offline"
            );
        }

        // reference
        let got = reference_of(&a);
        let path = golden_dir().join(format!("{name}.json"));
        if updating() {
            std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        } else {
            let want: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| {
                    panic!("{path:?} missing: run cargo xtask golden --update")
                }))
                .unwrap();
            errors.extend(compare_reference(name, &got, &want));
        }
        println!(
            "{name}: {} frames, {} LUFS",
            got["frames"], got["integrated_lufs"]
        );
    }
    assert!(
        errors.is_empty(),
        "golden mismatches:\n{}",
        errors.join("\n")
    );
}

/// Criterion 4: every golden project round-trips byte for byte.
#[test]
fn golden_projects_round_trip_byte_identical() {
    for name in project_names() {
        let dir = projects_dir().join(&name);
        let committed = std::fs::read_to_string(dir.join("project.json")).unwrap();
        let p = ph_formats::load_project(&dir).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        ph_formats::save_project(&p, tmp.path()).unwrap();
        let first = std::fs::read_to_string(tmp.path().join("project.json")).unwrap();
        let again = ph_formats::load_project(tmp.path()).unwrap();
        assert_eq!(again, p, "{name}: reloaded project differs");
        ph_formats::save_project(&again, tmp.path()).unwrap();
        let second = std::fs::read_to_string(tmp.path().join("project.json")).unwrap();
        assert_eq!(first, second, "{name}: saving twice is not byte-stable");
        // line endings may be converted by git on checkout; compare the content
        assert_eq!(
            first.replace("\r\n", "\n"),
            committed.replace("\r\n", "\n"),
            "{name}: saving the loaded project changes project.json"
        );
    }
}

/// The projects in `tests/projects/` are what [`build_projects`] makes (so the builders
/// above document them), and they cover the M1 feature list.
#[test]
fn golden_projects_cover_m1_features() {
    let built = build_projects();
    let mut instruments = Vec::new();
    let mut effects = std::collections::BTreeSet::new();
    let (mut sends, mut sidechain, mut swing, mut choke, mut muted_notes, mut loop_sampler) =
        (false, false, false, false, false, false);
    for (name, p) in &built {
        let on_disk = ph_formats::load_project(&projects_dir().join(name)).unwrap();
        assert_eq!(
            &on_disk, p,
            "{name}: tests/projects is stale (xtask golden --update)"
        );
        swing |= p.settings.swing > 0.0;
        for ch in p.channels.values() {
            match &ch.instrument {
                InstrumentSpec::Sampler { params, .. } => {
                    instruments.push("sampler");
                    choke |= params.get("choke_group").is_some_and(|g| *g > 0.0);
                    loop_sampler |= params.get("loop_mode").is_some_and(|m| *m > 0.0);
                }
                InstrumentSpec::Synth { .. } => instruments.push("synth"),
                _ => {}
            }
        }
        for pat in p.patterns.values() {
            for notes in pat.notes.values() {
                muted_notes |= notes.values().any(|n| n.muted);
            }
        }
        for ins in p.mixer.iter() {
            sends |= !ins.sends.is_empty();
            sidechain |= ins.sidechain_from.is_some();
            for slot in &ins.effects {
                if let EffectSpec::Builtin { name, .. } = &slot.effect {
                    effects.insert(name.clone());
                }
            }
        }
    }
    assert!(instruments.contains(&"sampler") && instruments.contains(&"synth"));
    let all: std::collections::BTreeSet<String> = [
        "eq",
        "compressor",
        "limiter",
        "reverb",
        "delay",
        "filter",
        "saturator",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(effects, all, "every M1 effect is used");
    assert!(sends && sidechain && swing && choke && muted_notes && loop_sampler);
}
