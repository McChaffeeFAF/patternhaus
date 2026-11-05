//! Reading a probe render of an instrument (M3/T9, `catalogue.probe`): which keys make a
//! sound, the playable range they suggest, and whether a candidate keyswitch changed the
//! sound of a test note.
//!
//! The render is one take of short notes, each in its own window with a gap after it (the
//! caller lays them out). A window *sounds* when its RMS level is above an absolute floor
//! **and** clearly above the tail still ringing from the previous note (measured in the last
//! part of the gap before it), so a long release never makes a silent key look playable.

use crate::{mono_mix, pow_db_opt, round, slice_seconds, spectrum};
use serde::Serialize;

/// Below this RMS level (dBFS) a window is silent.
pub const SILENCE_DB: f64 = -60.0;
/// A window must be this much louder (dB) than the tail before it to count as sounding.
pub const ABOVE_TAIL_DB: f64 = 6.0;

/// One note window of a probe render, in seconds from the start of the render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub key: u8,
    /// 0..=1
    pub velocity: f32,
    pub start: f64,
    pub end: f64,
    /// Where the tail of the previous note is measured: `(from, to)` seconds, or `None` for
    /// the first window.
    pub tail: Option<(f64, f64)>,
}

/// What one window measured.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WindowLevel {
    pub key: u8,
    pub velocity: f32,
    /// RMS level, dBFS (`None` = digital silence).
    pub rms_db: Option<f64>,
    /// Peak level, dBFS.
    pub peak_db: Option<f64>,
    /// Spectral centroid, Hz (when it sounds).
    pub centroid_hz: Option<f64>,
    pub sounds: bool,
}

fn rms_db(channels: &[Vec<f32>]) -> Option<f64> {
    let n: usize = channels.iter().map(Vec::len).sum();
    if n == 0 {
        return None;
    }
    let sum: f64 = channels.iter().flatten().map(|x| (*x as f64).powi(2)).sum();
    pow_db_opt(sum / n as f64)
}

fn peak_db(channels: &[Vec<f32>]) -> Option<f64> {
    let p = channels
        .iter()
        .flatten()
        .fold(0.0f64, |m, x| m.max((*x as f64).abs()));
    (p > 0.0).then(|| 20.0 * p.log10())
}

/// Measures every window of a probe render.
pub fn measure(channels: &[Vec<f32>], sample_rate: u32, windows: &[Window]) -> Vec<WindowLevel> {
    windows
        .iter()
        .map(|w| {
            let audio = slice_seconds(channels, sample_rate, w.start, w.end);
            let rms = rms_db(&audio);
            let tail = w
                .tail
                .and_then(|(a, b)| rms_db(&slice_seconds(channels, sample_rate, a, b)));
            let sounds =
                rms.is_some_and(|r| r > SILENCE_DB && tail.is_none_or(|t| r > t + ABOVE_TAIL_DB));
            let centroid = if sounds {
                spectrum::analyze(&[mono_mix(&audio)], sample_rate).centroid_hz
            } else {
                None
            };
            WindowLevel {
                key: w.key,
                velocity: w.velocity,
                rms_db: rms.map(|x| round(x, 1)),
                peak_db: peak_db(&audio).map(|x| round(x, 1)),
                centroid_hz: centroid.map(|x| round(x, 0)),
                sounds,
            }
        })
        .collect()
}

/// The keys that sounded at any velocity, and the proposed range: the lowest and highest of
/// them, or `None` when nothing sounded.
pub fn sounding_keys(levels: &[WindowLevel]) -> (Vec<u8>, Option<(u8, u8)>) {
    let mut keys: Vec<u8> = levels.iter().filter(|l| l.sounds).map(|l| l.key).collect();
    keys.sort_unstable();
    keys.dedup();
    let range = keys.first().copied().zip(keys.last().copied());
    (keys, range)
}

/// True when two test-note measurements differ audibly: level by more than 1.5 dB, or
/// centroid by more than 8 %.
pub fn differs(a: &WindowLevel, b: &WindowLevel) -> bool {
    if a.sounds != b.sounds {
        return true;
    }
    let level = match (a.rms_db, b.rms_db) {
        (Some(x), Some(y)) => (x - y).abs() > 1.5,
        (x, y) => x.is_some() != y.is_some(),
    };
    let tone = match (a.centroid_hz, b.centroid_hz) {
        (Some(x), Some(y)) => (x - y).abs() > 0.08 * x.max(y),
        _ => false,
    };
    level || tone
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::sine;

    #[test]
    fn sounding_windows_give_the_range_and_tails_do_not_count() {
        let sr = 48_000;
        // key 50: silent; 51: a tone; 52: silent but the tone of 51 still rings into it
        let mut l = vec![0.0f32; sr as usize * 3];
        let tone = sine(440.0, 0.5, sr, 1.0);
        l[sr as usize..2 * sr as usize].copy_from_slice(&tone);
        // a decaying tail into the next window (-30 dB)
        let tail = sine(440.0, 0.016, sr, 0.6);
        l[2 * sr as usize..2 * sr as usize + tail.len()].copy_from_slice(&tail);
        let ch = vec![l.clone(), l];
        let w = |key, start: f64| Window {
            key,
            velocity: 0.8,
            start,
            end: start + 0.5,
            tail: (start > 0.0).then_some((start - 0.2, start)),
        };
        let levels = measure(&ch, sr, &[w(50, 0.0), w(51, 1.0), w(52, 2.0)]);
        assert!(!levels[0].sounds && levels[1].sounds);
        assert!(
            levels[2].rms_db.unwrap() > SILENCE_DB,
            "the tail is audible"
        );
        assert!(!levels[2].sounds, "but it is the previous note's tail");
        assert!((levels[1].centroid_hz.unwrap() - 440.0).abs() < 30.0);
        assert_eq!(sounding_keys(&levels), (vec![51], Some((51, 51))));
        assert!(differs(&levels[0], &levels[1]));
        assert!(!differs(&levels[1], &levels[1].clone()));
    }
}
