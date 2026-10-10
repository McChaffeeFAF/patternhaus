//! Loudness per ITU-R BS.1770-4 and EBU R128 / Tech 3341, true peak, RMS and crest factor.

use crate::{db_opt, pow_db_opt, round};
use ph_dsp::filter::{Biquad, BiquadCoefs};
use ph_dsp::oversample::TruePeak4x;
use serde::Serialize;
use std::f64::consts::PI;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Loudness {
    /// Gated programme loudness (BS.1770-4: absolute gate -70 LUFS, relative gate -10 LU).
    /// `None` when everything is below the absolute gate (silence).
    pub integrated_lufs: Option<f64>,
    /// Loudest 400 ms block.
    pub momentary_max_lufs: Option<f64>,
    /// Loudest 3 s window.
    pub short_term_max_lufs: Option<f64>,
    /// The last 3 s window (what a meter would read at the end).
    pub short_term_last_lufs: Option<f64>,
    /// Loudness range in LU (EBU Tech 3342), `None` for material shorter than 3 s.
    pub loudness_range_lu: Option<f64>,
    /// 4x-oversampled peak over all channels, dBTP.
    pub true_peak_dbtp: Option<f64>,
    /// Plain sample peak, dBFS.
    pub sample_peak_dbfs: Option<f64>,
    /// RMS over the whole signal and all channels, dBFS.
    pub rms_dbfs: Option<f64>,
    /// Peak (sample) minus RMS, dB.
    pub crest_db: Option<f64>,
}

/// The two-stage K-weighting filter (BS.1770-4 pre-filter and RLB), derived for any rate.
pub fn k_weighting(sample_rate: f64) -> [BiquadCoefs; 2] {
    // Stage 1: high shelf modelling the head.
    let (f0, g, q) = (
        1681.974450955533f64,
        3.999843853973347f64,
        0.7071752369554196f64,
    );
    let k = (PI * f0 / sample_rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = BiquadCoefs {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    // Stage 2: RLB high-pass.
    let (f0, q) = (38.13547087602444f64, 0.5003270373238773f64);
    let k = (PI * f0 / sample_rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let rlb = BiquadCoefs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    [shelf, rlb]
}

const OFFSET: f64 = -0.691;

fn lufs(mean_square_sum: f64) -> Option<f64> {
    pow_db_opt(mean_square_sum).map(|d| OFFSET + d)
}

/// Per-100 ms-hop sums of K-weighted squares: `sums[hop][channel]`, plus sample counts.
struct Hops {
    sums: Vec<Vec<f64>>,
    counts: Vec<usize>,
}

fn k_weighted_hops(channels: &[Vec<f32>], sample_rate: u32) -> Hops {
    let hop = ((sample_rate as f64) * 0.1).round().max(1.0) as usize;
    let n = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    let nh = n.div_ceil(hop);
    let mut sums = vec![vec![0.0; channels.len()]; nh];
    let mut counts = vec![0usize; nh];
    let coefs = k_weighting(sample_rate as f64);
    for (ci, ch) in channels.iter().enumerate() {
        let mut f = [Biquad::new(coefs[0]), Biquad::new(coefs[1])];
        for (i, &x) in ch.iter().enumerate() {
            let s1 = f[0].process(x as f64);
            let y = f[1].process(s1);
            sums[i / hop][ci] += y * y;
        }
    }
    for (h, c) in counts.iter_mut().enumerate() {
        *c = (n - h * hop).min(hop);
    }
    Hops { sums, counts }
}

/// Mean-square sum over channels for `len` consecutive hops starting at `start`.
fn window_z(h: &Hops, start: usize, len: usize) -> f64 {
    let end = (start + len).min(h.sums.len());
    let n: usize = h.counts[start..end].iter().sum();
    if n == 0 {
        return 0.0;
    }
    let chans = h.sums.first().map_or(0, |s| s.len());
    (0..chans)
        .map(|c| h.sums[start..end].iter().map(|s| s[c]).sum::<f64>() / n as f64)
        .sum()
}

/// Windows of `len` hops, stepping one hop; a signal shorter than the window gives one
/// window over everything.
fn windows(h: &Hops, len: usize) -> Vec<f64> {
    let nh = h.sums.len();
    if nh == 0 {
        return vec![];
    }
    if nh < len {
        return vec![window_z(h, 0, nh)];
    }
    (0..=nh - len).map(|s| window_z(h, s, len)).collect()
}

/// Gated mean of block powers per BS.1770-4: absolute gate -70 LUFS, then `rel_gate` LU
/// below the loudness of what passed.
fn gated(blocks: &[f64], rel_gate: f64) -> Option<f64> {
    let abs_thr = 10f64.powf((-70.0 - OFFSET) / 10.0);
    let above: Vec<f64> = blocks.iter().copied().filter(|&z| z > abs_thr).collect();
    if above.is_empty() {
        return None;
    }
    let mean = above.iter().sum::<f64>() / above.len() as f64;
    let rel_thr = mean * 10f64.powf(rel_gate / 10.0);
    let kept: Vec<f64> = above.iter().copied().filter(|&z| z > rel_thr).collect();
    if kept.is_empty() {
        return None;
    }
    Some(kept.iter().sum::<f64>() / kept.len() as f64)
}

/// Integrated loudness only (BS.1770-4), in LUFS.
pub fn integrated_lufs(channels: &[Vec<f32>], sample_rate: u32) -> Option<f64> {
    let h = k_weighted_hops(channels, sample_rate);
    gated(&windows(&h, 4), -10.0).and_then(lufs)
}

/// True peak (4x oversampled, with parabolic refinement) over all channels, linear.
pub fn true_peak_linear(channels: &[Vec<f32>]) -> f64 {
    let mut peak = 0.0f64;
    for ch in channels {
        let mut tp = TruePeak4x::new();
        for &x in ch
            .iter()
            .chain(std::iter::repeat_n(&0.0, TruePeak4x::DELAY + 2))
        {
            peak = peak.max(tp.process(x) as f64);
        }
    }
    peak
}

/// Full loudness report.
pub fn analyze(channels: &[Vec<f32>], sample_rate: u32) -> Loudness {
    let h = k_weighted_hops(channels, sample_rate);
    let momentary = windows(&h, 4);
    let integrated = gated(&momentary, -10.0).and_then(lufs);
    let momentary_max = momentary.iter().copied().fold(0.0, f64::max);
    let short = if h.sums.len() >= 30 {
        windows(&h, 30)
    } else {
        vec![]
    };
    let short_max = short.iter().copied().fold(0.0, f64::max);
    let lra = loudness_range(&short);

    let mut sample_peak = 0.0f64;
    let mut sq = 0.0f64;
    let mut count = 0usize;
    for ch in channels {
        for &x in ch {
            sample_peak = sample_peak.max(x.abs() as f64);
            sq += (x as f64) * (x as f64);
        }
        count += ch.len();
    }
    let rms = if count > 0 {
        (sq / count as f64).sqrt()
    } else {
        0.0
    };
    let r = |v: Option<f64>| v.map(|x| round(x, 2));
    Loudness {
        integrated_lufs: r(integrated),
        momentary_max_lufs: r(lufs(momentary_max)),
        short_term_max_lufs: r(lufs(short_max)),
        short_term_last_lufs: r(short.last().copied().and_then(lufs)),
        loudness_range_lu: lra.map(|x| round(x, 1)),
        true_peak_dbtp: r(db_opt(true_peak_linear(channels))),
        sample_peak_dbfs: r(db_opt(sample_peak)),
        rms_dbfs: r(db_opt(rms)),
        crest_db: match (db_opt(sample_peak), db_opt(rms)) {
            (Some(p), Some(r)) => Some(round(p - r, 1)),
            _ => None,
        },
    }
}

/// EBU Tech 3342: 10th to 95th percentile of short-term loudness after gating at -70 LUFS
/// and 20 LU below the gated mean.
fn loudness_range(short: &[f64]) -> Option<f64> {
    let abs_thr = 10f64.powf((-70.0 - OFFSET) / 10.0);
    let above: Vec<f64> = short.iter().copied().filter(|&z| z > abs_thr).collect();
    if above.len() < 2 {
        return None;
    }
    let mean = above.iter().sum::<f64>() / above.len() as f64;
    let thr = mean * 10f64.powf(-20.0 / 10.0);
    let mut l: Vec<f64> = above
        .iter()
        .copied()
        .filter(|&z| z > thr)
        .filter_map(lufs)
        .collect();
    if l.len() < 2 {
        return None;
    }
    l.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |p: f64| l[((l.len() - 1) as f64 * p).round() as usize];
    Some(pick(0.95) - pick(0.10))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::*;

    fn stereo(l: Vec<f32>) -> Vec<Vec<f32>> {
        vec![l.clone(), l]
    }

    /// EBU Tech 3341 case 1: 20 s stereo sine at -23 dBFS reads -23.0 LUFS; case 2
    /// (-33 dBFS) reads -33.0 LUFS. 997 Hz as in BS.1770 Annex 1.
    #[test]
    fn ebu_3341_sine_references() {
        for level in [-23.0, -33.0, -20.0, -10.0] {
            let x = stereo(sine(997.0, dbfs(level), 48_000, 20.0));
            let l = integrated_lufs(&x, 48_000).unwrap();
            assert!((l - level).abs() < 0.1, "{level}: got {l}");
        }
    }

    #[test]
    fn bs1770_mono_sine_reads_minus_3_01() {
        // 0 dBFS 997 Hz in one channel only: -3.01 LKFS (BS.1770-4 Annex 1).
        let x = vec![sine(997.0, 1.0, 48_000, 10.0), vec![0.0; 480_000]];
        let l = integrated_lufs(&x, 48_000).unwrap();
        assert!((l - -3.01).abs() < 0.05, "{l}");
    }

    #[test]
    fn works_at_other_sample_rates() {
        for sr in [44_100u32, 96_000] {
            let x = stereo(sine(997.0, dbfs(-23.0), sr, 10.0));
            let l = integrated_lufs(&x, sr).unwrap();
            assert!((l - -23.0).abs() < 0.1, "{sr}: {l}");
        }
    }

    /// Tech 3341 gating case: quiet passages 13 LU below the loud one fall under the
    /// relative gate, so the result is the loud level.
    #[test]
    fn relative_gate_ignores_quiet_passages() {
        let mut x = sine(997.0, dbfs(-36.0), 48_000, 10.0);
        x.extend(sine(997.0, dbfs(-23.0), 48_000, 30.0));
        x.extend(sine(997.0, dbfs(-36.0), 48_000, 10.0));
        let l = integrated_lufs(&stereo(x), 48_000).unwrap();
        assert!((l - -23.0).abs() < 0.1, "{l}");
    }

    #[test]
    fn absolute_gate_and_silence() {
        let x = stereo(sine(997.0, dbfs(-80.0), 48_000, 5.0));
        assert_eq!(integrated_lufs(&x, 48_000), None);
        assert_eq!(
            analyze(&stereo(vec![0.0; 48_000]), 48_000).integrated_lufs,
            None
        );
    }

    /// Tech 3342: a constant tone has near-zero loudness range; two levels 10 LU apart
    /// (20 s each) give LRA = 10 LU.
    #[test]
    fn loudness_range() {
        let flat = analyze(&stereo(sine(997.0, dbfs(-23.0), 48_000, 30.0)), 48_000);
        assert!(flat.loudness_range_lu.unwrap() < 0.2);
        let mut x = sine(997.0, dbfs(-40.0), 48_000, 20.0);
        x.extend(sine(997.0, dbfs(-30.0), 48_000, 20.0));
        let r = analyze(&stereo(x), 48_000).loudness_range_lu.unwrap();
        assert!((r - 10.0).abs() < 0.5, "{r}");
    }

    #[test]
    fn true_peak_analytic_cases() {
        // Sine at fs/4, 45 degrees, amplitude 1: samples are +-0.7071, true peak is 1.0.
        let x: Vec<f32> = (0..4800)
            .map(|i| (PI * (0.5 * i as f64 + 0.25)).sin() as f32)
            .collect();
        let a = analyze(&[x], 48_000);
        assert!((a.sample_peak_dbfs.unwrap() - -3.01).abs() < 0.05);
        assert!(a.true_peak_dbtp.unwrap().abs() < 0.15, "{a:?}");

        // Amplitude sqrt2 at the same phase: samples are +-1, true peak +3.01 dBTP.
        let y: Vec<f32> = (0..4800)
            .map(|i| (2f64.sqrt() * (PI * (0.5 * i as f64 + 0.25)).sin()) as f32)
            .collect();
        let a = analyze(&[y], 48_000);
        assert!(a.sample_peak_dbfs.unwrap().abs() < 0.01);
        assert!(
            (a.true_peak_dbtp.unwrap() - 3.01).abs() < 0.2,
            "{:?}",
            a.true_peak_dbtp
        );

        // Ordinary tone: true peak equals the amplitude; RMS and crest are analytic.
        let z = sine(1000.0, dbfs(-6.0), 48_000, 1.0);
        let a = analyze(&[z], 48_000);
        assert!((a.true_peak_dbtp.unwrap() - -6.0).abs() < 0.05);
        assert!((a.rms_dbfs.unwrap() - -9.01).abs() < 0.05);
        assert!((a.crest_db.unwrap() - 3.0).abs() < 0.1);
    }
}
