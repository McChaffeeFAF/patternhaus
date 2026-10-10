//! `step.*`: the step sequencer, a view of the notes of one channel in a pattern.
//!
//! A lit step is a note exactly one `STEP` (a 16th) long that starts on the step grid. Notes
//! that do not look like that (drawn in the piano roll) are left alone.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "step.toggle",
        "Light or clear one step in a channel's row.",
        Scope::Compose,
        MUTATES,
        toggle,
    ));
    r.register(typed(
        "step.set",
        "Set a whole row of steps from a list of on/off values.",
        Scope::Compose,
        MUTATES,
        set_row,
    ));
    r.register(typed(
        "step.set_velocity",
        "Set the velocity of a lit step.",
        Scope::Compose,
        MUTATES,
        set_velocity,
    ));
    r.register(typed(
        "step.set_pitch",
        "Set the pitch (MIDI key) of a lit step.",
        Scope::Compose,
        MUTATES,
        set_pitch,
    ));
    r.register(typed(
        "step.get_row",
        "Read a channel's step row: which steps are lit, with velocities and keys.",
        Scope::Read,
        READ,
        get_row,
    ));
}

fn default_key(p: &Project, ch: ChannelId) -> CmdResult<u8> {
    Ok(match p.channel(ch)?.instrument {
        InstrumentSpec::Sampler { root_key, .. } => root_key,
        _ => 60,
    })
}

fn default_velocity() -> f32 {
    0.78
}

/// The note that lights `step`, if any.
fn lit(p: &Project, pat: PatternId, ch: ChannelId, step: u32) -> CmdResult<Option<Note>> {
    Ok(notes_of(p, pat, ch)?
        .into_iter()
        .find(|n| n.start == step as Tick * STEP && n.length == STEP))
}

fn step_spec(step: u32, key: u8, velocity: f32) -> NoteSpec {
    let mut s = NoteSpec::new(key, step as Tick * STEP, STEP);
    s.velocity = velocity;
    s
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Toggle {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id (the row).
    channel: ChannelId,
    /// Step index, 0 = first step.
    step: u32,
    /// Velocity when the step is lit, default 0.78.
    #[serde(default = "default_velocity")]
    velocity: f32,
    /// Key when the step is lit; defaults to the sampler's root key, or middle C.
    #[serde(default)]
    key: Option<u8>,
}

fn toggle(ctx: &mut CommandCtx<'_>, p: Toggle) -> CmdResult<CommandOutput> {
    range("velocity", p.velocity, 0.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    ctx.project.pattern(p.pattern)?;
    let mut ed = Edit::new(ctx.project);
    let on = match lit(ctx.project, p.pattern, p.channel, p.step)? {
        Some(n) => {
            ed.remove_note(p.pattern, p.channel, n.id)?;
            false
        }
        None => {
            let key = match p.key {
                Some(k) => k,
                None => default_key(ctx.project, p.channel)?,
            };
            ed.add_note(p.pattern, p.channel, step_spec(p.step, key, p.velocity))?;
            true
        }
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "step": p.step, "on": on }),
        format!(
            "{} step {} of {name}",
            if on { "lit" } else { "cleared" },
            p.step + 1
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRow {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id (the row).
    channel: ChannelId,
    /// One value per step starting at step 0: true = lit. Steps past the end are not touched.
    steps: Vec<bool>,
    /// Velocity for newly lit steps, default 0.78 (already lit steps keep theirs).
    #[serde(default = "default_velocity")]
    velocity: f32,
    /// Key for newly lit steps; defaults to the sampler's root key, or middle C.
    #[serde(default)]
    key: Option<u8>,
}

fn set_row(ctx: &mut CommandCtx<'_>, p: SetRow) -> CmdResult<CommandOutput> {
    range("velocity", p.velocity, 0.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    ctx.project.pattern(p.pattern)?;
    let key = match p.key {
        Some(k) => k,
        None => default_key(ctx.project, p.channel)?,
    };
    let mut ed = Edit::new(ctx.project);
    for (i, want) in p.steps.iter().enumerate() {
        let step = i as u32;
        match (lit(ctx.project, p.pattern, p.channel, step)?, want) {
            (None, true) => {
                ed.add_note(p.pattern, p.channel, step_spec(step, key, p.velocity))?;
            }
            (Some(n), false) => ed.remove_note(p.pattern, p.channel, n.id)?,
            _ => {}
        }
    }
    let on = p.steps.iter().filter(|s| **s).count();
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "steps": p.steps.len(), "lit": on }),
        format!("set {name}: {on} of {} steps lit", p.steps.len()),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVelocity {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id (the row).
    channel: ChannelId,
    /// Step index, 0 = first step. The step must be lit.
    step: u32,
    /// Velocity 0.0..=1.0.
    velocity: f32,
}

fn set_velocity(ctx: &mut CommandCtx<'_>, p: SetVelocity) -> CmdResult<CommandOutput> {
    range("velocity", p.velocity, 0.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    let Some(n) = lit(ctx.project, p.pattern, p.channel, p.step)? else {
        return invalid(format!("step {} of {name} is not lit", p.step));
    };
    let mut ed = Edit::new(ctx.project);
    ed.update_note(p.pattern, p.channel, n.id, |x| x.velocity = p.velocity)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "step": p.step }),
        format!(
            "step {} of {name} velocity {:.0}%",
            p.step + 1,
            p.velocity * 100.0
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetPitch {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id (the row).
    channel: ChannelId,
    /// Step index, 0 = first step. The step must be lit.
    step: u32,
    /// MIDI key, 0..=127 (60 = middle C).
    key: u8,
}

fn set_pitch(ctx: &mut CommandCtx<'_>, p: SetPitch) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    if p.key > 127 {
        return invalid("key must be 0..=127");
    }
    let Some(n) = lit(ctx.project, p.pattern, p.channel, p.step)? else {
        return invalid(format!("step {} of {name} is not lit", p.step));
    };
    let mut ed = Edit::new(ctx.project);
    ed.update_note(p.pattern, p.channel, n.id, |x| x.key = p.key)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "step": p.step, "key": p.key }),
        format!("step {} of {name} pitch set to key {}", p.step + 1, p.key),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetRow {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id (the row).
    channel: ChannelId,
    /// How many steps to report; defaults to the pattern's length in steps.
    #[serde(default)]
    steps: Option<u32>,
}

fn get_row(ctx: &mut CommandCtx<'_>, p: GetRow) -> CmdResult<CommandOutput> {
    ctx.project.channel(p.channel)?;
    let count = match p.steps {
        Some(s) => s,
        None => (ctx.project.pattern_length(p.pattern)? / STEP) as u32,
    };
    let (mut on, mut velocities, mut keys) = (Vec::new(), Vec::new(), Vec::new());
    for s in 0..count {
        let n = lit(ctx.project, p.pattern, p.channel, s)?;
        on.push(n.is_some());
        velocities.push(n.as_ref().map(|n| n.velocity));
        keys.push(n.as_ref().map(|n| n.key));
    }
    Ok(CommandOutput::value(json!({
        "steps": count, "on": on, "velocities": velocities, "keys": keys
    })))
}
