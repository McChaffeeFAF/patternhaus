//! Known-answer tests: each effect is measured against what its design says it does.

mod common;

use common::*;
use ph_dsp::analysis::{inharmonic_ratio_db_below, magnitude_response_db, peak, tone_amplitude};
use ph_dsp::filter::{BiquadCoefs, BiquadShape};
use ph_dsp::interp::SincInterpolator;
use ph_dsp::oversample::TruePeak4x;
use ph_node::{ProcessStatus, Processor, Quality};

fn fx(name: &str, params: &[(&str, f64)]) -> Box<dyn Processor> {
    let mut p = ph_effects::builtin(name).unwrap();
    for (k, v) in params {
        let id = p.descriptor().param_id(k).unwrap_or_else(|| panic!("{k}"));
        p.set_param(id, *v);
    }
    activate(p.as_mut(), 512, Quality::Live, 1);
    p
}

fn impulse(frames: usize, at: usize) -> Vec<Vec<f32>> {
    let mut x = vec![vec![0.0; frames]; 2];
    x[0][at] = 1.0;
    x[1][at] = 1.0;
    x
}

fn sine(frames: usize, f: f64, amp: f32) -> Vec<Vec<f32>> {
    let s: Vec<f32> = (0..frames)
        .map(|i| amp * (2.0 * std::f64::consts::PI * f * i as f64 / SR).sin() as f32)
        .collect();
    vec![s.clone(), s]
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// EQ: impulse response equals the product of the analytic band responses.
#[test]
fn eq_matches_analytic_response() {
    let mut p = fx(
        "eq",
        &[
            ("b1_on", 1.0),
            ("b1_type", 3.0), // low cut
            ("b1_freq", 60.0),
            ("b1_slope", 3.0), // 48 dB/oct
            ("b2_gain", -4.0), // low shelf 100 Hz
            ("b3_gain", 6.0),  // peak 300 Hz
            ("b3_q", 2.0),
            ("b4_on", 0.0),
            ("b5_gain", -9.0), // peak 3.5 kHz
            ("b6_gain", 3.0),  // high shelf 10 kHz
        ],
    );
    let n = 1 << 16;
    let out = render(p.as_mut(), n, 512, &[], Some(&impulse(n, 0)), None);
    let resp = magnitude_response_db(&out[0], n);
    let bands = [
        (
            BiquadShape::LowShelf { gain_db: -4.0 },
            100.0,
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        (BiquadShape::Peak { gain_db: 6.0 }, 300.0, 2.0),
        (
            BiquadShape::Peak { gain_db: -9.0 },
            3500.0,
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        (
            BiquadShape::HighShelf { gain_db: 3.0 },
            10000.0,
            std::f64::consts::FRAC_1_SQRT_2,
        ),
    ];
    let butter = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
    let mut worst = 0.0f64;
    for k in [10usize, 100, 300, 700, 1500, 4000, 8000, 15000, 20000] {
        let hz = k as f64 * SR / n as f64 * (n as f64 / SR); // exact bin centre for `k` Hz
        let bin = (hz * n as f64 / SR).round() as usize;
        let f = bin as f64 * SR / n as f64;
        let mut expect: f64 = bands
            .iter()
            .map(|(s, fc, q)| BiquadCoefs::design(*s, *fc, *q, SR).magnitude_db(f, SR))
            .sum();
        expect += butter
            .iter()
            .map(|q| BiquadCoefs::design(BiquadShape::HighPass, 60.0, *q, SR).magnitude_db(f, SR))
            .sum::<f64>();
        let got = resp[bin];
        if expect > -60.0 {
            worst = worst.max((got - expect).abs());
        }
    }
    eprintln!("EQ: worst deviation from analytic response {worst:.4} dB");
    assert!(worst < 0.01, "{worst}");
    // 48 dB/oct below the cut
    let b = |hz: f64| resp[(hz * n as f64 / SR).round() as usize];
    let slope = b(30.0) - b(15.0);
    assert!((slope - 48.0).abs() < 3.0, "slope {slope}");
}

/// Compressor: steady-state gain on a constant level matches the static curve.
#[test]
fn compressor_static_curve() {
    let (thr, ratio, knee, makeup) = (-20.0f32, 4.0f32, 6.0f32, 2.0f32);
    let mut worst = 0.0f32;
    for level_db in [
        -40.0f32, -26.0, -23.0, -21.0, -20.0, -18.0, -17.0, -10.0, -3.0, 0.0,
    ] {
        let mut p = fx(
            "compressor",
            &[
                ("threshold", thr as f64),
                ("ratio", ratio as f64),
                ("knee", knee as f64),
                ("makeup", makeup as f64),
                ("attack", 1.0),
                ("release", 20.0),
            ],
        );
        let lv = ph_dsp::db_to_lin(level_db);
        let frames = 48000;
        let x = vec![vec![lv; frames]; 2];
        let out = render(p.as_mut(), frames, 256, &[], Some(&x), None);
        let got = ph_dsp::lin_to_db(out[0][frames - 1]) - level_db;
        let want =
            ph_effects::compressor::static_curve(level_db, thr, ratio, knee) - level_db + makeup;
        worst = worst.max((got - want).abs());
    }
    eprintln!("compressor: worst static-curve error {worst:.4} dB");
    assert!(worst < 0.01, "{worst}");
}

/// Compressor: lookahead delays audio by exactly `latency()`; the external sidechain
/// drives the gain.
#[test]
fn compressor_lookahead_and_sidechain() {
    let mut p = fx("compressor", &[("lookahead", 2.0)]);
    let lat = p.latency() as usize;
    assert_eq!(lat, 96);
    let out = render(p.as_mut(), 1000, 64, &[], Some(&impulse(1000, 10)), None);
    let at = out[0].iter().position(|v| v.abs() > 0.0).unwrap();
    assert_eq!(at, 10 + lat);

    // a loud key on the sidechain ducks a quiet main signal
    let frames = 24000;
    let main = vec![vec![0.1f32; frames]; 2];
    let key = vec![vec![1.0f32; frames]; 2];
    let mut ext = fx("compressor", &[("sidechain", 1.0), ("ratio", 10.0)]);
    let o = render(ext.as_mut(), frames, 256, &[], Some(&main), Some(&key));
    assert!(
        o[0][frames - 1] < 0.1 * ph_dsp::db_to_lin(-14.0),
        "{}",
        o[0][frames - 1]
    );
    let mut int = fx("compressor", &[("sidechain", 0.0), ("ratio", 10.0)]);
    let o = render(int.as_mut(), frames, 256, &[], Some(&main), Some(&key));
    assert!(
        (o[0][frames - 1] - 0.1).abs() < 1e-3,
        "internal ignores the key"
    );
}

/// True peak of a signal measured at 16× with a long windowed sinc (independent of the
/// limiter's own 4× detector).
fn true_peak_16x(x: &[f32]) -> f32 {
    let s = SincInterpolator::new(32, 1.0);
    let fetch = |i: i64| {
        if i < 0 || i as usize >= x.len() {
            0.0
        } else {
            x[i as usize]
        }
    };
    let mut m = 0.0f32;
    for i in 0..x.len() {
        for k in 0..16 {
            m = m.max(s.interpolate(i as f64 + k as f64 / 16.0, 1.0, fetch).abs());
        }
    }
    m
}

/// Limiter: true-peak output never exceeds the ceiling (4× measurement), and a 16×
/// measurement stays within 0.1 dB; latency is as reported.
#[test]
fn limiter_ceiling_holds() {
    let frames = 96000;
    // loud noise + intersample-peak tones (fs/4 at 45°) + transients, band-limited to
    // 20 kHz like real program material (content right at Nyquist has no well-defined
    // true peak: every reconstruction filter's transition band treats it differently)
    let mut r = ph_dsp::Rng::new(4);
    let x: Vec<Vec<f32>> = (0..2)
        .map(|c| {
            (0..frames)
                .map(|i| {
                    let t = (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4)
                        .sin() as f32;
                    let burst = if (i / 4000) % 2 == 0 { 1.5 } else { 0.2 };
                    let click = if i % 9001 == c * 13 { 4.0 } else { 0.0 };
                    (r.bipolar() * 0.8 + t) * burst + click
                })
                .collect::<Vec<f32>>()
        })
        .map(|c| {
            let mut lp: Vec<_> = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4]
                .iter()
                .map(|q| {
                    ph_dsp::filter::Biquad::new(BiquadCoefs::design(
                        BiquadShape::LowPass,
                        20000.0,
                        *q,
                        SR,
                    ))
                })
                .collect();
            c.iter()
                .map(|v| lp.iter_mut().fold(*v as f64, |a, f| f.process(a)) as f32)
                .collect()
        })
        .collect();
    for (ceiling, release) in [(-1.0, 100.0), (-0.1, 5.0), (-6.0, 400.0)] {
        let mut p = fx(
            "limiter",
            &[
                ("ceiling", ceiling),
                ("release", release),
                ("input_gain", 6.0),
            ],
        );
        let out = render(p.as_mut(), frames, 128, &[], Some(&x), None);
        let ceil = ph_dsp::db_to_lin(ceiling as f32);
        let mut tp4 = 0.0f32;
        for c in &out {
            let mut d = TruePeak4x::new();
            for v in c {
                tp4 = tp4.max(d.process(*v));
            }
        }
        let tp16 = out.iter().map(|c| true_peak_16x(c)).fold(0.0, f32::max);
        eprintln!(
            "limiter ceiling {ceiling} dB: 4x true peak {:.3} dB, 16x {:.3} dB",
            ph_dsp::lin_to_db(tp4),
            ph_dsp::lin_to_db(tp16)
        );
        assert!(tp4 <= ceil * 1.000_01, "4x tp {} > {}", tp4, ceil);
        assert!(
            tp16 <= ceil * ph_dsp::db_to_lin(0.05),
            "16x tp over ceiling"
        );
        // it is limiting, not just attenuating everything
        assert!(peak(&out[0]) > ceil * 0.8);
    }
    let mut p = fx("limiter", &[("lookahead", 2.0)]);
    let lat = p.latency() as usize;
    let out = render(p.as_mut(), 2000, 64, &[], Some(&impulse(2000, 100)), None);
    let at = out[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert_eq!(at, 100 + lat);
}

/// Reverb: impulse-response decay (Schroeder backward integration) matches `decay` as an
/// RT60, and the tail reported by `tail()` is below −80 dB.
#[test]
fn reverb_decay_matches_rt60() {
    for rt in [1.0, 2.5, 6.0] {
        let mut p = fx(
            "reverb",
            &[
                ("decay", rt),
                ("mix", 1.0),
                ("damping", 0.0),
                ("predelay", 0.0),
                ("low_cut", 20.0),
                ("high_cut", 20000.0),
            ],
        );
        let tail = p.tail() as usize;
        let frames = tail + 48000;
        let out = render(
            p.as_mut(),
            frames,
            512,
            &[],
            Some(&impulse(frames, 0)),
            None,
        );
        let e: Vec<f64> = (0..frames)
            .map(|i| (out[0][i] as f64).powi(2) + (out[1][i] as f64).powi(2))
            .collect();
        let mut edc = vec![0.0f64; frames + 1];
        for i in (0..frames).rev() {
            edc[i] = edc[i + 1] + e[i];
        }
        let edc_db = |i: usize| 10.0 * (edc[i] / edc[0]).max(1e-30).log10();
        let t_at = |lvl: f64| (0..frames).find(|&i| edc_db(i) < lvl).unwrap() as f64 / SR;
        // T30: −5 → −35 dB, extrapolated to 60 dB
        let t30 = 2.0 * (t_at(-35.0) - t_at(-5.0));
        let at_tail = edc_db(tail);
        eprintln!("reverb RT60 {rt} s: measured T30 {t30:.2} s, EDC at tail() {at_tail:.1} dB");
        assert!((t30 / rt - 1.0).abs() < 0.2, "rt {rt} measured {t30}");
        assert!(at_tail < -80.0, "{at_tail}");
        // stereo output is decorrelated
        let corr: f64 = (0..frames)
            .map(|i| out[0][i] as f64 * out[1][i] as f64)
            .sum::<f64>()
            / edc[0].max(1e-30)
            * 2.0;
        assert!(corr.abs() < 0.5, "L/R correlation {corr}");
    }
    // goes to sleep once the tail has passed
    let mut p = fx("reverb", &[("decay", 0.5)]);
    let frames = p.tail() as usize + 2048;
    let mut last = ProcessStatus::Continue;
    let x = impulse(frames, 0);
    let mut out = vec![vec![0.0; frames]; 2];
    let mut ev = Vec::new();
    let zeros = vec![0.0; 512];
    let mut n = 0;
    let sleeps = render_with(
        p.as_mut(),
        frames,
        &mut || {
            n += 1;
            512
        },
        &mut |_, _| {},
        &[],
        Some(&x),
        None,
        &mut out,
        &mut ev,
        &zeros,
    );
    if sleeps > 0 {
        last = ProcessStatus::Sleep;
    }
    assert_eq!(last, ProcessStatus::Sleep);
}

/// Delay: echoes land at the synced time with feedback^n gain; ping-pong alternates sides.
#[test]
fn delay_echo_times_and_gains() {
    // 1/8 at 120 bpm = 0.25 s = 12000 samples
    let mut p = fx(
        "delay",
        &[
            ("div_l", 7.0),
            ("div_r", 7.0),
            ("feedback", 0.5),
            ("mix", 1.0),
            ("low_cut", 20.0),
            ("high_cut", 20000.0),
        ],
    );
    let frames = 48000;
    let out = render(
        p.as_mut(),
        frames,
        300,
        &[],
        Some(&impulse(frames, 0)),
        None,
    );
    // the first echo is read before the loop filters: exact time and gain
    assert_eq!(out[0][12000], 1.0);
    assert!(out[0][..12000].iter().all(|v| *v == 0.0));
    // repeat gains, measured on a 1 kHz tone burst (an impulse's peak would be shaved by
    // the band-limiting loop filters)
    let burst: Vec<f32> = (0..480)
        .map(|i| {
            let w = (std::f64::consts::PI * i as f64 / 480.0).sin().powi(2);
            (w * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / SR).sin()) as f32
        })
        .collect();
    let mut x = vec![vec![0.0f32; frames]; 2];
    for c in &mut x {
        c[..480].copy_from_slice(&burst);
    }
    let mut p = fx(
        "delay",
        &[
            ("div_l", 7.0),
            ("div_r", 7.0),
            ("feedback", 0.5),
            ("mix", 1.0),
        ],
    );
    let out = render(p.as_mut(), frames, 300, &[], Some(&x), None);
    let e1 = peak(&out[0][12000..12480]);
    for k in 2..=3 {
        let ek = peak(&out[0][12000 * k - 100..12000 * k + 580]);
        let want = 0.5f32.powi(k as i32 - 1);
        assert!(
            (ek / e1 / want - 1.0).abs() < 0.03,
            "echo {k}: {} vs {want}",
            ek / e1
        );
    }

    let mut p = fx("delay", &[("mode", 1.0), ("div_l", 7.0), ("mix", 1.0)]);
    let mut x = impulse(frames, 0);
    x[1][0] = 1.0;
    let out = render(p.as_mut(), frames, 300, &[], Some(&x), None);
    let e = |c: usize, at: usize| {
        out[c][at - 100..at + 100]
            .iter()
            .map(|v| v.abs())
            .fold(0.0, f32::max)
    };
    assert!(e(0, 12000) > 0.3 && e(1, 12000) < 1e-3, "first echo left");
    assert!(e(1, 24000) > 0.1 && e(0, 24000) < 1e-3, "second echo right");
}

/// Filter: low-pass response through the oversampled path matches the TPT SVF prototype
/// (Q = 0.5 at resonance 0 → −6 dB at the cutoff); latency is as reported.
#[test]
fn filter_response_and_latency() {
    for (model, at_fc) in [(0.0, -6.02), (1.0, -12.04)] {
        let mut gains = Vec::new();
        for f in [100.0, 1000.0, 8000.0] {
            let mut p = fx(
                "filter",
                &[("model", model), ("cutoff", 1000.0), ("resonance", 0.0)],
            );
            let frames = 48000;
            let out = render(
                p.as_mut(),
                frames,
                256,
                &[],
                Some(&sine(frames, f, 0.01)),
                None,
            );
            gains.push(db(tone_amplitude(&out[0][24000..], SR, f) / 0.01));
        }
        eprintln!(
            "filter model {model}: 100 Hz {:.2} dB, 1 kHz {:.2} dB, 8 kHz {:.2} dB",
            gains[0], gains[1], gains[2]
        );
        assert!(gains[0].abs() < 0.3, "{gains:?}");
        assert!((gains[1] - at_fc).abs() < 0.3, "{gains:?}");
        assert!(gains[2] < at_fc - 30.0, "{gains:?}");
    }
    let mut p = fx("filter", &[("cutoff", 20000.0), ("mix", 0.0)]);
    let lat = p.latency() as usize;
    let out = render(p.as_mut(), 2000, 64, &[], Some(&impulse(2000, 100)), None);
    assert_eq!(out[0][100 + lat], 1.0, "dry path is latency-aligned");
}

/// Saturator: a hard-clipped 5 kHz sine aliases far less (in the audible band) than naive
/// clipping thanks to oversampling + ADAA; a small signal through the soft curve stays nearly linear.
#[test]
fn saturator_aliasing_and_linearity() {
    let frames = 1 << 16;
    let f0 = 4987.0;
    let mut p = fx("saturator", &[("curve", 1.0), ("drive", 18.0)]);
    let out = render(
        p.as_mut(),
        frames + 4096,
        512,
        &[],
        Some(&sine(frames + 4096, f0, 0.9)),
        None,
    );
    let os = inharmonic_ratio_db_below(&out[0][4096..], SR, f0, 8, 20000.0);
    let naive: Vec<f32> = sine(frames, f0, 0.9)[0]
        .iter()
        .map(|v| (v * ph_dsp::db_to_lin(18.0)).clamp(-1.0, 1.0))
        .collect();
    let nv = inharmonic_ratio_db_below(&naive, SR, f0, 8, 20000.0);
    eprintln!("saturator hard clip aliasing: oversampled {os:.1} dB, naive {nv:.1} dB");
    assert!(os < nv - 40.0 && os < -60.0, "os {os} naive {nv}");

    let mut p = fx("saturator", &[("curve", 0.0), ("drive", 0.0)]);
    let out = render(
        p.as_mut(),
        48000,
        512,
        &[],
        Some(&sine(48000, 1000.0, 0.01)),
        None,
    );
    let g = tone_amplitude(&out[0][24000..], SR, 1000.0) / 0.01;
    assert!((db(g)).abs() < 0.05, "{}", db(g));
}
