//! `plugin.*` (M2/T3): scan and list installed CLAP/VST3 plugins, add them as instruments or
//! mixer effects, read and set their parameters by the plugin's own ids, and open their
//! editors. Loading and running plugins is the host's job ([`Host::plugin_scan`] and friends);
//! these commands only edit the project and ask.
//!
//! A plugin added here names a state file, `plugins/<name>.state`, unique in the project:
//! `file.save` writes the running plugin's state there and loading the project reads it back
//! (DESIGN §10).

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "plugin.scan",
        "Scan plugin folders for CLAP and VST3 plugins and update the plugin list.",
        Scope::Full,
        READ,
        scan,
    ));
    r.register(typed(
        "plugin.list",
        "List scanned plugins with their format, kind and status (ok, isolate, blocked).",
        Scope::Read,
        READ,
        list,
    ));
    r.register(typed(
        "plugin.add_instrument",
        "Add a channel playing a plugin instrument.",
        Scope::Full,
        MUTATES,
        add_instrument,
    ));
    r.register(typed(
        "plugin.add_effect",
        "Add a plugin effect to a mixer insert.",
        Scope::Full,
        MUTATES,
        add_effect,
    ));
    r.register(typed(
        "plugin.set_param",
        "Set a plugin parameter by the plugin's own parameter id.",
        Scope::Full,
        MUTATES,
        set_param,
    ));
    r.register(typed(
        "plugin.get_params",
        "List a plugin's parameters (ids, ranges, current values and the plugin's own text).",
        Scope::Read,
        READ,
        get_params,
    ));
    r.register(typed(
        "plugin.open_editor",
        "Open a plugin's own editor window.",
        Scope::Full,
        READ,
        open_editor,
    ));
    r.register(typed(
        "plugin.close_editor",
        "Close a plugin's editor window.",
        Scope::Full,
        READ,
        close_editor,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Scan {
    /// Folders or plugin files to scan; empty = the standard plugin folders.
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddInstrument {
    /// `clap` or `vst3`.
    format: PluginFormat,
    /// The plugin's id from plugin.list.
    plugin: String,
    /// Channel name; defaults to the plugin's name.
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddEffect {
    /// Mixer insert, 0 = master.
    insert: u32,
    /// `clap` or `vst3`.
    format: PluginFormat,
    /// The plugin's id from plugin.list.
    plugin: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PluginRef {
    /// A channel playing a plugin instrument; or give `insert` and `slot`.
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Mixer insert of a plugin effect.
    #[serde(default)]
    insert: Option<u32>,
    /// Effect slot in that insert.
    #[serde(default)]
    slot: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParam {
    /// A channel playing a plugin instrument; or give `insert` and `slot`.
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Mixer insert of a plugin effect.
    #[serde(default)]
    insert: Option<u32>,
    /// Effect slot in that insert.
    #[serde(default)]
    slot: Option<u32>,
    /// The plugin's own parameter id (see query.node_params).
    param: String,
    /// Plain value in the parameter's range.
    value: f64,
}

/// The plugin a reference names: where it is, its format and id, and a display name.
fn resolve(
    p: &Project,
    channel: Option<ChannelId>,
    insert: Option<u32>,
    slot: Option<u32>,
) -> CmdResult<(PluginTarget, PluginFormat, String, String)> {
    match (channel, insert, slot) {
        (Some(c), None, None) => {
            let ch = p.channel(c)?;
            match &ch.instrument {
                InstrumentSpec::Plugin {
                    format, plugin_id, ..
                } => Ok((
                    PluginTarget::Channel(c),
                    *format,
                    plugin_id.clone(),
                    ch.name.clone(),
                )),
                _ => invalid(format!("channel {} does not play a plugin", ch.name)),
            }
        }
        (None, Some(i), Some(s)) => {
            let ins = p.insert(insert_id(p, i)?)?;
            let slot = ins.effects.get(s as usize).ok_or_else(|| {
                CmdError::Model(ModelError::Invalid(format!(
                    "{} has no effect slot {s}",
                    ins.name
                )))
            })?;
            match &slot.effect {
                EffectSpec::Plugin {
                    format, plugin_id, ..
                } => Ok((
                    PluginTarget::Effect { insert: i, slot: s },
                    *format,
                    plugin_id.clone(),
                    format!("{} slot {s}", ins.name),
                )),
                EffectSpec::Builtin { name, .. } => invalid(format!(
                    "{} slot {s} is the built-in {name}, not a plugin",
                    ins.name
                )),
            }
        }
        _ => invalid("give `channel`, or `insert` and `slot`"),
    }
}

fn insert_id(p: &Project, i: u32) -> CmdResult<InsertId> {
    insert(p, i)
}

/// Every `plugins/<file>` state reference the project uses.
fn state_refs(p: &Project) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for ch in p.channels.values() {
        if let InstrumentSpec::Plugin { state: Some(s), .. } = &ch.instrument {
            out.insert(s.clone());
        }
    }
    for ins in p.mixer.iter() {
        for e in &ins.effects {
            if let EffectSpec::Plugin { state: Some(s), .. } = &e.effect {
                out.insert(s.clone());
            }
        }
    }
    out
}

/// A new state reference for a plugin named `name`: `plugins/<slug>.state`, numbered when
/// taken (`-2`, `-3`, ...).
pub(crate) fn new_state_ref(p: &Project, name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "plugin" } else { slug };
    let taken = state_refs(p);
    (1..)
        .map(|n| match n {
            1 => format!("plugins/{slug}.state"),
            n => format!("plugins/{slug}-{n}.state"),
        })
        .find(|r| !taken.contains(r))
        .expect("an unused name")
}

/// The scanned plugin `format`/`id`, refusing ones that cannot be loaded.
fn scanned(host: &dyn Host, format: PluginFormat, id: &str) -> CmdResult<PluginEntry> {
    let entry = host
        .plugin_list()?
        .into_iter()
        .find(|e| e.format == format && e.id == id)
        .ok_or_else(|| {
            CmdError::Model(ModelError::Invalid(format!(
                "no scanned {format:?} plugin {id:?} (run plugin.scan, then plugin.list)"
            )))
        })?;
    if entry.status != "ok" {
        return invalid(format!(
            "{} is {} ({}) and cannot be loaded",
            entry.name, entry.status, entry.reason
        ));
    }
    Ok(entry)
}

fn scan(ctx: &mut CommandCtx<'_>, p: Scan) -> CmdResult<CommandOutput> {
    let paths: Vec<std::path::PathBuf> = p.paths.iter().map(std::path::PathBuf::from).collect();
    let r = ctx.host.plugin_scan(&paths)?;
    let summary = format!(
        "found {} ({} could not be read)",
        plural(r.plugins.len(), "plugin", "plugins"),
        plural(r.failed.len(), "file", "files")
    );
    Ok(CommandOutput {
        summary: Some(summary),
        ..CommandOutput::value(r)
    })
}

fn list(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    let plugins = ctx.host.plugin_list()?;
    Ok(CommandOutput::value(json!({ "plugins": plugins })))
}

fn add_instrument(ctx: &mut CommandCtx<'_>, p: AddInstrument) -> CmdResult<CommandOutput> {
    let e = scanned(&*ctx.host, p.format, &p.plugin)?;
    if e.kind != "instrument" {
        return invalid(format!(
            "{} is an {} plugin; add it with plugin.add_effect",
            e.name, e.kind
        ));
    }
    let name = p.name.unwrap_or_else(|| e.name.clone());
    let state = new_state_ref(ctx.project, &e.name);
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_channel(
        &name,
        InstrumentSpec::Plugin {
            format: p.format,
            plugin_id: p.plugin.clone(),
            state: Some(state.clone()),
            params: Default::default(),
        },
    )?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": id, "state": state }),
        format!("added channel {name} playing {}", e.name),
    ))
}

fn add_effect(ctx: &mut CommandCtx<'_>, p: AddEffect) -> CmdResult<CommandOutput> {
    let e = scanned(&*ctx.host, p.format, &p.plugin)?;
    if e.kind == "instrument" {
        return invalid(format!(
            "{} is an instrument; add it with plugin.add_instrument",
            e.name
        ));
    }
    let i = insert(ctx.project, p.insert)?;
    let ins = ctx.project.insert(i)?;
    if ins.effects.len() >= super::mixer::MAX_EFFECTS {
        return invalid(format!(
            "{} already has {} effects",
            ins.name,
            super::mixer::MAX_EFFECTS
        ));
    }
    let (ins_name, slot) = (ins.name.clone(), ins.effects.len());
    let state = new_state_ref(ctx.project, &e.name);
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| {
        x.effects.push(EffectSlot {
            effect: EffectSpec::Plugin {
                format: p.format,
                plugin_id: p.plugin.clone(),
                state: Some(state.clone()),
                params: Default::default(),
            },
            bypass: false,
            mix: 1.0,
        })
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "slot": slot, "state": state }),
        format!("added {} to {ins_name}", e.name),
    ))
}

fn set_param(ctx: &mut CommandCtx<'_>, p: SetParam) -> CmdResult<CommandOutput> {
    if !p.value.is_finite() {
        return invalid("value must be a finite number");
    }
    let (target, _, _, where_) = resolve(ctx.project, p.channel, p.insert, p.slot)?;
    let params = ctx.host.plugin_params(ctx.project, target)?;
    let info = params
        .params
        .iter()
        .find(|x| x.id == p.param)
        .ok_or_else(|| {
            CmdError::Model(ModelError::Invalid(format!(
                "{} has no parameter {:?} (see plugin.get_params)",
                params.name, p.param
            )))
        })?;
    let mut value = p.value.clamp(info.min, info.max);
    if info.stepped {
        value = value.round();
    }
    let (param, label) = (p.param.clone(), info.name.clone());
    let mut ed = Edit::new(ctx.project);
    match target {
        PluginTarget::Channel(c) => ed.update_channel(c, |ch| {
            if let InstrumentSpec::Plugin { params, .. } = &mut ch.instrument {
                params.insert(param.clone(), value);
            }
        })?,
        PluginTarget::Effect { insert, slot } => ed.update_insert(InsertId(insert), |ins| {
            if let Some(EffectSlot {
                effect: EffectSpec::Plugin { params, .. },
                ..
            }) = ins.effects.get_mut(slot as usize)
            {
                params.insert(param.clone(), value);
            }
        })?,
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "target": target, "param": p.param, "value": value }),
        format!("set {label} of {} ({where_}) to {value}", params.name),
    ))
}

fn get_params(ctx: &mut CommandCtx<'_>, p: PluginRef) -> CmdResult<CommandOutput> {
    let (target, ..) = resolve(ctx.project, p.channel, p.insert, p.slot)?;
    let params = ctx.host.plugin_params(ctx.project, target)?;
    Ok(CommandOutput::value(params))
}

fn editor(ctx: &mut CommandCtx<'_>, p: PluginRef, open: bool) -> CmdResult<CommandOutput> {
    let (target, ..) = resolve(ctx.project, p.channel, p.insert, p.slot)?;
    let ui = ctx.host.plugin_editor(target, open)?;
    Ok(CommandOutput::value(json!({
        "target": target,
        "open": open,
        "ui": ui,
        "note": if ui { "" } else { "no UI is attached; editors open only in the app" },
    })))
}

fn open_editor(ctx: &mut CommandCtx<'_>, p: PluginRef) -> CmdResult<CommandOutput> {
    editor(ctx, p, true)
}

fn close_editor(ctx: &mut CommandCtx<'_>, p: PluginRef) -> CmdResult<CommandOutput> {
    editor(ctx, p, false)
}
