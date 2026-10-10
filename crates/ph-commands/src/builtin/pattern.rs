//! `pattern.*`: the pattern list.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "pattern.add",
        "Create a new empty pattern.",
        Scope::Compose,
        MUTATES,
        add,
    ));
    r.register(typed(
        "pattern.remove",
        "Delete a pattern with its notes and playlist clips (undoable).",
        Scope::Compose,
        MUTATES,
        remove,
    ));
    r.register(typed(
        "pattern.rename",
        "Rename a pattern.",
        Scope::Compose,
        MUTATES,
        rename,
    ));
    r.register(typed(
        "pattern.set_color",
        "Set a pattern's colour (0xRRGGBB).",
        Scope::Compose,
        MUTATES,
        set_color,
    ));
    r.register(typed(
        "pattern.set_length",
        "Set a pattern's length in ticks or steps, or let it follow its notes.",
        Scope::Compose,
        MUTATES,
        set_length,
    ));
    r.register(typed(
        "pattern.clone",
        "Duplicate a pattern with all of its notes.",
        Scope::Compose,
        MUTATES,
        clone,
    ));
    r.register(typed(
        "pattern.move",
        "Move a pattern to a position in the pattern list.",
        Scope::Compose,
        MUTATES,
        move_pattern,
    ));
    r.register(typed(
        "pattern.clear_channel",
        "Delete every note one channel has in a pattern.",
        Scope::Compose,
        MUTATES,
        clear_channel,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Add {
    /// Display name; defaults to "Pattern N".
    #[serde(default)]
    name: Option<String>,
}

fn add(ctx: &mut CommandCtx<'_>, p: Add) -> CmdResult<CommandOutput> {
    let n = ctx.project.patterns.len() + 1;
    let name = p.name.unwrap_or_else(|| format!("Pattern {n}"));
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_pattern(&name)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": id }),
        format!("created pattern {name} ({id})"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PatternOnly {
    /// Pattern id.
    pattern: PatternId,
}

fn remove(ctx: &mut CommandCtx<'_>, p: PatternOnly) -> CmdResult<CommandOutput> {
    let pat = ctx.project.pattern(p.pattern)?;
    let (name, notes) = (pat.name.clone(), pat.note_count());
    let clips = ctx
        .project
        .clips
        .values()
        .filter(|c| c.pattern == p.pattern)
        .count();
    let mut ed = Edit::new(ctx.project);
    ed.remove_pattern(p.pattern)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": p.pattern, "notes": notes, "clips": clips }),
        format!(
            "deleted pattern {name} ({}, {})",
            plural(notes, "note", "notes"),
            plural(clips, "clip", "clips")
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    /// Pattern id.
    pattern: PatternId,
    /// New display name.
    name: String,
}

fn rename(ctx: &mut CommandCtx<'_>, p: Rename) -> CmdResult<CommandOutput> {
    let old = ctx.project.pattern(p.pattern)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.update_pattern(p.pattern, |h| h.name = p.name.clone())?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": p.pattern }),
        format!("renamed pattern {old} to {}", p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetColor {
    /// Pattern id.
    pattern: PatternId,
    /// Colour as 0xRRGGBB (a plain integer in JSON).
    color: Color,
}

fn set_color(ctx: &mut CommandCtx<'_>, p: SetColor) -> CmdResult<CommandOutput> {
    if p.color > 0xFF_FFFF {
        return invalid("colour must be 0xRRGGBB");
    }
    let name = ctx.project.pattern(p.pattern)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.update_pattern(p.pattern, |h| h.color = p.color)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": p.pattern }),
        format!("recoloured pattern {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetLength {
    /// Pattern id.
    pattern: PatternId,
    /// Length in ticks (960 per quarter note). Give this or `steps`; give neither to let the
    /// pattern follow its last note (rounded up to a bar).
    #[serde(default)]
    length: Option<Tick>,
    /// Length in steps (one step = 240 ticks).
    #[serde(default)]
    steps: Option<u32>,
}

fn set_length(ctx: &mut CommandCtx<'_>, p: SetLength) -> CmdResult<CommandOutput> {
    let length = match (p.length, p.steps) {
        (Some(_), Some(_)) => return invalid("give length or steps, not both"),
        (Some(l), None) => Some(l),
        (None, Some(s)) => Some(s as Tick * STEP),
        (None, None) => None,
    };
    if length == Some(0) {
        return invalid("length must be greater than zero");
    }
    let name = ctx.project.pattern(p.pattern)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.update_pattern(p.pattern, |h| h.length = length)?;
    let now = ed.project().pattern_length(p.pattern)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": p.pattern, "length": now, "steps": now / STEP }),
        format!(
            "{name} is now {} long",
            plural((now / STEP) as usize, "step", "steps")
        ),
    ))
}

fn clone(ctx: &mut CommandCtx<'_>, p: PatternOnly) -> CmdResult<CommandOutput> {
    let src = ctx.project.pattern(p.pattern)?.clone();
    let mut ed = Edit::new(ctx.project);
    let name = format!("{} copy", src.name);
    let id = ed.add_pattern(&name)?;
    ed.update_pattern(id, |h| {
        h.color = src.color;
        h.length = src.length;
    })?;
    let at = ctx
        .project
        .pattern_order
        .iter()
        .position(|x| *x == p.pattern)
        .map_or(usize::MAX, |i| i + 1);
    ed.move_pattern(id, at)?;
    let mut count = 0;
    for (ch, notes) in &src.notes {
        for n in notes.values() {
            let mut spec = NoteSpec::new(n.key, n.start, n.length);
            spec.velocity = n.velocity;
            spec.pan = n.pan;
            spec.fine_pitch = n.fine_pitch;
            ed.add_note(id, *ch, spec)?;
            count += 1;
        }
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": id, "notes": count }),
        format!("cloned pattern {} as {name}", src.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Move {
    /// Pattern id.
    pattern: PatternId,
    /// New position in the list, 0 = first (clamped to the end).
    index: usize,
}

fn move_pattern(ctx: &mut CommandCtx<'_>, p: Move) -> CmdResult<CommandOutput> {
    let name = ctx.project.pattern(p.pattern)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.move_pattern(p.pattern, p.index)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": p.pattern }),
        format!("moved pattern {name} to position {}", p.index),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClearChannel {
    /// Pattern id.
    pattern: PatternId,
    /// Channel whose notes to delete.
    channel: ChannelId,
}

fn clear_channel(ctx: &mut CommandCtx<'_>, p: ClearChannel) -> CmdResult<CommandOutput> {
    let cname = channel_name(ctx.project, p.channel)?;
    let pname = ctx.project.pattern(p.pattern)?.name.clone();
    let ids: Vec<NoteId> = notes_of(ctx.project, p.pattern, p.channel)?
        .iter()
        .map(|n| n.id)
        .collect();
    let mut ed = Edit::new(ctx.project);
    for id in &ids {
        ed.remove_note(p.pattern, p.channel, *id)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "removed": ids.len() }),
        format!(
            "cleared {} from {cname} in {pname}",
            plural(ids.len(), "note", "notes")
        ),
    ))
}
