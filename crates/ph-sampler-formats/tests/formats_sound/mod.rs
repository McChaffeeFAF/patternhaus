//! What the SF2, DecentSampler and EXS fixtures that waited for M3/T2's DSP must sound like
//! (F-SF2-05, 07, 08; F-DS-06, 07, 08, 10, 12; F-EXS-05, 08): checks on the render itself,
//! beside the fingerprint references (the SFZ ones are in `sfz_sound`).
//!
//! The phrase runs at 120 BPM, so a step is 6000 frames at 48 kHz. Every sample is a tagged
//! tone: a short DC level of `0.05 × n` (n = the sample's place in the fixture's list), then
//! a 0.4 sine at `220 × (1 + 0.25 (n − 1))` Hz (DecentSampler numbers its tones from 0, so
//! there `n` starts at 1 but the sine at 220 Hz). SF2 samples run at 44.1 kHz, the others at
//! 48 kHz. Levels are compared with a reference stretch of the same render, so channel gain,
//! pan law and velocity response drop out.

#![allow(dead_code)]

use ph_engine::AudioBuffer;
use ph_sampler_formats::Imported;

const STEP: usize = 6000;
const SR: f64 = 48_000.0;
/// Below this RMS a stretch counts as silent.
const QUIET: f64 = 1e-4;

fn mono(a: &AudioBuffer) -> Vec<f64> {
    (0..a.frames())
        .map(|i| a.channels.iter().map(|c| c[i] as f64).sum::<f64>() / a.channels.len() as f64)
        .collect()
}

fn channel(a: &AudioBuffer, c: usize) -> Vec<f64> {
    a.channels[c].iter().map(|v| *v as f64).collect()
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
            "{what}: {got:.5}, expected {want:.5} (±{:.1}%)",
            tol * 100.0
        ))
    }
}

fn holds(what: &str, ok: bool) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

/// `what` sounds in `from..to`: at least `share` of the reference level.
fn sounds(
    x: &[f64],
    what: &str,
    from: usize,
    to: usize,
    reference: f64,
    share: f64,
) -> Result<(), String> {
    let l = rms(x, from, to);
    holds(
        &format!(
            "{what} ({:.1} dB against the reference)",
            db(l) - db(reference)
        ),
        l > reference * share,
    )
}

/// `what` is silent in `from..to`.
fn silent(x: &[f64], what: &str, from: usize, to: usize) -> Result<(), String> {
    let l = rms(x, from, to);
    holds(&format!("{what} (rms {l:.6})"), l < QUIET)
}

/// Checks fixture `id`'s render; fixtures without a sound check pass.
pub fn check(id: &str, a: &AudioBuffer, _i: &Imported) -> Result<(), String> {
    let x = mono(a);
    let r = |e: Result<(), String>| e.map_err(|m| format!("{id} sound: {m}"));
    let s = |n: usize| n * STEP;
    match id {
        // SF2 sampleModes on 44.1 kHz samples at 48 kHz: mode 0 and 2 stop at the sample's
        // end, mode 1 loops while held (also the loop that ends on the last frame), mode 3
        // loops while held
        "F-SF2-05" => {
            let reference = rms(&x, 1000, 12_000);
            // key 60: 12000 frames at 0.919 per output frame end near 13060
            r(silent(&x, "mode 0 stops at the sample's end", 13_600, s(3)))?;
            // key 62 from 100 to 11800 at 1.031: past 11 400 only the loop sounds
            r(sounds(
                &x,
                "mode 1 loops while held",
                s(4) + 14_000,
                s(10) - 500,
                reference,
                0.3,
            ))?;
            // key 64: 4464 played frames at 1.158 last 3860 frames unlooped
            r(sounds(
                &x,
                "mode 3 loops while held",
                s(12) + 6_000,
                s(18) - 500,
                reference,
                0.3,
            ))?;
            // key 65: 6000 frames at 1.227 last 4890 frames unlooped
            r(sounds(
                &x,
                "a loop ending on the last frame",
                s(20) + 7_000,
                s(24) - 500,
                reference,
                0.3,
            ))?;
            // key 67: 12000 frames at 1.377 end near 8720
            r(silent(&x, "mode 2 plays once", s(26) + 9_300, s(29)))
        }
        // the volume envelope: 0.5 s delay, 0.25 s attack, 0.125 s hold, decay towards
        // −20 dB, 0.5 s release from the note-off at 1 s; the modulation envelope bends the
        // 275 Hz tone up (+200 cents at its peak); key 75 has a zero attack
        "F-SF2-07" => {
            r(silent(&x, "silent through the delay", 0, 23_500))?;
            let full = rms(&x, 36_500, 41_500);
            r(holds(
                "the attack rises",
                rms(&x, 25_000, 27_000) < 0.6 * rms(&x, 33_000, 35_500),
            ))?;
            r(holds(
                "the decay falls after the hold",
                rms(&x, 46_000, 47_900) < 0.95 * full,
            ))?;
            r(holds(
                "the release falls",
                rms(&x, 60_000, 62_000) < 0.7 * rms(&x, 48_000, 50_000),
            ))?;
            // the release keeps falling until key 75 starts at 72 000 (its shape is the
            // sampler's own: approximated, see the parity table)
            let tail: Vec<f64> = (0..4)
                .map(|k| db(rms(&x, 48_000 + k * 6000, 50_000 + k * 6000)) - db(full))
                .collect();
            r(holds(
                &format!("the release falls steadily ({tail:.1?} dB)"),
                tail.windows(2).all(|w| w[1] < w[0] - 1.0) && tail[3] < -10.0,
            ))?;
            let hz = dominant_hz(&x, 36_500, 41_500, 250.0, 340.0);
            r(holds(
                &format!("the modulation envelope bends the pitch up ({hz} Hz, 275 Hz unbent)"),
                hz > 285.0 && hz < 315.0,
            ))?;
            let onset = rms(&x, s(12) + 500, s(12) + 1500);
            r(holds(
                "a zero attack is at full level at once",
                onset > 0.7 * rms(&x, s(12) + 3000, s(12) + 6000),
            ))
        }
        // the mod LFO (16 Hz) moves the volume by ±6 dB on key 60; key 72 has no LFO
        "F-SF2-08" => {
            // 400-frame windows over 8000 frames (key 72's sample lasts 10 880)
            let swing = |from: usize| {
                let env: Vec<f64> = (0..20)
                    .map(|w| db(rms(&x, from + w * 400, from + w * 400 + 400)))
                    .collect();
                let (lo, hi) = env
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
                hi - lo
            };
            let lfo = swing(2000);
            r(holds(
                &format!("the mod LFO moves the level ({lfo:.1} dB)"),
                lfo > 4.0,
            ))?;
            let flat = swing(s(10) + 1000);
            r(holds(
                &format!("no LFO on key 72 ({flat:.1} dB)"),
                flat < 1.5,
            ))
        }
        // loops: the file's marker and the explicit loops sound past the 12000-frame
        // sample's end while held; loopEnabled=false and a plain sample stop; the one-shot
        // plays on after its note-off
        "F-DS-06" => {
            let reference = rms(&x, 1000, 11_000);
            r(sounds(
                &x,
                "the smpl marker loop",
                13_000,
                s(4) - 500,
                reference,
                0.3,
            ))?;
            // key 62 at 1.122: unlooped it ends near 10 690
            r(sounds(
                &x,
                "the explicit equal-power loop",
                s(5) + 12_000,
                s(9) - 500,
                reference,
                0.3,
            ))?;
            // key 64 at 1.26: near 9 520
            r(sounds(
                &x,
                "the linear-crossfade loop",
                s(10) + 11_000,
                s(14) - 500,
                reference,
                0.3,
            ))?;
            // key 65 at 1.335 ends near 8 990, key 67 at 1.498 near 8 010
            r(silent(
                &x,
                "loopEnabled=false plays once",
                s(15) + 9_600,
                s(17),
            ))?;
            r(silent(&x, "no marker, no loop", s(18) + 8_600, s(20)))?;
            // key 69 at 1.682: 7 135 frames, the note is let go at 6 000
            r(sounds(
                &x,
                "the one-shot plays past its note-off",
                s(21) + 6_200,
                s(21) + 6_900,
                reference,
                0.3,
            ))?;
            r(silent(
                &x,
                "the one-shot ends with its sample",
                s(21) + 7_400,
                s(23),
            ))
        }
        // the inherited AHDSR (0.5 s attack, 0.4 s decay to 0.3) and the Long group's own
        // (50 ms delay, 10 ms attack, 0.1 s hold, sustain 0.8)
        "F-DS-07" => {
            r(holds(
                "the 0.5 s attack rises",
                rms(&x, 1000, 3000) < 0.3 * rms(&x, 21_000, 24_000),
            ))?;
            r(holds(
                "the decay falls after the attack",
                rms(&x, 29_000, 31_500) < 0.9 * rms(&x, 22_500, 25_000),
            ))?;
            let o = s(8);
            r(silent(
                &x,
                "silent through the 50 ms delay",
                o + 100,
                o + 2_300,
            ))?;
            let full = rms(&x, o + 3_500, o + 7_000);
            r(holds(
                "full level 10 ms after the delay",
                rms(&x, o + 3_000, o + 3_400) > 0.8 * full,
            ))
        }
        // silencing: the closed hat cuts the open one (the closed note's level is one tone,
        // as loud as the open hat alone)
        "F-DS-08" => {
            let open = rms(&x, s(2) + 2_000, s(3));
            let closed = rms(&x, s(4) + 2_000, s(5) - 200);
            r(near(
                "closed hat alone after the open one is silenced",
                closed,
                open,
                0.15,
            ))
        }
        // the effect chains: echoes and reverb after each dry sample has ended
        "F-DS-10" => {
            // key 55: a.wav at 0.749 ends near 16 020; key 65: b.wav at 1.335 near 8 990
            r(holds(
                "delay and reverb tail after key 55",
                rms(&x, 17_000, s(6)) > 1e-3,
            ))?;
            r(holds(
                "delay and reverb tail after key 65",
                rms(&x, s(6) + 10_000, s(12)) > 1e-3,
            ))
        }
        // modulators: a 6 Hz ±100-cent LFO on group A's pitch (key 55: 164.8 Hz), velocity
        // to group B's pitch (+50 cents at full velocity; key 65 at 0.6: 367.1 Hz + 30
        // cents), the envelope pans group B right as it opens
        "F-DS-12" => {
            let hz: Vec<f64> = (0..8)
                .map(|w| dominant_hz(&x, 2000 + w * 1000, 4000 + w * 1000, 140.0, 190.0))
                .collect();
            let (lo, hi) = hz
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
            r(holds(
                &format!("the LFO moves group A's pitch ({lo} – {hi} Hz)"),
                hi - lo > 6.0,
            ))?;
            let o = s(8);
            let want = 220.0 * 1.25 * 2f64.powf(5.0 / 12.0) * 2f64.powf(30.0 / 1200.0);
            r(near(
                "velocity raises group B's pitch",
                dominant_hz(&x, o + 4000, o + 8800, 355.0, 385.0),
                want,
                0.003,
            ))?;
            let (left, right) = (channel(a, 0), channel(a, 1));
            let bal =
                |from: usize, to: usize| rms(&right, from, to) / rms(&left, from, to).max(1e-12);
            let (early, open) = (bal(o + 200, o + 1200), bal(o + 8800, o + 10_000));
            r(holds(
                &format!("the envelope pans group B right ({early:.2} → {open:.2})"),
                open > early * 1.2,
            ))
        }
        // loops on a 12000-frame sample played at its root: sounding past its end while
        // held (forward, backward read as forward, ping-pong, play-to-end, crossfaded); a
        // zone with its loop off stops
        "F-EXS-05" => {
            let reference = rms(&x, 1000, 9000);
            r(sounds(
                &x,
                "the crossfaded forward loop",
                13_000,
                s(6) - 500,
                reference,
                0.3,
            ))?;
            for (n, what) in [
                (6, "the backward loop (played forward)"),
                (10, "the ping-pong loop"),
                (14, "the play-to-end loop while held"),
                (22, "the linear-crossfade loop"),
            ] {
                r(sounds(
                    &x,
                    what,
                    s(n) + 13_000,
                    s(n + 4) - 500,
                    reference,
                    0.3,
                ))?;
            }
            r(silent(
                &x,
                "loop off plays once",
                s(18) + 12_500,
                s(22) - 200,
            ))
        }
        // parameters: transpose 3 and +10 cents on the 275 Hz tone, the attack of
        // 10·(64/127)⁴ s rising through the note
        "F-EXS-08" => {
            let want = 275.0 * 2f64.powf(310.0 / 1200.0);
            r(near(
                "transpose and fine tune",
                dominant_hz(&x, 6000, 14_000, 300.0, 360.0),
                want,
                0.003,
            ))?;
            r(holds(
                "the attack rises",
                rms(&x, 1000, 3000) < 0.5 * rms(&x, 12_000, 15_000),
            ))
        }
        _ => Ok(()),
    }
}
