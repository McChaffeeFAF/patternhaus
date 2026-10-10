//! Masking and clash warnings: stems that fight over the same frequency band, and pitched
//! channels sounding notes a semitone apart at the same time.

use crate::music::{SongNote, note_name};
use crate::spectrum::{BANDS, band_powers, mean_power_bins};
use crate::{Timing, mono_mix, pow_db_opt, round};
use ph_core::{ChannelId, Tick};
use serde::Serialize;
use std::collections::HashSet;

/// Band powers of one isolated channel (or insert).
#[derive(Clone, Debug)]
pub struct StemBands {
    pub name: String,
    pub powers: [f64; 7],
}

pub fn stem_bands(name: &str, channels: &[Vec<f32>], sample_rate: u32) -> StemBands {
    let bins = mean_power_bins(&mono_mix(channels));
    StemBands {
        name: name.into(),
        powers: band_powers(&bins, sample_rate),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BandMask {
    pub a: String,
    pub b: String,
    pub band: &'static str,
    pub a_level_db: f64,
    pub b_level_db: f64,
    /// `high` (both mostly live here, similar level), `medium`, `low`.
    pub severity: &'static str,
    pub advice: String,
}

/// Pairs of stems with a significant share of their energy in the same band.
pub fn band_masking(stems: &[StemBands]) -> Vec<BandMask> {
    let mut out = Vec::new();
    for (i, a) in stems.iter().enumerate() {
        for b in &stems[i + 1..] {
            let (ta, tb): (f64, f64) = (a.powers.iter().sum(), b.powers.iter().sum());
            // Ignore stems that are effectively silent.
            if pow_db_opt(ta).is_none_or(|d| d < -60.0) || pow_db_opt(tb).is_none_or(|d| d < -60.0)
            {
                continue;
            }
            for (k, (band, ..)) in BANDS.iter().enumerate() {
                let (sa, sb) = (a.powers[k] / ta, b.powers[k] / tb);
                let (la, lb) = (
                    pow_db_opt(a.powers[k]).unwrap_or(-120.0),
                    pow_db_opt(b.powers[k]).unwrap_or(-120.0),
                );
                if sa < 0.2 || sb < 0.2 || la < -55.0 || lb < -55.0 {
                    continue;
                }
                let severity = if sa.min(sb) >= 0.4 && (la - lb).abs() < 6.0 {
                    "high"
                } else if sa.min(sb) >= 0.3 {
                    "medium"
                } else {
                    "low"
                };
                out.push(BandMask {
                    a: a.name.clone(),
                    b: b.name.clone(),
                    band,
                    a_level_db: round(la, 1),
                    b_level_db: round(lb, 1),
                    severity,
                    advice: format!(
                        "{} and {} both sit in the {band} band; cut one with an EQ, pan them apart, or sidechain",
                        a.name, b.name
                    ),
                });
            }
        }
    }
    let rank = |s: &str| match s {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    };
    out.sort_by_key(|m| rank(m.severity));
    out
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NoteClash {
    pub channels: [String; 2],
    pub notes: [String; 2],
    pub bar: u32,
    pub position: String,
    pub overlap_ticks: Tick,
}

/// Notes from different **pitched** channels a semitone apart that sound together for at
/// least a 32nd note. One report per channel pair, key pair and bar.
pub fn note_clashes(
    notes: &[SongNote],
    pitched: &dyn Fn(ChannelId) -> bool,
    name: &dyn Fn(ChannelId) -> String,
    timing: &Timing,
) -> Vec<NoteClash> {
    let mut sorted: Vec<&SongNote> = notes.iter().filter(|n| pitched(n.channel)).collect();
    sorted.sort_by_key(|n| n.start);
    let mut seen: HashSet<(ChannelId, ChannelId, u8, u8, u32)> = HashSet::new();
    let mut out = Vec::new();
    for (i, a) in sorted.iter().enumerate() {
        for b in sorted[i + 1..].iter() {
            if b.start >= a.start + a.length {
                break;
            }
            if a.channel == b.channel || (a.key as i16 - b.key as i16).abs() != 1 {
                continue;
            }
            let ov = (a.start + a.length).min(b.start + b.length) - b.start;
            if ov < crate::PPQ_32 {
                continue;
            }
            let (lo, hi) = if a.key < b.key { (a, b) } else { (b, a) };
            let (bar, _, _) = timing.bar_beat_tick(b.start);
            if seen.insert((lo.channel, hi.channel, lo.key, hi.key, bar)) {
                out.push(NoteClash {
                    channels: [name(lo.channel), name(hi.channel)],
                    notes: [note_name(lo.key), note_name(hi.key)],
                    bar,
                    position: timing.position(b.start),
                    overlap_ticks: ov,
                });
                if out.len() >= 200 {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::*;
    use ph_core::TimeSignature;

    #[test]
    fn two_bass_heavy_stems_mask_each_other_but_not_a_hat() {
        let kick = vec![sine(55.0, 0.5, 48_000, 2.0)];
        let bass = vec![sine(80.0, 0.5, 48_000, 2.0)];
        let hat = vec![sine(9000.0, 0.3, 48_000, 2.0)];
        let stems = vec![
            stem_bands("Kick", &kick, 48_000),
            stem_bands("Bass", &bass, 48_000),
            stem_bands("Hat", &hat, 48_000),
        ];
        let m = band_masking(&stems);
        // 55 Hz is "sub", 80 Hz is "bass": different bands, so nothing yet.
        assert!(m.is_empty(), "{m:?}");
        let bass2 = vec![sine(100.0, 0.5, 48_000, 2.0)];
        let kick2 = vec![sine(70.0, 0.5, 48_000, 2.0)];
        let stems = vec![
            stem_bands("Kick", &kick2, 48_000),
            stem_bands("Bass", &bass2, 48_000),
            stem_bands("Hat", &hat, 48_000),
        ];
        let m = band_masking(&stems);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            (m[0].a.as_str(), m[0].b.as_str(), m[0].band),
            ("Kick", "Bass", "bass")
        );
        assert_eq!(m[0].severity, "high");
    }

    fn note(ch: u32, key: u8, start: Tick, length: Tick) -> SongNote {
        SongNote {
            channel: ChannelId(ch),
            key,
            start,
            length,
            velocity: 0.8,
        }
    }

    #[test]
    fn semitone_clashes_between_pitched_channels() {
        let t = Timing::new(120.0, TimeSignature::default());
        let notes = vec![
            note(1, 60, 0, 960),
            note(2, 61, 0, 960), // C vs C#: clash
            note(1, 64, 1920, 960),
            note(2, 67, 1920, 960), // E vs G: fine
            note(1, 62, 3840, 960),
            note(2, 63, 3840 + 900, 960), // overlaps only 60 ticks: ignored
            note(3, 60, 0, 960),
            note(4, 61, 0, 960), // drum channels are not pitched: ignored
        ];
        let c = note_clashes(&notes, &|ch| ch.0 <= 2, &|ch| format!("ch{}", ch.0), &t);
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].notes, ["C4".to_string(), "C#4".to_string()]);
        assert_eq!(c[0].position, "1.1.0");
    }
}
