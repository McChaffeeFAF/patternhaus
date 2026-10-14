//! Objective stretch quality (M2 criterion 5): generated material through the real clip
//! player in Stretch mode, tempo-synced, at time ratios 0.7 to 1.3. Run with
//! `cargo test -p ph-engine --lib stretch_quality -- --nocapture` for the table recorded in
//! `docs/milestones/M2.md` ("Stretch quality (T1)"); the bounds asserted here are the ones
//! written there.

use super::audio::{audio_project, clip, song};
use super::*;
use crate::render_offline;

/// Project tempos against 120 BPM material: ratios 0.7 … 1.3.
const TEMPOS: [f64; 7] = [84.0, 96.0, 108.0, 120.0, 132.0, 144.0, 156.0];

// Bounds (see M2.md).
const MAX_PITCH_ERROR_CENTS: f64 = 1.0;
const MAX_PITCH_DRIFT_CENTS: f64 = 2.0;
const MAX_LEVEL_SWING_DB: f64 = 0.5;
const MAX_LEVEL_ERROR_DB: f64 = 0.5;
const MAX_ONSET_ERROR_MS: f64 = 3.0;
/// Extra time a click spends above −20 dB / −40 dB of its peak, against the original.
const MAX_SMEAR_MS: f64 = 5.0;
const MAX_SMEAR_40_MS: f64 = 25.0;

/// Renders `beats` beats of `path` (120 BPM material) stretched to `bpm`.
fn stretched(path: &str, bpm: f64, beats: Tick, quality: Quality) -> Vec<f32> {
    let p = audio_project(bpm, |e| {
        clip(e, path, Some(120.0), 0, 0, beats * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Stretch;
        });
    });
    let mut rs = song(512);
    rs.quality = quality;
    render_offline(&p, &rs, &GenAssets).unwrap().channels[0].clone()
}

fn cents(f: f64, want: f64) -> f64 {
    1200.0 * (f / want).log2()
}

/// Fundamental frequency of `x` near `want` by normalised autocorrelation (the best lag
/// within ±30% of the expected period, refined by a parabola). Works for harmonic tones,
/// whose waveform can cross zero more than once per period.
fn acf_hz(x: &[f32], want: f64) -> f64 {
    let period = SR as f64 / want;
    let (lo, hi) = ((period * 0.7) as usize, (period * 1.3) as usize + 1);
    let n = x.len() - hi;
    let r = |lag: usize| -> f64 {
        let (mut num, mut e0, mut e1) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            let (a, b) = (x[i] as f64, x[i + lag] as f64);
            num += a * b;
            e0 += a * a;
            e1 += b * b;
        }
        num / (e0 * e1).sqrt().max(1e-20)
    };
    let vals: Vec<f64> = (lo..=hi).map(r).collect();
    let (k, _) = vals.iter().enumerate().fold(
        (0, f64::MIN),
        |a, (i, v)| if *v > a.1 { (i, *v) } else { a },
    );
    let mut lag = (lo + k) as f64;
    if k > 0 && k + 1 < vals.len() {
        let (a, b, c) = (vals[k - 1], vals[k], vals[k + 1]);
        let d = a - 2.0 * b + c;
        if d < 0.0 {
            lag += 0.5 * (a - c) / d;
        }
    }
    SR as f64 / lag
}

/// Frequency per 100 ms window over the steady part (skipping the first and last 10%).
fn windowed_hz(x: &[f32], want: f64) -> Vec<f64> {
    let w = SR as usize / 10;
    let (a, b) = (x.len() / 10, x.len() * 9 / 10);
    (a..b.saturating_sub(w))
        .step_by(w)
        .map(|s| acf_hz(&x[s..s + w], want))
        .collect()
}

fn rms_db(x: &[f32]) -> f64 {
    let p = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    10.0 * p.max(1e-20).log10()
}

/// Envelope (1 ms RMS) of `x`.
fn envelope(x: &[f32]) -> Vec<f64> {
    let w = SR as usize / 1000;
    x.chunks(w)
        .map(|c| (c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len() as f64).sqrt())
        .collect()
}

/// For each expected click time (in 1 ms envelope frames): the onset error (first frame at
/// `db` below the click's peak, against the expected time) and the spread (frames above
/// that level), in ms.
fn clicks(env: &[f64], expected: &[usize], db: f64) -> Vec<(f64, f64)> {
    expected
        .iter()
        .filter(|e| **e + 200 < env.len())
        .map(|&e| {
            let lo = e.saturating_sub(150);
            let hi = (e + 200).min(env.len());
            let peak = env[lo..hi].iter().copied().fold(0.0, f64::max);
            let thr = peak * 10f64.powf(-db / 20.0);
            let first = (lo..hi).find(|i| env[*i] >= thr).unwrap_or(e);
            let above = (lo..hi).filter(|i| env[*i] >= thr).count();
            (first as f64 - e as f64, above as f64)
        })
        .collect()
}

/// Live playback (Signalsmith's cheaper preset) and high-quality renders (its default).
#[test]
fn stretch_quality_across_seventy_to_one_thirty_percent() {
    let mut failures = Vec::new();
    for quality in [Quality::Live, Quality::High] {
        failures.extend(quality_table(quality));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn quality_table(quality: Quality) -> Vec<String> {
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    // the original's spread, for reference
    let src = &gen_audio("gen:clicks").unwrap()[0];
    let src_env = envelope(src);
    let spread_of = |env: &[f64], at: &[usize], db: f64| {
        clicks(env, at, db).iter().map(|c| c.1).fold(0.0, f64::max)
    };
    let src_at = [500, 1000, 1500, 2000, 2500, 3000];
    let src_spread = spread_of(&src_env, &src_at, 20.0);
    let src_spread40 = spread_of(&src_env, &src_at, 40.0);
    let tone_db = rms_db(&gen_audio("gen:tone").unwrap()[0]);
    for &bpm in &TEMPOS {
        let ratio = bpm / 120.0;
        // pitch and level: a sine and a harmonic tone (6 beats = all 3 s of material)
        let sine = stretched("gen:tone", bpm, 6, quality);
        let harm = stretched("gen:harm", bpm, 6, quality);
        let mut pitch_err: f64 = 0.0;
        let mut drift: f64 = 0.0;
        for (x, want) in [(&sine, 440.0), (&harm, 220.0)] {
            let mid = &x[x.len() / 4..3 * x.len() / 4];
            pitch_err = pitch_err.max(cents(acf_hz(mid, want), want).abs());
            let w = windowed_hz(x, want);
            let (lo, hi) = w.iter().fold((f64::MAX, f64::MIN), |(a, b), f| {
                (a.min(cents(*f, want)), b.max(cents(*f, want)))
            });
            drift = drift.max(hi - lo);
        }
        let n = sine.len();
        let win = SR as usize / 20;
        let levels: Vec<f64> = (n / 10..n * 9 / 10 - win)
            .step_by(win)
            .map(|s| rms_db(&sine[s..s + win]))
            .collect();
        let (lmin, lmax) = levels
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        let level_err = levels.iter().sum::<f64>() / levels.len() as f64 - tone_db;
        // transients: the click train (a click every beat), 8 beats
        let x = stretched("gen:clicks", bpm, 8, quality);
        let env = envelope(&x);
        let beat_ms = 60_000.0 / bpm;
        let expected: Vec<usize> = (1..8)
            .map(|k| (k as f64 * beat_ms).round() as usize)
            .collect();
        let c = clicks(&env, &expected, 20.0);
        let onset_err = c.iter().map(|v| v.0.abs()).fold(0.0, f64::max);
        let smear = spread_of(&env, &expected, 20.0) - src_spread;
        let smear40 = spread_of(&env, &expected, 40.0) - src_spread40;
        rows.push(format!(
            "| {ratio:.1}× ({bpm} BPM) | {pitch_err:.2} | {drift:.2} | {:.2} | {level_err:+.2} | {onset_err:.0} | {smear:.0} | {smear40:.0} |",
            lmax - lmin
        ));
        let mut check = |ok: bool, what: String| {
            if !ok {
                failures.push(format!("ratio {ratio:.1}: {what}"));
            }
        };
        check(
            pitch_err <= MAX_PITCH_ERROR_CENTS,
            format!("pitch error {pitch_err:.2} cents"),
        );
        check(
            drift <= MAX_PITCH_DRIFT_CENTS,
            format!("pitch drift {drift:.2} cents"),
        );
        check(
            lmax - lmin <= MAX_LEVEL_SWING_DB,
            format!("level swing {:.2} dB", lmax - lmin),
        );
        check(
            level_err.abs() <= MAX_LEVEL_ERROR_DB,
            format!("level error {level_err:.2} dB"),
        );
        check(
            onset_err <= MAX_ONSET_ERROR_MS,
            format!("onset error {onset_err} ms"),
        );
        check(smear <= MAX_SMEAR_MS, format!("transient smear {smear} ms"));
        check(
            smear40 <= MAX_SMEAR_40_MS,
            format!("transient smear at -40 dB {smear40} ms"),
        );
    }
    println!("\n{quality:?} quality:\n");
    println!(
        "| ratio | pitch error (cents) | pitch drift (cents) | level swing (dB) | level error (dB) | onset error (ms) | smear at -20 dB (ms) | smear at -40 dB (ms) |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    println!("{}", rows.join("\n"));
    println!(
        "(original click: {src_spread} ms above -20 dB of its peak, {src_spread40} ms above -40 dB)"
    );
    failures
        .into_iter()
        .map(|f| format!("{quality:?}: {f}"))
        .collect()
}
