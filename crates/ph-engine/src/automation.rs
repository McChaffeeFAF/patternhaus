//! Automation evaluator (M2/T0 interface with a reference implementation; M2/T7 wires it
//! into the render plan and may optimise it).
//!
//! Compiled off the audio thread from a project snapshot ([`AutomationEvaluator::new`]);
//! queried on the audio thread without allocating ([`AutomationEvaluator::fill`],
//! [`AutomationEvaluator::param_events`]). The model rules it follows (plain values, point
//! ticks relative to the clip source, the latest-starting clip wins on overlap, values held
//! before the first and after the last point *inside* a clip) are in `ph_core::automation`.
//! Outside every clip of a target there is no automation value: the target keeps its
//! model value.
//!
//! Tempo automation does not go through `Param` events: [`AutomationEvaluator::tempo_map`]
//! turns it into a [`TempoMap`], which the scheduler already uses for every tick ↔ sample
//! conversion.

use crate::tempo::TempoMap;
use ph_core::*;
use ph_node::{AUTOMATION_INTERVAL, Event, EventKind, ParamId};
use std::collections::BTreeMap;

/// One automation clip, flattened for evaluation.
#[derive(Clone, Debug, PartialEq)]
struct Lane {
    /// Song ticks covered: `start..end`.
    start: Tick,
    end: Tick,
    /// Clip source offset (point ticks are relative to the source).
    offset: Tick,
    /// In evaluation order (tick, id); never empty.
    points: Vec<AutomationPoint>,
}

impl Lane {
    fn covers(&self, tick: f64) -> bool {
        self.start as f64 <= tick && tick < self.end as f64
    }

    fn value(&self, tick: f64) -> f64 {
        let src = tick - self.start as f64 + self.offset as f64;
        value_at(&self.points, src).expect("lanes have points")
    }
}

/// Automation of one project snapshot, by target.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutomationEvaluator {
    /// Lanes per target, sorted by (start, clip id): the last covering lane wins.
    lanes: BTreeMap<ParamPath, Vec<Lane>>,
}

impl AutomationEvaluator {
    /// Compiles every automation clip with points on an unmuted track. Off the audio thread.
    pub fn new(project: &Project) -> Self {
        let mut clips: Vec<&Clip> = project
            .clips
            .values()
            .filter(|c| {
                !project
                    .tracks
                    .get(c.track as usize)
                    .is_some_and(|t| t.muted)
            })
            .collect();
        clips.sort_by_key(|c| (c.start, c.id));
        let mut lanes: BTreeMap<ParamPath, Vec<Lane>> = BTreeMap::new();
        for c in clips {
            let Some(a) = c.automation() else { continue };
            if a.points.is_empty() {
                continue;
            }
            lanes.entry(a.target.clone()).or_default().push(Lane {
                start: c.start,
                end: c.start + c.length,
                offset: c.offset,
                points: a.points_in_order().into_iter().cloned().collect(),
            });
        }
        Self { lanes }
    }

    /// Every automated target.
    pub fn targets(&self) -> impl Iterator<Item = &ParamPath> {
        self.lanes.keys()
    }

    pub fn is_automated(&self, target: &ParamPath) -> bool {
        self.lanes.contains_key(target)
    }

    fn lane_at(&self, target: &ParamPath, tick: f64) -> Option<&Lane> {
        self.lanes
            .get(target)?
            .iter()
            .rev()
            .find(|l| l.covers(tick))
    }

    /// The automated value at song tick `tick`, or `None` outside every clip of `target`.
    /// Allocation-free.
    pub fn value_at(&self, target: &ParamPath, tick: f64) -> Option<f64> {
        self.lane_at(target, tick).map(|l| l.value(tick))
    }

    /// Real-time: per-frame values for the block starting at song sample `start`, one per
    /// element of `out`. Frames outside every clip are `NAN` (keep the model value). Returns
    /// false, leaving `out` untouched, when `target` is not automated at all.
    pub fn fill(&self, target: &ParamPath, tempo: &TempoMap, start: i64, out: &mut [f64]) -> bool {
        if !self.is_automated(target) {
            return false;
        }
        for (i, o) in out.iter_mut().enumerate() {
            *o = self
                .value_at(target, tempo.tick_at(start + i as i64))
                .unwrap_or(f64::NAN);
        }
        true
    }

    /// Real-time: the `Param` events for parameter `id` driven by `target` in the block of
    /// `frames` frames starting at song sample `start` (the ph-node automation contract): one
    /// at each frame where an automation point falls, and at frame 0 and every
    /// [`AUTOMATION_INTERVAL`] frames while a clip covers the frame. Sorted by frame; at most
    /// `out.len()` are written. Returns how many.
    pub fn param_events(
        &self,
        target: &ParamPath,
        id: ParamId,
        tempo: &TempoMap,
        start: i64,
        frames: usize,
        out: &mut [Event],
    ) -> usize {
        let Some(lanes) = self.lanes.get(target) else {
            return 0;
        };
        let end = start + frames as i64;
        let mut n = 0;
        let mut f = 0usize;
        while f < frames && n < out.len() {
            // the next point frame in this interval, if any, comes first
            let grid_next = (f / AUTOMATION_INTERVAL as usize + 1) * AUTOMATION_INTERVAL as usize;
            let point = next_point_frame(lanes, tempo, start + f as i64 + 1, end)
                .map(|s| (s - start) as usize)
                .filter(|p| *p < grid_next.min(frames));
            let tick = tempo.tick_at(start + f as i64);
            if let Some(v) = self.value_at(target, tick) {
                out[n] = Event {
                    frame: f as u32,
                    kind: EventKind::Param { id, value: v },
                };
                n += 1;
            }
            f = point.unwrap_or(grid_next);
        }
        n
    }

    /// The tempo map with tempo automation applied. The automated tempo is sampled every
    /// `resolution` ticks (one constant step each, clamped to 10..=999 BPM); outside tempo
    /// clips the tempo is `bpm`. Without tempo automation this is `TempoMap::constant`.
    pub fn tempo_map(&self, bpm: f64, sample_rate: u32, resolution: Tick) -> TempoMap {
        let at = |t: Tick| {
            self.value_at(&ParamPath::Tempo, t as f64)
                .map_or(bpm, |v| v.clamp(10.0, 999.0))
        };
        let mut map = TempoMap::constant(at(0), sample_rate);
        let Some(lanes) = self.lanes.get(&ParamPath::Tempo) else {
            return map;
        };
        let last = lanes.iter().map(|l| l.end).max().unwrap_or(0);
        let step = resolution.max(1);
        let mut current = at(0);
        let mut t = step;
        while t <= last {
            let v = at(t);
            if v != current {
                map.push_change(t, v);
                current = v;
            }
            t += step;
        }
        map
    }
}

/// The first song sample in `from..end` where a point of any lane sits, if any.
fn next_point_frame(lanes: &[Lane], tempo: &TempoMap, from: i64, end: i64) -> Option<i64> {
    let mut best: Option<i64> = None;
    for l in lanes {
        for p in &l.points {
            let Some(song) = (l.start + p.tick).checked_sub(l.offset) else {
                continue;
            };
            if song < l.start || song >= l.end {
                continue;
            }
            let s = tempo.sample_at(song);
            if s >= from && s < end && best.is_none_or(|b| s < b) {
                best = Some(s);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (Project, ClipId) {
        let mut ed = Edit::new(&Project::empty("a"));
        let target = ParamPath::InsertVolume(InsertId(1));
        let a = ed
            .add_automation_clip(0, 0, 4 * PPQ, target.clone())
            .unwrap();
        ed.add_point(a, 0, 0.0, CurveKind::Linear, 0.0).unwrap();
        ed.add_point(a, 2 * PPQ, 1.0, CurveKind::Hold, 0.0).unwrap();
        // a later clip on the same target wins where it overlaps
        let b = ed.add_automation_clip(1, 3 * PPQ, PPQ, target).unwrap();
        ed.add_point(b, 0, 0.25, CurveKind::Hold, 0.0).unwrap();
        let t = ed
            .add_automation_clip(2, 0, 8 * PPQ, ParamPath::Tempo)
            .unwrap();
        ed.add_point(t, 0, 120.0, CurveKind::Linear, 0.0).unwrap();
        ed.add_point(t, 4 * PPQ, 180.0, CurveKind::Hold, 0.0)
            .unwrap();
        let (p, _) = ed.finish();
        (p, a)
    }

    #[test]
    fn values_follow_clips_points_and_overlap_rules() {
        let (p, _) = project();
        let e = AutomationEvaluator::new(&p);
        let v = ParamPath::InsertVolume(InsertId(1));
        assert_eq!(e.value_at(&v, 0.0), Some(0.0));
        assert_eq!(e.value_at(&v, PPQ as f64), Some(0.5));
        assert_eq!(e.value_at(&v, 2.5 * PPQ as f64), Some(1.0));
        assert_eq!(e.value_at(&v, 3.5 * PPQ as f64), Some(0.25));
        assert_eq!(e.value_at(&v, 4.0 * PPQ as f64), None, "after the clips");
        assert!(!e.is_automated(&ParamPath::InsertPan(InsertId(1))));
        assert_eq!(e.targets().count(), 2);
        // muted tracks do not automate
        let mut ed = Edit::new(&p);
        ed.update_track(1, |t| t.muted = true).unwrap();
        let e = AutomationEvaluator::new(ed.project());
        assert_eq!(e.value_at(&v, 3.5 * PPQ as f64), Some(1.0));
    }

    #[test]
    fn param_events_meet_the_ph_node_contract() {
        let (p, _) = project();
        let e = AutomationEvaluator::new(&p);
        let v = ParamPath::InsertVolume(InsertId(1));
        let tempo = TempoMap::constant(120.0, 48_000);
        let mut out = [Event {
            frame: 0,
            kind: EventKind::AllNotesOff,
        }; 64];
        // a block straddling the point at 2 beats (= 48000 samples at 120 BPM)
        let n = e.param_events(&v, ParamId(7), &tempo, 47_990, 100, &mut out);
        let frames: Vec<u32> = out[..n].iter().map(|e| e.frame).collect();
        assert_eq!(frames, vec![0, 10, 32, 64, 96]);
        assert!(
            frames
                .windows(2)
                .all(|w| w[1] - w[0] <= AUTOMATION_INTERVAL)
        );
        let EventKind::Param { id, value } = out[1].kind else {
            panic!()
        };
        assert_eq!((id, value), (ParamId(7), 1.0));
        // exact values: the event at frame 0 carries the curve value at its own frame
        let EventKind::Param { value, .. } = out[0].kind else {
            panic!()
        };
        assert_eq!(Some(value), e.value_at(&v, tempo.tick_at(47_990)));
        // nothing outside the clips, nothing for other targets, never past `out`
        assert_eq!(
            e.param_events(&v, ParamId(7), &tempo, 10 * 48_000, 64, &mut out),
            0
        );
        assert_eq!(
            e.param_events(&ParamPath::Tempo, ParamId(0), &tempo, 0, 64, &mut out[..1]),
            1
        );
        let mut vals = [0.0; 4];
        assert!(e.fill(&v, &tempo, 0, &mut vals));
        assert_eq!(vals[0], 0.0);
        assert!(!e.fill(&ParamPath::InsertPan(InsertId(0)), &tempo, 0, &mut vals));
    }

    #[test]
    fn tempo_automation_becomes_a_tempo_map() {
        let (p, _) = project();
        let e = AutomationEvaluator::new(&p);
        let map = e.tempo_map(130.0, 48_000, PPQ);
        assert_eq!(map.bpm_at(0), 120.0);
        assert_eq!(map.bpm_at(PPQ), 135.0);
        assert_eq!(map.bpm_at(5 * PPQ), 180.0);
        assert_eq!(
            map.bpm_at(9 * PPQ),
            130.0,
            "the project tempo after the clip"
        );
        let plain = AutomationEvaluator::new(&Project::empty("x"));
        assert_eq!(
            plain.tempo_map(130.0, 48_000, PPQ),
            TempoMap::constant(130.0, 48_000)
        );
    }
}
