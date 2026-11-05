//! `sampler.*`: the Haus Sampler (M3). Instruments, groups, zones, keyswitches and the
//! modulation matrix are project objects (`ph_core::HausInstrument`), so every edit here is an
//! ordinary undoable patch set; see `docs/formats/haus-instrument.md`.
//!
//! Implemented in M3/T0 (model only): `new`, `set`, `group_add`, `group_set`,
//! `group_remove`, `zone_add`, `zone_move`, `zone_set`, `zone_remove`, `keyswitch_set`,
//! `modulation_set`, `report`. M3/T3: `load`, `save_native`, `collect` and `import` for SFZ
//! and sample folders (in `sampler_files.rs`); M3/T4: `import` of SF2/SF3, DecentSampler and
//! EXS24 (the `preset` parameter picks an SF2 preset); M3/T6: `import` also reads the formats
//! of importers registered through the importer hook (`Host::importers`).

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

pub fn register(r: &mut Registry) {
    r.register(typed(
        "sampler.new",
        "Create an empty Haus Sampler instrument with one group, on a new channel or replacing a channel's instrument.",
        Scope::Compose,
        MUTATES,
        new,
    ));
    r.register(typed(
        "sampler.set",
        "Rename a Haus Sampler instrument or change its global settings.",
        Scope::Compose,
        MUTATES,
        set_instrument,
    ));
    r.register(typed(
        "sampler.group_add",
        "Add a group to a Haus Sampler instrument.",
        Scope::Compose,
        MUTATES,
        group_add,
    ));
    r.register(typed(
        "sampler.group_set",
        "Change a sampler group's settings (envelopes, filter, voices, round robin, output, effects).",
        Scope::Compose,
        MUTATES,
        group_set,
    ));
    r.register(typed(
        "sampler.group_remove",
        "Remove a sampler group with its zones.",
        Scope::Compose,
        MUTATES,
        group_remove,
    ));
    r.register(typed(
        "sampler.zone_add",
        "Map samples to keys and velocities as new zones of a sampler group.",
        Scope::Compose,
        MUTATES,
        zone_add,
    ));
    r.register(typed(
        "sampler.zone_move",
        "Move zones on the key and velocity map, or into another group.",
        Scope::Compose,
        MUTATES,
        zone_move,
    ));
    r.register(typed(
        "sampler.zone_set",
        "Change zone settings: tuning, level, pan, start and end, loop, triggers, round robin, conditions.",
        Scope::Compose,
        MUTATES,
        zone_set,
    ));
    r.register(typed(
        "sampler.zone_remove",
        "Remove zones from a Haus Sampler instrument.",
        Scope::Compose,
        MUTATES,
        zone_remove,
    ));
    r.register(typed(
        "sampler.keyswitch_set",
        "Add, change or remove an articulation (keyswitch or controller switch) and choose the groups that play it.",
        Scope::Compose,
        MUTATES,
        keyswitch_set,
    ));
    r.register(typed(
        "sampler.modulation_set",
        "Replace a Haus Sampler instrument's modulation matrix.",
        Scope::Compose,
        MUTATES,
        modulation_set,
    ));
    r.register(typed(
        "sampler.report",
        "Describe a Haus Sampler instrument: where it came from, its groups and zone counts, and what its import approximated or skipped.",
        Scope::Read,
        READ,
        report,
    ));
    r.register(typed(
        "sampler.prune",
        "Remove Haus Sampler instruments that no channel plays, as one undo step.",
        Scope::Compose,
        MUTATES,
        prune,
    ));
    // ---- stubs (final ids and schemas)
    r.register(typed(
        "sampler.load",
        "Load a native Haus Sampler instrument folder (instrument.toml) onto a channel.",
        Scope::Full,
        MUTATES,
        load,
    ));
    r.register(typed(
        "sampler.import",
        "Import an SFZ, SF2/SF3, DecentSampler or EXS24 instrument (or a format an installed importer adds) onto a channel, with an import report.",
        Scope::Full,
        MUTATES,
        import,
    ));
    r.register(typed(
        "sampler.save_native",
        "Save a Haus Sampler instrument as a native instrument folder (instrument.toml and its samples).",
        Scope::Full,
        READ,
        save_native,
    ));
    r.register(typed(
        "sampler.collect",
        "Copy a linked instrument's samples into the project folder so the project no longer needs the library.",
        Scope::Full,
        MUTATES,
        collect,
    ));
}

// ------------------------------------------------------------------------- helpers

fn instrument_name(p: &Project, id: InstrumentId) -> CmdResult<String> {
    Ok(p.instrument(id)?.name.clone())
}

/// Zone fields `zone_add` and `zone_set` accept (everything but `id`).
const ZONE_FIELDS: &[&str] = &[
    "group",
    "sample",
    "keys",
    "velocity",
    "root_key",
    "tune",
    "fine",
    "volume_db",
    "pan",
    "start",
    "end",
    "loop",
    "reverse",
    "trigger",
    "one_shot",
    "release_decay_db",
    "round_robin",
    "cc_conditions",
    "crossfades",
];

/// Group fields `group_add` and `group_set` accept (everything but `id`).
const GROUP_FIELDS: &[&str] = &[
    "name",
    "color",
    "amp_env",
    "filter_env",
    "filter",
    "lfos",
    "voice_limit",
    "stealing",
    "polyphony",
    "portamento",
    "round_robin",
    "output",
    "effects",
    "articulations",
    "volume_db",
    "extra",
];

/// Instrument settings `sampler.set` accepts.
const SETTINGS_FIELDS: &[&str] = &[
    "volume_db",
    "pan",
    "transpose",
    "tune_cents",
    "voice_limit",
    "bend_up",
    "bend_down",
    "default_articulation",
    "preload_frames",
];

/// `base` (an object) with the fields of `set` merged in; a `null` clears an optional field.
/// Unknown field names are refused, so a misspelt field is an error, not a silent no-op.
fn merged<T: serde::Serialize + serde::de::DeserializeOwned>(
    what: &str,
    base: &T,
    set: &Map<String, Value>,
    allowed: &[&str],
) -> CmdResult<T> {
    if let Some(k) = set.keys().find(|k| !allowed.contains(&k.as_str())) {
        return invalid(format!(
            "{what} has no field {k:?}; known fields: {}",
            allowed.join(", ")
        ));
    }
    let mut v = serde_json::to_value(base).map_err(|e| CmdError::Other(e.to_string()))?;
    let obj = v
        .as_object_mut()
        .expect("model objects serialize as objects");
    for (k, x) in set {
        if x.is_null() {
            obj.remove(k);
        } else {
            obj.insert(k.clone(), x.clone());
        }
    }
    serde_json::from_value(v)
        .map_err(|e| CmdError::Model(ModelError::Invalid(format!("{what}: {e}"))))
}

// ------------------------------------------------------------------------- instrument

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct New {
    /// Instrument name; also the new channel's name.
    name: String,
    /// Put the instrument on this existing channel (replacing what it plays) instead of
    /// adding a channel.
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Name of the first group; defaults to "Group 1".
    #[serde(default)]
    group: Option<String>,
}

fn new(ctx: &mut CommandCtx<'_>, p: New) -> CmdResult<CommandOutput> {
    let mut ed = Edit::new(ctx.project);
    let inst = ed.add_instrument(HausInstrument::new(&p.name))?;
    let group = ed.add_group(inst, Group::new(p.group.as_deref().unwrap_or("Group 1")))?;
    let spec = InstrumentSpec::MultiSampler {
        library: None,
        instrument: Some(inst),
        params: Default::default(),
    };
    let channel = match p.channel {
        Some(c) => {
            ed.update_channel(c, |ch| ch.instrument = spec)?;
            c
        }
        None => ed.add_channel(&p.name, spec)?,
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "instrument": inst, "group": group, "channel": channel }),
        format!("made an empty sampler instrument {}", p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInstrument {
    /// Instrument id (get_state view zones).
    instrument: InstrumentId,
    /// New name.
    #[serde(default)]
    name: Option<String>,
    /// Global settings to change: volume_db, pan, transpose (semitones), tune_cents,
    /// voice_limit, bend_up, bend_down (semitones), default_articulation, preload_frames
    /// (null = engine default).
    #[serde(default)]
    set: Map<String, Value>,
}

fn set_instrument(ctx: &mut CommandCtx<'_>, p: SetInstrument) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?;
    let settings = merged("settings", &inst.settings, &p.set, SETTINGS_FIELDS)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_instrument(p.instrument, |i| {
        i.settings = settings;
        if let Some(n) = &p.name {
            i.name = n.clone();
        }
    })?;
    let name = instrument_name(ed.project(), p.instrument)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "instrument": p.instrument }),
        format!("changed the settings of {name}"),
    ))
}

// ------------------------------------------------------------------------- groups

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GroupAdd {
    /// Instrument id.
    instrument: InstrumentId,
    /// Group name.
    name: String,
    /// Settings to start from (same fields as sampler.group_set).
    #[serde(default)]
    set: Map<String, Value>,
}

fn group_add(ctx: &mut CommandCtx<'_>, p: GroupAdd) -> CmdResult<CommandOutput> {
    let name = instrument_name(ctx.project, p.instrument)?;
    let mut g = merged("group", &Group::new(&p.name), &p.set, GROUP_FIELDS)?;
    g.name = p.name.clone();
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_group(p.instrument, g)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "group": id }),
        format!("added group {} to {name}", p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GroupSet {
    /// Instrument id.
    instrument: InstrumentId,
    /// Group id.
    group: GroupId,
    /// Fields to change: name, color, amp_env and filter_env ({delay, attack, hold, decay,
    /// sustain 0..1, release}, seconds), filter ({kind, cutoff Hz, resonance dB, key_track,
    /// env_depth cents}), lfos, voice_limit, stealing (oldest|quietest|newest|none),
    /// polyphony (poly|mono|legato), portamento (s), round_robin (sequential|random), output
    /// (bus, 0 = main), effects ([{name, params, bypass, mix}]), articulations (ids), volume_db.
    /// null clears an optional field.
    set: Map<String, Value>,
}

fn group_set(ctx: &mut CommandCtx<'_>, p: GroupSet) -> CmdResult<CommandOutput> {
    let g = ctx.project.instrument(p.instrument)?.group(p.group)?;
    let new = merged("group", g, &p.set, GROUP_FIELDS)?;
    let mut ed = Edit::new(ctx.project);
    ed.update_group(p.instrument, p.group, |g| *g = new)?;
    let name = ed
        .project()
        .instrument(p.instrument)?
        .group(p.group)?
        .name
        .clone();
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "group": p.group }),
        format!("changed group {name}"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GroupRef {
    /// Instrument id.
    instrument: InstrumentId,
    /// Group id.
    group: GroupId,
}

fn group_remove(ctx: &mut CommandCtx<'_>, p: GroupRef) -> CmdResult<CommandOutput> {
    let g = ctx.project.instrument(p.instrument)?.group(p.group)?;
    let (name, zones) = (
        g.name.clone(),
        ctx.project
            .instrument(p.instrument)?
            .zones_of(p.group)
            .count(),
    );
    let mut ed = Edit::new(ctx.project);
    ed.remove_group(p.instrument, p.group)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "group": p.group, "zones_removed": zones }),
        format!(
            "removed group {name} and {}",
            plural(zones, "zone", "zones")
        ),
    ))
}

// ------------------------------------------------------------------------- zones

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZoneAdd {
    /// Instrument id.
    instrument: InstrumentId,
    /// Zones to add. Each needs group (id), sample (a registry sample id, or a path relative
    /// to the instrument folder) and root_key; optional: keys [lo, hi] and velocity [lo, hi]
    /// (inclusive, 0..=127, default all), tune (semitones), fine (cents), volume_db, pan,
    /// start, end, loop {mode forward|ping_pong, start, end, crossfade, on_release
    /// continue|exit} (frames of the file), reverse, trigger (attack|release), one_shot,
    /// release_decay_db, round_robin (position, 0-based), cc_conditions, crossfades.
    zones: Vec<Map<String, Value>>,
}

fn zone_add(ctx: &mut CommandCtx<'_>, p: ZoneAdd) -> CmdResult<CommandOutput> {
    let name = instrument_name(ctx.project, p.instrument)?;
    if p.zones.is_empty() {
        return invalid("give at least one zone");
    }
    let mut ed = Edit::new(ctx.project);
    let mut ids = Vec::new();
    for z in &p.zones {
        for need in ["group", "sample", "root_key"] {
            if !z.contains_key(need) {
                return invalid(format!("a zone needs {need}"));
            }
        }
        let mut full = z.clone();
        full.insert("id".into(), json!(0));
        if let Some(k) = full
            .keys()
            .find(|k| *k != "id" && !ZONE_FIELDS.contains(&k.as_str()))
        {
            return invalid(format!(
                "zone has no field {k:?}; known fields: {}",
                ZONE_FIELDS.join(", ")
            ));
        }
        let zone: Zone = serde_json::from_value(Value::Object(full))
            .map_err(|e| CmdError::Model(ModelError::Invalid(format!("zone: {e}"))))?;
        ids.push(ed.add_zone(p.instrument, zone)?);
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "zones": ids }),
        format!("mapped {} in {name}", plural(ids.len(), "zone", "zones")),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZoneMove {
    /// Instrument id.
    instrument: InstrumentId,
    /// Zone ids to move.
    zones: Vec<ZoneId>,
    /// Move every zone this many keys up (negative: down). Cannot be combined with `keys`.
    #[serde(default)]
    shift: Option<i32>,
    /// New key range [lo, hi] for every zone.
    #[serde(default)]
    keys: Option<KeyRange>,
    /// New velocity range [lo, hi] (0..=127) for every zone: a velocity split.
    #[serde(default)]
    velocity: Option<VelRange>,
    /// Move the zones into this group of the same instrument.
    #[serde(default)]
    group: Option<GroupId>,
    /// The root key moves with the keys, so the zone keeps its pitch mapping (default true).
    /// False keeps the root key, so the zone plays at other pitches.
    #[serde(default = "yes")]
    root_follows: bool,
}

fn yes() -> bool {
    true
}

fn zone_move(ctx: &mut CommandCtx<'_>, p: ZoneMove) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?;
    if p.shift.is_some() && p.keys.is_some() {
        return invalid("give shift or keys, not both");
    }
    if p.zones.is_empty() {
        return invalid("give at least one zone");
    }
    let name = inst.name.clone();
    let mut ed = Edit::new(ctx.project);
    for z in &p.zones {
        let zone = inst.zone(*z)?;
        let keys = match p.shift {
            Some(n) => {
                let (lo, hi) = (zone.keys.lo as i32 + n, zone.keys.hi as i32 + n);
                if lo < 0 || hi > 127 {
                    return invalid(format!("zone {z} would leave the keyboard"));
                }
                Some(KeyRange {
                    lo: lo as u8,
                    hi: hi as u8,
                })
            }
            None => p.keys,
        };
        ed.move_zone(
            p.instrument,
            *z,
            ZonePlace {
                keys,
                velocity: p.velocity,
                group: p.group,
            },
            p.root_follows,
        )?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "zones": p.zones }),
        format!("moved {} in {name}", plural(p.zones.len(), "zone", "zones")),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZoneSet {
    /// Instrument id.
    instrument: InstrumentId,
    /// Zone ids to change (the same fields on each).
    zones: Vec<ZoneId>,
    /// Fields to change (the zone fields listed in sampler.zone_add); null clears an
    /// optional field (end, loop, round_robin).
    set: Map<String, Value>,
}

fn zone_set(ctx: &mut CommandCtx<'_>, p: ZoneSet) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?;
    let name = inst.name.clone();
    let mut ed = Edit::new(ctx.project);
    for z in &p.zones {
        let new = merged("zone", inst.zone(*z)?, &p.set, ZONE_FIELDS)?;
        ed.update_zone(p.instrument, *z, |zone| *zone = new)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "zones": p.zones }),
        format!(
            "changed {} in {name}",
            plural(p.zones.len(), "zone", "zones")
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZoneRemove {
    /// Instrument id.
    instrument: InstrumentId,
    /// Zone ids to remove.
    zones: Vec<ZoneId>,
}

fn zone_remove(ctx: &mut CommandCtx<'_>, p: ZoneRemove) -> CmdResult<CommandOutput> {
    let name = instrument_name(ctx.project, p.instrument)?;
    let mut ed = Edit::new(ctx.project);
    for z in &p.zones {
        ed.remove_zone(p.instrument, *z)?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "zones": p.zones }),
        format!(
            "removed {} from {name}",
            plural(p.zones.len(), "zone", "zones")
        ),
    ))
}

// ------------------------------------------------------------------------- articulations

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct KeyswitchSet {
    /// Instrument id.
    instrument: InstrumentId,
    /// Articulation to change or remove; omit to add a new one.
    #[serde(default)]
    articulation: Option<ArticulationId>,
    /// Name (needed when adding).
    #[serde(default)]
    name: Option<String>,
    /// What selects it (needed when adding): {"kind": "key", "key": 24} for a keyswitch, or
    /// {"kind": "cc", "controller": 32, "lo": 0.5, "hi": 1.0}.
    #[serde(default)]
    trigger: Option<ArticulationTrigger>,
    /// Keyboard colour 0xRRGGBB.
    #[serde(default)]
    color: Option<Color>,
    /// The groups that play this articulation (they join it; other groups leave it).
    #[serde(default)]
    groups: Option<Vec<GroupId>>,
    /// Make it the articulation active before any keyswitch is played.
    #[serde(default)]
    default: bool,
    /// Remove the articulation (groups leave it first).
    #[serde(default)]
    remove: bool,
}

fn keyswitch_set(ctx: &mut CommandCtx<'_>, p: KeyswitchSet) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?.clone();
    let mut ed = Edit::new(ctx.project);
    let id = match p.articulation {
        Some(a) => {
            if inst.articulation(a).is_none() {
                return invalid(format!("no articulation {a} in {}", inst.name));
            }
            a
        }
        None => {
            if p.remove {
                return invalid("name the articulation to remove");
            }
            inst.next_articulation_id()
        }
    };
    // groups first leave an articulation that is being removed, or that they no longer play
    let members: Option<Vec<GroupId>> = if p.remove {
        Some(Vec::new())
    } else {
        p.groups.clone()
    };
    if let Some(m) = &members {
        for g in m {
            inst.group(*g)?;
        }
        for g in inst.groups.values() {
            if g.articulations.contains(&id) && !m.contains(&g.id) {
                ed.update_group(p.instrument, g.id, |g| g.articulations.retain(|a| *a != id))?;
            }
        }
    }
    let label;
    if p.remove {
        label = inst
            .articulation(id)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        ed.update_instrument(p.instrument, |i| {
            i.articulations.retain(|a| a.id != id);
            if i.settings.default_articulation == Some(id) {
                i.settings.default_articulation = None;
            }
        })?;
    } else {
        let existing = inst.articulation(id).cloned();
        let art = match existing {
            Some(mut a) => {
                if let Some(n) = &p.name {
                    a.name = n.clone();
                }
                if let Some(t) = &p.trigger {
                    a.trigger = t.clone();
                }
                if p.color.is_some() {
                    a.color = p.color;
                }
                a
            }
            None => Articulation {
                id,
                name: p.name.clone().ok_or_else(|| {
                    CmdError::Model(ModelError::Invalid(
                        "a new articulation needs a name".into(),
                    ))
                })?,
                trigger: p.trigger.clone().ok_or_else(|| {
                    CmdError::Model(ModelError::Invalid(
                        "a new articulation needs a trigger".into(),
                    ))
                })?,
                color: p.color,
            },
        };
        label = art.name.clone();
        ed.update_instrument(p.instrument, |i| {
            match i.articulations.iter_mut().find(|a| a.id == id) {
                Some(a) => *a = art,
                None => i.articulations.push(art),
            }
            if p.default {
                i.settings.default_articulation = Some(id);
            }
        })?;
        if let Some(m) = &members {
            for g in m {
                if !ed
                    .project()
                    .instrument(p.instrument)?
                    .group(*g)?
                    .articulations
                    .contains(&id)
                {
                    ed.update_group(p.instrument, *g, |g| g.articulations.push(id))?;
                }
            }
        }
    }
    let verb = if p.remove {
        "removed"
    } else if p.articulation.is_some() {
        "changed"
    } else {
        "added"
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "articulation": id }),
        format!("{verb} articulation {label} in {}", inst.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ModulationSet {
    /// Instrument id.
    instrument: InstrumentId,
    /// The whole matrix, in order. Each route: source ({"kind": velocity | key | cc
    /// (+controller) | pitch_bend | aftertouch | poly_pressure | lfo (+index) | envelope
    /// (+index: 0 amp, 1 filter) | random}), target (pitch cents | volume dB | pan | cutoff
    /// cents | resonance dB | sample_start frames), depth (target unit at full source),
    /// curve (linear | convex | concave | switch), group (optional: only that group).
    routes: Vec<ModRoute>,
}

fn modulation_set(ctx: &mut CommandCtx<'_>, p: ModulationSet) -> CmdResult<CommandOutput> {
    let name = instrument_name(ctx.project, p.instrument)?;
    let n = p.routes.len();
    let mut ed = Edit::new(ctx.project);
    ed.update_instrument(p.instrument, |i| i.modulation = p.routes)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "instrument": p.instrument, "routes": n }),
        format!(
            "set {} in {name}",
            plural(n, "modulation route", "modulation routes")
        ),
    ))
}

// ------------------------------------------------------------------------- prune

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Prune {
    /// Only these instruments (each must be unused); default every instrument no channel
    /// plays.
    #[serde(default)]
    instruments: Option<Vec<InstrumentId>>,
}

fn prune(ctx: &mut CommandCtx<'_>, p: Prune) -> CmdResult<CommandOutput> {
    let unused: Vec<InstrumentId> = match &p.instruments {
        Some(list) => {
            for i in list {
                let inst = ctx.project.instrument(*i)?;
                if let Some(c) = ctx.project.channels_playing(*i).next() {
                    return invalid(format!(
                        "{} is played by channel {}; remove or change that channel first",
                        inst.name, c.name
                    ));
                }
            }
            let mut l = list.clone();
            l.sort();
            l.dedup();
            l
        }
        None => ctx
            .project
            .instruments
            .keys()
            .copied()
            .filter(|i| ctx.project.channels_playing(*i).next().is_none())
            .collect(),
    };
    let names: Vec<String> = unused
        .iter()
        .map(|i| ctx.project.instruments[i].name.clone())
        .collect();
    let mut ed = Edit::new(ctx.project);
    for i in &unused {
        ed.remove_instrument(*i)?;
    }
    let summary = if unused.is_empty() {
        "no unused instruments".to_string()
    } else {
        format!(
            "removed {} ({})",
            plural(unused.len(), "unused instrument", "unused instruments"),
            names.join(", ")
        )
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "removed": unused }),
        summary,
    ))
}

// ------------------------------------------------------------------------- report

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstrumentOnly {
    /// Instrument id.
    pub instrument: InstrumentId,
}

fn report(ctx: &mut CommandCtx<'_>, p: InstrumentOnly) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?;
    let groups: Vec<Value> = inst
        .groups
        .values()
        .map(|g| {
            json!({
                "id": g.id, "name": g.name, "zones": inst.zones_of(g.id).count(),
                "articulations": g.articulations, "output": g.output,
            })
        })
        .collect();
    let channels: Vec<ChannelId> = ctx
        .project
        .channels_playing(p.instrument)
        .map(|c| c.id)
        .collect();
    Ok(CommandOutput::value(json!({
        "instrument": inst.id,
        "name": inst.name,
        "folder": inst.folder,
        "source": inst.source,
        "zones": inst.zones.len(),
        "groups": groups,
        "articulations": inst.articulations,
        "modulation_routes": inst.modulation.len(),
        "channels": channels,
        "import_notes": inst.import_notes,
        "kept_for_later": inst.extra.keys().collect::<Vec<_>>(),
    })))
}

// ------------------------------------------------------------------------- files (M3/T3)

pub(crate) use super::sampler_files::{collect, import, load, save_native};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Load {
    /// The instrument folder (or its instrument.toml) for sampler.load; for sampler.import
    /// an instrument file (.sfz, ...) or a folder of samples to map by the note names in
    /// their file names. Inside the project or an allowed library folder.
    pub path: String,
    /// Put it on this channel instead of adding one.
    #[serde(default)]
    pub channel: Option<ChannelId>,
    /// Copy the samples into the project (true) instead of playing them from the library
    /// folder (false, the default).
    #[serde(default)]
    pub collect: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Import {
    /// An instrument file (.sfz, .sf2, .sf3, .dspreset, .exs, or a format an installed
    /// importer adds) or a folder of samples to map by the note names in their file names. Inside the project or an allowed
    /// library folder.
    pub path: String,
    /// Put it on this channel instead of adding one.
    #[serde(default)]
    pub channel: Option<ChannelId>,
    /// Copy the samples into the project (true) instead of playing them from the library
    /// folder (false, the default). SF2/SF3 samples live inside the SoundFont, so those
    /// imports are always collected.
    #[serde(default)]
    pub collect: bool,
    /// SF2/SF3 only: the preset to import, as "bank:program" (e.g. "128:0" for the first
    /// drum kit) or a preset name. Default: the first preset by bank, then program. The
    /// report lists every preset in the file.
    #[serde(default)]
    pub preset: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveNative {
    /// Instrument id.
    pub instrument: InstrumentId,
    /// The folder to write (instrument.toml plus samples/); inside the project or an
    /// allowed library folder. An existing instrument.toml there is replaced.
    pub path: String,
}
