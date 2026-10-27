//! Reference-track comparison (M2/T9): how a mix differs from a reference you supply, in
//! numbers and in a few plain sentences. Pure: audio in, a report out.
//!
//! - **Tonal balance:** per band, the dB difference of each band's level *relative to the
//!   whole signal*, so a quieter mix with the same balance reads as no difference. Positive
//!   means the mix has more of that band than the reference.
//! - **Loudness:** integrated and short-term (3 s) LUFS, loudness range, true peak and crest
//!   factor of both, and the differences.
//! - **Stereo:** L/R correlation and the side-to-mid level, which is how wide it sounds.

use crate::loudness::{self, Loudness};
use crate::spectrum::{self, Spectrum};
use crate::{health, round};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BandCompare {
    pub band: &'static str,
    pub low_hz: f64,
    pub high_hz: f64,
    /// Level of the mix in this band, dBFS.
    pub mix_db: Option<f64>,
    /// Level of the reference in this band, dBFS.
    pub reference_db: Option<f64>,
    /// Mix minus reference, with both normalised to the same overall level. Positive: the
    /// mix has more of this band.
    pub diff_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LoudnessCompare {
    pub mix_integrated_lufs: Option<f64>,
    pub reference_integrated_lufs: Option<f64>,
    /// Mix minus reference, in LU. Negative: the mix is quieter.
    pub integrated_diff_lu: Option<f64>,
    pub mix_short_term_max_lufs: Option<f64>,
    pub reference_short_term_max_lufs: Option<f64>,
    pub short_term_max_diff_lu: Option<f64>,
    pub mix_loudness_range_lu: Option<f64>,
    pub reference_loudness_range_lu: Option<f64>,
    pub mix_true_peak_dbtp: Option<f64>,
    pub reference_true_peak_dbtp: Option<f64>,
    pub mix_crest_db: Option<f64>,
    pub reference_crest_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StereoCompare {
    /// -1 (out of phase) .. +1 (mono); `None` for mono audio.
    pub mix_correlation: Option<f64>,
    pub reference_correlation: Option<f64>,
    /// Side level relative to mid, dB. Higher is wider; very negative is nearly mono.
    pub mix_side_to_mid_db: Option<f64>,
    pub reference_side_to_mid_db: Option<f64>,
    /// Mix minus reference. Positive: the mix is wider.
    pub width_diff_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReferenceComparison {
    pub bands: Vec<BandCompare>,
    pub loudness: LoudnessCompare,
    pub stereo: StereoCompare,
    /// A few plain sentences, most important first.
    pub suggestions: Vec<String>,
    /// How to read the numbers.
    pub note: &'static str,
}

fn sub(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(round(a? - b?, 1))
}

/// Each band's level minus the overall level of the band sum (dB), `None` for silent bands.
fn relative(s: &Spectrum) -> Vec<Option<f64>> {
    let total: f64 = s
        .bands
        .iter()
        .filter_map(|b| b.level_db)
        .map(|d| 10f64.powf(d / 10.0))
        .sum();
    s.bands
        .iter()
        .map(|b| {
            b.level_db
                .filter(|_| total > 0.0)
                .map(|d| d - 10.0 * total.log10())
        })
        .collect()
}

/// A band this far under the whole signal is effectively empty.
const NEGLIGIBLE_DB: f64 = -40.0;
/// Differences are reported up to this size; beyond it a band is just "absent on one side".
const MAX_BAND_DIFF_DB: f64 = 24.0;

/// Mix band minus reference band, both relative to their own overall level. `None` when the
/// band is empty on both sides, or silent on both.
fn band_diff(mix: Option<f64>, reference: Option<f64>) -> Option<f64> {
    if mix.is_none() && reference.is_none() {
        return None;
    }
    let (m, r) = (mix.unwrap_or(-80.0), reference.unwrap_or(-80.0));
    if m < NEGLIGIBLE_DB && r < NEGLIGIBLE_DB {
        return None;
    }
    Some(round((m - r).clamp(-MAX_BAND_DIFF_DB, MAX_BAND_DIFF_DB), 1))
}

/// Side level relative to mid (dB) of a stereo signal; `None` for mono or silence.
fn side_to_mid_db(ch: &[Vec<f32>]) -> Option<f64> {
    if ch.len() < 2 {
        return None;
    }
    let n = ch[0].len().min(ch[1].len());
    let (mut m2, mut s2) = (0.0f64, 0.0f64);
    for (l, r) in ch[0][..n].iter().zip(&ch[1][..n]) {
        let (l, r) = (*l as f64, *r as f64);
        m2 += ((l + r) * 0.5).powi(2);
        s2 += ((l - r) * 0.5).powi(2);
    }
    if m2 + s2 < 1e-12 {
        return None;
    }
    // A pure mono signal has no side: report a -40 dB floor instead of -inf.
    Some(round(10.0 * (s2 / m2.max(1e-12)).max(1e-4).log10(), 1))
}

fn band_advice(band: &str, more: bool) -> &'static str {
    match (band, more) {
        ("sub", true) => {
            "Sub bass (20-60 Hz) is heavier than the reference: high-pass the non-bass parts or trim the sub a little."
        }
        ("sub", false) => {
            "Sub bass (20-60 Hz) is lighter than the reference: add weight with a sine/sub layer or lift the 808/bass."
        }
        ("bass", true) => {
            "Bass (60-250 Hz) is heavier than the reference: turn the bass or kick down a touch, or cut around 100-200 Hz in competing parts."
        }
        ("bass", false) => {
            "Bass (60-250 Hz) is lighter than the reference: raise the bass and kick, or add low-end body around 80-150 Hz."
        }
        ("low_mid", true) => {
            "Low mids (250-500 Hz) are fuller than the reference: that is often mud; cut around 250-400 Hz on pads, guitars and keys."
        }
        ("low_mid", false) => {
            "Low mids (250-500 Hz) are thinner than the reference: the mix may sound hollow; add body to the chords and snares around 300 Hz."
        }
        ("mid", true) => {
            "Mids (500 Hz-2 kHz) are more prominent than the reference: try pulling leads and chords back or notching around 1 kHz."
        }
        ("mid", false) => {
            "Mids (500 Hz-2 kHz) are weaker than the reference: bring the main melodic parts forward."
        }
        ("high_mid", true) => {
            "Upper mids (2-4 kHz) are harsher than the reference: soften them with a gentle cut around 3 kHz on bright parts."
        }
        ("high_mid", false) => {
            "Upper mids (2-4 kHz) are weaker than the reference: lift vocals, leads and snares around 3 kHz for presence."
        }
        ("presence", true) => {
            "Presence (4-6 kHz) is stronger than the reference: it can sound edgy; ease hats and bright synths."
        }
        ("presence", false) => {
            "Presence (4-6 kHz) is weaker than the reference: add definition to hats, plucks and transients."
        }
        ("air", true) => {
            "Air (above 6 kHz) is brighter than the reference: reduce hi-hat and top-end shelf boosts, or add a gentle high shelf cut."
        }
        ("air", false) => {
            "Air (above 6 kHz) is duller than the reference: a high shelf or brighter hats and cymbals would open it up."
        }
        _ => "",
    }
}

/// Compares a mix with a reference. Both are `[channel][frame]` at their own sample rates.
pub fn compare(
    mix: &[Vec<f32>],
    mix_rate: u32,
    reference: &[Vec<f32>],
    reference_rate: u32,
) -> ReferenceComparison {
    let (ms, rs) = (
        spectrum::analyze(mix, mix_rate),
        spectrum::analyze(reference, reference_rate),
    );
    let (mr, rr) = (relative(&ms), relative(&rs));
    let bands: Vec<BandCompare> = ms
        .bands
        .iter()
        .zip(&rs.bands)
        .zip(mr.iter().zip(&rr))
        .map(|((m, r), (mrel, rrel))| BandCompare {
            band: m.band,
            low_hz: m.low_hz,
            high_hz: m.high_hz,
            mix_db: m.level_db,
            reference_db: r.level_db,
            diff_db: band_diff(*mrel, *rrel),
        })
        .collect();

    let (ml, rl): (Loudness, Loudness) = (
        loudness::analyze(mix, mix_rate),
        loudness::analyze(reference, reference_rate),
    );
    let loud = LoudnessCompare {
        mix_integrated_lufs: ml.integrated_lufs,
        reference_integrated_lufs: rl.integrated_lufs,
        integrated_diff_lu: sub(ml.integrated_lufs, rl.integrated_lufs),
        mix_short_term_max_lufs: ml.short_term_max_lufs,
        reference_short_term_max_lufs: rl.short_term_max_lufs,
        short_term_max_diff_lu: sub(ml.short_term_max_lufs, rl.short_term_max_lufs),
        mix_loudness_range_lu: ml.loudness_range_lu,
        reference_loudness_range_lu: rl.loudness_range_lu,
        mix_true_peak_dbtp: ml.true_peak_dbtp,
        reference_true_peak_dbtp: rl.true_peak_dbtp,
        mix_crest_db: ml.crest_db,
        reference_crest_db: rl.crest_db,
    };

    let (mw, rw) = (side_to_mid_db(mix), side_to_mid_db(reference));
    let stereo = StereoCompare {
        mix_correlation: health::analyze_correlation(mix),
        reference_correlation: health::analyze_correlation(reference),
        mix_side_to_mid_db: mw,
        reference_side_to_mid_db: rw,
        width_diff_db: sub(mw, rw),
    };

    let mut suggestions = Vec::new();
    // loudness first: it colours everything else
    if let Some(d) = loud.integrated_diff_lu {
        if d <= -1.5 {
            suggestions.push(format!(
                "The mix is {:.1} LU quieter than the reference. Raise the level into a limiter on the master (mind the {} dBTP peak) before judging the balance.",
                -d,
                loud.mix_true_peak_dbtp.map_or("?".into(), |p| format!("{p:.1}"))
            ));
        } else if d >= 1.5 {
            suggestions.push(format!(
                "The mix is {d:.1} LU louder than the reference. Compare at matched loudness; if it sounds squashed, ease the master limiter or compressor."
            ));
        }
    }
    let mut tonal: Vec<&BandCompare> = bands
        .iter()
        .filter(|b| b.diff_db.is_some_and(|d| d.abs() >= 2.0))
        .collect();
    tonal.sort_by(|a, b| {
        b.diff_db
            .unwrap_or(0.0)
            .abs()
            .total_cmp(&a.diff_db.unwrap_or(0.0).abs())
    });
    for b in tonal.iter().take(3) {
        let d = b.diff_db.unwrap_or(0.0);
        suggestions.push(format!(
            "{} ({:+.1} dB vs the reference)",
            band_advice(b.band, d > 0.0),
            d
        ));
    }
    if let (Some(m), Some(r)) = (loud.mix_crest_db, loud.reference_crest_db) {
        if m - r <= -3.0 {
            suggestions.push(format!(
                "The mix is more compressed than the reference (crest {m:.1} dB vs {r:.1} dB): back off the compression or limiting to keep the punch."
            ));
        } else if m - r >= 3.0 {
            suggestions.push(format!(
                "The mix is more dynamic than the reference (crest {m:.1} dB vs {r:.1} dB): the peaks stand out; some bus compression or limiting would glue it."
            ));
        }
    }
    if let Some(d) = stereo.width_diff_db {
        if d >= 3.0 {
            suggestions.push(format!(
                "The mix is {d:.1} dB wider than the reference: check it in mono and keep bass, kick and lead nearer the centre."
            ));
        } else if d <= -3.0 {
            suggestions.push(format!(
                "The mix is {:.1} dB narrower than the reference: pan or widen pads, hats and backing parts, and add stereo reverb or delay.",
                -d
            ));
        }
    }
    if stereo.mix_correlation.is_some_and(|c| c < 0.0) {
        suggestions.push(
            "The left and right channels are partly out of phase (negative correlation): the mix will lose parts in mono; narrow or re-phase the widest sounds.".into(),
        );
    }
    if suggestions.is_empty() {
        suggestions.push(
            "The mix is within about 2 dB of the reference in every band and within 1.5 LU in loudness; nothing stands out.".into(),
        );
    }
    ReferenceComparison {
        bands,
        loudness: loud,
        stereo,
        suggestions,
        note: "diff_db is mix minus reference after matching overall level, so it shows balance, not volume; positive means the mix has more of that band. Loudness differences are separate.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::sine;

    const SR: u32 = 48_000;

    fn stereo(l: Vec<f32>, r: Vec<f32>) -> Vec<Vec<f32>> {
        vec![l, r]
    }

    fn mix_of(parts: &[(f64, f64)]) -> Vec<f32> {
        let mut out = vec![0.0f32; SR as usize * 3];
        for (f, a) in parts {
            for (o, s) in out.iter_mut().zip(sine(*f, *a, SR, 3.0)) {
                *o += s;
            }
        }
        out
    }

    #[test]
    fn identical_signals_have_no_difference() {
        let x = mix_of(&[(100.0, 0.2), (1000.0, 0.2)]);
        let a = stereo(x.clone(), x);
        let c = compare(&a, SR, &a, SR);
        assert!(c.bands.iter().all(|b| b.diff_db.is_none_or(|d| d == 0.0)));
        assert_eq!(c.loudness.integrated_diff_lu, Some(0.0));
        assert_eq!(c.stereo.width_diff_db, Some(0.0));
        assert!(c.suggestions[0].contains("within about 2 dB"), "{c:?}");
    }

    #[test]
    fn a_bass_heavy_mix_reads_as_heavy_in_the_bass_band_only() {
        let reference = mix_of(&[(100.0, 0.1), (1000.0, 0.1), (5000.0, 0.1)]);
        let heavy = mix_of(&[(100.0, 0.3), (1000.0, 0.1), (5000.0, 0.1)]);
        let c = compare(
            &stereo(heavy.clone(), heavy),
            SR,
            &stereo(reference.clone(), reference),
            SR,
        );
        let bass = c.bands.iter().find(|b| b.band == "bass").unwrap();
        let mid = c.bands.iter().find(|b| b.band == "mid").unwrap();
        assert!(bass.diff_db.unwrap() > 3.0, "{bass:?}");
        assert!(mid.diff_db.unwrap() < -3.0, "{mid:?}");
        assert!(
            c.stereo.width_diff_db == Some(0.0),
            "both are mono: {:?}",
            c.stereo
        );
        assert!(
            c.bands
                .iter()
                .any(|b| b.band == "bass" && b.diff_db.unwrap() > 3.0),
            "{:?}",
            c.suggestions
        );
        assert!(c.loudness.integrated_diff_lu.unwrap() > 0.0);
    }

    #[test]
    fn same_balance_at_a_lower_level_is_a_loudness_difference_not_a_tonal_one() {
        let loud = mix_of(&[(100.0, 0.2), (3000.0, 0.1)]);
        let quiet: Vec<f32> = loud.iter().map(|x| x * 0.25).collect();
        let c = compare(
            &stereo(quiet.clone(), quiet),
            SR,
            &stereo(loud.clone(), loud),
            SR,
        );
        assert!(
            c.bands
                .iter()
                .all(|b| b.diff_db.is_none_or(|d| d.abs() < 0.3)),
            "{:?}",
            c.bands
        );
        let d = c.loudness.integrated_diff_lu.unwrap();
        assert!((d + 12.0).abs() < 0.3, "12 dB quieter, got {d}");
        assert!(c.suggestions[0].contains("quieter"), "{:?}", c.suggestions);
    }

    #[test]
    fn stereo_width_and_correlation() {
        let l = sine(220.0, 0.3, SR, 3.0);
        let r = sine(330.0, 0.3, SR, 3.0);
        let wide = stereo(l.clone(), r);
        let mono = stereo(l.clone(), l);
        let c = compare(&wide, SR, &mono, SR);
        assert!(c.stereo.width_diff_db.unwrap() > 20.0, "{:?}", c.stereo);
        assert!(c.stereo.mix_correlation.unwrap().abs() < 0.1);
        assert_eq!(c.stereo.reference_correlation, Some(1.0));
        assert!(
            c.suggestions
                .iter()
                .any(|s| s.contains("wider than the reference")),
            "{:?}",
            c.suggestions
        );
    }

    #[test]
    fn differing_sample_rates_compare_fine() {
        let a = mix_of(&[(200.0, 0.2)]);
        let b: Vec<f32> = sine(200.0, 0.2, 44_100, 3.0);
        let c = compare(&stereo(a.clone(), a), SR, &stereo(b.clone(), b), 44_100);
        assert!(c.loudness.integrated_diff_lu.unwrap().abs() < 0.2);
    }
}
