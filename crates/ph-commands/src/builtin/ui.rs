//! `ui.*`: drive what the human sees, through the [`Host`]. Headless hosts ignore these.

use crate::*;
use ph_core::{ChannelId, ObjectPath, PatternId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

const PANELS: &str = "browser, channel_rack, piano_roll, playlist, mixer, history, agents, instrument:<channel_id>, effect:<insert>:<slot>, audio_editor:<sample_id>";

pub fn register(r: &mut Registry) {
    r.register(typed(
        "ui.open_panel",
        "Open or focus a panel, or `instrument:<channel_id>` / `effect:<insert>:<slot>` windows (see the param).",
        Scope::Compose,
        READ,
        open_panel,
    ));
    r.register(typed(
        "ui.close_panel",
        "Close a panel.",
        Scope::Compose,
        READ,
        close_panel,
    ));
    r.register(typed(
        "ui.select",
        "Select objects so the person sees what you are talking about.",
        Scope::Compose,
        READ,
        select,
    ));
    r.register(typed(
        "ui.show_piano_roll",
        "Open the piano roll on one pattern and channel.",
        Scope::Compose,
        READ,
        show_piano_roll,
    ));
    r.register(typed(
        "ui.notify",
        "Show the person a short message.",
        Scope::Compose,
        READ,
        notify,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Panel {
    /// Panel name: browser, channel_rack, piano_roll, playlist, mixer, history or agents.
    /// Also `instrument:<channel_id>` (a channel's instrument window, e.g. `instrument:2`)
    /// and `effect:<insert>:<slot>` (an effect's window, slots counted from 0, e.g. `effect:4:1`),
    /// and `audio_editor:<sample_id>` (the audio editor on a sample, see editor.open).
    panel: String,
}

fn check_panel(project: &ph_core::Project, p: &Panel) -> CmdResult<()> {
    let bad = |message: String| CmdError::BadParams {
        command: "ui".into(),
        message,
    };
    if matches!(
        p.panel.as_str(),
        "browser" | "channel_rack" | "piano_roll" | "playlist" | "mixer" | "history" | "agents"
    ) {
        return Ok(());
    }
    if let Some(id) = p.panel.strip_prefix("instrument:") {
        let c = id
            .parse::<u32>()
            .map_err(|_| bad(format!("bad channel id in {:?}", p.panel)))?;
        project
            .channel(ChannelId(c))
            .map_err(|_| bad(format!("no channel {c}")))?;
        return Ok(());
    }
    if let Some(id) = p.panel.strip_prefix("audio_editor:") {
        let s = id
            .parse::<u32>()
            .map_err(|_| bad(format!("bad sample id in {:?}", p.panel)))?;
        project
            .sample(ph_core::SampleId(s))
            .map_err(|_| bad(format!("no sample {s}")))?;
        return Ok(());
    }
    if let Some(rest) = p.panel.strip_prefix("effect:") {
        let parsed = rest
            .split_once(':')
            .and_then(|(i, s)| Some((i.parse::<u32>().ok()?, s.parse::<usize>().ok()?)));
        let Some((i, s)) = parsed else {
            return Err(bad(format!(
                "bad effect window {:?}; use effect:<insert>:<slot>",
                p.panel
            )));
        };
        let has = project
            .mixer
            .get(i as usize)
            .is_some_and(|m| s < m.effects.len());
        return if has {
            Ok(())
        } else {
            Err(bad(format!("insert {i} has no effect in slot {s}")))
        };
    }
    Err(bad(format!(
        "unknown panel {:?}; use one of {PANELS}",
        p.panel
    )))
}

fn open_panel(ctx: &mut CommandCtx<'_>, p: Panel) -> CmdResult<CommandOutput> {
    check_panel(ctx.project, &p)?;
    ctx.host.ui(UiRequest::Open {
        panel: p.panel.clone(),
    })?;
    Ok(CommandOutput::value(json!({ "panel": p.panel })))
}

fn close_panel(ctx: &mut CommandCtx<'_>, p: Panel) -> CmdResult<CommandOutput> {
    check_panel(ctx.project, &p)?;
    ctx.host.ui(UiRequest::Close {
        panel: p.panel.clone(),
    })?;
    Ok(CommandOutput::value(json!({ "panel": p.panel })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Select {
    /// Object paths to select, e.g. `channel/2`, `note/0/1/17`, `clip/3`.
    paths: Vec<ObjectPath>,
}

fn select(ctx: &mut CommandCtx<'_>, p: Select) -> CmdResult<CommandOutput> {
    for path in &p.paths {
        if ph_core::get(ctx.project, *path).is_none() {
            return Err(CmdError::Other(format!("nothing at {path}")));
        }
    }
    let n = p.paths.len();
    ctx.host.ui(UiRequest::Select { paths: p.paths })?;
    Ok(CommandOutput::value(json!({ "selected": n })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShowPianoRoll {
    /// Pattern to show.
    pattern: PatternId,
    /// Channel to edit.
    channel: ChannelId,
}

fn show_piano_roll(ctx: &mut CommandCtx<'_>, p: ShowPianoRoll) -> CmdResult<CommandOutput> {
    ctx.project.pattern(p.pattern)?;
    ctx.project.channel(p.channel)?;
    ctx.host.ui(UiRequest::PianoRoll {
        pattern: p.pattern,
        channel: p.channel,
    })?;
    Ok(CommandOutput::value(
        json!({ "pattern": p.pattern, "channel": p.channel }),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Notify {
    /// Message text.
    message: String,
}

fn notify(ctx: &mut CommandCtx<'_>, p: Notify) -> CmdResult<CommandOutput> {
    ctx.host.ui(UiRequest::Notify { message: p.message })?;
    Ok(CommandOutput::value(json!({ "ok": true })))
}
