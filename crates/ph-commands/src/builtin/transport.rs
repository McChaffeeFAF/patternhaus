//! `transport.*`: playback, through the [`Host`].

use super::util::*;
use crate::*;
use ph_core::{PatternId, Tick};
use schemars::JsonSchema;
use serde::Deserialize;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "transport.play",
        "Start playback.",
        Scope::Compose,
        READ,
        |ctx, _: NoParams| run(ctx, TransportCmd::Play),
    ));
    r.register(typed(
        "transport.stop",
        "Stop playback.",
        Scope::Compose,
        READ,
        |ctx, _: NoParams| run(ctx, TransportCmd::Stop),
    ));
    r.register(typed(
        "transport.toggle",
        "Start playback if stopped, stop it if playing (the space bar).",
        Scope::Compose,
        READ,
        |ctx, _: NoParams| run(ctx, TransportCmd::Toggle),
    ));
    r.register(typed(
        "transport.record",
        "Toggle recording.",
        Scope::Compose,
        READ,
        |ctx, _: NoParams| run(ctx, TransportCmd::Record),
    ));
    r.register(typed(
        "transport.seek",
        "Jump to a song position in ticks.",
        Scope::Compose,
        READ,
        seek,
    ));
    r.register(typed(
        "transport.set_mode",
        "Choose pattern mode (loop the selected pattern) or song mode (play the playlist).",
        Scope::Compose,
        READ,
        set_mode,
    ));
    r.register(typed(
        "transport.set_loop",
        "Set the song-mode loop region in ticks, or clear it.",
        Scope::Compose,
        READ,
        set_loop,
    ));
    r.register(typed(
        "transport.set_metronome",
        "Turn the metronome on or off.",
        Scope::Compose,
        READ,
        set_metronome,
    ));
    r.register(typed(
        "transport.get_state",
        "Read the transport: playing, recording, mode, position, loop and metronome.",
        Scope::Read,
        READ,
        |ctx, _: NoParams| Ok(CommandOutput::value(ctx.host.transport_state())),
    ));
}

fn run(ctx: &mut CommandCtx<'_>, cmd: TransportCmd) -> CmdResult<CommandOutput> {
    Ok(CommandOutput::value(ctx.host.transport(cmd)?))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Seek {
    /// Song position in ticks (960 per quarter note).
    position: Tick,
}

fn seek(ctx: &mut CommandCtx<'_>, p: Seek) -> CmdResult<CommandOutput> {
    run(
        ctx,
        TransportCmd::Seek {
            position: p.position,
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMode {
    /// `pattern` loops one pattern; `song` plays the playlist.
    mode: TransportMode,
    /// Pattern to loop in pattern mode; defaults to the first pattern in the list.
    #[serde(default)]
    pattern: Option<PatternId>,
}

fn set_mode(ctx: &mut CommandCtx<'_>, p: SetMode) -> CmdResult<CommandOutput> {
    let pattern = match p.mode {
        TransportMode::Song => None,
        TransportMode::Pattern => {
            let id =
                match p.pattern {
                    Some(id) => id,
                    None => *ctx.project.pattern_order.first().ok_or_else(|| {
                        CmdError::Other("the project has no pattern to loop".into())
                    })?,
                };
            ctx.project.pattern(id)?;
            Some(id)
        }
    };
    run(
        ctx,
        TransportCmd::SetMode {
            mode: p.mode,
            pattern,
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetLoop {
    /// `[start, end]` in ticks, or null to clear the loop.
    range: Option<(Tick, Tick)>,
}

fn set_loop(ctx: &mut CommandCtx<'_>, p: SetLoop) -> CmdResult<CommandOutput> {
    if let Some((s, e)) = p.range
        && s >= e
    {
        return invalid("the loop must end after it starts");
    }
    run(ctx, TransportCmd::SetLoop { range: p.range })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMetronome {
    /// True to turn the metronome on.
    on: bool,
}

fn set_metronome(ctx: &mut CommandCtx<'_>, p: SetMetronome) -> CmdResult<CommandOutput> {
    run(ctx, TransportCmd::SetMetronome { on: p.on })
}
