//! `project.*`: project-wide settings.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "project.new",
        "Start a new project: an empty song with one pattern. Replaces the current song (undoable).",
        Scope::Full,
        DESTRUCTIVE,
        new,
    ));
    r.register(typed(
        "project.get_settings",
        "Read the project title, tempo, time signature and swing.",
        Scope::Read,
        READ,
        get_settings,
    ));
    r.register(typed(
        "project.set_tempo",
        "Set the project tempo in BPM.",
        Scope::Compose,
        MUTATES,
        set_tempo,
    ));
    r.register(typed(
        "project.set_time_signature",
        "Set the project time signature, e.g. 3/4.",
        Scope::Compose,
        MUTATES,
        set_time_signature,
    ));
    r.register(typed(
        "project.set_swing",
        "Set the swing amount applied to off-beat 16th notes.",
        Scope::Compose,
        MUTATES,
        set_swing,
    ));
    r.register(typed(
        "project.rename",
        "Set the project title.",
        Scope::Compose,
        MUTATES,
        rename,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct New {
    /// Project title; defaults to "Untitled".
    #[serde(default)]
    title: Option<String>,
}

fn new(ctx: &mut CommandCtx<'_>, p: New) -> CmdResult<CommandOutput> {
    let title = p.title.unwrap_or_else(|| "Untitled".into());
    // Ids are never reused, so the fresh song continues counting from the current one.
    let mut target = Project::empty(&title);
    target.next_ids = ctx.project.next_ids.clone();
    let mut t = Edit::new(&target);
    let pattern = t.add_pattern("Pattern 1")?;
    let (target, _) = t.finish();
    let mut ed = Edit::new(ctx.project);
    replace_project(&mut ed, &target)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": pattern }),
        format!("started new project {title}"),
    ))
}

fn get_settings(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    Ok(CommandOutput::value(&ctx.project.settings))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTempo {
    /// Tempo in beats per minute, 10–999.
    bpm: f64,
}

fn set_tempo(ctx: &mut CommandCtx<'_>, p: SetTempo) -> CmdResult<CommandOutput> {
    let mut ed = Edit::new(ctx.project);
    ed.update_settings(|s| s.bpm = p.bpm)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "bpm": p.bpm }),
        format!("tempo set to {} BPM", p.bpm),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTimeSignature {
    /// Beats per bar, 1–32.
    numerator: u8,
    /// Note value of one beat: 1, 2, 4, 8, 16 or 32.
    denominator: u8,
}

fn set_time_signature(ctx: &mut CommandCtx<'_>, p: SetTimeSignature) -> CmdResult<CommandOutput> {
    if !(1..=32).contains(&p.numerator) || !matches!(p.denominator, 1 | 2 | 4 | 8 | 16 | 32) {
        return invalid(format!(
            "{}/{} is not a usable time signature",
            p.numerator, p.denominator
        ));
    }
    let mut ed = Edit::new(ctx.project);
    ed.update_settings(|s| {
        s.time_signature = TimeSignature {
            numerator: p.numerator,
            denominator: p.denominator,
        }
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "numerator": p.numerator, "denominator": p.denominator }),
        format!("time signature set to {}/{}", p.numerator, p.denominator),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetSwing {
    /// Swing amount, 0.0 (straight) to 1.0.
    swing: f32,
}

fn set_swing(ctx: &mut CommandCtx<'_>, p: SetSwing) -> CmdResult<CommandOutput> {
    range("swing", p.swing, 0.0, 1.0)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_settings(|s| s.swing = p.swing)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "swing": p.swing }),
        format!("swing set to {:.0}%", p.swing * 100.0),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    /// New project title.
    title: String,
}

fn rename(ctx: &mut CommandCtx<'_>, p: Rename) -> CmdResult<CommandOutput> {
    let mut ed = Edit::new(ctx.project);
    ed.update_settings(|s| s.title = p.title.clone())?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "title": p.title }),
        format!("project renamed to {}", p.title),
    ))
}
