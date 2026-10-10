//! `playlist.*`: tracks and pattern clips.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "playlist.add_clip",
        "Place a pattern on a playlist track.",
        Scope::Compose,
        MUTATES,
        add_clip,
    ));
    r.register(typed(
        "playlist.remove_clip",
        "Delete a playlist clip.",
        Scope::Compose,
        MUTATES,
        remove_clip,
    ));
    r.register(typed(
        "playlist.move_clip",
        "Move a clip in time and/or to another track.",
        Scope::Compose,
        MUTATES,
        move_clip,
    ));
    r.register(typed(
        "playlist.resize_clip",
        "Change how long a clip plays, and optionally where in the pattern it begins.",
        Scope::Compose,
        MUTATES,
        resize_clip,
    ));
    r.register(typed(
        "playlist.set_track_mute",
        "Mute or unmute a playlist track.",
        Scope::Compose,
        MUTATES,
        set_track_mute,
    ));
    r.register(typed(
        "playlist.rename_track",
        "Rename a playlist track.",
        Scope::Compose,
        MUTATES,
        rename_track,
    ));
    r.register(typed(
        "playlist.fill",
        "Repeat a pattern back to back on a track, N times.",
        Scope::Compose,
        MUTATES,
        fill,
    ));
}

fn track_ok(p: &Project, track: u32) -> CmdResult<()> {
    if (track as usize) < p.tracks.len() {
        Ok(())
    } else {
        Err(ModelError::NoTrack(track as usize).into())
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddClip {
    /// Playlist track index, 0 = first track.
    track: u32,
    /// Pattern to place.
    pattern: PatternId,
    /// Start in ticks (960 per quarter note).
    start: Tick,
    /// Length in ticks; defaults to the pattern's length.
    #[serde(default)]
    length: Option<Tick>,
}

fn add_clip(ctx: &mut CommandCtx<'_>, p: AddClip) -> CmdResult<CommandOutput> {
    track_ok(ctx.project, p.track)?;
    let name = ctx.project.pattern(p.pattern)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_clip(p.track, p.pattern, p.start)?;
    if let Some(l) = p.length {
        ed.update_clip(id, |c| c.length = l)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": id }),
        format!("placed {name} on track {} at tick {}", p.track + 1, p.start),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClipOnly {
    /// Clip id.
    clip: ClipId,
}

fn remove_clip(ctx: &mut CommandCtx<'_>, p: ClipOnly) -> CmdResult<CommandOutput> {
    let pat = ctx.project.clip(p.clip)?.pattern;
    let name = ctx.project.pattern(pat)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.remove_clip(p.clip)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip }),
        format!("removed a clip of {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveClip {
    /// Clip id.
    clip: ClipId,
    /// New start in ticks; omit to keep.
    #[serde(default)]
    start: Option<Tick>,
    /// New track index; omit to keep.
    #[serde(default)]
    track: Option<u32>,
}

fn move_clip(ctx: &mut CommandCtx<'_>, p: MoveClip) -> CmdResult<CommandOutput> {
    ctx.project.clip(p.clip)?;
    if let Some(t) = p.track {
        track_ok(ctx.project, t)?;
    }
    let mut ed = Edit::new(ctx.project);
    ed.update_clip(p.clip, |c| {
        if let Some(s) = p.start {
            c.start = s;
        }
        if let Some(t) = p.track {
            c.track = t;
        }
    })?;
    let c = ed.project().clip(p.clip)?.clone();
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "track": c.track, "start": c.start }),
        format!("moved clip to track {} at tick {}", c.track + 1, c.start),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResizeClip {
    /// Clip id.
    clip: ClipId,
    /// New length in ticks; the pattern repeats if this is longer than the pattern.
    length: Tick,
    /// Where in the pattern the clip starts playing; omit to keep.
    #[serde(default)]
    offset: Option<Tick>,
}

fn resize_clip(ctx: &mut CommandCtx<'_>, p: ResizeClip) -> CmdResult<CommandOutput> {
    ctx.project.clip(p.clip)?;
    if p.length == 0 {
        return invalid("clip length must be greater than zero");
    }
    let mut ed = Edit::new(ctx.project);
    ed.update_clip(p.clip, |c| {
        c.length = p.length;
        if let Some(o) = p.offset {
            c.offset = o;
        }
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "length": p.length }),
        format!("resized clip to {} ticks", p.length),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTrackMute {
    /// Playlist track index, 0 = first track.
    track: u32,
    /// True to mute, false to unmute.
    muted: bool,
}

fn set_track_mute(ctx: &mut CommandCtx<'_>, p: SetTrackMute) -> CmdResult<CommandOutput> {
    track_ok(ctx.project, p.track)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_track(p.track, |t| t.muted = p.muted)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "track": p.track, "muted": p.muted }),
        format!(
            "{} playlist track {}",
            if p.muted { "muted" } else { "unmuted" },
            p.track + 1
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameTrack {
    /// Playlist track index, 0 = first track.
    track: u32,
    /// New name.
    name: String,
}

fn rename_track(ctx: &mut CommandCtx<'_>, p: RenameTrack) -> CmdResult<CommandOutput> {
    track_ok(ctx.project, p.track)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_track(p.track, |t| t.name = p.name.clone())?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "track": p.track }),
        format!("renamed playlist track {} to {}", p.track + 1, p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Fill {
    /// Playlist track index, 0 = first track.
    track: u32,
    /// Pattern to repeat.
    pattern: PatternId,
    /// Start of the first copy in ticks.
    start: Tick,
    /// How many copies, 1..=1024.
    count: u32,
}

fn fill(ctx: &mut CommandCtx<'_>, p: Fill) -> CmdResult<CommandOutput> {
    track_ok(ctx.project, p.track)?;
    if !(1..=1024).contains(&p.count) {
        return invalid("count must be 1..=1024");
    }
    let name = ctx.project.pattern(p.pattern)?.name.clone();
    let len = ctx.project.pattern_length(p.pattern)?;
    let mut ed = Edit::new(ctx.project);
    let ids = (0..p.count as Tick)
        .map(|i| ed.add_clip(p.track, p.pattern, p.start + i * len))
        .collect::<ModelResult<Vec<_>>>()?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clips": ids, "end": p.start + p.count as Tick * len }),
        format!("repeated {name} {} times on track {}", p.count, p.track + 1),
    ))
}
