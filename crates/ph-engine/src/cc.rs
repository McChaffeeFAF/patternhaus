//! Controller lanes in the plan (M3/T10).
//!
//! # Design
//!
//! Lane data is compiled with the plan, off the audio thread, into one sorted event list per
//! `(channel, controller)`: [`CcLaneStream`]. The audio thread only binary-searches and reads
//! these lists, so it never allocates and never keeps lane state: what a block sends depends
//! only on the transport position and the plan, so live and offline renders send the same
//! events however the audio is chopped into blocks.
//!
//! - **Control rate.** Points are exact (sent at their own sample). A ramp between two points
//!   is sampled every [`CC_RATE`] frames (64: about 1.33 ms at 48 kHz) counted from the point
//!   it leaves, on the *timeline* (so tempo changes and trims behave like notes do).
//! - **Dedupe.** The value sent is the 7-bit CC value (14-bit pitch bend,
//!   [`Controller::wire`]); an event is dropped when it equals the one before it.
//! - **Chase.** The first event of every clip window (song mode) or of the pattern (pattern
//!   mode) is *forced*: it is kept even if equal to its predecessor, and carries the lane's
//!   value at the window's first tick. When playback starts, seeks, wraps a loop or a new plan
//!   is installed at a position inside a stream, the runtime first sends the last event
//!   before that position (see `schedule_events`). At a clip's end the last value stays; the
//!   next clip's forced first event sets its own.
//! - Several clips feeding one channel and controller merge into one list, sorted by sample;
//!   at one sample the later event wins.

use crate::tempo::TempoMap;
use ph_core::{AutomationPoint, ChannelId, Controller, Pattern, Tick, value_at};
use std::collections::BTreeMap;

/// Frames between the samples of a ramp.
pub(crate) const CC_RATE: i64 = 64;

/// One controller message of a lane, stamped in timeline samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CcEv {
    pub sample: i64,
    /// Already rounded to what MIDI can carry ([`Controller::wire`]).
    pub value: f32,
    /// Kept even when equal to the previous event (clip start chase).
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CcLaneStream {
    pub controller: Controller,
    pub events: Vec<CcEv>,
}

/// A stretch of a pattern played at some position of the timeline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    /// First pattern tick played.
    pub lo: Tick,
    /// One past the last pattern tick played (`> lo`).
    pub hi: Tick,
    /// Timeline tick at which pattern tick `lo` sounds.
    pub origin: Tick,
}

/// The lanes `channel` plays out of these `(pattern, window)` pairs.
pub(crate) fn compile(
    parts: &[(&Pattern, Window)],
    channel: ChannelId,
    tempo: &TempoMap,
) -> Vec<CcLaneStream> {
    let mut raw: BTreeMap<Controller, Vec<CcEv>> = BTreeMap::new();
    for (pat, w) in parts {
        if w.hi <= w.lo {
            continue;
        }
        for lane in pat.lanes(channel) {
            let pts = lane.auto_points();
            let out = raw.entry(lane.controller).or_default();
            segment(out, lane.controller, &pts, w, tempo);
        }
    }
    raw.into_iter()
        .filter_map(|(controller, mut ev)| {
            // stable: equal samples keep their order of arrival (clip, then point)
            ev.sort_by_key(|e| e.sample);
            let mut out: Vec<CcEv> = Vec::with_capacity(ev.len());
            for e in ev {
                if let Some(last) = out.last_mut()
                    && last.sample == e.sample
                {
                    // the later event wins; a forced one stays forced
                    let force = last.force || e.force;
                    *last = CcEv { force, ..e };
                    continue;
                }
                if !e.force && out.last().is_some_and(|l| l.value == e.value) {
                    continue;
                }
                out.push(e);
            }
            (!out.is_empty()).then_some(CcLaneStream {
                controller,
                events: out,
            })
        })
        .collect()
}

fn segment(
    out: &mut Vec<CcEv>,
    c: Controller,
    pts: &[AutomationPoint],
    w: &Window,
    tempo: &TempoMap,
) {
    let timeline = |pattern_tick: Tick| w.origin + (pattern_tick - w.lo);
    let s_start = tempo.sample_at(w.origin);
    let s_end = tempo.sample_at(timeline(w.hi));
    if s_end <= s_start {
        return;
    }
    let at = |tick: f64| value_at(pts, tick).unwrap_or(0.0);
    let mut last = c.wire(at(w.lo as f64));
    out.push(CcEv {
        sample: s_start,
        value: last,
        force: true,
    });
    // knots: every point strictly inside the window, then the window's end
    let mut knots: Vec<Tick> = pts
        .iter()
        .map(|p| p.tick)
        .filter(|t| *t > w.lo && *t < w.hi)
        .collect();
    knots.dedup();
    knots.push(w.hi);
    let mut prev = s_start;
    for k in knots {
        let ks = if k == w.hi {
            s_end
        } else {
            tempo.sample_at(timeline(k)).clamp(s_start, s_end)
        };
        // the ramp from the previous knot up to this one
        let mut g = prev + CC_RATE;
        while g < ks {
            let tick = tempo.tick_at(g) - w.origin as f64 + w.lo as f64;
            let v = c.wire(at(tick));
            if v != last {
                out.push(CcEv {
                    sample: g,
                    value: v,
                    force: false,
                });
                last = v;
            }
            g += CC_RATE;
        }
        if k < w.hi {
            let v = c.wire(at(k as f64));
            // a knot is always sent, so a Hold jump or a repeated tick lands on its own sample
            out.push(CcEv {
                sample: ks,
                value: v,
                force: false,
            });
            last = v;
        }
        prev = ks;
    }
}
