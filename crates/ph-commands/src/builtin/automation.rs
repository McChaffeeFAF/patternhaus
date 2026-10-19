//! `automation.*`: automation clips and their points (M2). The engine plays them (M2/T7).
//! Moving, removing and resizing clips stays with `playlist.*`.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "automation.add_clip",
        "Place an automation clip for a parameter (tempo, channel or mixer volume/pan, send level, an instrument or effect parameter), optionally with points. Without a track it goes on the first track that is free over the clip.",
        Scope::Mix,
        MUTATES,
        add_clip,
    ));
    r.register(typed(
        "automation.add_point",
        "Add a point to an automation clip.",
        Scope::Mix,
        MUTATES,
        add_point,
    ));
    r.register(typed(
        "automation.move_point",
        "Move an automation point in time and/or change its value.",
        Scope::Mix,
        MUTATES,
        move_point,
    ));
    r.register(typed(
        "automation.set_curve",
        "Set the curve shape (and tension) of the segment that starts at an automation point.",
        Scope::Mix,
        MUTATES,
        set_curve,
    ));
    r.register(typed(
        "automation.remove_points",
        "Delete points from an automation clip.",
        Scope::Mix,
        MUTATES,
        remove_points,
    ));
    r.register(typed(
        "automation.set_target",
        "Point an automation clip at another parameter.",
        Scope::Mix,
        MUTATES,
        set_target,
    ));
    r.register(typed(
        "automation.record",
        "Write recorded knob movements of a parameter as a new automation clip: the samples are thinned to a reasonable point density (one undo step). The UI collects them while the song plays with record armed; an agent can pass any sampled curve.",
        Scope::Mix,
        MUTATES,
        record,
    ));
}

/// A point to add.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PointSpec {
    /// Position in ticks from the clip's start (960 per quarter note).
    tick: Tick,
    /// Plain value in the target's unit (BPM for tempo, 0..1 channel volume, -1..1 pan,
    /// 0..2 mixer volume, 0/1 mute, a parameter's own range).
    value: f64,
    /// Shape of the segment to the next point: linear (default), hold, smooth or s_curve.
    #[serde(default)]
    curve: CurveKind,
    /// -1..=1, bends linear and s_curve segments (default 0).
    #[serde(default)]
    tension: f32,
}

/// Refuses values outside a model-defined target's range.
fn check_value(target: &ParamPath, v: f64) -> CmdResult<()> {
    if !v.is_finite() {
        return invalid("automation values must be finite");
    }
    if let Some((lo, hi)) = target.fixed_range()
        && !(lo..=hi).contains(&v)
    {
        return invalid(format!("value {v} is outside {lo}..{hi} for {target}"));
    }
    Ok(())
}

fn automation_of(p: &Project, id: ClipId) -> CmdResult<AutomationClip> {
    let c = p.clip(id)?;
    match c.automation() {
        Some(a) => Ok(a.clone()),
        None => invalid(format!(
            "clip {id} is a {} clip, not an automation clip",
            c.kind_name()
        )),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddClip {
    /// What to automate: `tempo`, `channel/<id>/volume`, `channel/<id>/pan`,
    /// `channel/<id>/param/<param>`, `insert/<n>/volume`, `insert/<n>/pan`,
    /// `insert/<n>/mute`, `insert/<n>/send/<m>` or `insert/<n>/slot/<s>/param/<param>`.
    target: ParamPath,
    /// Playlist track index, 0 = first track; omit for the first track with no clip over
    /// the range.
    #[serde(default)]
    track: Option<u32>,
    /// Start in ticks.
    start: Tick,
    /// Length in ticks; defaults to one bar.
    #[serde(default)]
    length: Option<Tick>,
    /// Points to add. Omitted: one point at the parameter's current value (like "create
    /// automation clip" on a knob), when the model knows it.
    #[serde(default)]
    points: Option<Vec<PointSpec>>,
}

fn add_clip(ctx: &mut CommandCtx<'_>, p: AddClip) -> CmdResult<CommandOutput> {
    let length = match p.length {
        Some(0) => return invalid("length must be greater than zero"),
        Some(l) => l,
        None => ctx.project.settings.time_signature.ticks_per_bar(),
    };
    let track = match p.track {
        Some(t) if (t as usize) >= ctx.project.tracks.len() => {
            return Err(ModelError::NoTrack(t as usize).into());
        }
        Some(t) => t,
        None => free_track(ctx.project, p.start, length)?,
    };
    let points = match p.points {
        Some(v) => v,
        None => p
            .target
            .model_value(ctx.project)
            .map(|value| PointSpec {
                tick: 0,
                value,
                curve: CurveKind::Linear,
                tension: 0.0,
            })
            .into_iter()
            .collect(),
    };
    for pt in &points {
        check_value(&p.target, pt.value)?;
    }
    let mut ed = Edit::new(ctx.project);
    let clip = ed.add_automation_clip(track, p.start, length, p.target.clone())?;
    let ids = points
        .iter()
        .map(|pt| ed.add_point(clip, pt.tick, pt.value, pt.curve, pt.tension))
        .collect::<ModelResult<Vec<_>>>()?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": clip, "track": track, "points": ids }),
        format!(
            "added automation of {} with {}",
            p.target,
            plural(ids.len(), "point", "points")
        ),
    ))
}

/// The first playlist track with no clip overlapping `start..start + length`.
fn free_track(p: &Project, start: Tick, length: Tick) -> CmdResult<u32> {
    let end = start.saturating_add(length);
    (0..p.tracks.len() as u32)
        .find(|t| {
            !p.clips
                .values()
                .any(|c| c.track == *t && c.start < end && start < c.start + c.length)
        })
        .map_or_else(|| invalid("no playlist track is free over that range"), Ok)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddPoint {
    /// Automation clip id.
    clip: ClipId,
    /// Position in ticks from the clip's start.
    tick: Tick,
    /// Plain value in the target's unit.
    value: f64,
    /// Segment shape to the next point: linear (default), hold, smooth or s_curve.
    #[serde(default)]
    curve: CurveKind,
    /// -1..=1 (default 0).
    #[serde(default)]
    tension: f32,
}

fn add_point(ctx: &mut CommandCtx<'_>, p: AddPoint) -> CmdResult<CommandOutput> {
    let a = automation_of(ctx.project, p.clip)?;
    check_value(&a.target, p.value)?;
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_point(p.clip, p.tick, p.value, p.curve, p.tension)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "point": id }),
        format!("added a point to the automation of {}", a.target),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MovePoint {
    /// Automation clip id.
    clip: ClipId,
    /// Point id.
    point: PointId,
    /// New position in ticks from the clip's start; omit to keep.
    #[serde(default)]
    tick: Option<Tick>,
    /// New value; omit to keep.
    #[serde(default)]
    value: Option<f64>,
}

fn move_point(ctx: &mut CommandCtx<'_>, p: MovePoint) -> CmdResult<CommandOutput> {
    let a = automation_of(ctx.project, p.clip)?;
    let old = ctx.project.point(p.clip, p.point)?.clone();
    if let Some(v) = p.value {
        check_value(&a.target, v)?;
    }
    let mut ed = Edit::new(ctx.project);
    ed.move_point(p.clip, p.point, p.tick.unwrap_or(old.tick), p.value)?;
    let new = ed.project().point(p.clip, p.point)?.clone();
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "point": p.point, "tick": new.tick, "value": new.value }),
        format!("moved a point of the automation of {}", a.target),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetCurve {
    /// Automation clip id.
    clip: ClipId,
    /// Point id: the segment from this point to the next changes.
    point: PointId,
    /// linear, hold, smooth or s_curve.
    curve: CurveKind,
    /// -1..=1; omit to keep.
    #[serde(default)]
    tension: Option<f32>,
}

fn set_curve(ctx: &mut CommandCtx<'_>, p: SetCurve) -> CmdResult<CommandOutput> {
    let old = ctx.project.point(p.clip, p.point)?.clone();
    let mut ed = Edit::new(ctx.project);
    ed.set_point_curve(p.clip, p.point, p.curve, p.tension.unwrap_or(old.tension))?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "point": p.point }),
        format!("set an automation segment to {:?}", p.curve).to_lowercase(),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemovePoints {
    /// Automation clip id.
    clip: ClipId,
    /// Point ids to delete.
    points: Vec<PointId>,
}

fn remove_points(ctx: &mut CommandCtx<'_>, p: RemovePoints) -> CmdResult<CommandOutput> {
    automation_of(ctx.project, p.clip)?;
    let mut ed = Edit::new(ctx.project);
    for id in &p.points {
        ed.remove_point(p.clip, *id)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "points": p.points }),
        format!(
            "removed {}",
            plural(p.points.len(), "automation point", "automation points")
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTarget {
    /// Automation clip id.
    clip: ClipId,
    /// The new target (see automation.add_clip). Existing points keep their values.
    target: ParamPath,
}

fn set_target(ctx: &mut CommandCtx<'_>, p: SetTarget) -> CmdResult<CommandOutput> {
    let a = automation_of(ctx.project, p.clip)?;
    for pt in a.points.values() {
        check_value(&p.target, pt.value)?;
    }
    let mut ed = Edit::new(ctx.project);
    ed.update_clip(p.clip, |c| {
        if let ClipKind::Automation(a) = &mut c.kind {
            a.target = p.target.clone();
        }
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "target": p.target }),
        format!("automation clip {} now drives {}", p.clip, p.target),
    ))
}

/// One recorded value.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Sample {
    /// Song position in ticks.
    tick: Tick,
    /// Plain value in the target's unit.
    value: f64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Record {
    /// The parameter whose knob movements were recorded.
    target: ParamPath,
    /// The recorded values with their song positions, in any order (at least one).
    samples: Vec<Sample>,
    /// Playlist track for the new clip, 0 = first track; omit for the first free track.
    #[serde(default)]
    track: Option<u32>,
    /// How far, as a fraction of the target's range, the thinned curve may stray from the
    /// samples (default 0.005).
    #[serde(default)]
    tolerance: Option<f64>,
    /// A silence between two samples longer than this many ticks (default 480, an eighth
    /// note) was a pause in the movement: the value is held across it (default 480).
    #[serde(default)]
    hold_gap: Option<Tick>,
}

/// Ramer-Douglas-Peucker on `(tick, value)`: the indices to keep.
fn thin(pts: &[(f64, f64)], tol: f64) -> Vec<usize> {
    let n = pts.len();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ta, va) = pts[a];
        let (tb, vb) = pts[b];
        let mut worst = (0.0f64, a);
        for (k, &(t, v)) in pts.iter().enumerate().take(b).skip(a + 1) {
            let f = if tb > ta { (t - ta) / (tb - ta) } else { 0.0 };
            let d = (v - (va + (vb - va) * f)).abs();
            if d > worst.0 {
                worst = (d, k);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    (0..n).filter(|k| keep[*k]).collect()
}

fn record(ctx: &mut CommandCtx<'_>, p: Record) -> CmdResult<CommandOutput> {
    if p.samples.is_empty() {
        return invalid("record at least one sample");
    }
    for s in &p.samples {
        check_value(&p.target, s.value)?;
    }
    let tol_frac = p.tolerance.unwrap_or(0.005);
    if !(0.0..=1.0).contains(&tol_frac) {
        return invalid("tolerance is a fraction of the range, 0..1");
    }
    let hold_gap = p.hold_gap.unwrap_or(480);
    // by position; the latest sample at one tick wins
    let mut all: Vec<(Tick, f64)> = p.samples.iter().map(|s| (s.tick, s.value)).collect();
    all.sort_by_key(|s| s.0);
    all.dedup_by(|b, a| {
        if a.0 == b.0 {
            a.1 = b.1;
            true
        } else {
            false
        }
    });
    let from = all[0].0;
    let to = all[all.len() - 1].0;
    // the scale of "a reasonable density": the target's range, or what was recorded
    let span = match p.target.fixed_range() {
        Some((lo, hi)) => hi - lo,
        None => {
            let (lo, hi) = all
                .iter()
                .fold((f64::MAX, f64::MIN), |m, s| (m.0.min(s.1), m.1.max(s.1)));
            hi - lo
        }
    };
    let tol = span * tol_frac;
    // thin each stretch between pauses on its own, so a pause keeps its two ends
    let mut kept: Vec<(Tick, f64, bool)> = Vec::new(); // tick, value, hold the segment after
    let mut start = 0;
    while start < all.len() {
        let mut end = start;
        while end + 1 < all.len() && all[end + 1].0 - all[end].0 <= hold_gap {
            end += 1;
        }
        let run: Vec<(f64, f64)> = all[start..=end].iter().map(|s| (s.0 as f64, s.1)).collect();
        for k in thin(&run, tol) {
            kept.push((all[start + k].0, all[start + k].1, false));
        }
        if end + 1 < all.len() {
            kept.last_mut().expect("a run keeps its ends").2 = true;
        }
        start = end + 1;
    }
    let track = match p.track {
        Some(t) if (t as usize) >= ctx.project.tracks.len() => {
            return Err(ModelError::NoTrack(t as usize).into());
        }
        Some(t) => t,
        None => free_track(ctx.project, from, (to - from).max(1))?,
    };
    let mut ed = Edit::new(ctx.project);
    let clip = ed.add_automation_clip(track, from, (to - from).max(STEP), p.target.clone())?;
    let ids = kept
        .iter()
        .map(|&(t, v, hold)| {
            let curve = if hold {
                CurveKind::Hold
            } else {
                CurveKind::Linear
            };
            ed.add_point(clip, t - from, v, curve, 0.0)
        })
        .collect::<ModelResult<Vec<_>>>()?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": clip, "track": track, "start": from, "length": (to - from).max(STEP), "points": ids, "samples": all.len() }),
        format!(
            "recorded {} of {} into {}",
            plural(ids.len(), "point", "points"),
            plural(all.len(), "sample", "samples"),
            p.target
        ),
    ))
}
