//! Mix health: clipping, DC offset, phase correlation, mono compatibility, energy per bar.

use crate::{Timing, db_opt, round};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Health {
    /// Samples at or beyond full scale (|x| >= 0.9999), all channels.
    pub clipped_samples: u64,
    /// Separate runs of consecutive clipped samples (how many times it clipped).
    pub clip_events: u64,
    /// Mean value per channel, linear (0 is ideal; > 0.01 is worth fixing).
    pub dc_offset: Vec<f64>,
    /// Stereo correlation, -1 (out of phase) to +1 (mono). `None` for mono or silent audio.
    pub phase_correlation: Option<f64>,
    /// Level lost when summing to mono, dB (0 = none, 3 = uncorrelated, large = out of
    /// phase). `None` for mono or silent audio.
    pub mono_loss_db: Option<f64>,
    /// Headline problems, in words.
    pub warnings: Vec<String>,
    /// RMS and peak per bar, so a song's shape is visible as numbers.
    pub energy_per_bar: Vec<BarEnergy>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BarEnergy {
    /// 1-based bar number (absolute, so ranges report the real bar).
    pub bar: u32,
    pub rms_dbfs: Option<f64>,
    pub peak_dbfs: Option<f64>,
}

/// Stereo correlation of the first two channels, -1 (out of phase) to +1 (mono); `None` for
/// mono or silent audio.
pub fn analyze_correlation(channels: &[Vec<f32>]) -> Option<f64> {
    if channels.len() < 2 {
        return None;
    }
    let n = channels[0].len().min(channels[1].len());
    let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
    for (l, r) in channels[0][..n].iter().zip(&channels[1][..n]) {
        let (l, r) = (*l as f64, *r as f64);
        ll += l * l;
        rr += r * r;
        lr += l * r;
    }
    (ll > 1e-12 && rr > 1e-12).then(|| round(lr / (ll * rr).sqrt(), 3))
}

const CLIP_LEVEL: f32 = 0.9999;

/// `first_bar` is the (1-based) bar number of the first sample.
pub fn analyze(channels: &[Vec<f32>], sample_rate: u32, timing: &Timing, first_bar: u32) -> Health {
    let mut clipped = 0u64;
    let mut events = 0u64;
    let mut dc = Vec::new();
    for ch in channels {
        let mut run = false;
        let mut sum = 0.0f64;
        for &x in ch {
            sum += x as f64;
            if x.abs() >= CLIP_LEVEL {
                clipped += 1;
                if !run {
                    events += 1;
                }
                run = true;
            } else {
                run = false;
            }
        }
        dc.push(round(sum / ch.len().max(1) as f64, 5));
    }

    let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
    let (phase, mono_loss) = if channels.len() >= 2 {
        let n = channels[0].len().min(channels[1].len());
        for (l, r) in channels[0][..n].iter().zip(&channels[1][..n]) {
            let (l, r) = (*l as f64, *r as f64);
            ll += l * l;
            rr += r * r;
            lr += l * r;
        }
        let stereo = 0.5 * (ll + rr);
        let mono = 0.25 * (ll + rr + 2.0 * lr);
        if ll > 1e-12 && rr > 1e-12 {
            let corr = lr / (ll * rr).sqrt();
            let loss = if mono > 1e-12 {
                -10.0 * (mono / stereo).log10()
            } else {
                60.0
            };
            (Some(round(corr, 3)), Some(round(loss.clamp(0.0, 60.0), 2)))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let mut warnings = Vec::new();
    if events > 0 {
        warnings.push(format!(
            "clipping: {clipped} samples at full scale in {events} place(s)"
        ));
    }
    for (i, d) in dc.iter().enumerate() {
        if d.abs() > 0.01 {
            warnings.push(format!("DC offset {d:.3} on channel {}", i + 1));
        }
    }
    if let Some(c) = phase.filter(|c| *c < -0.2) {
        warnings.push(format!(
            "phase correlation {c:.2}: parts of the mix cancel in mono"
        ));
    }
    if let Some(l) = mono_loss.filter(|l| *l > 4.5) {
        warnings.push(format!("mono sum loses {l:.1} dB"));
    }

    // Energy per bar.
    let bar_len = (timing.seconds_per_bar() * sample_rate as f64)
        .round()
        .max(1.0) as usize;
    let n = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    let mut bars = Vec::new();
    for (b, start) in (0..n).step_by(bar_len).enumerate() {
        let end = (start + bar_len).min(n);
        let (mut sq, mut cnt, mut pk) = (0.0f64, 0usize, 0.0f64);
        for ch in channels {
            for &x in &ch[start.min(ch.len())..end.min(ch.len())] {
                sq += (x as f64) * (x as f64);
                pk = pk.max(x.abs() as f64);
            }
            cnt += end.min(ch.len()).saturating_sub(start.min(ch.len()));
        }
        let rms = if cnt > 0 {
            (sq / cnt as f64).sqrt()
        } else {
            0.0
        };
        bars.push(BarEnergy {
            bar: first_bar + b as u32,
            rms_dbfs: db_opt(rms).map(|d| round(d, 1)),
            peak_dbfs: db_opt(pk).map(|d| round(d, 1)),
        });
    }

    Health {
        clipped_samples: clipped,
        clip_events: events,
        dc_offset: dc,
        phase_correlation: phase,
        mono_loss_db: mono_loss,
        warnings,
        energy_per_bar: bars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::*;
    use ph_core::TimeSignature;

    fn timing() -> Timing {
        Timing::new(120.0, TimeSignature::default())
    }

    #[test]
    fn clipping_is_counted_in_runs() {
        let mut x = sine(100.0, 0.5, 48_000, 1.0);
        for i in [100, 101, 102, 5000, 9000, 9001] {
            x[i] = 1.0;
        }
        let h = analyze(&[x], 48_000, &timing(), 1);
        assert_eq!(h.clipped_samples, 6);
        assert_eq!(h.clip_events, 3);
        assert!(h.warnings.iter().any(|w| w.contains("clipping")));
        let clean = analyze(&[sine(100.0, 0.5, 48_000, 1.0)], 48_000, &timing(), 1);
        assert_eq!(clean.clip_events, 0);
    }

    #[test]
    fn dc_offset_is_measured() {
        let x: Vec<f32> = sine(100.0, 0.3, 48_000, 1.0)
            .iter()
            .map(|s| s + 0.1)
            .collect();
        let h = analyze(&[x.clone(), x], 48_000, &timing(), 1);
        assert!((h.dc_offset[0] - 0.1).abs() < 1e-3);
        assert!(h.warnings.iter().any(|w| w.contains("DC")));
    }

    #[test]
    fn phase_and_mono_compatibility_known_answers() {
        let a = sine(220.0, 0.5, 48_000, 1.0);
        let inv: Vec<f32> = a.iter().map(|x| -x).collect();
        // Identical channels: perfectly correlated, nothing lost in mono.
        let h = analyze(&[a.clone(), a.clone()], 48_000, &timing(), 1);
        assert_eq!(h.phase_correlation, Some(1.0));
        assert!(h.mono_loss_db.unwrap() < 0.01);
        // Inverted: -1 and total cancellation (clamped to 60 dB).
        let h = analyze(&[a.clone(), inv], 48_000, &timing(), 1);
        assert_eq!(h.phase_correlation, Some(-1.0));
        assert!(h.mono_loss_db.unwrap() > 40.0);
        assert!(!h.warnings.is_empty());
        // Quadrature (uncorrelated): 3.01 dB.
        let q = sine(220.0, 0.5, 48_000, 1.0);
        let q: Vec<f32> = (0..q.len())
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 48_000.0).cos()) as f32
            })
            .collect();
        let h = analyze(&[a, q], 48_000, &timing(), 1);
        assert!(h.phase_correlation.unwrap().abs() < 0.01);
        assert!((h.mono_loss_db.unwrap() - 3.01).abs() < 0.05);
    }

    #[test]
    fn energy_curve_follows_bars() {
        // 120 bpm 4/4: one bar = 2 s. Bars at -20, -14, silence.
        let mut x = sine(220.0, dbfs(-17.0), 48_000, 2.0);
        x.extend(sine(220.0, dbfs(-11.0), 48_000, 2.0));
        x.extend(vec![0.0; 96_000]);
        let h = analyze(&[x], 48_000, &timing(), 5);
        assert_eq!(h.energy_per_bar.len(), 3);
        assert_eq!(h.energy_per_bar[0].bar, 5);
        assert!((h.energy_per_bar[0].rms_dbfs.unwrap() - -20.0).abs() < 0.2);
        assert!((h.energy_per_bar[1].rms_dbfs.unwrap() - -14.0).abs() < 0.2);
        assert_eq!(h.energy_per_bar[2].rms_dbfs, None);
    }
}
