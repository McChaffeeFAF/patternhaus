//! `mixer.*`: inserts, effects, sends and sidechains. Insert 0 is the master.

use super::util::*;
use crate::session_cmd::{SessionOut, parse};
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

/// FL-style limit on effect slots per insert.
const MAX_EFFECTS: usize = 10;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "mixer.set_volume",
        "Set a mixer insert's volume (0.0-2.0, 0.8 default).",
        Scope::Mix,
        MUTATES,
        set_volume,
    ));
    r.register(typed(
        "mixer.set_pan",
        "Set a mixer insert's pan (-1.0 left to 1.0 right).",
        Scope::Mix,
        MUTATES,
        set_pan,
    ));
    r.register(typed(
        "mixer.set_mute",
        "Mute or unmute a mixer insert.",
        Scope::Mix,
        MUTATES,
        set_mute,
    ));
    r.register(typed(
        "mixer.solo",
        "Solo or unsolo a mixer insert. Solo is a view of the mix kept in the session, not saved in the project.",
        Scope::Mix,
        READ,
        crate::session_cmd::stub::<SoloInsert>,
    ));
    r.register(typed(
        "mixer.rename",
        "Rename a mixer insert.",
        Scope::Mix,
        MUTATES,
        rename,
    ));
    r.register(typed(
        "mixer.add_effect",
        "Add an effect to an insert's chain (built-in effects: eq, compressor, limiter, reverb, delay, filter, saturator).",
        Scope::Mix,
        MUTATES,
        add_effect,
    ));
    r.register(typed(
        "mixer.remove_effect",
        "Remove an effect from an insert's chain.",
        Scope::Mix,
        MUTATES,
        remove_effect,
    ));
    r.register(typed(
        "mixer.move_effect",
        "Move an effect to another position in the insert's chain.",
        Scope::Mix,
        MUTATES,
        move_effect,
    ));
    r.register(typed(
        "mixer.set_effect_param",
        "Set one parameter of a built-in effect by its stable parameter id.",
        Scope::Mix,
        MUTATES,
        set_effect_param,
    ));
    r.register(typed(
        "mixer.set_effect_bypass",
        "Bypass or re-enable an effect.",
        Scope::Mix,
        MUTATES,
        set_effect_bypass,
    ));
    r.register(typed(
        "mixer.set_effect_mix",
        "Set an effect's dry/wet mix (0.0 dry to 1.0 wet).",
        Scope::Mix,
        MUTATES,
        set_effect_mix,
    ));
    r.register(typed(
        "mixer.add_send",
        "Send one insert to another at a level.",
        Scope::Mix,
        MUTATES,
        add_send,
    ));
    r.register(typed(
        "mixer.remove_send",
        "Remove a send between two inserts.",
        Scope::Mix,
        MUTATES,
        remove_send,
    ));
    r.register(typed(
        "mixer.set_send_level",
        "Set the level of an existing send.",
        Scope::Mix,
        MUTATES,
        set_send_level,
    ));
    r.register(typed(
        "mixer.set_sidechain",
        "Feed an insert's sidechain input from another insert, or clear it.",
        Scope::Mix,
        MUTATES,
        set_sidechain,
    ));
    r.register(typed(
        "mixer.set_route_to_master",
        "Choose whether an insert's output also goes to the master.",
        Scope::Mix,
        MUTATES,
        set_route_to_master,
    ));
}

fn name_of(p: &Project, i: InsertId) -> CmdResult<String> {
    Ok(p.insert(i)?.name.clone())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVolume {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Volume 0.0..=2.0.
    volume: f32,
}

fn set_volume(ctx: &mut CommandCtx<'_>, p: SetVolume) -> CmdResult<CommandOutput> {
    range("volume", p.volume, 0.0, 2.0)?;
    let i = insert(ctx.project, p.insert)?;
    let name = name_of(ctx.project, i)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.volume = p.volume)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "volume": p.volume }),
        format!("{name} volume set to {:.0}%", p.volume * 100.0),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetPan {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Pan -1.0 (left) ..= 1.0 (right).
    pan: f32,
}

fn set_pan(ctx: &mut CommandCtx<'_>, p: SetPan) -> CmdResult<CommandOutput> {
    range("pan", p.pan, -1.0, 1.0)?;
    let i = insert(ctx.project, p.insert)?;
    let name = name_of(ctx.project, i)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.pan = p.pan)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "pan": p.pan }),
        format!("{name} pan set to {:+.2}", p.pan),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMute {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// True to mute, false to unmute.
    muted: bool,
}

fn set_mute(ctx: &mut CommandCtx<'_>, p: SetMute) -> CmdResult<CommandOutput> {
    let i = insert(ctx.project, p.insert)?;
    let name = name_of(ctx.project, i)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.muted = p.muted)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "muted": p.muted }),
        format!("{} {name}", if p.muted { "muted" } else { "unmuted" }),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoloInsert {
    /// Insert index, 1..=32.
    insert: u32,
    /// True to solo, false to unsolo; omit to toggle.
    #[serde(default)]
    solo: Option<bool>,
}

/// Session-handled: solo lives in the session, not in the project.
pub(crate) fn solo_insert(s: &mut Session, params: Value) -> CmdResult<SessionOut> {
    let p: SoloInsert = parse("mixer.solo", params)?;
    let i = insert(s.project(), p.insert)?;
    let name = name_of(s.project(), i)?;
    let on = p.solo.unwrap_or(!s.solo.inserts.contains(&p.insert));
    if on {
        s.solo.inserts.insert(p.insert);
    } else {
        s.solo.inserts.remove(&p.insert);
    }
    Ok(SessionOut {
        result: json!({ "insert": p.insert, "solo": on, "soloed": s.solo.inserts }),
        summary: Some(format!("{} {name}", if on { "soloed" } else { "unsoloed" })),
        ..Default::default()
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// New name.
    name: String,
}

fn rename(ctx: &mut CommandCtx<'_>, p: Rename) -> CmdResult<CommandOutput> {
    let i = insert(ctx.project, p.insert)?;
    let old = name_of(ctx.project, i)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.name = p.name.clone())?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert }),
        format!("renamed {old} to {}", p.name),
    ))
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddEffect {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// The effect, e.g. `{ "kind": "builtin", "name": "reverb" }`.
    effect: EffectSpec,
    /// Position in the chain; omit to append (at most 10 effects per insert).
    #[serde(default)]
    index: Option<usize>,
    /// Dry/wet mix 0.0..=1.0, default 1.0.
    #[serde(default = "one")]
    mix: f32,
}

fn effect_label(e: &EffectSpec) -> String {
    match e {
        EffectSpec::Builtin { name, .. } => name.clone(),
        EffectSpec::Plugin { plugin_id, .. } => plugin_id.clone(),
    }
}

fn add_effect(ctx: &mut CommandCtx<'_>, p: AddEffect) -> CmdResult<CommandOutput> {
    range("mix", p.mix, 0.0, 1.0)?;
    let i = insert(ctx.project, p.insert)?;
    let ins = ctx.project.insert(i)?;
    if ins.effects.len() >= MAX_EFFECTS {
        return invalid(format!("{} already has {MAX_EFFECTS} effects", ins.name));
    }
    let (name, label) = (ins.name.clone(), effect_label(&p.effect));
    let at = p.index.unwrap_or(ins.effects.len()).min(ins.effects.len());
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| {
        x.effects.insert(
            at,
            EffectSlot {
                effect: p.effect.clone(),
                bypass: false,
                mix: p.mix,
            },
        )
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "index": at }),
        format!("added {label} to {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SlotRef {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Position of the effect in the chain, 0 = first.
    index: usize,
}

fn slot(p: &Project, r: &SlotRef) -> CmdResult<(InsertId, String, EffectSlot)> {
    let i = insert(p, r.insert)?;
    let ins = p.insert(i)?;
    match ins.effects.get(r.index) {
        Some(s) => Ok((i, ins.name.clone(), s.clone())),
        None => invalid(format!("{} has no effect at index {}", ins.name, r.index)),
    }
}

fn remove_effect(ctx: &mut CommandCtx<'_>, p: SlotRef) -> CmdResult<CommandOutput> {
    let (i, name, s) = slot(ctx.project, &p)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| {
        x.effects.remove(p.index);
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert }),
        format!("removed {} from {name}", effect_label(&s.effect)),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveEffect {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Current position of the effect in the chain.
    from: usize,
    /// New position (clamped to the end of the chain).
    to: usize,
}

fn move_effect(ctx: &mut CommandCtx<'_>, p: MoveEffect) -> CmdResult<CommandOutput> {
    let (i, name, s) = slot(
        ctx.project,
        &SlotRef {
            insert: p.insert,
            index: p.from,
        },
    )?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| {
        let e = x.effects.remove(p.from);
        x.effects.insert(p.to.min(x.effects.len()), e);
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert }),
        format!("moved {} in {name}", effect_label(&s.effect)),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetEffectParam {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Position of the effect in the chain, 0 = first.
    index: usize,
    /// Stable parameter id, e.g. `threshold`.
    param: String,
    /// New value in the parameter's own units.
    value: f64,
}

fn set_effect_param(ctx: &mut CommandCtx<'_>, p: SetEffectParam) -> CmdResult<CommandOutput> {
    let (i, name, s) = slot(
        ctx.project,
        &SlotRef {
            insert: p.insert,
            index: p.index,
        },
    )?;
    if !matches!(s.effect, EffectSpec::Builtin { .. }) {
        return invalid("plugin parameters are set through the plugin's own state");
    }
    if !p.value.is_finite() {
        return invalid("value must be a finite number");
    }
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| {
        if let EffectSpec::Builtin { params, .. } = &mut x.effects[p.index].effect {
            params.insert(p.param.clone(), p.value);
        }
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "param": p.param, "value": p.value }),
        format!(
            "set {} on {} in {name} to {}",
            p.param,
            effect_label(&s.effect),
            p.value
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetEffectBypass {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Position of the effect in the chain, 0 = first.
    index: usize,
    /// True to bypass, false to enable.
    bypass: bool,
}

fn set_effect_bypass(ctx: &mut CommandCtx<'_>, p: SetEffectBypass) -> CmdResult<CommandOutput> {
    let (i, name, s) = slot(
        ctx.project,
        &SlotRef {
            insert: p.insert,
            index: p.index,
        },
    )?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.effects[p.index].bypass = p.bypass)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "bypass": p.bypass }),
        format!(
            "{} {} in {name}",
            if p.bypass { "bypassed" } else { "enabled" },
            effect_label(&s.effect)
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetEffectMix {
    /// Insert index, 0 = master, 1..=32.
    insert: u32,
    /// Position of the effect in the chain, 0 = first.
    index: usize,
    /// Dry/wet 0.0..=1.0.
    mix: f32,
}

fn set_effect_mix(ctx: &mut CommandCtx<'_>, p: SetEffectMix) -> CmdResult<CommandOutput> {
    range("mix", p.mix, 0.0, 1.0)?;
    let (i, name, s) = slot(
        ctx.project,
        &SlotRef {
            insert: p.insert,
            index: p.index,
        },
    )?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.effects[p.index].mix = p.mix)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "mix": p.mix }),
        format!(
            "{} mix in {name} set to {:.0}%",
            effect_label(&s.effect),
            p.mix * 100.0
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddSend {
    /// Source insert index.
    from: u32,
    /// Destination insert index (not the same as `from`).
    to: u32,
    /// Send level 0.0..=2.0.
    level: f32,
}

fn add_send(ctx: &mut CommandCtx<'_>, p: AddSend) -> CmdResult<CommandOutput> {
    range("level", p.level, 0.0, 2.0)?;
    let (from, to) = (insert(ctx.project, p.from)?, insert(ctx.project, p.to)?);
    if from == to {
        return invalid("an insert cannot send to itself");
    }
    let src = ctx.project.insert(from)?;
    if src.sends.iter().any(|s| s.to == to) {
        return invalid(format!("{} already sends to insert {}", src.name, p.to));
    }
    let (a, b) = (src.name.clone(), name_of(ctx.project, to)?);
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(from, |x| x.sends.push(Send { to, level: p.level }))?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "from": p.from, "to": p.to }),
        format!("sending {a} to {b} at {:.0}%", p.level * 100.0),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendRef {
    /// Source insert index.
    from: u32,
    /// Destination insert index.
    to: u32,
}

fn find_send(p: &Project, r: &SendRef) -> CmdResult<(InsertId, InsertId, String)> {
    let (from, to) = (insert(p, r.from)?, insert(p, r.to)?);
    let src = p.insert(from)?;
    if src.sends.iter().any(|s| s.to == to) {
        Ok((from, to, src.name.clone()))
    } else {
        invalid(format!("{} has no send to insert {}", src.name, r.to))
    }
}

fn remove_send(ctx: &mut CommandCtx<'_>, p: SendRef) -> CmdResult<CommandOutput> {
    let (from, to, name) = find_send(ctx.project, &p)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(from, |x| x.sends.retain(|s| s.to != to))?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "from": p.from, "to": p.to }),
        format!("removed the send from {name} to insert {}", p.to),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetSendLevel {
    /// Source insert index.
    from: u32,
    /// Destination insert index.
    to: u32,
    /// Send level 0.0..=2.0.
    level: f32,
}

fn set_send_level(ctx: &mut CommandCtx<'_>, p: SetSendLevel) -> CmdResult<CommandOutput> {
    range("level", p.level, 0.0, 2.0)?;
    let (from, to, name) = find_send(
        ctx.project,
        &SendRef {
            from: p.from,
            to: p.to,
        },
    )?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(from, |x| {
        for s in x.sends.iter_mut().filter(|s| s.to == to) {
            s.level = p.level;
        }
    })?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "from": p.from, "to": p.to, "level": p.level }),
        format!(
            "send from {name} to insert {} set to {:.0}%",
            p.to,
            p.level * 100.0
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetSidechain {
    /// Insert whose sidechain input to set.
    insert: u32,
    /// Insert that feeds the sidechain; omit or null to clear.
    #[serde(default)]
    source: Option<u32>,
}

fn set_sidechain(ctx: &mut CommandCtx<'_>, p: SetSidechain) -> CmdResult<CommandOutput> {
    let i = insert(ctx.project, p.insert)?;
    let src = p.source.map(|s| insert(ctx.project, s)).transpose()?;
    if src == Some(i) {
        return invalid("an insert cannot sidechain from itself");
    }
    let name = name_of(ctx.project, i)?;
    let mut ed = Edit::new(ctx.project);
    // A compressor only listens to the sidechain port when its `sidechain` choice says so
    // (0 = internal, 1 = external), so the route and the param change together.
    let external = if src.is_some() { 1.0 } else { 0.0 };
    ed.update_insert(i, |x| {
        x.sidechain_from = src;
        for slot in &mut x.effects {
            if let EffectSpec::Builtin { name, params } = &mut slot.effect
                && name == "compressor"
            {
                params.insert("sidechain".into(), external);
            }
        }
    })?;
    let summary = match p.source {
        Some(s) => format!("{name} sidechain now follows insert {s}"),
        None => format!("cleared the sidechain of {name}"),
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "source": p.source }),
        summary,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRoute {
    /// Insert index, 1..=32 (the master has no route).
    insert: u32,
    /// True to also send the insert's output to the master.
    route: bool,
}

fn set_route_to_master(ctx: &mut CommandCtx<'_>, p: SetRoute) -> CmdResult<CommandOutput> {
    let i = insert(ctx.project, p.insert)?;
    if i.0 == 0 {
        return invalid("the master has no route to itself");
    }
    let name = name_of(ctx.project, i)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_insert(i, |x| x.route_to_master = p.route)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "insert": p.insert, "route": p.route }),
        format!(
            "{name} {} the master",
            if p.route {
                "now goes to"
            } else {
                "no longer goes to"
            }
        ),
    ))
}
