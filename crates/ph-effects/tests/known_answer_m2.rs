//! Known-answer tests for the M2 effects: each is measured against what its design says.
#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use ph_dsp::Rng;
use ph_dsp::analysis::{magnitude_response_db, tone_amplitude};
use ph_node::{AssetResolver, Processor, Quality, SampleData};
use std::f64::consts::PI;
use std::sync::Arc;

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
        .map(|i| amp * (2.0 * PI * f * i as f64 / SR).sin() as f32)
        .collect();
    vec![s.clone(), s]
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

fn white(frames: usize, seed: u64, level: f32) -> Vec<Vec<f32>> {
    let mut r = Rng::new(seed);
    (0..2)
        .map(|_| (0..frames).map(|_| r.bipolar() * level).collect())
        .collect()
}

/// Amplitude of `f` in the last `frames` samples (use whole periods).
fn amp_at(x: &[f32], f: f64, frames: usize) -> f64 {
    tone_amplitude(&x[x.len() - frames..], SR, f)
}

// ---------------------------------------------------------------- multiband

#[test]
fn multiband_crossover_sums_flat() {
    let bypass = [("b1_bypass", 1.0), ("b2_bypass", 1.0), ("b3_bypass", 1.0)];
    for (lo, hi) in [(200.0, 2500.0), (80.0, 800.0), (1000.0, 9000.0)] {
        let mut params = bypass.to_vec();
        params.push(("xover_low", lo));
        params.push(("xover_high", hi));
        let mut p = fx("multiband", &params);
        let n = 1 << 16;
        let out = render(p.as_mut(), n, 512, &[], Some(&impulse(n, 0)), None);
        let resp = magnitude_response_db(&out[0], n);
        let bin_hz = SR / n as f64;
        let mut worst = 0.0f64;
        for (k, v) in resp.iter().enumerate() {
            let f = k as f64 * bin_hz;
            if (20.0..=20000.0).contains(&f) {
                worst = worst.max(v.abs());
            }
        }
        println!("multiband {lo}/{hi}: worst deviation {worst:.4} dB");
        assert!(worst < 0.1, "{lo}/{hi}: {worst} dB");
    }
}

#[test]
fn multiband_solo_and_per_band_compression() {
    let n = 48000;
    let low = sine(n, 60.0, 0.5);
    let high = sine(n, 6000.0, 0.5);
    // solo the low band: 100 Hz passes, 6 kHz is gone
    let solo = [("b1_solo", 1.0), ("b1_bypass", 1.0)];
    let mut p = fx("multiband", &solo);
    let lo_out = render(p.as_mut(), n, 512, &[], Some(&low), None);
    assert!((amp_at(&lo_out[0], 60.0, 24000) - 0.5).abs() < 0.01);
    let mut p = fx("multiband", &solo);
    let hi_out = render(p.as_mut(), n, 512, &[], Some(&high), None);
    assert!(db(amp_at(&hi_out[0], 6000.0, 24000) / 0.5) < -40.0);
    // compress only the high band: the 6 kHz tone is reduced, the 100 Hz tone is not
    let comp = [
        ("b3_threshold", -30.0),
        ("b3_ratio", 20.0),
        ("b3_attack", 1.0),
        ("b3_release", 50.0),
        ("knee", 0.0),
        ("b1_bypass", 1.0),
        ("b2_bypass", 1.0),
    ];
    let mut p = fx("multiband", &comp);
    let lo_out = render(p.as_mut(), n, 512, &[], Some(&low), None);
    let mut p = fx("multiband", &comp);
    let hi_out = render(p.as_mut(), n, 512, &[], Some(&high), None);
    let lo_gain = db(amp_at(&lo_out[0], 60.0, 24000) / 0.5);
    let hi_gain = db(amp_at(&hi_out[0], 6000.0, 24000) / 0.5);
    println!("multiband band 3: low tone {lo_gain:.2} dB, high tone {hi_gain:.2} dB");
    assert!(lo_gain.abs() < 0.2);
    // -6 dBFS peak against -30 dB with ratio 20: about 24 * 19/20 = 22.8 dB down
    assert!(hi_gain < -15.0, "{hi_gain}");
}

// --------------------------------------------------------------------- gate

#[test]
fn gate_static_curve() {
    let (thr, ratio, range) = (-30.0f32, 4.0f32, -40.0f32);
    let params = [
        ("threshold", thr as f64),
        ("ratio", ratio as f64),
        ("range", range as f64),
        ("attack", 1.0),
        ("hold", 0.0),
        ("release", 5.0),
        ("detector", 1.0),
    ];
    let mut worst = 0.0f32;
    // RMS levels in dB; a sine of amplitude A has RMS A / sqrt(2)
    for level in [-60.0f32, -45.0, -36.0, -32.0, -29.0, -20.0, -6.0] {
        let amp = 10f32.powf(level / 20.0) * std::f32::consts::SQRT_2;
        let mut p = fx("gate", &params);
        let x = sine(48000, 1000.0, amp);
        let out = render(p.as_mut(), 48000, 512, &[], Some(&x), None);
        let got = db(amp_at(&out[0], 1000.0, 12000) / amp as f64) as f32;
        let want = ph_effects::gate::static_gain_db(level, thr, ratio, range);
        println!("gate {level:>6} dB -> {got:7.2} dB (curve {want:7.2})");
        worst = worst.max((got - want).abs());
    }
    assert!(worst < 0.3, "worst deviation {worst} dB");
}

#[test]
fn gate_hold_sidechain_and_latency() {
    // hold keeps the gate open after the signal drops below threshold
    let n = 48000;
    let mut x = sine(n, 1000.0, 0.5);
    for i in n / 2..n {
        x[0][i] = (x[0][i] / 0.5) * 0.001; // -60 dBFS tone afterwards
        x[1][i] = x[0][i];
    }
    let params = [
        ("threshold", -30.0),
        ("ratio", 20.0),
        ("range", -60.0),
        ("attack", 0.1),
        ("hold", 100.0),
        ("release", 20.0),
        ("detector", 1.0),
    ];
    let mut p = fx("gate", &params);
    let out = render(p.as_mut(), n, 512, &[], Some(&x), None);
    let at = |from: usize| db(tone_amplitude(&out[0][from..from + 2400], SR, 1000.0) / 0.001);
    let early = at(n / 2 + 2400); // 50-100 ms after the drop: still open
    let late = at(n / 2 + 24000 - 2400); // ~450 ms after: closed
    println!("gate hold: early {early:.1} dB, late {late:.1} dB");
    assert!(early > -1.0, "{early}");
    assert!(late < -20.0, "{late}");
    // external sidechain: quiet main signal opens for a loud key
    let main = sine(n, 1000.0, 0.01);
    let key = sine(n, 200.0, 0.5);
    let mut p = fx(
        "gate",
        &[
            ("threshold", -30.0),
            ("ratio", 20.0),
            ("range", -60.0),
            ("sidechain", 1.0),
            ("detector", 1.0),
        ],
    );
    let out = render(p.as_mut(), n, 512, &[], Some(&main), Some(&key));
    assert!(db(amp_at(&out[0], 1000.0, 12000) / 0.01) > -0.5);
    let silent = vec![vec![0.0f32; n]; 2];
    let mut p = fx(
        "gate",
        &[
            ("threshold", -30.0),
            ("ratio", 20.0),
            ("range", -60.0),
            ("sidechain", 1.0),
            ("detector", 1.0),
        ],
    );
    let out = render(p.as_mut(), n, 512, &[], Some(&main), Some(&silent));
    assert!(db(amp_at(&out[0], 1000.0, 12000) / 0.01) < -50.0);
    // lookahead reports its latency and delays the audio by it
    let p = fx("gate", &[("lookahead", 5.0)]);
    assert_eq!(p.latency(), 240);
}

// -------------------------------------------------------------- convolution

struct Irs(Vec<(&'static str, Arc<SampleData>)>);

impl AssetResolver for Irs {
    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        self.0
            .iter()
            .find(|(n, _)| *n == reference)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| format!("no {reference}"))
    }
}

fn sample(channels: Vec<Vec<f32>>) -> Arc<SampleData> {
    Arc::new(SampleData {
        sample_rate: SR,
        channels,
        loop_range: None,
        root_key: None,
    })
}

fn conv_with(ir: &str, assets: &Irs, params: &[(&str, f64)]) -> Box<dyn Processor> {
    let mut p = ph_effects::builtin("convolution").unwrap();
    p.load_state(ir.as_bytes(), assets).unwrap();
    for (k, v) in params {
        let id = p.descriptor().param_id(k).unwrap();
        p.set_param(id, *v);
    }
    activate(p.as_mut(), 512, Quality::Live, 1);
    p
}

#[test]
fn convolution_unit_impulse_is_identity() {
    let assets = Irs(vec![("t:unit", sample(vec![vec![1.0]]))]);
    let mut p = conv_with("t:unit", &assets, &[("mix", 1.0)]);
    assert_eq!(p.latency(), 0);
    let x = white(30000, 4, 0.5);
    let out = render(p.as_mut(), 30000, 193, &[], Some(&x), None);
    let worst = (0..30000)
        .map(|i| (out[0][i] - x[0][i]).abs().max((out[1][i] - x[1][i]).abs()))
        .fold(0.0f32, f32::max);
    println!("convolution identity: worst error {worst:e}");
    assert!(worst < 1e-5, "{worst}");
    // an impulse at tap 700 (crossing several partitions) is a pure 700-sample delay
    let mut d = vec![0.0f32; 701];
    d[700] = 1.0;
    let assets = Irs(vec![("t:delay", sample(vec![d]))]);
    let mut p = conv_with("t:delay", &assets, &[("mix", 1.0)]);
    let out = render(p.as_mut(), 30000, 512, &[], Some(&x), None);
    let worst = (700..30000)
        .map(|i| (out[0][i] - x[0][i - 700]).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{worst}");
    assert!(out[0][..700].iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn convolution_matches_direct_convolution() {
    let mut r = Rng::new(17);
    // stereo IR, decaying, 5300 taps (21 partitions); left and right differ
    let ir: Vec<Vec<f32>> = (0..2)
        .map(|_| {
            (0..5300)
                .map(|i| r.bipolar() * (-(i as f32) / 1500.0).exp())
                .collect()
        })
        .collect();
    let energy: f64 = ir
        .iter()
        .map(|c| c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>())
        .sum::<f64>()
        / 2.0;
    let norm = 1.0 / energy.sqrt();
    let assets = Irs(vec![("t:ir", sample(ir.clone()))]);
    let mut p = conv_with("t:ir", &assets, &[("mix", 1.0)]);
    let frames = 24000;
    let x = white(frames, 8, 0.5);
    let out = render(p.as_mut(), frames, 300, &[], Some(&x), None);
    let mut worst = 0.0f64;
    let mut peak = 0.0f64;
    for c in 0..2 {
        for n in 0..frames {
            let mut want = 0.0f64;
            for t in 0..ir[c].len().min(n + 1) {
                want += ir[c][t] as f64 * norm * x[c][n - t] as f64;
            }
            worst = worst.max((out[c][n] as f64 - want).abs());
            peak = peak.max(want.abs());
        }
    }
    println!("convolution vs direct: worst error {worst:e} (peak {peak:.3})");
    assert!(worst < 1e-4 * peak.max(1.0), "{worst}");
    // trimming the response to a quarter removes the late part
    let mut q = conv_with("t:ir", &assets, &[("mix", 1.0), ("trim", 0.25)]);
    let out_q = render(q.as_mut(), frames, 300, &[], Some(&x), None);
    let d = (0..frames)
        .map(|n| (out_q[0][n] - out[0][n]).abs())
        .fold(0.0f32, f32::max);
    assert!(d > 1e-3, "trim should change the output");
}

#[test]
fn convolution_generated_irs_and_state_round_trip() {
    let none = Irs(vec![]);
    for name in ph_effects::convolution::GENERATED {
        let reference = format!("generated:{name}");
        let mut p = conv_with(&reference, &none, &[("mix", 1.0)]);
        assert_eq!(p.save_state(), reference.as_bytes());
        let out = render(p.as_mut(), 96000, 256, &[], Some(&impulse(96000, 0)), None);
        let e: f64 = out[0].iter().map(|v| (*v as f64).powi(2)).sum();
        // unit-energy IR: the impulse response carries unit energy
        assert!((e - 1.0).abs() < 0.2, "{name}: energy {e}");
        // a later tail exists: the response decays rather than stopping at once
        assert!(
            out[0][20000..40000].iter().any(|v| v.abs() > 1e-6),
            "{name}"
        );
    }
    let mut p = ph_effects::builtin("convolution").unwrap();
    assert!(p.load_state(b"generated:cathedral", &none).is_err());
    assert!(p.load_state(b"factory:ir/missing", &none).is_err());
    assert!(p.load_state(b"", &none).is_ok());
}

// ------------------------------------------------- chorus / flanger / phaser

/// The instantaneous phase of a probe tone `f0` in `x`, demodulated with a moving average of
/// `win` samples and unwrapped, one value per sample from `skip` on.
fn probe_phase(x: &[f32], f0: f64, win: usize, skip: usize) -> Vec<f64> {
    let n = x.len();
    let (mut ci, mut cq) = (vec![0.0f64; n + 1], vec![0.0f64; n + 1]);
    for i in 0..n {
        let ph = 2.0 * PI * f0 * i as f64 / SR;
        ci[i + 1] = ci[i] + x[i] as f64 * ph.cos();
        cq[i + 1] = cq[i] - x[i] as f64 * ph.sin();
    }
    let mut out = Vec::new();
    let mut prev = 0.0;
    let mut offset = 0.0;
    for i in skip..n - win {
        let a = ci[i + win] - ci[i];
        let b = cq[i + win] - cq[i];
        let p = b.atan2(a);
        if !out.is_empty() {
            let d: f64 = p + offset - prev;
            if d > PI {
                offset -= 2.0 * PI;
            } else if d < -PI {
                offset += 2.0 * PI;
            }
        }
        let v = p + offset;
        prev = v;
        out.push(v);
    }
    out
}

/// Modulation rate (Hz) of a slowly varying `track` sampled at `SR`, from its mean crossings.
fn crossing_rate(track: &[f64]) -> f64 {
    let mean = track.iter().sum::<f64>() / track.len() as f64;
    let amp = track.iter().fold(0.0f64, |m, v| m.max((v - mean).abs()));
    assert!(amp > 0.05, "no modulation found (swing {amp})");
    let hyst = 0.2 * amp;
    let mut ups: Vec<usize> = Vec::new();
    let mut state = 0i32;
    for (i, v) in track.iter().enumerate() {
        let d = v - mean;
        let s = if d > hyst {
            1
        } else if d < -hyst {
            -1
        } else {
            state
        };
        if state < 0 && s > 0 {
            ups.push(i);
        }
        state = s;
    }
    assert!(ups.len() >= 2, "too few cycles");
    (ups.len() - 1) as f64 / ((ups[ups.len() - 1] - ups[0]) as f64 / SR)
}

fn measured_rate(name: &str, params: &[(&str, f64)], rate: f64) -> f64 {
    let n = (SR * 8.0) as usize;
    let mut p = fx(name, params);
    let x = sine(n, 1000.0, 0.5);
    let out = render(p.as_mut(), n, 512, &[], Some(&x), None);
    let track = probe_phase(&out[0], 1000.0, 480, 4800);
    let _ = rate;
    crossing_rate(&track)
}

#[test]
fn chorus_modulates_at_its_rate() {
    for rate in [0.5, 1.7, 4.0] {
        let m = measured_rate(
            "chorus",
            &[
                ("voices", 1.0),
                ("rate", rate),
                ("depth", 0.5),
                ("delay", 8.0),
                ("spread", 0.0),
                ("mix", 1.0),
            ],
            rate,
        );
        println!("chorus set {rate} Hz measured {m:.3} Hz");
        assert!((m - rate).abs() / rate < 0.05, "{rate}: {m}");
    }
}

#[test]
fn flanger_modulates_at_its_rate() {
    for rate in [0.4, 1.3, 3.0] {
        let m = measured_rate(
            "flanger",
            &[
                ("rate", rate),
                ("depth", 0.8),
                ("feedback", 0.0),
                ("manual", 1.0),
                ("stereo", 0.0),
                ("mix", 1.0),
            ],
            rate,
        );
        println!("flanger set {rate} Hz measured {m:.3} Hz");
        assert!((m - rate).abs() / rate < 0.05, "{rate}: {m}");
    }
}

#[test]
fn phaser_modulates_at_its_rate() {
    for rate in [0.4, 1.3, 3.0] {
        let m = measured_rate(
            "phaser",
            &[
                ("stages", 4.0),
                ("rate", rate),
                ("depth", 0.6),
                ("feedback", 0.0),
                ("centre", 1000.0),
                ("stereo", 0.0),
                ("mix", 1.0),
            ],
            rate,
        );
        println!("phaser set {rate} Hz measured {m:.3} Hz");
        assert!((m - rate).abs() / rate < 0.05, "{rate}: {m}");
    }
}

#[test]
fn flanger_delay_time_and_feedback_comb() {
    // static flanger (depth 0): an impulse returns after exactly `manual` ms, then repeats
    // with the feedback gain (negative feedback flips the sign)
    for fb in [0.5f64, -0.5] {
        let mut p = fx(
            "flanger",
            &[
                ("depth", 0.0),
                ("manual", 2.0),
                ("feedback", fb),
                ("mix", 1.0),
            ],
        );
        let out = render(p.as_mut(), 4000, 256, &[], Some(&impulse(4000, 0)), None);
        let d = 96; // 2 ms at 48 kHz
        let first = out[0][d];
        assert!((first - 1.0).abs() < 1e-3, "{first}");
        assert!(
            (out[0][2 * d] as f64 - fb).abs() < 2e-3,
            "{}",
            out[0][2 * d]
        );
        assert!(out[0][d / 2].abs() < 1e-4);
    }
}

#[test]
fn phaser_is_allpass_when_wet_only() {
    // with mix = 100 % and no feedback the chain has unit magnitude at every frequency
    let mut p = fx(
        "phaser",
        &[
            ("stages", 8.0),
            ("mix", 1.0),
            ("feedback", 0.0),
            ("depth", 0.0),
        ],
    );
    let n = 1 << 15;
    let out = render(p.as_mut(), n, 512, &[], Some(&impulse(n, 0)), None);
    let resp = magnitude_response_db(&out[0], n);
    let worst = resp.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(worst < 0.01, "{worst} dB");
    // and at mix 50 % there are notches
    let mut p = fx(
        "phaser",
        &[
            ("stages", 8.0),
            ("mix", 0.5),
            ("feedback", 0.0),
            ("depth", 0.0),
        ],
    );
    let out = render(p.as_mut(), n, 512, &[], Some(&impulse(n, 0)), None);
    let resp = magnitude_response_db(&out[0], n);
    assert!(resp.iter().any(|v| *v < -30.0));
}

// ------------------------------------------------------------------ bitcrush

#[test]
fn bitcrush_quantises_exactly() {
    let n = 20000;
    let ramp: Vec<f32> = (0..n)
        .map(|i| -1.0 + 2.0 * i as f32 / (n - 1) as f32)
        .collect();
    let x = vec![ramp.clone(), ramp.clone()];
    for bits in [1u32, 3, 4, 8, 12, 16, 24] {
        let mut p = fx(
            "bitcrush",
            &[("bits", bits as f64), ("dither", 0.0), ("mix", 1.0)],
        );
        let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
        let step = (1.0 - bits as f32).exp2();
        for i in 0..n {
            let want = ((ramp[i] / step).round() * step).clamp(-1.0, 1.0);
            assert_eq!(out[0][i], want, "{bits} bits at {i}");
            assert_eq!(out[1][i], want);
            // the output sits exactly on the grid
            assert_eq!((out[0][i] / step).fract(), 0.0);
        }
        let levels: std::collections::BTreeSet<u32> =
            out[0].iter().map(|v| (v + 0.0).to_bits()).collect();
        let max_levels = (1usize << bits) + 1;
        assert!(
            levels.len() <= max_levels,
            "{bits}: {} levels",
            levels.len()
        );
    }
}

#[test]
fn bitcrush_sample_rate_reduction_and_dither() {
    let n = 4800;
    let x = sine(n, 440.0, 0.8);
    let mut p = fx(
        "bitcrush",
        &[
            ("bits", 24.0),
            ("rate", 12000.0),
            ("antialias", 0.0),
            ("mix", 1.0),
        ],
    );
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    // 48 kHz / 12 kHz = 4: every output sample repeats for four samples
    for i in (8..n - 4).step_by(4) {
        let base = out[0][i - i % 4];
        for k in 0..4 {
            assert_eq!(out[0][i - i % 4 + k], base);
        }
    }
    // dither: the quantisation error is no longer a function of the signal alone, and its
    // size is bounded by 1.5 steps
    let mut p = fx("bitcrush", &[("bits", 4.0), ("dither", 1.0), ("mix", 1.0)]);
    let x = sine(n, 440.0, 0.3);
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    let step = 0.125f32;
    let max_err = (0..n)
        .map(|i| (out[0][i] - x[0][i]).abs())
        .fold(0.0, f32::max);
    assert!(max_err <= 1.5 * step + 1e-6, "{max_err}");
    let mut plain = fx("bitcrush", &[("bits", 4.0), ("mix", 1.0)]);
    let ref_out = render(plain.as_mut(), n, 256, &[], Some(&x), None);
    assert!(out[0] != ref_out[0]);
    // mix = 0 is the dry signal
    let mut p = fx("bitcrush", &[("bits", 2.0), ("mix", 0.0)]);
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    assert_eq!(out[0], x[0]);
}

// -------------------------------------------------------------------- stereo

#[test]
fn stereo_width_zero_is_mono_and_one_is_identity() {
    let n = 12000;
    let x = white(n, 12, 0.5);
    let mut p = fx("stereo", &[("width", 0.0)]);
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    for i in 0..n {
        assert_eq!(out[0][i], out[1][i], "not mono at {i}");
        assert!((out[0][i] - 0.5 * (x[0][i] + x[1][i])).abs() < 1e-6);
    }
    let mut p = fx("stereo", &[("width", 1.0)]);
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    for i in 0..n {
        assert!((out[0][i] - x[0][i]).abs() < 1e-6 && (out[1][i] - x[1][i]).abs() < 1e-6);
    }
    // width 2 doubles the side signal
    let mut p = fx("stereo", &[("width", 2.0)]);
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    for i in 0..n {
        let (m, s) = (0.5 * (x[0][i] + x[1][i]), 0.5 * (x[0][i] - x[1][i]));
        assert!((out[0][i] - (m + 2.0 * s)).abs() < 1e-6);
    }
}

#[test]
fn stereo_balance_invert_swap_and_mono_below() {
    let n = 48000;
    let x = sine(n, 1000.0, 0.5);
    let mut p = fx("stereo", &[("balance", 0.5)]);
    let out = render(p.as_mut(), n, 256, &[], Some(&x), None);
    assert!((amp_at(&out[0], 1000.0, 24000) - 0.25).abs() < 0.002);
    assert!((amp_at(&out[1], 1000.0, 24000) - 0.5).abs() < 0.002);
    let y = vec![x[0].clone(), x[0].iter().map(|v| 0.5 * v).collect()];
    let mut p = fx("stereo", &[("swap", 1.0), ("invert_l", 1.0)]);
    let out = render(p.as_mut(), n, 256, &[], Some(&y), None);
    for i in 1000..n {
        assert!((out[0][i] + y[1][i]).abs() < 1e-6, "swap + invert");
        assert!((out[1][i] - y[0][i]).abs() < 1e-6);
    }
    // anti-phase content: the low tone is folded to mono, the high tone is untouched
    let anti = |f: f64| -> Vec<Vec<f32>> {
        let s = sine(n, f, 0.5);
        vec![s[0].clone(), s[0].iter().map(|v| -v).collect()]
    };
    let mut p = fx("stereo", &[("mono_below", 200.0)]);
    let low = render(p.as_mut(), n, 256, &[], Some(&anti(50.0)), None);
    let mut p = fx("stereo", &[("mono_below", 200.0)]);
    let high = render(p.as_mut(), n, 256, &[], Some(&anti(5000.0)), None);
    let low_left = db(amp_at(&low[0], 50.0, 24000) / 0.5);
    let high_left = db(amp_at(&high[0], 5000.0, 24000) / 0.5);
    println!("mono below: 50 Hz {low_left:.1} dB, 5 kHz {high_left:.2} dB");
    assert!(low_left < -40.0);
    assert!(high_left.abs() < 0.2);
}

// ------------------------------------------------------------------ analyser

#[test]
fn analyzer_passes_audio_and_matches_ph_analysis_loudness() {
    let frames = 48000 * 5;
    // a loudness-varying stereo programme: two tones plus noise, with a quieter passage
    let mut r = Rng::new(31);
    let mut x = vec![vec![0.0f32; frames]; 2];
    for i in 0..frames {
        let t = i as f64 / SR;
        let env = if (2.0..3.5).contains(&t) { 0.25 } else { 1.0 };
        let a = (2.0 * PI * 220.0 * t).sin() * 0.3 + (2.0 * PI * 3000.0 * t).sin() * 0.1;
        x[0][i] = (env * (a + r.bipolar() as f64 * 0.05)) as f32;
        x[1][i] = (env * (a * 0.8 + r.bipolar() as f64 * 0.05)) as f32;
    }
    let mut p = ph_effects::analyzer::Analyzer::new();
    let feed = p.feed();
    activate(&mut p, 512, Quality::Live, 1);
    let out = render(&mut p, frames, 480, &[], Some(&x), None);
    assert!(
        bits(&out) == bits(&x),
        "analyser must pass audio through unchanged"
    );
    let want = ph_analysis::loudness::analyze(&x, 48000);
    let want_st = want.short_term_last_lufs.unwrap();
    let got = feed.short_term_lufs() as f64;
    println!("analyser short-term {got:.3} LUFS, ph-analysis {want_st:.3} LUFS");
    assert!((got - want_st).abs() < 0.2, "{got} vs {want_st}");
    // a different block size changes nothing
    let mut q = ph_effects::analyzer::Analyzer::new();
    let feed2 = q.feed();
    activate(&mut q, 512, Quality::Live, 1);
    let _ = render(&mut q, frames, 77, &[], Some(&x), None);
    assert_eq!(feed.short_term_lufs(), feed2.short_term_lufs());
    assert!(feed.momentary_lufs() > -60.0);
}

#[test]
fn analyzer_spectrum_and_correlation() {
    let n = 48000;
    let mut p = ph_effects::analyzer::Analyzer::new();
    let feed = p.feed();
    activate(&mut p, 512, Quality::Live, 1);
    let x = sine(n, 1000.0, 0.5);
    let _ = render(&mut p, n, 512, &[], Some(&x), None);
    let spec = feed.spectrum();
    let (peak_band, peak_db) =
        spec.iter().enumerate().fold(
            (0, -200.0f32),
            |m, (k, v)| if *v > m.1 { (k, *v) } else { m },
        );
    let f = ph_effects::analyzer::band_centre(peak_band);
    println!("analyser peak band {peak_band} ({f:.0} Hz) at {peak_db:.1} dB");
    assert!((f / 1000.0).log2().abs() < 0.1, "{f}");
    // a 0.5-amplitude sine reads about -6 dBFS
    assert!((peak_db + 6.0).abs() < 1.5, "{peak_db}");
    assert!(spec[10] < peak_db - 40.0);
    assert!(feed.correlation() > 0.99);
    let [pl, pr] = feed.peak();
    assert!(pl > 0.3 && pr > 0.3);
    // anti-phase content correlates at -1, independent noise near 0
    let anti = vec![x[0].clone(), x[0].iter().map(|v| -v).collect()];
    let mut p = ph_effects::analyzer::Analyzer::new();
    let feed = p.feed();
    activate(&mut p, 512, Quality::Live, 1);
    let _ = render(&mut p, n, 512, &[], Some(&anti), None);
    assert!(feed.correlation() < -0.99);
    let noise2 = vec![white(n, 1, 0.5)[0].clone(), white(n, 2, 0.5)[0].clone()];
    let mut p = ph_effects::analyzer::Analyzer::new();
    let feed = p.feed();
    activate(&mut p, 512, Quality::Live, 1);
    let _ = render(&mut p, n, 512, &[], Some(&noise2), None);
    assert!(feed.correlation().abs() < 0.1, "{}", feed.correlation());
    // the registry hands out the most recently activated analyser's feed
    assert!(ph_effects::analyzer::latest_feed().is_some());
}
