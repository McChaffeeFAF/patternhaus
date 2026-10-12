//! `automation.*`: automation clips and their points (M2). Model-only; the engine applies
//! automation from M2/T7. Moving, removing and resizing clips stays with `playlist.*`.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "automation.add_clip",
        "Place an automation clip for a parameter (tempo, channel or mixer volume/pan, an instrument or effect parameter), optionally with points.",
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
        "Record knob movements of a parameter into an automation clip while the song plays.",
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
    /// `insert/<n>/mute` or `insert/<n>/slot/<s>/param/<param>`.
    target: ParamPath,
    /// Playlist track index, 0 = first track.
    track: u32,
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
    if (p.track as usize) >= ctx.project.tracks.len() {
        return Err(ModelError::NoTrack(p.track as usize).into());
    }
    let length = match p.length {
        Some(0) => return invalid("length must be greater than zero"),
        Some(l) => l,
        None => ctx.project.settings.time_signature.ticks_per_bar(),
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
    let clip = ed.add_automation_clip(p.track, p.start, length, p.target.clone())?;
    let ids = points
        .iter()
        .map(|pt| ed.add_point(clip, pt.tick, pt.value, pt.curve, pt.tension))
        .collect::<ModelResult<Vec<_>>>()?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": clip, "points": ids }),
        format!(
            "added automation of {} with {}",
            p.target,
            plural(ids.len(), "point", "points")
        ),
    ))
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // read by M2/T7
struct Record {
    /// The parameter whose knob movements are recorded.
    target: ParamPath,
    /// Playlist track for the new clip, 0 = first track.
    track: u32,
}

fn record(_: &mut CommandCtx<'_>, _: Record) -> CmdResult<CommandOutput> {
    unavailable("automation.record", "automation recording", "T7")
}
