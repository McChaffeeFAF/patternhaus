//! `cc.*`: controller lanes (M3/T10). CC 0-119 and pitch bend curves that live in a pattern,
//! per channel, and are sent to the channel's instrument while the pattern plays. Dynamics for
//! sampled orchestral instruments usually live on CC1 (mod wheel) or CC11 (expression).

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn register(r: &mut Registry) {
    r.register(typed(
        "cc.set",
        "Replace a controller lane's points (CC or pitch bend) in a tick range, or the whole lane.",
        Scope::Compose,
        MUTATES,
        set,
    ));
    r.register(typed(
        "cc.add_points",
        "Add points to a controller lane without touching the ones already there.",
        Scope::Compose,
        MUTATES,
        add_points,
    ));
    r.register(typed(
        "cc.remove_points",
        "Delete controller lane points by id, or the points of one controller in a tick range.",
        Scope::Compose,
        MUTATES,
        remove_points,
    ));
    r.register(typed(
        "cc.clear",
        "Delete one controller lane of a channel in a pattern, or all of its lanes.",
        Scope::Compose,
        MUTATES,
        clear,
    ));
    r.register(typed(
        "cc.list",
        "List the controller lanes of a pattern (or one channel or controller) with their points and shape.",
        Scope::Read,
        READ,
        list,
    ));
    r.register(typed(
        "cc.shape",
        "Write a ramp, swell or hold on a controller over a tick range from a short spec (the musical way to shape dynamics); returns the points it wrote.",
        Scope::Compose,
        MUTATES,
        shape,
    ));
}

/// A lane point as agents write it.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LanePoint {
    /// Position in ticks, relative to the pattern start (960 per quarter note).
    tick: Tick,
    /// Controller value: 0.0-1.0 for a CC, -1.0 to 1.0 for pitch bend.
    value: f64,
    /// Shape of the segment from this point to the next: linear (default), hold, smooth or s_curve.
    #[serde(default)]
    curve: CurveKind,
    /// -1.0 to 1.0, bends linear and s_curve segments (0 = their plain shape).
    #[serde(default)]
    tension: f32,
}

impl LanePoint {
    fn spec(&self, controller: Controller) -> CcPointSpec {
        CcPointSpec {
            controller,
            tick: self.tick,
            value: self.value,
            curve: self.curve,
            tension: self.tension,
        }
    }
}

fn point_json(p: &CcPoint) -> Value {
    json!({ "id": p.id, "tick": p.tick, "value": p.value, "curve": p.curve, "tension": p.tension })
}

fn controller_json(c: Controller) -> Value {
    json!({ "controller": c, "name": c.name(), "label": c.label() })
}

/// The lane's points inside `[start, end)` (open ends are unbounded), as ids.
fn in_range(lane: &CcLane, start: Option<Tick>, end: Option<Tick>) -> Vec<PointId> {
    lane.points
        .iter()
        .filter(|p| start.is_none_or(|s| p.tick >= s) && end.is_none_or(|e| p.tick < e))
        .map(|p| p.id)
        .collect()
}

fn check_range(start: Option<Tick>, end: Option<Tick>) -> CmdResult<()> {
    if let (Some(s), Some(e)) = (start, end)
        && e <= s
    {
        return Err(CmdError::Other(format!(
            "end ({e}) must be after start ({s})"
        )));
    }
    Ok(())
}

fn range_text(start: Option<Tick>, end: Option<Tick>) -> String {
    match (start, end) {
        (Some(s), Some(e)) => format!("ticks {s}-{e}"),
        (Some(s), None) => format!("from tick {s}"),
        (None, Some(e)) => format!("up to tick {e}"),
        (None, None) => "the whole lane".into(),
    }
}

/// Points of a lane at or past the pattern's end are never played.
fn past_end(project: &Project, pattern: PatternId, ticks: impl Iterator<Item = Tick>) -> usize {
    let len = project.pattern_length(pattern).unwrap_or(Tick::MAX);
    ticks.filter(|t| *t >= len).count()
}

fn warn_past_end(
    project: &Project,
    pattern: PatternId,
    ticks: impl Iterator<Item = Tick>,
) -> Value {
    let n = past_end(project, pattern, ticks);
    if n == 0 {
        Value::Null
    } else {
        json!(format!(
            "{n} point(s) lie at or past the pattern's end ({} ticks) and will not play; lengthen the pattern with pattern.set_length",
            project.pattern_length(pattern).unwrap_or(0)
        ))
    }
}

// ------------------------------------------------------------------ set / add_points

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Set {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// The controller: its CC number (1), a name ("mod wheel", "expression") or "pitch bend".
    controller: Controller,
    /// The new points of the lane (in the range, if one is given).
    points: Vec<LanePoint>,
    /// Replace only points at or after this tick. Omit to start from the beginning of the lane.
    #[serde(default)]
    start: Option<Tick>,
    /// Replace only points before this tick (exclusive). Omit to run to the end of the lane.
    #[serde(default)]
    end: Option<Tick>,
}

fn set(ctx: &mut CommandCtx<'_>, p: Set) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    check_range(p.start, p.end)?;
    let lane = ctx
        .project
        .pattern(p.pattern)?
        .lane(p.channel, p.controller);
    let old = in_range(&lane, p.start, p.end);
    let mut ed = Edit::new(ctx.project);
    for id in &old {
        ed.remove_cc_point(p.pattern, p.channel, *id)?;
    }
    let ids = p
        .points
        .iter()
        .map(|pt| ed.add_cc_point(p.pattern, p.channel, pt.spec(p.controller)))
        .collect::<ModelResult<Vec<_>>>()?;
    let written = ed
        .project()
        .pattern(p.pattern)?
        .lane(p.channel, p.controller);
    let warning = warn_past_end(ed.project(), p.pattern, p.points.iter().map(|x| x.tick));
    let summary = format!(
        "set {} on {name}, {}: {}",
        p.controller.label(),
        range_text(p.start, p.end),
        written.describe()
    );
    Ok(CommandOutput::from_edit(
        ed,
        json!({
            "controller": p.controller, "name": p.controller.name(),
            "added": ids, "removed": old.len(), "warning": warning,
        }),
        summary,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddPoints {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// The controller: its CC number (1), a name ("mod wheel", "expression") or "pitch bend".
    controller: Controller,
    /// Points to add.
    points: Vec<LanePoint>,
}

fn add_points(ctx: &mut CommandCtx<'_>, p: AddPoints) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    ctx.project.pattern(p.pattern)?;
    let mut ed = Edit::new(ctx.project);
    let ids = p
        .points
        .iter()
        .map(|pt| ed.add_cc_point(p.pattern, p.channel, pt.spec(p.controller)))
        .collect::<ModelResult<Vec<_>>>()?;
    let warning = warn_past_end(ed.project(), p.pattern, p.points.iter().map(|x| x.tick));
    let summary = format!(
        "added {} to {} on {name}",
        plural(ids.len(), "point", "points"),
        p.controller.label()
    );
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "controller": p.controller, "name": p.controller.name(), "points": ids, "warning": warning }),
        summary,
    ))
}

// ------------------------------------------------------------------ remove / clear

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemovePoints {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Ids of the points to delete (from cc.list). Use this, or `controller` with a range.
    #[serde(default)]
    ids: Option<Vec<PointId>>,
    /// Delete this controller's points in the range below (the whole lane if no range).
    #[serde(default)]
    controller: Option<Controller>,
    /// With `controller`: delete points at or after this tick.
    #[serde(default)]
    start: Option<Tick>,
    /// With `controller`: delete points before this tick (exclusive).
    #[serde(default)]
    end: Option<Tick>,
}

fn remove_points(ctx: &mut CommandCtx<'_>, p: RemovePoints) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let pat = ctx.project.pattern(p.pattern)?;
    check_range(p.start, p.end)?;
    let (ids, what) = match (&p.ids, p.controller) {
        (Some(ids), None) => (ids.clone(), format!("{} point(s)", ids.len())),
        (None, Some(c)) => (
            in_range(&pat.lane(p.channel, c), p.start, p.end),
            format!("{} points", c.label()),
        ),
        _ => {
            return Err(CmdError::Other(
                "give either ids, or a controller (with an optional start/end)".into(),
            ));
        }
    };
    let mut ed = Edit::new(ctx.project);
    for id in &ids {
        ed.remove_cc_point(p.pattern, p.channel, *id)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "removed": ids }),
        format!("deleted {} ({what}) from {name}", ids.len()),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Clear {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Clear only this controller's lane; omit to clear every lane of the channel.
    #[serde(default)]
    controller: Option<Controller>,
}

fn clear(ctx: &mut CommandCtx<'_>, p: Clear) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let pat = ctx.project.pattern(p.pattern)?;
    let ids: Vec<PointId> = pat
        .cc
        .get(&p.channel)
        .into_iter()
        .flat_map(|m| m.values())
        .filter(|x| p.controller.is_none_or(|c| c == x.controller))
        .map(|x| x.id)
        .collect();
    let mut ed = Edit::new(ctx.project);
    for id in &ids {
        ed.remove_cc_point(p.pattern, p.channel, *id)?;
    }
    let what = p
        .controller
        .map_or("all controller lanes".to_string(), |c| c.label());
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "removed": ids.len() }),
        format!(
            "cleared {what} on {name} ({})",
            plural(ids.len(), "point", "points")
        ),
    ))
}

// ------------------------------------------------------------------ list

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct List {
    /// Pattern id.
    pattern: PatternId,
    /// Only this channel; omit for every channel with lanes.
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Only this controller (CC number, name or "pitch bend").
    #[serde(default)]
    controller: Option<Controller>,
    /// Include every point (default true); false gives just each lane's shape and range.
    #[serde(default = "yes")]
    points: bool,
}

fn yes() -> bool {
    true
}

fn list(ctx: &mut CommandCtx<'_>, p: List) -> CmdResult<CommandOutput> {
    let pat = ctx.project.pattern(p.pattern)?;
    if let Some(c) = p.channel {
        ctx.project.channel(c)?;
    }
    let mut out = Vec::new();
    for ch in pat.cc.keys() {
        if p.channel.is_some_and(|c| c != *ch) {
            continue;
        }
        let cname = ctx
            .project
            .channels
            .get(ch)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        for lane in pat.lanes(*ch) {
            if p.controller.is_some_and(|c| c != lane.controller) {
                continue;
            }
            let (lo, hi) = lane
                .points
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
                    (lo.min(x.value), hi.max(x.value))
                });
            let mut v = json!({
                "channel": ch, "channel_name": cname,
                "controller": lane.controller, "name": lane.controller.name(),
                "label": lane.controller.label(),
                "count": lane.points.len(),
                "first_tick": lane.span().map(|s| s.0), "last_tick": lane.span().map(|s| s.1),
                "min": lo, "max": hi,
                "shape": lane.describe(),
            });
            if p.points {
                v["points"] = lane.points.iter().map(point_json).collect();
            }
            out.push(v);
        }
    }
    let n = out.len();
    Ok(CommandOutput::value(json!({
        "pattern": p.pattern, "length": ctx.project.pattern_length(p.pattern)?,
        "count": n, "lanes": out,
    })))
}

// ------------------------------------------------------------------ shape

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Shape {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// The controller: its CC number (1 = mod wheel, 11 = expression), a name or "pitch bend".
    controller: Controller,
    /// Start of the shape in ticks, relative to the pattern start.
    start: Tick,
    /// End of the shape in ticks (after start). The shape's last point sits here.
    end: Tick,
    /// Value at `start`. Default: the lane's current value at `start`, else 0.
    #[serde(default)]
    from: Option<f64>,
    /// Value at `end`. Default: the same as `from` (so a plain `from` is a hold).
    #[serde(default)]
    to: Option<f64>,
    /// Makes it a swell: the value reached at `peak_at` between `start` and `end`
    /// (e.g. from 0.3, peak 0.9, to 0.4). A value below both ends makes a dip.
    #[serde(default)]
    peak: Option<f64>,
    /// Where the peak sits, 0.0-1.0 of the way from start to end (default 0.5).
    #[serde(default = "half")]
    peak_at: f64,
    /// Segment shape: smooth (default, no overshoot), linear, s_curve or hold.
    #[serde(default = "smooth")]
    curve: CurveKind,
    /// -1.0 to 1.0, bends linear and s_curve segments.
    #[serde(default)]
    tension: f32,
    /// Remove the lane's existing points from `start` to `end` inclusive first (default
    /// true); false keeps them.
    #[serde(default = "yes")]
    replace: bool,
}

fn half() -> f64 {
    0.5
}

fn smooth() -> CurveKind {
    CurveKind::Smooth
}

fn shape(ctx: &mut CommandCtx<'_>, p: Shape) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let pat = ctx.project.pattern(p.pattern)?;
    if p.end <= p.start {
        return Err(CmdError::Other(format!(
            "end ({}) must be after start ({})",
            p.end, p.start
        )));
    }
    if !(0.0..=1.0).contains(&p.peak_at) {
        return Err(CmdError::Other("peak_at must be between 0 and 1".into()));
    }
    let lane = pat.lane(p.channel, p.controller);
    let from = p
        .from
        .unwrap_or_else(|| lane.value_at(p.start as f64).unwrap_or(0.0));
    let to = p.to.unwrap_or(from);
    // the points: start, [peak], end. Only the last one holds; the others take `curve`.
    let mut pts = vec![(p.start, from, p.curve)];
    if let Some(peak) = p.peak {
        let at = p.start + ((p.end - p.start) as f64 * p.peak_at).round() as Tick;
        let at = at.clamp(p.start + 1, p.end - 1);
        if p.end - p.start >= 2 {
            pts.push((at, peak, p.curve));
        }
    }
    pts.push((p.end, to, CurveKind::Hold));
    let remove = if p.replace {
        lane.points
            .iter()
            .filter(|x| x.tick >= p.start && x.tick <= p.end)
            .map(|x| x.id)
            .collect()
    } else {
        Vec::new()
    };
    let mut ed = Edit::new(ctx.project);
    for id in &remove {
        ed.remove_cc_point(p.pattern, p.channel, *id)?;
    }
    let mut written = Vec::new();
    for (tick, value, curve) in &pts {
        let id = ed.add_cc_point(
            p.pattern,
            p.channel,
            CcPointSpec {
                controller: p.controller,
                tick: *tick,
                value: *value,
                curve: *curve,
                tension: if *curve == CurveKind::Hold {
                    0.0
                } else {
                    p.tension
                },
            },
        )?;
        written.push(ed.cc_point(p.pattern, p.channel, id)?.clone());
    }
    let shape_lane = CcLane {
        controller: p.controller,
        points: written.clone(),
    };
    let warning = warn_past_end(ed.project(), p.pattern, pts.iter().map(|x| x.0));
    let summary = format!(
        "{} on {name}, ticks {}-{}: {}",
        p.controller.label(),
        p.start,
        p.end,
        shape_lane.describe()
    );
    let mut result = controller_json(p.controller);
    result["points"] = written.iter().map(point_json).collect();
    result["replaced"] = json!(remove.len());
    result["shape"] = json!(shape_lane.describe());
    result["warning"] = warning;
    Ok(CommandOutput::from_edit(ed, result, summary))
}
