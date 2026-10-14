//! `channel.*`: the channel rack.

use super::util::*;
use crate::session_cmd::{SessionOut, parse};
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn register(r: &mut Registry) {
    r.register(typed(
        "channel.add",
        "Add a channel to the channel rack.",
        Scope::Compose,
        MUTATES,
        add,
    ));
    r.register(typed(
        "channel.remove",
        "Remove a channel and all of its notes (undoable).",
        Scope::Compose,
        MUTATES,
        remove,
    ));
    r.register(typed(
        "channel.rename",
        "Rename a channel.",
        Scope::Compose,
        MUTATES,
        rename,
    ));
    r.register(typed(
        "channel.set_color",
        "Set a channel's colour (0xRRGGBB).",
        Scope::Compose,
        MUTATES,
        set_color,
    ));
    r.register(typed(
        "channel.set_volume",
        "Set a channel's volume (0.0-1.0).",
        Scope::Compose,
        MUTATES,
        set_volume,
    ));
    r.register(typed(
        "channel.set_pan",
        "Set a channel's pan (-1.0 left to 1.0 right).",
        Scope::Compose,
        MUTATES,
        set_pan,
    ));
    r.register(typed(
        "channel.set_mute",
        "Mute or unmute a channel.",
        Scope::Compose,
        MUTATES,
        set_mute,
    ));
    r.register(typed(
        "channel.solo",
        "Solo or unsolo a channel. Solo is a view of the mix kept in the session, not saved in the project.",
        Scope::Compose,
        READ,
        crate::session_cmd::stub::<SoloChannel>,
    ));
    r.register(typed(
        "channel.set_insert",
        "Route a channel to a mixer insert (0 = master).",
        Scope::Compose,
        MUTATES,
        set_insert,
    ));
    r.register(typed(
        "channel.move",
        "Move a channel to a position in the channel rack.",
        Scope::Compose,
        MUTATES,
        move_channel,
    ));
    r.register(typed(
        "channel.set_instrument",
        "Replace what makes the sound for a channel.",
        Scope::Compose,
        MUTATES,
        set_instrument,
    ));
    r.register(typed(
        "channel.set_param",
        "Set one instrument parameter of a channel by its stable parameter id.",
        Scope::Compose,
        MUTATES,
        set_param,
    ));
    r.register(typed(
        "channel.clone",
        "Duplicate a channel with its settings and notes.",
        Scope::Compose,
        MUTATES,
        clone,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Add {
    /// Display name, e.g. "Kick".
    name: String,
    /// What makes the sound.
    instrument: InstrumentSpec,
    /// Mixer insert to route to (0 = master).
    #[serde(default)]
    insert: Option<u32>,
}

fn add(ctx: &mut CommandCtx<'_>, p: Add) -> CmdResult<CommandOutput> {
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_channel(&p.name, p.instrument)?;
    if let Some(i) = p.insert {
        insert(ctx.project, i)?;
        ed.update_channel(id, |c| c.insert = InsertId(i))?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": id }),
        format!("added channel {} ({id})", p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChannelOnly {
    /// Channel id.
    channel: ChannelId,
}

fn remove(ctx: &mut CommandCtx<'_>, p: ChannelOnly) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.remove_channel(p.channel)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel }),
        format!("removed channel {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    /// Channel id.
    channel: ChannelId,
    /// New display name.
    name: String,
}

fn rename(ctx: &mut CommandCtx<'_>, p: Rename) -> CmdResult<CommandOutput> {
    let old = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.name = p.name.clone())?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel }),
        format!("renamed channel {old} to {}", p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetColor {
    /// Channel id.
    channel: ChannelId,
    /// Colour as 0xRRGGBB (a plain integer in JSON).
    color: Color,
}

fn set_color(ctx: &mut CommandCtx<'_>, p: SetColor) -> CmdResult<CommandOutput> {
    if p.color > 0xFF_FFFF {
        return invalid("colour must be 0xRRGGBB");
    }
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.color = p.color)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel }),
        format!("recoloured channel {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVolume {
    /// Channel id.
    channel: ChannelId,
    /// Volume 0.0..=1.0 (FL default 0.78).
    volume: f32,
}

fn set_volume(ctx: &mut CommandCtx<'_>, p: SetVolume) -> CmdResult<CommandOutput> {
    range("volume", p.volume, 0.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.volume = p.volume)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel, "volume": p.volume }),
        format!("{name} volume set to {:.0}%", p.volume * 100.0),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetPan {
    /// Channel id.
    channel: ChannelId,
    /// Pan -1.0 (left) ..= 1.0 (right).
    pan: f32,
}

fn set_pan(ctx: &mut CommandCtx<'_>, p: SetPan) -> CmdResult<CommandOutput> {
    range("pan", p.pan, -1.0, 1.0)?;
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.pan = p.pan)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel, "pan": p.pan }),
        format!("{name} pan set to {:+.2}", p.pan),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMute {
    /// Channel id.
    channel: ChannelId,
    /// True to mute, false to unmute.
    muted: bool,
}

fn set_mute(ctx: &mut CommandCtx<'_>, p: SetMute) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.muted = p.muted)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel, "muted": p.muted }),
        format!("{} {name}", if p.muted { "muted" } else { "unmuted" }),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoloChannel {
    /// Channel id.
    channel: ChannelId,
    /// True to solo, false to unsolo; omit to toggle.
    #[serde(default)]
    solo: Option<bool>,
}

/// Session-handled: solo lives in the session, not in the project.
pub(crate) fn solo_channel(s: &mut Session, params: Value) -> CmdResult<SessionOut> {
    let p: SoloChannel = parse("channel.solo", params)?;
    let name = channel_name(s.project(), p.channel)?;
    let on = p.solo.unwrap_or(!s.solo.channels.contains(&p.channel));
    if on {
        s.solo.channels.insert(p.channel);
    } else {
        s.solo.channels.remove(&p.channel);
    }
    Ok(SessionOut {
        result: json!({ "channel": p.channel, "solo": on, "soloed": s.solo.channels }),
        summary: Some(format!("{} {name}", if on { "soloed" } else { "unsoloed" })),
        ..Default::default()
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInsert {
    /// Channel id.
    channel: ChannelId,
    /// Mixer insert, 0 = master, 1..=32.
    insert: u32,
}

fn set_insert(ctx: &mut CommandCtx<'_>, p: SetInsert) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let target = insert(ctx.project, p.insert)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.insert = target)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel, "insert": p.insert }),
        format!("routed {name} to insert {}", p.insert),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Move {
    /// Channel id.
    channel: ChannelId,
    /// New position in the rack, 0 = top (clamped to the end).
    index: usize,
}

fn move_channel(ctx: &mut CommandCtx<'_>, p: Move) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.move_channel(p.channel, p.index)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel }),
        format!("moved channel {name} to position {}", p.index),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInstrument {
    /// Channel id.
    channel: ChannelId,
    /// The new instrument.
    instrument: InstrumentSpec,
}

fn set_instrument(ctx: &mut CommandCtx<'_>, p: SetInstrument) -> CmdResult<CommandOutput> {
    let name = channel_name(ctx.project, p.channel)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| c.instrument = p.instrument.clone())?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel }),
        format!("changed the instrument of {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParam {
    /// Channel id.
    channel: ChannelId,
    /// Stable parameter id of the instrument, e.g. `cutoff`.
    param: String,
    /// New value in the parameter's own units.
    value: f64,
}

fn set_param(ctx: &mut CommandCtx<'_>, p: SetParam) -> CmdResult<CommandOutput> {
    let ch = ctx.project.channel(p.channel)?;
    if matches!(ch.instrument, InstrumentSpec::Plugin { .. }) {
        return invalid("this channel plays a plugin: set its parameters with plugin.set_param");
    }
    if !p.value.is_finite() {
        return invalid("value must be a finite number");
    }
    let name = ch.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.update_channel(p.channel, |c| {
        if let InstrumentSpec::Sampler { params, .. }
        | InstrumentSpec::Synth { params, .. }
        | InstrumentSpec::MultiSampler { params, .. } = &mut c.instrument
        {
            params.insert(p.param.clone(), p.value);
        }
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": p.channel, "param": p.param, "value": p.value }),
        format!("set {} on {name} to {}", p.param, p.value),
    ))
}

fn clone(ctx: &mut CommandCtx<'_>, p: ChannelOnly) -> CmdResult<CommandOutput> {
    let src = ctx.project.channel(p.channel)?.clone();
    let mut ed = Edit::new(ctx.project);
    let name = format!("{} copy", src.name);
    let id = ed.add_channel(&name, src.instrument.clone())?;
    ed.update_channel(id, |c| {
        c.color = src.color;
        c.volume = src.volume;
        c.pan = src.pan;
        c.muted = src.muted;
        c.insert = src.insert;
    })?;
    let at = ctx
        .project
        .channel_order
        .iter()
        .position(|c| *c == p.channel)
        .map_or(usize::MAX, |i| i + 1);
    ed.move_channel(id, at)?;
    let mut count = 0;
    for pat in ctx.project.patterns.values() {
        for n in notes_of(ctx.project, pat.id, p.channel)? {
            let mut spec = NoteSpec::new(n.key, n.start, n.length);
            spec.velocity = n.velocity;
            spec.pan = n.pan;
            spec.fine_pitch = n.fine_pitch;
            spec.muted = n.muted;
            ed.add_note(pat.id, id, spec)?;
            count += 1;
        }
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": id, "notes": count }),
        format!(
            "cloned {} as {name} with {}",
            src.name,
            plural(count, "note", "notes")
        ),
    ))
}
