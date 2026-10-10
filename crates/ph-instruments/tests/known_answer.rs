//! Known-answer tests for the instruments: tuning, aliasing, resampling quality, loops,
//! reverse, voice limits, mono/legato, glide, choke, and the factory.

#[path = "../../ph-effects/tests/common/mod.rs"]
mod common;
mod util;

use common::*;
use ph_core::InstrumentSpec;
use ph_dsp::analysis::{inharmonic_ratio_db_below, peak, power_spectrum, tone_amplitude};
use ph_node::{EventKind, Processor, Quality};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

fn on(at: usize, id: u32, key: u8) -> (usize, EventKind) {
    (
        at,
        EventKind::NoteOn {
            note_id: id,
            key,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
        },
    )
}

fn off(at: usize, id: u32, key: u8) -> (usize, EventKind) {
    (
        at,
        EventKind::NoteOff {
            note_id: id,
            key,
            velocity: 0.0,
        },
    )
}

/// Frequency of the strongest spectral peak, refined by parabolic interpolation.
fn dominant_hz(x: &[f32]) -> f64 {
    let p = power_spectrum(x);
    let (k, _) = p
        .iter()
        .enumerate()
        .skip(2)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let (a, b, c) = (p[k - 1].ln(), p[k].ln(), p[k + 1].ln());
    let d = 0.5 * (a - c) / (a - 2.0 * b + c);
    (k as f64 + d) * SR / x.len() as f64
}

fn play(
    p: &mut dyn Processor,
    frames: usize,
    events: &[(usize, EventKind)],
    q: Quality,
) -> Vec<Vec<f32>> {
    activate(p, 512, q, 1);
    render(p, frames, 256, events, None, None)
}

/// Known-answer: A4 (key 69) sounds at 440 Hz; an octave up at 880 Hz.
#[test]
fn synth_tuning() {
    for (key, hz) in [(69u8, 440.0), (81, 880.0), (45, 110.0)] {
        let mut s = util::synth_with(&[("osc1_wave", 3.0), ("f1_on", 0.0)]);
        let out = play(s.as_mut(), 1 << 16, &[on(0, 1, key)], Quality::Live);
        let f = dominant_hz(&out[0][1 << 15..]);
        assert!((f / hz - 1.0).abs() < 1e-4, "key {key}: {f} Hz");
    }
}

/// Known-answer: an unfiltered saw at 1245 Hz has audible-band aliasing below -80 dB
/// (oversampled 4-point PolyBLEP), versus about -41 dB for PolyBLEP at the base rate.
#[test]
fn synth_saw_aliasing() {
    for q in [Quality::Live, Quality::High] {
        let key = 75u8; // ≈ 1244.5 Hz
        let f0 = ph_dsp::key_to_hz(key as f32) as f64;
        let mut s = util::synth_with(&[("f1_on", 0.0), ("volume", 0.0)]);
        let n = 1 << 16;
        let out = play(s.as_mut(), n + 8192, &[on(0, 1, key)], q);
        let r = inharmonic_ratio_db_below(&out[0][8192..], SR, f0, 6, 20000.0);
        eprintln!("synth saw aliasing ({q:?}): {r:.1} dB");
        assert!(r < -80.0, "{q:?}: {r}");
    }
}

/// The ladder low-pass in the synth: a saw through a 1 kHz cutoff loses its upper
/// harmonics at 24 dB/oct.
#[test]
fn synth_filter_shapes_harmonics() {
    let key = 45u8; // 110 Hz
    let mut s = util::synth_with(&[
        ("f1_cutoff", 1000.0),
        ("f1_res", 0.0),
        ("volume", 0.0),
        ("osc1_level", 0.5),
    ]);
    let out = play(s.as_mut(), 1 << 16, &[on(0, 1, key)], Quality::Live);
    let x = &out[0][1 << 15..];
    let h = |k: f64| tone_amplitude(x, SR, 110.0 * k);
    // saw harmonic k has amplitude ∝ 1/k; normalise that out
    let rel = |k: f64| 20.0 * (h(k) * k / h(1.0)).log10();
    let at_fc = rel(9.0); // 990 Hz
    let two_oct = rel(36.0); // 3960 Hz
    // 4 matched poles at 220 Hz / 1 kHz: 4 · 10log10(1 + 0.22²) ≈ 0.8 dB (relative to the
    // fundamental's 0.2 dB)
    assert!((rel(2.0) + 0.6).abs() < 0.3, "{}", rel(2.0));
    // analog 4-pole: ≈ -12 dB at fc, ≈ -49 dB two octaves above
    assert!((at_fc + 12.0).abs() < 1.5, "fc {at_fc}");
    assert!((two_oct - at_fc) < -33.0, "fc {at_fc} 2oct {two_oct}");
}

/// Voice limit with stealing: four held notes with polyphony 2 leave two sounding.
#[test]
fn synth_voice_limit() {
    let mut s = util::synth_with(&[("polyphony", 2.0), ("osc1_wave", 3.0), ("f1_on", 0.0)]);
    let ev = [on(0, 1, 60), on(100, 2, 64), on(200, 3, 67), on(300, 4, 72)];
    let out = play(s.as_mut(), 48000, &ev, Quality::Live);
    let x = &out[0][24000..];
    let amp = |k: u8| tone_amplitude(x, SR, ph_dsp::key_to_hz(k as f32) as f64);
    // stolen notes leave only spectral leakage of the others (< 1 %)
    let present = |k: u8| amp(k) > 0.01;
    assert!(!present(60) && !present(64), "oldest notes stolen");
    assert!(present(67) && present(72));
}

/// Mono legato: the second overlapping note changes pitch without retriggering (the
/// envelope never dips); glide makes the pitch sweep.
#[test]
fn synth_mono_legato_and_glide() {
    let mut s = util::synth_with(&[
        ("voice_mode", 1.0),
        ("osc1_wave", 3.0),
        ("f1_on", 0.0),
        ("amp_attack", 0.05),
        ("glide_mode", 1.0),
        ("glide_time", 0.1),
    ]);
    let ev = [on(0, 1, 57), on(24000, 2, 69), off(36000, 1, 57)];
    let out = play(s.as_mut(), 60000, &ev, Quality::Live);
    // envelope stays up across the legato change
    let env = |a: usize| peak(&out[0][a..a + 480]);
    assert!(env(23000) > 0.2 && env(24200) > 0.9 * env(23000));
    // pitch: 220 Hz before, 440 Hz after the glide, in between during it
    assert!((dominant_hz(&out[0][8192..8192 + 8192]) / 220.0 - 1.0).abs() < 0.01);
    assert!((dominant_hz(&out[0][40000..40000 + 16384]) / 440.0 - 1.0).abs() < 0.01);
    let mid = dominant_hz(&out[0][24000 + 1500..24000 + 1500 + 2048]);
    assert!(mid > 230.0 && mid < 430.0, "mid-glide {mid}");
    // releasing the first (no longer sounding) note does not stop the second
    assert!(env(38000) > 0.2);
}

/// Known-answer: the sampler at root key and equal rates reproduces the sample exactly;
/// reverse reproduces it backwards.
#[test]
fn sampler_identity_and_reverse() {
    let data = util::sine_sample(441.0, 48000.0, 0.25);
    let n = data.frames();
    for reverse in [false, true] {
        let mut s = util::sampler_with(
            data.clone(),
            &[
                ("root_key", 60.0),
                ("attack", 0.0),
                ("release", 0.0),
                ("vel_amp", 0.0),
                ("reverse", reverse as u8 as f64),
            ],
        );
        let out = play(s.as_mut(), n + 100, &[on(10, 1, 60)], Quality::Live);
        let src = &data.channels[0];
        for i in 0..n {
            let want = if reverse { src[n - 1 - i] } else { src[i] };
            assert_eq!(out[0][10 + i], want, "reverse {reverse} at {i}");
        }
        assert!(
            out[0][10 + n..].iter().all(|v| *v == 0.0),
            "stops at the end"
        );
    }
}

/// Known-answer: resampling pitch. A 440 Hz sample recorded at 44.1 kHz, root 69, played
/// at key 76 sounds at 440·2^(7/12) Hz in a 48 kHz render.
#[test]
fn sampler_pitch() {
    let data = util::sine_sample(440.0, 44100.0, 2.0);
    for q in [Quality::Live, Quality::High] {
        let mut s = util::sampler_with(data.clone(), &[("root_key", 69.0)]);
        let out = play(s.as_mut(), 1 << 16, &[on(0, 1, 76)], q);
        let f = dominant_hz(&out[0][8192..8192 + 32768]);
        let want = 440.0 * (7.0f64 / 12.0).exp2();
        assert!((f / want - 1.0).abs() < 1e-4, "{q:?} {f}");
    }
}

/// Known-answer: high quality (windowed sinc with cutoff scaling) removes the alias of a
/// tone pitched above Nyquist; live (Hermite) lets it through.
#[test]
fn sampler_high_quality_antialiasing() {
    // 15 kHz sine at 48 kHz played an octave up → 30 kHz, aliasing to 18 kHz
    let data = util::sine_sample(15000.0, 48000.0, 2.0);
    let mut level = Vec::new();
    for q in [Quality::Live, Quality::High] {
        let mut s = util::sampler_with(data.clone(), &[("root_key", 60.0)]);
        let out = play(s.as_mut(), 48000, &[on(0, 1, 72)], q);
        let a = tone_amplitude(&out[0][4800..43200], SR, 18000.0) / 0.5;
        level.push(20.0 * a.max(1e-12).log10());
    }
    eprintln!(
        "sampler alias of 30 kHz: live {:.1} dB, high {:.1} dB",
        level[0], level[1]
    );
    assert!(level[1] < -70.0, "high {}", level[1]);
    assert!(level[0] > level[1] + 30.0);
}

/// Forward loop sustains past the sample end; ping-pong too; a one-shot stops.
#[test]
fn sampler_loops() {
    let data = util::test_sample(); // 0.5 s at 44.1 kHz, file loop 0.1–0.4 s
    for (mode, sustains) in [(0.0, false), (1.0, true), (2.0, true)] {
        let mut s = util::sampler_with(data.clone(), &[("loop_mode", mode), ("root_key", 57.0)]);
        let out = play(s.as_mut(), 96000, &[on(0, 1, 57)], Quality::Live);
        let late = peak(&out[0][72000..96000]);
        assert_eq!(late > 1e-3, sustains, "mode {mode}: {late}");
    }
    // start/end loop source plays the chosen range only
    let mut s = util::sampler_with(
        data,
        &[
            ("loop_mode", 1.0),
            ("loop_source", 1.0),
            ("start", 0.5),
            ("end", 0.6),
        ],
    );
    let out = play(s.as_mut(), 48000, &[on(0, 1, 57)], Quality::Live);
    assert!(peak(&out[0][24000..]) > 1e-3);
}

/// Choke group: a new note fades the previous one within a few milliseconds.
#[test]
fn sampler_choke() {
    let data = util::sine_sample(441.0, 48000.0, 1.0);
    let ev = [on(0, 1, 60), on(4800, 2, 48)];
    let mut s = util::sampler_with(data.clone(), &[("root_key", 60.0), ("choke_group", 1.0)]);
    let out = play(s.as_mut(), 24000, &ev, Quality::Live);
    let x = &out[0][4800 + 480..24000];
    assert!(tone_amplitude(x, SR, 441.0) < 1e-4, "first note choked");
    assert!(tone_amplitude(x, SR, 220.5) > 0.1, "second note plays");
}

#[test]
fn factory() {
    let sample = util::test_sample();
    let res = util::MapResolver(HashMap::from([("factory:test".to_string(), sample)]));
    let spec = InstrumentSpec::Sampler {
        sample: Some("factory:test".into()),
        root_key: 57,
        params: BTreeMap::from([("tune".to_string(), 2.0)]),
    };
    let p = ph_instruments::create(&spec, &res).unwrap();
    assert_eq!(p.descriptor().id, "ph.sampler");
    assert_eq!(p.param(p.descriptor().param_id("root_key").unwrap()), 57.0);
    assert_eq!(p.param(p.descriptor().param_id("tune").unwrap()), 2.0);
    assert_eq!(p.save_state(), b"factory:test");

    let missing = InstrumentSpec::Sampler {
        sample: Some("nope".into()),
        root_key: 60,
        params: BTreeMap::new(),
    };
    assert!(ph_instruments::create(&missing, &res).is_err());

    let synth = InstrumentSpec::Synth {
        preset: None,
        params: BTreeMap::from([("unison_voices".to_string(), 99.0)]),
    };
    let p = ph_instruments::create(&synth, &res).unwrap();
    assert_eq!(p.descriptor().params.len(), 101);
    assert_eq!(
        p.param(p.descriptor().param_id("unison_voices").unwrap()),
        8.0
    );

    let bad = InstrumentSpec::Synth {
        preset: None,
        params: BTreeMap::from([("wobble".to_string(), 1.0)]),
    };
    assert!(
        ph_instruments::create(&bad, &res)
            .err()
            .unwrap()
            .contains("wobble")
    );
    let multi = InstrumentSpec::MultiSampler {
        library: None,
        params: BTreeMap::new(),
    };
    assert!(
        ph_instruments::create(&multi, &res)
            .err()
            .unwrap()
            .contains("M3")
    );
    let _ = Arc::new(0);
}
