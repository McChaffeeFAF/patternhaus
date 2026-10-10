//! `query.*`: read-only views of the project as JSON.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "query.project",
        "Read the whole project as JSON.",
        Scope::Read,
        READ,
        |ctx, _: NoParams| Ok(CommandOutput::value(ctx.project)),
    ));
    r.register(typed(
        "query.channels",
        "List the channels in rack order with their settings.",
        Scope::Read,
        READ,
        channels,
    ));
    r.register(typed(
        "query.patterns",
        "List the patterns in list order with length and note counts.",
        Scope::Read,
        READ,
        patterns,
    ));
    r.register(typed(
        "query.pattern_notes",
        "List the notes of a pattern, optionally for one channel.",
        Scope::Read,
        READ,
        pattern_notes,
    ));
    r.register(typed(
        "query.mixer",
        "Read the mixer inserts: levels, effects, sends and sidechains.",
        Scope::Read,
        READ,
        mixer,
    ));
    r.register(typed(
        "query.playlist",
        "Read the playlist tracks and clips.",
        Scope::Read,
        READ,
        playlist,
    ));
    r.register(typed(
        "query.object",
        "Read any one object by its path, e.g. `channel/2` or `note/0/1/17`.",
        Scope::Read,
        READ,
        object,
    ));
}

fn channels(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    let list: Vec<_> = ctx.project.channels_in_order().collect();
    Ok(CommandOutput::value(json!({ "channels": list })))
}

fn patterns(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    let list: Vec<_> = ctx
        .project
        .patterns_in_order()
        .map(|p| {
            json!({
                "id": p.id, "name": p.name, "color": p.color, "length": p.length,
                "effective_length": ctx.project.pattern_length(p.id).unwrap_or(0),
                "notes": p.note_count(),
            })
        })
        .collect();
    Ok(CommandOutput::value(json!({ "patterns": list })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PatternNotes {
    /// Pattern id.
    pattern: PatternId,
    /// Only this channel; omit for every channel.
    #[serde(default)]
    channel: Option<ChannelId>,
}

fn pattern_notes(ctx: &mut CommandCtx<'_>, p: PatternNotes) -> CmdResult<CommandOutput> {
    let pat = ctx.project.pattern(p.pattern)?;
    let mut by_channel = serde_json::Map::new();
    for (ch, notes) in &pat.notes {
        if p.channel.is_none_or(|c| c == *ch) {
            let list: Vec<_> = notes.values().collect();
            by_channel.insert(ch.to_string(), json!(list));
        }
    }
    Ok(CommandOutput::value(json!({
        "pattern": p.pattern, "length": ctx.project.pattern_length(p.pattern)?, "channels": by_channel
    })))
}

fn mixer(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    Ok(CommandOutput::value(
        json!({ "inserts": &*ctx.project.mixer }),
    ))
}

fn playlist(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    let clips: Vec<_> = ctx.project.clips.values().collect();
    Ok(CommandOutput::value(json!({
        "tracks": &*ctx.project.tracks, "clips": clips, "song_length": ctx.project.song_length()
    })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Object {
    /// Object path: settings | channel/<id> | channel_order | pattern/<id> | pattern_order |
    /// note/<pattern>/<channel>/<note> | insert/<n> | track/<n> | clip/<id>.
    path: ObjectPath,
}

fn object(ctx: &mut CommandCtx<'_>, p: Object) -> CmdResult<CommandOutput> {
    match get(ctx.project, p.path) {
        Some(v) => Ok(CommandOutput::value(json!({ "path": p.path, "value": v }))),
        None => Err(CmdError::Other(format!("nothing at {}", p.path))),
    }
}
