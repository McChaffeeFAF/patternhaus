//! Tick ↔ sample conversion through a tempo map.
//!
//! M1 projects have one constant tempo, but everything goes through [`TempoMap`] so tempo
//! automation can add segments later without touching the scheduler.
//!
//! Conversion is exact: tempo is held in milli-BPM (0.001 BPM resolution) and a tick
//! position becomes a sample position by integer arithmetic on `u128`, rounded half up.
//! There is no accumulated floating-point error, so a note an hour into a song lands on the
//! same sample as `round(t × samples_per_tick)` computed directly.

use ph_core::{PPQ, Tick};

/// One constant-tempo stretch, starting at `tick` (= `sample`).
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    tick: Tick,
    /// Exact sample position of `tick` (rounded once, at the segment start).
    sample: i64,
    /// Tempo in thousandths of a BPM.
    mbpm: u64,
}

/// Piecewise-constant tempo map from song ticks to samples at one sample rate.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoMap {
    sample_rate: u64,
    segments: Vec<Segment>,
}

impl TempoMap {
    /// A single tempo for the whole song.
    pub fn constant(bpm: f64, sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as u64,
            segments: vec![Segment {
                tick: 0,
                sample: 0,
                mbpm: to_mbpm(bpm),
            }],
        }
    }

    /// Adds a tempo change at `tick` (must be after the previous change). Each segment start
    /// is rounded to a sample once; positions inside a segment are exact.
    pub fn push_change(&mut self, tick: Tick, bpm: f64) {
        let last = self.segments.last().expect("tempo map has a segment");
        assert!(
            tick > last.tick,
            "tempo changes must be in increasing tick order"
        );
        let sample = last.sample + self.span(last.mbpm, (tick - last.tick) as u128, 1);
        self.segments.push(Segment {
            tick,
            sample,
            mbpm: to_mbpm(bpm),
        });
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }

    /// Tempo at `tick`, in BPM.
    pub fn bpm_at(&self, tick: Tick) -> f64 {
        self.segment_for_tick(tick).mbpm as f64 / 1000.0
    }

    /// Sample position of `tick`, rounded to the nearest sample (half up).
    pub fn sample_at(&self, tick: Tick) -> i64 {
        self.sample_at_frac(tick as u128, 1)
    }

    /// Sample position of the fractional tick `num / den` (used for swing offsets).
    pub fn sample_at_frac(&self, num: u128, den: u128) -> i64 {
        let whole = (num / den.max(1)) as Tick;
        let seg = self.segment_for_tick(whole);
        let rel_num = num - seg.tick as u128 * den;
        seg.sample + self.span(seg.mbpm, rel_num, den)
    }

    /// Song position in (fractional) ticks of a sample position. Inverse of
    /// [`Self::sample_at`] up to rounding.
    pub fn tick_at(&self, sample: i64) -> f64 {
        let seg = self
            .segments
            .iter()
            .rev()
            .find(|s| s.sample <= sample)
            .unwrap_or(&self.segments[0]);
        let ticks_per_sample = seg.mbpm as f64 * PPQ as f64 / (self.sample_rate as f64 * 60_000.0);
        seg.tick as f64 + (sample - seg.sample) as f64 * ticks_per_sample
    }

    /// Whole tick at or before a sample position.
    pub fn tick_floor(&self, sample: i64) -> Tick {
        if sample <= 0 {
            return 0;
        }
        let seg = self
            .segments
            .iter()
            .rev()
            .find(|s| s.sample <= sample)
            .unwrap_or(&self.segments[0]);
        // ticks = (sample - seg.sample) * mbpm * PPQ / (sr * 60000), floored, exact
        let num = (sample - seg.sample) as u128 * seg.mbpm as u128 * PPQ as u128;
        let den = self.sample_rate as u128 * 60_000;
        let mut t = seg.tick + (num / den) as Tick;
        // sample_at rounds half up, so step back if this tick starts after `sample`
        while t > 0 && self.sample_at(t) > sample {
            t -= 1;
        }
        while self.sample_at(t + 1) <= sample {
            t += 1;
        }
        t
    }

    /// Samples spanned by `num / den` ticks at a tempo: round(ticks × sr × 60000 / (mbpm × PPQ)).
    fn span(&self, mbpm: u64, num: u128, den: u128) -> i64 {
        let n = num * self.sample_rate as u128 * 60_000;
        let d = den * mbpm as u128 * PPQ as u128;
        ((2 * n + d) / (2 * d)) as i64
    }

    fn segment_for_tick(&self, tick: Tick) -> &Segment {
        self.segments
            .iter()
            .rev()
            .find(|s| s.tick <= tick)
            .unwrap_or(&self.segments[0])
    }
}

fn to_mbpm(bpm: f64) -> u64 {
    let b = if bpm.is_finite() { bpm } else { 120.0 };
    ((b.clamp(1.0, 1000.0) * 1000.0).round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_rounding_and_no_drift_over_an_hour() {
        // 130 BPM at 48 kHz: 23.0769… samples per tick, irrational-looking in binary
        let map = TempoMap::constant(130.0, 48_000);
        for t in [0u64, 1, 2, 240, 960, 3840, 123_457] {
            let exact = t as f64 * 48_000.0 * 60.0 / (130.0 * 960.0);
            assert_eq!(map.sample_at(t), exact.round() as i64, "tick {t}");
        }
        // one hour of song time: 130 * 60 beats
        let hour_ticks = 130 * 60 * PPQ;
        assert_eq!(map.sample_at(hour_ticks), 48_000 * 3600);
        // a 16th just before the hour: exact rational, no accumulated error
        let t = hour_ticks - 240;
        let n = t as u128 * 48_000 * 60_000;
        let d = 130_000u128 * 960;
        assert_eq!(map.sample_at(t), ((2 * n + d) / (2 * d)) as i64);
        // summing per-step conversions never drifts from the direct conversion
        let mut step_sum = 0i64;
        let mut prev = 0i64;
        for k in 1..=(hour_ticks / 240) {
            let s = map.sample_at(k * 240);
            step_sum += s - prev;
            prev = s;
        }
        assert_eq!(step_sum, 48_000 * 3600);
    }

    #[test]
    fn inverse_and_floor() {
        let map = TempoMap::constant(97.5, 44_100);
        for t in [0u64, 1, 17, 960, 99_999] {
            let s = map.sample_at(t);
            assert!((map.tick_at(s) - t as f64).abs() < 0.05);
            assert_eq!(map.tick_floor(s), t);
            assert_eq!(map.tick_floor(map.sample_at(t + 1) - 1), t);
        }
        assert_eq!(map.sample_at_frac(480, 2), map.sample_at(240));
    }

    #[test]
    fn tempo_changes_are_piecewise_exact() {
        let mut map = TempoMap::constant(120.0, 48_000);
        map.push_change(960 * 4, 60.0);
        assert_eq!(map.sample_at(960 * 4), 4 * 24_000);
        assert_eq!(map.sample_at(960 * 5), 4 * 24_000 + 48_000);
        assert_eq!(map.bpm_at(960 * 5), 60.0);
        assert_eq!(map.tick_floor(4 * 24_000 + 48_000), 960 * 5);
    }
}
