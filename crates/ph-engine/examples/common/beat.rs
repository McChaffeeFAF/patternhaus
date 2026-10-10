//! The demo beat shared by `render_beat` and `play_beat`.
//!
//! Drums are generated in code (no factory samples needed): a pitched-sine kick, a
//! noise-and-tone snare, and high-passed noise hats, served by [`DemoDrums`] under
//! `demo:` references. The bass and chords are Haus Synth.

use ph_core::*;
use ph_node::{AssetResolver, SampleData};
use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::sync::Arc;

/// Serves generated drum one-shots: `demo:kick`, `demo:snare`, `demo:hat`, `demo:openhat`.
pub struct DemoDrums;

impl AssetResolver for DemoDrums {
    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        let sr = 48_000.0;
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let n = |secs: f64| (secs * sr) as usize;
        let data: Vec<f32> = match reference {
            "demo:kick" => {
                let mut phase = 0.0;
                (0..n(0.5))
                    .map(|i| {
                        let t = i as f64 / sr;
                        let f = 48.0 + 170.0 * (-t * 38.0).exp();
                        phase += f / sr;
                        let body = (phase * TAU).sin() * (-t * 6.5).exp();
                        let click = noise() * (-t * 900.0).exp() * 0.3;
                        ((body + click) * 0.95).tanh() as f32
                    })
                    .collect()
            }
            "demo:snare" => (0..n(0.3))
                .map(|i| {
                    let t = i as f64 / sr;
                    let tone = (t * 185.0 * TAU).sin() * (-t * 22.0).exp()
                        + (t * 330.0 * TAU).sin() * (-t * 30.0).exp() * 0.5;
                    let rattle = noise() * (-t * 13.0).exp();
                    (0.45 * tone + 0.55 * rattle) as f32 * 0.8
                })
                .collect(),
            "demo:hat" | "demo:openhat" => {
                let decay = if reference == "demo:hat" { 60.0 } else { 9.0 };
                let mut prev = 0.0;
                let mut prev2 = 0.0;
                (0..n(if decay > 20.0 { 0.08 } else { 0.5 }))
                    .map(|i| {
                        let t = i as f64 / sr;
                        let x = noise();
                        // second difference: a steep high-pass
                        let hp = x - 2.0 * prev + prev2;
                        prev2 = prev;
                        prev = x;
                        (hp * 0.18 * (-t * decay).exp()) as f32
                    })
                    .collect()
            }
            other => return Err(format!("the demo has no sample `{other}`")),
        };
        Ok(Arc::new(SampleData {
            sample_rate: sr,
            channels: vec![data],
            loop_range: None,
            root_key: None,
        }))
    }
}

fn map(p: &[(&str, f64)]) -> BTreeMap<String, f64> {
    p.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn drum(sample: &str, p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Sampler {
        sample: Some(sample.into()),
        root_key: 60,
        params: map(p),
    }
}

fn synth(p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Synth {
        preset: None,
        params: map(p),
    }
}

fn fx(name: &str, p: &[(&str, f64)]) -> EffectSlot {
    EffectSlot {
        effect: EffectSpec::Builtin {
            name: name.into(),
            params: map(p),
        },
        bypass: false,
        mix: 1.0,
    }
}

fn note(key: u8, start: Tick, len: Tick, vel: f32) -> NoteSpec {
    let mut n = NoteSpec::new(key, start, len);
    n.velocity = vel;
    n
}

/// Eight bars at 124 BPM: four-on-the-floor kick, snare on 2 and 4, closed and open hats
/// (choked), a sidechained Haus Synth bass, and chords. Inserts: kick (EQ), snare (EQ +
/// compressor, reverb send), hats, bass (compressor ducked by the kick + EQ), chords
/// (reverb send), a reverb return, and a limiter on the master.
pub fn project() -> Project {
    let mut e = Edit::new(&Project::empty("Demo beat"));
    e.update_settings(|s| {
        s.bpm = 124.0;
        s.swing = 0.15;
    })
    .unwrap();
    let kick = e.add_channel("Kick", drum("demo:kick", &[])).unwrap();
    let snare = e.add_channel("Snare", drum("demo:snare", &[])).unwrap();
    let hat = e
        .add_channel("Hat", drum("demo:hat", &[("choke_group", 1.0)]))
        .unwrap();
    let open = e
        .add_channel(
            "Open Hat",
            drum("demo:openhat", &[("choke_group", 1.0), ("release", 0.03)]),
        )
        .unwrap();
    let bass = e
        .add_channel(
            "Bass",
            synth(&[
                ("osc1_wave", 0.0),
                ("osc2_wave", 1.0),
                ("osc2_level", 0.4),
                ("osc2_oct", -1.0),
                ("sub_level", 0.6),
                ("sub_wave", 1.0),
                ("f1_cutoff", 380.0),
                ("f1_res", 0.35),
                ("f1_env", 3.0),
                ("fenv_decay", 0.18),
                ("fenv_sustain", 0.1),
                ("amp_release", 0.06),
                ("voice_mode", 1.0),
            ]),
        )
        .unwrap();
    let chords = e
        .add_channel(
            "Chords",
            synth(&[
                ("osc1_wave", 0.0),
                ("osc2_wave", 0.0),
                ("osc2_level", 0.7),
                ("osc2_fine", 9.0),
                ("unison_voices", 3.0),
                ("unison_detune", 18.0),
                ("f1_cutoff", 2400.0),
                ("amp_attack", 0.01),
                ("amp_release", 0.5),
                ("volume", -12.0),
            ]),
        )
        .unwrap();
    let route = [
        (kick, 1, 0.9),
        (snare, 2, 0.75),
        (hat, 3, 0.55),
        (open, 3, 0.45),
        (bass, 4, 0.8),
        (chords, 5, 0.55),
    ];
    for (c, ins, vol) in route {
        e.update_channel(c, |ch| {
            ch.insert = InsertId(ins);
            ch.volume = vol;
        })
        .unwrap();
    }
    let send = |to: u32, level: f32| ph_core::Send {
        to: InsertId(to),
        level,
    };
    e.update_insert(InsertId(1), |i| {
        i.name = "Kick".into();
        i.effects.push(fx(
            "eq",
            &[
                ("b2_gain", 3.0),
                ("b2_freq", 60.0),
                ("b4_gain", -4.0),
                ("b4_freq", 350.0),
            ],
        ));
    })
    .unwrap();
    e.update_insert(InsertId(2), |i| {
        i.name = "Snare".into();
        i.effects.push(fx(
            "eq",
            &[("b5_gain", 3.0), ("b5_freq", 5000.0), ("b2_gain", -3.0)],
        ));
        i.effects.push(fx(
            "compressor",
            &[
                ("threshold", -22.0),
                ("ratio", 4.0),
                ("attack", 5.0),
                ("release", 120.0),
                ("makeup", 3.0),
            ],
        ));
        i.sends.push(send(6, 0.35));
    })
    .unwrap();
    e.update_insert(InsertId(3), |i| {
        i.name = "Hats".into();
        i.pan = 0.15;
        i.sends.push(send(6, 0.15));
    })
    .unwrap();
    e.update_insert(InsertId(4), |i| {
        i.name = "Bass".into();
        i.effects.push(fx(
            "compressor",
            &[
                ("threshold", -28.0),
                ("ratio", 8.0),
                ("attack", 1.0),
                ("release", 140.0),
                ("sidechain", 1.0),
            ],
        ));
        i.effects
            .push(fx("eq", &[("b2_gain", 2.0), ("b2_freq", 80.0)]));
        i.sidechain_from = Some(InsertId(1));
    })
    .unwrap();
    e.update_insert(InsertId(5), |i| {
        i.name = "Chords".into();
        i.pan = -0.1;
        i.sends.push(send(6, 0.4));
    })
    .unwrap();
    e.update_insert(InsertId(6), |i| {
        i.name = "Reverb".into();
        i.effects.push(fx(
            "reverb",
            &[
                ("mix", 1.0),
                ("decay", 2.2),
                ("size", 0.7),
                ("predelay", 20.0),
            ],
        ));
    })
    .unwrap();
    e.update_insert(InsertId(0), |i| {
        i.effects
            .push(fx("limiter", &[("ceiling", -0.3), ("input_gain", 2.0)]));
    })
    .unwrap();

    // patterns
    let beat = 4 * STEP;
    let groove = e.add_pattern("Groove").unwrap();
    let fill = e.add_pattern("Fill").unwrap();
    for (p, is_fill) in [(groove, false), (fill, true)] {
        for b in 0..4 {
            e.add_note(p, kick, note(60, b * beat, STEP, 1.0)).unwrap();
        }
        e.add_note(p, snare, note(60, beat, STEP, 0.9)).unwrap();
        e.add_note(p, snare, note(60, 3 * beat, STEP, 0.95))
            .unwrap();
        for s in 0..16 {
            if s % 4 == 2 {
                e.add_note(p, open, note(60, s * STEP, 2 * STEP, 0.7))
                    .unwrap();
            } else {
                let v = if s % 2 == 0 { 0.8 } else { 0.5 };
                e.add_note(p, hat, note(60, s * STEP, STEP, v)).unwrap();
            }
        }
        if is_fill {
            for s in [13, 14, 15] {
                e.add_note(
                    p,
                    snare,
                    note(60, s * STEP, STEP, 0.5 + 0.15 * (s - 12) as f32),
                )
                .unwrap();
            }
        }
    }
    let bassline = e.add_pattern("Bass").unwrap();
    // A minor: root on the off-beats, a pickup into the next bar
    for (s, k, len) in [
        (2, 33u8, 2),
        (6, 33, 2),
        (10, 33, 2),
        (14, 36, 1),
        (15, 38, 1),
    ] {
        e.add_note(bassline, bass, note(k, s * STEP, len * STEP, 0.9))
            .unwrap();
    }
    let prog = e.add_pattern("Chords").unwrap();
    let bar = 16 * STEP;
    for (b, keys) in [[57u8, 60, 64, 67], [53, 57, 60, 64]].iter().enumerate() {
        for k in keys {
            e.add_note(prog, chords, note(*k, b as Tick * bar, bar - STEP, 0.7))
                .unwrap();
        }
    }
    // playlist: 8 bars
    for b in 0..8 {
        let p = if b == 7 { fill } else { groove };
        e.add_clip(0, p, b * bar).unwrap();
        if b >= 2 {
            e.add_clip(1, bassline, b * bar).unwrap();
        }
        if b % 2 == 0 {
            e.add_clip(2, prog, b * bar).unwrap();
        }
    }
    e.finish().0
}
