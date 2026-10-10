//! `note.*`: piano-roll editing. Notes are addressed by pattern, channel and note id.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "note.add",
        "Add notes for one channel to a pattern.",
        Scope::Compose,
        MUTATES,
        add,
    ));
    r.register(typed(
        "note.remove",
        "Delete notes by id.",
        Scope::Compose,
        MUTATES,
        remove,
    ));
    r.register(typed(
        "note.update",
        "Change fields of several notes at once; fields you leave out stay as they are.",
        Scope::Compose,
        MUTATES,
        update,
    ));
    r.register(typed(
        "note.move",
        "Move notes in time and pitch by a delta.",
        Scope::Compose,
        MUTATES,
        move_notes,
    ));
    r.register(typed(
        "note.resize",
        "Set notes to a length, or lengthen or shorten them by a delta.",
        Scope::Compose,
        MUTATES,
        resize,
    ));
    r.register(typed(
        "note.set_velocity",
        "Set the velocity of notes.",
        Scope::Compose,
        MUTATES,
        set_velocity,
    ));
    r.register(typed(
        "note.transpose",
        "Transpose notes by semitones (all notes of the channel when no ids are given).",
        Scope::Compose,
        MUTATES,
        transpose,
    ));
    r.register(typed(
        "note.quantize",
        "Pull note starts toward a grid, fully or by a strength.",
        Scope::Compose,
        MUTATES,
        quantize,
    ));
    r.register(typed(
        "note.copy",
        "Copy notes later in time, or into another pattern or channel.",
        Scope::Compose,
        MUTATES,
        copy,
    ));
    r.register(typed(
        "note.select_range",
        "Find the notes inside a time and pitch box and return their ids.",
        Scope::Read,
        READ,
        select_range,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Add {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Notes to add. Times are in ticks (960 per quarter note; one step = 240).
    notes: Vec<NoteSpec>,
}

fn add(ctx: &mut CommandCtx<'_>, p: Add) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    let ids = p
        .notes
        .into_iter()
        .map(|n| ed.add_note(p.pattern, p.channel, n))
        .collect::<ModelResult<Vec<_>>>()?;
    let summary = format!("added {} note(s) to {name}", ids.len());
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": ids }),
        summary,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Remove {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Ids of the notes to delete.
    ids: Vec<NoteId>,
}

fn remove(ctx: &mut CommandCtx<'_>, p: Remove) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    for id in &p.ids {
        ed.remove_note(p.pattern, p.channel, *id)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "removed": p.ids }),
        format!(
            "deleted {} from {name}",
            plural(p.ids.len(), "note", "notes")
        ),
    ))
}

/// Partial changes to one note.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoteUpdate {
    /// Id of the note to change.
    id: NoteId,
    /// New MIDI key (60 = middle C).
    #[serde(default)]
    key: Option<u8>,
    /// New start in ticks.
    #[serde(default)]
    start: Option<Tick>,
    /// New length in ticks.
    #[serde(default)]
    length: Option<Tick>,
    /// New velocity 0.0..=1.0.
    #[serde(default)]
    velocity: Option<f32>,
    /// New pan -1.0..=1.0.
    #[serde(default)]
    pan: Option<f32>,
    /// New fine pitch in cents.
    #[serde(default)]
    fine_pitch: Option<f32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Update {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// One entry per note; only the fields present are changed.
    updates: Vec<NoteUpdate>,
}

/// Rewrites notes: `f` gets each current note and returns its replacement.
fn rewrite(
    ctx: &CommandCtx<'_>,
    pattern: PatternId,
    channel: ChannelId,
    ids: Option<&[NoteId]>,
    mut f: impl FnMut(&Note) -> CmdResult<Note>,
) -> CmdResult<(Edit, Vec<NoteId>)> {
    let notes = pick_notes(ctx.project, pattern, channel, ids)?;
    let mut ed = Edit::new(ctx.project);
    let mut done = Vec::new();
    for n in notes {
        let new = f(&n)?;
        validate_note(&new)?;
        ed.update_note(pattern, channel, n.id, |x| *x = new)?;
        done.push(n.id);
    }
    Ok((ed, done))
}

fn update(ctx: &mut CommandCtx<'_>, p: Update) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let ids: Vec<NoteId> = p.updates.iter().map(|u| u.id).collect();
    let (ed, done) = rewrite(ctx, p.pattern, p.channel, Some(&ids), |n| {
        // Duplicate ids in one request apply in order; the first match is enough here.
        let u = p
            .updates
            .iter()
            .find(|u| u.id == n.id)
            .expect("picked by id");
        let mut n = n.clone();
        if let Some(k) = u.key {
            n.key = k;
        }
        if let Some(s) = u.start {
            n.start = s;
        }
        if let Some(l) = u.length {
            n.length = l;
        }
        if let Some(v) = u.velocity {
            n.velocity = v;
        }
        if let Some(x) = u.pan {
            n.pan = x;
        }
        if let Some(f) = u.fine_pitch {
            n.fine_pitch = f;
        }
        Ok(n)
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": done }),
        format!("edited {} on {name}", plural(done.len(), "note", "notes")),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Move {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Notes to move.
    ids: Vec<NoteId>,
    /// Shift in ticks; negative moves earlier.
    #[serde(default)]
    delta_ticks: i64,
    /// Shift in semitones; negative moves lower.
    #[serde(default)]
    delta_keys: i32,
}

fn move_notes(ctx: &mut CommandCtx<'_>, p: Move) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let (ed, done) = rewrite(ctx, p.pattern, p.channel, Some(&p.ids), |n| {
        let start = n.start as i64 + p.delta_ticks;
        let key = n.key as i32 + p.delta_keys;
        if start < 0 || !(0..=127).contains(&key) {
            return invalid(format!(
                "moving note {} would leave the pattern or the keyboard",
                n.id
            ));
        }
        Ok(Note {
            start: start as Tick,
            key: key as u8,
            ..n.clone()
        })
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": done }),
        format!("moved {} on {name}", plural(done.len(), "note", "notes")),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Resize {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Notes to resize.
    ids: Vec<NoteId>,
    /// Set every note to this length in ticks. Give this or `delta`.
    #[serde(default)]
    length: Option<Tick>,
    /// Add this many ticks to every note (negative shortens; a note never goes below 1 tick).
    #[serde(default)]
    delta: Option<i64>,
}

fn resize(ctx: &mut CommandCtx<'_>, p: Resize) -> CmdResult<CommandOutput> {
    if p.length.is_some() == p.delta.is_some() {
        return invalid("give exactly one of length or delta");
    }
    let name = channel_name(ctx.project, p.channel)?;
    let (ed, done) = rewrite(ctx, p.pattern, p.channel, Some(&p.ids), |n| {
        let length = match (p.length, p.delta) {
            (Some(l), _) => l,
            (_, Some(d)) => (n.length as i64 + d).max(1) as Tick,
            _ => unreachable!(),
        };
        Ok(Note {
            length,
            ..n.clone()
        })
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": done }),
        format!("resized {} on {name}", plural(done.len(), "note", "notes")),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVelocity {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Notes to change.
    ids: Vec<NoteId>,
    /// Velocity 0.0..=1.0.
    velocity: f32,
}

fn set_velocity(ctx: &mut CommandCtx<'_>, p: SetVelocity) -> CmdResult<CommandOutput> {
    range("velocity", p.velocity, 0.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    let (ed, done) = rewrite(ctx, p.pattern, p.channel, Some(&p.ids), |n| {
        Ok(Note {
            velocity: p.velocity,
            ..n.clone()
        })
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": done }),
        format!(
            "set velocity of {} on {name} to {:.0}%",
            plural(done.len(), "note", "notes"),
            p.velocity * 100.0
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Transpose {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Notes to transpose; omit for every note of the channel in the pattern.
    #[serde(default)]
    ids: Option<Vec<NoteId>>,
    /// Semitones; negative goes down.
    semitones: i32,
}

fn transpose(ctx: &mut CommandCtx<'_>, p: Transpose) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let (ed, done) = rewrite(ctx, p.pattern, p.channel, p.ids.as_deref(), |n| {
        let key = n.key as i32 + p.semitones;
        if !(0..=127).contains(&key) {
            return invalid(format!("note {} would leave the keyboard", n.id));
        }
        Ok(Note {
            key: key as u8,
            ..n.clone()
        })
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": done }),
        format!(
            "transposed {} on {name} by {:+} semitones",
            plural(done.len(), "note", "notes"),
            p.semitones
        ),
    ))
}

fn default_grid() -> Tick {
    STEP
}

fn full() -> f32 {
    1.0
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Quantize {
    /// Pattern id.
    pattern: PatternId,
    /// Channel id.
    channel: ChannelId,
    /// Notes to quantize; omit for every note of the channel in the pattern.
    #[serde(default)]
    ids: Option<Vec<NoteId>>,
    /// Grid size in ticks; default one step (240).
    #[serde(default = "default_grid")]
    grid: Tick,
    /// How far to pull toward the grid, 0.0 (not at all) to 1.0 (fully; default).
    #[serde(default = "full")]
    strength: f32,
}

fn quantize(ctx: &mut CommandCtx<'_>, p: Quantize) -> CmdResult<CommandOutput> {
    if p.grid == 0 {
        return invalid("grid must be greater than zero");
    }
    range("strength", p.strength, 0.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    let (ed, done) = rewrite(ctx, p.pattern, p.channel, p.ids.as_deref(), |n| {
        let nearest = ((n.start + p.grid / 2) / p.grid) * p.grid;
        let moved = n.start as f64 + (nearest as f64 - n.start as f64) * p.strength as f64;
        Ok(Note {
            start: moved.round().max(0.0) as Tick,
            ..n.clone()
        })
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": done }),
        format!(
            "quantized {} on {name}",
            plural(done.len(), "note", "notes")
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CopyNotes {
    /// Pattern the notes are in.
    pattern: PatternId,
    /// Channel the notes are in.
    channel: ChannelId,
    /// Notes to copy; omit for every note of the channel in the pattern.
    #[serde(default)]
    ids: Option<Vec<NoteId>>,
    /// Pattern to copy into; default the same pattern.
    #[serde(default)]
    to_pattern: Option<PatternId>,
    /// Channel to copy into; default the same channel.
    #[serde(default)]
    to_channel: Option<ChannelId>,
    /// Shift the copies by this many ticks (negative earlier).
    #[serde(default)]
    offset: i64,
}

fn copy(ctx: &mut CommandCtx<'_>, p: CopyNotes) -> CmdResult<CommandOutput> {
    let notes = pick_notes(ctx.project, p.pattern, p.channel, p.ids.as_deref())?;
    let to_pattern = p.to_pattern.unwrap_or(p.pattern);
    let to_channel = p.to_channel.unwrap_or(p.channel);
    let name = channel_name(ctx.project, to_channel)?;
    ctx.project.pattern(to_pattern)?;
    let mut ed = Edit::new(ctx.project);
    let mut ids = Vec::new();
    for n in notes {
        let start = n.start as i64 + p.offset;
        if start < 0 {
            return invalid("a copy would start before the pattern");
        }
        let mut spec = NoteSpec::new(n.key, start as Tick, n.length);
        spec.velocity = n.velocity;
        spec.pan = n.pan;
        spec.fine_pitch = n.fine_pitch;
        ids.push(ed.add_note(to_pattern, to_channel, spec)?);
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": ids }),
        format!("copied {} to {name}", plural(ids.len(), "note", "notes")),
    ))
}

fn low() -> u8 {
    0
}

fn high() -> u8 {
    127
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectRange {
    /// Pattern id.
    pattern: PatternId,
    /// Only this channel; omit to search every channel.
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Start of the time range in ticks (inclusive).
    start: Tick,
    /// End of the time range in ticks (exclusive). A note is inside when it overlaps the range.
    end: Tick,
    /// Lowest key, default 0.
    #[serde(default = "low")]
    low_key: u8,
    /// Highest key, default 127.
    #[serde(default = "high")]
    high_key: u8,
}

fn select_range(ctx: &mut CommandCtx<'_>, p: SelectRange) -> CmdResult<CommandOutput> {
    let pat = ctx.project.pattern(p.pattern)?;
    let mut found = Vec::new();
    for (ch, notes) in &pat.notes {
        if p.channel.is_some_and(|c| c != *ch) {
            continue;
        }
        for n in notes.values() {
            if n.start < p.end
                && n.start + n.length > p.start
                && (p.low_key..=p.high_key).contains(&n.key)
            {
                found.push(json!({ "channel": ch, "id": n.id, "key": n.key, "start": n.start, "length": n.length }));
            }
        }
    }
    Ok(CommandOutput::value(
        json!({ "count": found.len(), "notes": found }),
    ))
}
