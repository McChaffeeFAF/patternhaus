//! What the fixtures that waited for M3/T2's DSP must sound like (F-SFZ-03, 07, 08, 09, 10,
//! 11, 13, 14, 16): checks on the render itself, beside the fingerprint references.
//!
//! The phrase runs at 120 BPM, so a step is 6000 frames. Every sample is a tagged tone: its
//! first 480 frames are a DC level of `0.05 × n` (n = the sample's place in the fixture's
//! list), then a 0.4 sine at `220 × (1 + 0.25 (n − 1))` Hz. Levels are compared as ratios to
//! a reference note in the same render, so channel gain and pan law drop out.

use ph_core::*;
use ph_engine::AudioBuffer;
use ph_sampler_formats::Imported;

const STEP: usize = 6000;
const SR: f64 = 48_000.0;

fn mono(a: &AudioBuffer) -> Vec<f64> {
    (0..a.frames())
        .map(|i| a.channels.iter().map(|c| c[i] as f64).sum::<f64>() / a.channels.len() as f64)
        .collect()
}

/// Mean over `from..to` (the DC tag of a sample).
fn mean(x: &[f64], from: usize, to: usize) -> f64 {
    x[from..to].iter().sum::<f64>() / (to - from) as f64
}

fn rms(x: &[f64], from: usize, to: usize) -> f64 {
    (x[from..to].iter().map(|v| v * v).sum::<f64>() / (to - from) as f64).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// The strongest frequency in `from..to` between `lo` and `hi` Hz (0.25 Hz steps).
fn dominant_hz(x: &[f64], from: usize, to: usize, lo: f64, hi: f64) -> f64 {
    let mut best = (0.0, lo);
    let mut f = lo;
    while f <= hi {
        let w = std::f64::consts::TAU * f / SR;
        let (mut re, mut im) = (0.0, 0.0);
        for (k, v) in x[from..to].iter().enumerate() {
            re += v * (w * k as f64).cos();
            im += v * (w * k as f64).sin();
        }
        let m = re * re + im * im;
        if m > best.0 {
            best = (m, f);
        }
        f += 0.25;
    }
    best.1
}

fn near(what: &str, got: f64, want: f64, tol: f64) -> Result<(), String> {
    if (got - want).abs() <= tol * want.abs().max(1e-9) {
        Ok(())
    } else {
        Err(format!(
            "{what}: {got:.5}, expected {want:.5} (±{:.0}%)",
            tol * 100.0
        ))
    }
}

fn holds(what: &str, ok: bool) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

/// The DC tag heard at a note starting on `step` (frames 50..250 after its start).
fn tag(x: &[f64], step: usize) -> f64 {
    mean(x, step * STEP + 50, step * STEP + 250)
}

fn zone_of<'a>(i: &'a Imported, sample: &str) -> &'a Zone {
    i.instrument
        .zones
        .values()
        .find(|z| matches!(&z.sample, SampleRef::File(f) if f.ends_with(sample)))
        .unwrap()
}

/// Checks fixture `id`'s render; fixtures without a sound check pass.
pub fn check(id: &str, a: &AudioBuffer, i: &Imported) -> Result<(), String> {
    let x = mono(a);
    let r = |e: Result<(), String>| e.map_err(|m| format!("{id} sound: {m}"));
    match id {
        // velocity layers with crossfades: each note's DC tags are the layers' tags weighted
        // by their crossfade gains (power for p and f, linear for m), times the velocity
        "F-SFZ-03" => {
            let cc = [0.0f32; 128];
            let expect = |v: f32| -> f64 {
                [("p.wav", 1.0), ("m.wav", 2.0), ("f.wav", 3.0)]
                    .iter()
                    .filter(|(n, _)| {
                        let z = zone_of(i, n);
                        let v7 = midi_velocity(v);
                        (z.velocity.lo..=z.velocity.hi).contains(&v7)
                    })
                    .map(|(n, t)| {
                        let g = ph_sampler::render::crossfade_gain(
                            &zone_of(i, n).crossfades,
                            v,
                            &cc,
                            None,
                        );
                        0.05 * t * g as f64 * v as f64
                    })
                    .sum()
            };
            let k = tag(&x, 0) / expect(0.2);
            for (step, v) in [(4, 0.35f32), (8, 0.55), (12, 0.68), (16, 0.9)] {
                r(near(
                    &format!("velocity {v} layer mix"),
                    tag(&x, step),
                    k * expect(v),
                    0.03,
                ))?;
            }
            // the two in-between notes really are mixes of two layers
            r(holds(
                "velocity 0.35 sounds two layers",
                expect(0.35) > 0.05 * 0.35 * 1.05,
            ))
        }
        // CC layers and a CC crossfade, with every controller at 0 (`set_cc1` is kept as data:
        // the model has no initial controller values, see the parity table): key 60 plays the
        // low layer, key 62 only the soft side of the crossfade, key 64 nothing (its crossfade
        // on CC 2 starts at 0 and CC 3 is below its condition)
        "F-SFZ-07" => {
            let k = tag(&x, 0) / 0.05;
            r(near("CC 1 low layer alone", tag(&x, 0), k * 0.05, 1e-9))?;
            r(near(
                "crossfade at CC 0: soft only",
                tag(&x, 4),
                k * 0.15,
                0.03,
            ))?;
            r(holds(
                "CC 2 crossfade at 0 and CC 3 condition: silent",
                rms(&x, 8 * STEP, 10 * STEP) < 1e-4,
            ))
        }
        // loops: the smpl loop and the explicit ones keep sounding past the sample's length
        // while held; a sustain loop is left at note-off; one-shot ignores note-off; reverse
        // plays end to offset, reaching the DC tag last
        "F-SFZ-08" => {
            let k = tag(&x, 12) / 0.05; // the one-shot note: loop.wav from its start
            r(holds(
                "smpl loop sounds past the end of loop.wav",
                rms(&x, 15_000, 23_000) > 0.05 * k,
            ))?;
            r(holds(
                "sustain loop sounds while held",
                rms(&x, 4 * STEP + 14_000, 8 * STEP - 500) > 0.05 * k,
            ))?;
            r(holds(
                "ping-pong loop sounds while held",
                rms(&x, 8 * STEP + 14_000, 12 * STEP - 500) > 0.05 * k,
            ))?;
            r(holds(
                "one-shot plays on after its note-off",
                rms(&x, 12 * STEP + 7_000, 12 * STEP + 11_000) > 0.05 * k,
            ))?;
            // reverse from frame 3999 down to 100: plain.wav's DC tag (0.1) arrives last
            let rev = 16 * STEP + (3999 - 300);
            r(near(
                "reverse ends on the tag",
                mean(&x, rev, rev + 100),
                0.1 * k,
                0.03,
            ))?;
            r(holds(
                "reverse stops at its offset",
                rms(&x, 16 * STEP + 3950, 17 * STEP) < 1e-4,
            ))
        }
        // DAHDSR: silent through the delay, rising in the attack, full in the hold, half
        // level at sustain; the pitch envelope bends key 62 down by up to 200 cents
        "F-SFZ-09" => {
            let full = rms(&x, 1500, 2800);
            r(holds("silent during the delay", rms(&x, 0, 470) < 1e-4))?;
            r(near(
                "attack half way",
                rms(&x, 860, 1060),
                full * 0.5,
                0.15,
            ))?;
            r(near(
                "sustain at 50%",
                rms(&x, 8200, 11_500),
                full * 0.5,
                0.1,
            ))?;
            let s = 8 * STEP;
            let early = dominant_hz(&x, s + 500, s + 1500, 220.0, 300.0);
            let late = dominant_hz(&x, s + 3800, s + 4800, 220.0, 300.0);
            r(holds(
                &format!("pitch envelope bends down ({early} Hz → {late} Hz)"),
                early > 265.0 && late < 255.0,
            ))
        }
        // LFOs: amplitude LFO ±3 dB at 5 Hz, a square volume LFO ±2 dB, a pitch LFO ±30 cents
        "F-SFZ-10" => {
            let env: Vec<f64> = (0..20)
                .map(|w| db(rms(&x, 1000 + w * 480, 1480 + w * 480)))
                .collect();
            let (lo, hi) = env
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
            r(holds(
                &format!("amplitude LFO moves the level ({:.1} dB)", hi - lo),
                hi - lo > 4.0,
            ))?;
            let s = 9 * STEP; // key 63: square, 3 Hz, ±2 dB
            let d = db(rms(&x, s + 1000, s + 7000)) - db(rms(&x, s + 8500, s + 11_500));
            r(near("square volume LFO step", d, 4.0, 0.15))?;
            let s = 3 * STEP; // key 61: sine, 4 Hz, 30 cents
            let up = dominant_hz(&x, s + 2000, s + 4000, 200.0, 240.0);
            let down = dominant_hz(&x, s + 8000, s + 10_000, 200.0, 240.0);
            r(holds(
                &format!("pitch LFO ({up} Hz vs {down} Hz)"),
                up > down + 3.0,
            ))
        }
        // filters on a 220 Hz tone, cutoff around 1.6–2 kHz: low-pass and notch pass it,
        // high-pass and band-pass cut it, the bypassed peaking filter is the reference
        "F-SFZ-11" => {
            let lvl = |k: usize| {
                let s = k * 3 * STEP;
                db(rms(&x, s + 1000, s + 11_000))
            };
            let dry = lvl(7);
            for (k, name) in [(0, "lpf_1p"), (2, "lpf_2p"), (6, "lpf_4p"), (5, "brf_2p")] {
                r(holds(
                    &format!("{name} passes 220 Hz ({:.1} dB)", lvl(k) - dry),
                    (lvl(k) - dry).abs() < 3.0,
                ))?;
            }
            for (k, name, min) in [(1, "hpf_1p", 15.0), (3, "hpf_2p", 15.0), (4, "bpf_2p", 8.0)] {
                r(holds(
                    &format!("{name} cuts 220 Hz ({:.1} dB)", lvl(k) - dry),
                    dry - lvl(k) > min,
                ))?;
            }
            Ok(())
        }
        // the open hat gives way to the closed one; three notes on a two-voice group
        "F-SFZ-13" => {
            let k = tag(&x, 2) / 0.05;
            r(near("closed hat alone", tag(&x, 2), 0.05 * k, 0.03))?;
            r(near("third note alone", tag(&x, 10), 0.15 * k, 0.03))
        }
        // controller curves with every controller at 0: unity gain, centred
        "F-SFZ-14" => r(near("both groups at unity", tag(&x, 4), tag(&x, 0), 0.02)),
        // the group effect chains: echoes and reverb after the dry tone has ended
        "F-SFZ-16" => {
            r(holds(
                "reverb + delay tail after key 60",
                rms(&x, 12_500, 4 * STEP) > 1e-3,
            ))?;
            r(holds(
                "delay tail after key 62",
                rms(&x, 4 * STEP + 12_500, 8 * STEP) > 1e-3,
            ))
        }
        _ => Ok(()),
    }
}
