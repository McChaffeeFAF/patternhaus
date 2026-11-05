//! `catalogue.*` (M3/T9): the plugin instrument catalogue. The owner sets an instrument up
//! once in its plugin's own editor and saves it here; from then on anyone (agents included)
//! can find it, add it to a channel, and read how to play it.
//!
//! The store is a per-user folder outside any project ([`crate::catalogue`],
//! `docs/formats/catalogue.md`). Plugin states are opaque: saved and restored, never parsed.
//! A channel added from an entry gets its own copy of the state (`plugins/<file>.state`, like
//! any plugin) and of the guide (`plugins/<file>.catalogue.toml`), so the project never
//! depends on the catalogue.

use super::plugin::new_state_ref;
use super::util::*;
use crate::catalogue::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn register(r: &mut Registry) {
    r.register(typed(
        "catalogue.save",
        "Save a plugin instrument channel (its current state) to the catalogue, with a name, tags and a playing guide.",
        Scope::Full,
        READ,
        save,
    ));
    r.register(typed(
        "catalogue.list",
        "List the catalogue's instruments, with whether each plugin is installed.",
        Scope::Read,
        READ,
        list,
    ));
    r.register(typed(
        "catalogue.search",
        "Find catalogue instruments by text (name, description, tags, plugin, articulations) and tags.",
        Scope::Read,
        READ,
        search,
    ));
    r.register(typed(
        "catalogue.get",
        "Read one catalogue instrument in full, including its playing guide.",
        Scope::Read,
        READ,
        get,
    ));
    r.register(typed(
        "catalogue.add_to_channel",
        "Add a channel playing a catalogue instrument, restored from its saved state.",
        Scope::Full,
        MUTATES,
        add_to_channel,
    ));
    r.register(typed(
        "catalogue.update_guide",
        "Replace a catalogue instrument's playing guide (and optionally its description and tags).",
        Scope::Full,
        READ,
        update_guide,
    ));
    r.register(typed(
        "catalogue.rename",
        "Rename a catalogue instrument (its id stays the same).",
        Scope::Full,
        READ,
        rename,
    ));
    r.register(typed(
        "catalogue.remove",
        "Delete a catalogue instrument and its saved state from the catalogue folder.",
        Scope::Full,
        DESTRUCTIVE,
        remove,
    ));
    r.register(typed(
        "catalogue.probe",
        "Play probe notes through a catalogue instrument offline and propose its playable range and keyswitches.",
        Scope::Full,
        READ,
        probe,
    ));
}

fn store(host: &dyn Host) -> CmdResult<Catalogue> {
    host.catalogue_dir()
        .map(Catalogue::open)
        .ok_or_else(|| CmdError::Host("this session has no plugin catalogue folder".into()))
}

/// Whether an entry's plugin can be loaded here: `status` (`ok`, `missing`, `isolate`,
/// `blocked`, or `unknown` when this host lists no plugins), the installed version, and a
/// plain warning when something is off.
fn availability(host: &dyn Host, e: &CatalogueEntry) -> (String, Option<String>, Option<String>) {
    let Ok(list) = host.plugin_list() else {
        return ("unknown".into(), None, None);
    };
    let Some(p) = list
        .into_iter()
        .find(|p| p.format == e.plugin.format && p.id == e.plugin.id)
    else {
        return (
            "missing".into(),
            None,
            Some(format!(
                "the plugin {} ({:?} {}) is not installed, or not scanned yet (run plugin.scan)",
                display_plugin(e),
                e.plugin.format,
                e.plugin.id
            )),
        );
    };
    if p.status != "ok" {
        let why = format!("{} is {} ({})", p.name, p.status, p.reason);
        return (p.status, Some(p.version), Some(why));
    }
    let warn = (!e.plugin.version.is_empty() && p.version != e.plugin.version).then(|| {
        format!(
            "saved with {} {}, installed is {}; the state is restored anyway and the plugin may convert it",
            display_plugin(e),
            e.plugin.version,
            p.version
        )
    });
    ("ok".into(), Some(p.version), warn)
}

fn display_plugin(e: &CatalogueEntry) -> &str {
    if e.plugin.name.is_empty() {
        &e.plugin.id
    } else {
        &e.plugin.name
    }
}

/// A list row: the essentials an agent needs to choose.
fn row(host: &dyn Host, id: &str, e: &CatalogueEntry) -> Value {
    let (status, installed, warning) = availability(host, e);
    let mut v = json!({
        "entry": id,
        "name": e.name,
        "tags": e.tags,
        "plugin": display_plugin(e),
        "format": e.plugin.format,
        "status": status,
    });
    if !e.description.is_empty() {
        v["description"] = json!(e.description);
    }
    if let Some([lo, hi]) = e.guide.range {
        v["range"] = json!([lo, hi]);
        v["range_text"] = json!(format!("{}-{}", note_name(lo), note_name(hi)));
    }
    if !e.guide.keyswitches.is_empty() {
        v["articulations"] = json!(
            e.guide
                .keyswitches
                .iter()
                .map(|k| k.articulation.clone())
                .collect::<Vec<_>>()
        );
    }
    if let Some(w) = warning {
        v["warning"] = json!(w);
    }
    if let Some(i) = installed
        && i != e.plugin.version
    {
        v["installed_version"] = json!(i);
    }
    v
}

fn check_guide(g: &PlayingGuide) -> CmdResult<()> {
    if let Some([lo, hi]) = g.range
        && (lo > hi || hi > 127)
    {
        return invalid(format!("range [{lo}, {hi}] must be low <= high <= 127"));
    }
    for k in &g.keyswitches {
        if k.key > 127 {
            return invalid(format!("keyswitch key {} is above 127", k.key));
        }
    }
    for c in &g.ccs {
        if c.cc > 127 {
            return invalid(format!("CC {} is above 127", c.cc));
        }
    }
    Ok(())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Save {
    /// A channel playing a plugin instrument; its running state is saved.
    channel: ChannelId,
    /// Entry name; defaults to the channel's name.
    #[serde(default)]
    name: Option<String>,
    /// Free text: what it sounds like, what it is good for.
    #[serde(default)]
    description: String,
    /// Tags: instrument family, library, vendor, character.
    #[serde(default)]
    tags: Vec<String>,
    /// How to play it: range, keyswitches, CCs, velocity notes, mono/legato. Can be filled in
    /// later with catalogue.probe and catalogue.update_guide.
    #[serde(default)]
    guide: PlayingGuide,
}

fn save(ctx: &mut CommandCtx<'_>, p: Save) -> CmdResult<CommandOutput> {
    check_guide(&p.guide)?;
    let cat = store(&*ctx.host)?;
    let ch = ctx.project.channel(p.channel)?;
    if !matches!(ch.instrument, InstrumentSpec::Plugin { .. }) {
        return invalid(format!(
            "channel {} does not play a plugin; only plugin instruments go in the catalogue",
            ch.name
        ));
    }
    let name = p.name.unwrap_or_else(|| ch.name.clone());
    if name.trim().is_empty() {
        return invalid("the name is empty");
    }
    let st = ctx
        .host
        .plugin_state(ctx.project, PluginTarget::Channel(p.channel))?;
    let entry = CatalogueEntry {
        name: name.trim().to_string(),
        description: p.description,
        tags: p.tags,
        plugin: EntryPlugin {
            format: st.format,
            id: st.plugin_id,
            name: st.name,
            vendor: st.vendor,
            version: st.version,
        },
        guide: p.guide,
    };
    let id = cat.create(&entry, &st.file)?;
    Ok(CommandOutput {
        summary: Some(format!("saved {} to the catalogue as {id}", entry.name)),
        ..CommandOutput::value(json!({
            "entry": id,
            "name": entry.name,
            "plugin": entry.plugin,
            "state_bytes": st.file.len(),
        }))
    })
}

fn list(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    let cat = store(&*ctx.host)?;
    let (ok, bad) = cat.list();
    let rows: Vec<Value> = ok.iter().map(|(id, e)| row(&*ctx.host, id, e)).collect();
    let mut out = json!({ "entries": rows, "folder": cat.dir() });
    if !bad.is_empty() {
        out["unreadable"] = json!(bad);
    }
    Ok(CommandOutput::value(out))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Search {
    /// Words that must all appear (any case) in the name, description, tags, plugin name or
    /// vendor, guide notes or articulation names. Empty = any.
    #[serde(default)]
    text: String,
    /// Tags the entry must all have (any case).
    #[serde(default)]
    tags: Vec<String>,
}

fn search(ctx: &mut CommandCtx<'_>, p: Search) -> CmdResult<CommandOutput> {
    let cat = store(&*ctx.host)?;
    let (ok, _) = cat.list();
    let rows: Vec<Value> = ok
        .iter()
        .filter(|(_, e)| e.matches_text(&p.text) && e.has_tags(&p.tags))
        .map(|(id, e)| row(&*ctx.host, id, e))
        .collect();
    Ok(CommandOutput::value(json!({ "entries": rows })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EntryRef {
    /// The catalogue entry id (from catalogue.list or catalogue.search).
    entry: String,
}

/// The full entry, with the guide in readable form.
fn full(host: &dyn Host, id: &str, e: &CatalogueEntry) -> Value {
    let mut v = row(host, id, e);
    v["plugin"] = json!(e.plugin);
    v["description"] = json!(e.description);
    v["guide"] = json!(e.guide);
    v["guide_text"] = json!(guide_text(&e.guide));
    v
}

fn get(ctx: &mut CommandCtx<'_>, p: EntryRef) -> CmdResult<CommandOutput> {
    let cat = store(&*ctx.host)?;
    let e = cat.get(&p.entry)?;
    Ok(CommandOutput::value(full(&*ctx.host, &p.entry, &e)))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddToChannel {
    /// The catalogue entry id.
    entry: String,
    /// Channel name; defaults to the entry's name.
    #[serde(default)]
    name: Option<String>,
}

fn add_to_channel(ctx: &mut CommandCtx<'_>, p: AddToChannel) -> CmdResult<CommandOutput> {
    let cat = store(&*ctx.host)?;
    let e = cat.get(&p.entry)?;
    let (status, _, warning) = availability(&*ctx.host, &e);
    match status.as_str() {
        "ok" => {}
        // a host that lists no plugins cannot play one either
        "unknown" => return Err(CmdError::Host("this host cannot load plugins".into())),
        _ => {
            return invalid(format!(
                "cannot add {}: {}",
                e.name,
                warning.unwrap_or_default()
            ));
        }
    }
    let state = cat.state(&p.entry)?;
    let name = p.name.unwrap_or_else(|| e.name.clone());
    let state_ref = new_state_ref(ctx.project, &e.name);
    let guide_ref = project_guide_ref(&state_ref);
    let copy = ProjectGuide {
        entry: p.entry.clone(),
        name: e.name.clone(),
        plugin: e.plugin.clone(),
        guide: e.guide.clone(),
    };
    let copy = toml::to_string_pretty(&copy).map_err(|x| CmdError::Host(x.to_string()))?;
    // the files first: the engine reads the state when it builds the new channel
    ctx.host.write_project_file(&state_ref, &state)?;
    ctx.host.write_project_file(&guide_ref, copy.as_bytes())?;
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_channel(
        &name,
        InstrumentSpec::Plugin {
            format: e.plugin.format,
            plugin_id: e.plugin.id.clone(),
            state: Some(state_ref.clone()),
            params: Default::default(),
        },
    )?;
    let mut out = json!({
        "channel": id,
        "state": state_ref,
        "entry": p.entry,
        "guide": e.guide,
        "guide_text": guide_text(&e.guide),
    });
    if let Some(w) = &warning {
        out["warnings"] = json!([w]);
    }
    Ok(CommandOutput::from_edit(
        ed,
        out,
        format!("added channel {name} playing {} from the catalogue", e.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateGuide {
    /// The catalogue entry id.
    entry: String,
    /// The new guide; it replaces the old one entirely (read it with catalogue.get first to
    /// keep what is there).
    guide: PlayingGuide,
    /// New description; unchanged when omitted.
    #[serde(default)]
    description: Option<String>,
    /// New tags; unchanged when omitted.
    #[serde(default)]
    tags: Option<Vec<String>>,
}

fn update_guide(ctx: &mut CommandCtx<'_>, p: UpdateGuide) -> CmdResult<CommandOutput> {
    check_guide(&p.guide)?;
    let cat = store(&*ctx.host)?;
    let mut e = cat.get(&p.entry)?;
    e.guide = p.guide;
    if let Some(d) = p.description {
        e.description = d;
    }
    if let Some(t) = p.tags {
        e.tags = t;
    }
    cat.put(&p.entry, &e)?;
    Ok(CommandOutput {
        summary: Some(format!("updated the playing guide of {}", e.name)),
        ..CommandOutput::value(full(&*ctx.host, &p.entry, &e))
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    /// The catalogue entry id.
    entry: String,
    /// The new name.
    name: String,
}

fn rename(ctx: &mut CommandCtx<'_>, p: Rename) -> CmdResult<CommandOutput> {
    let name = p.name.trim();
    if name.is_empty() {
        return invalid("the name is empty");
    }
    let cat = store(&*ctx.host)?;
    let mut e = cat.get(&p.entry)?;
    let old = std::mem::replace(&mut e.name, name.to_string());
    cat.put(&p.entry, &e)?;
    Ok(CommandOutput {
        summary: Some(format!("renamed {old} to {name}")),
        ..CommandOutput::value(json!({ "entry": p.entry, "name": name, "was": old }))
    })
}

fn remove(ctx: &mut CommandCtx<'_>, p: EntryRef) -> CmdResult<CommandOutput> {
    let cat = store(&*ctx.host)?;
    let name = cat.get(&p.entry).map(|e| e.name).unwrap_or(p.entry.clone());
    cat.remove(&p.entry)?;
    Ok(CommandOutput {
        summary: Some(format!("removed {name} from the catalogue")),
        ..CommandOutput::value(json!({ "entry": p.entry, "removed": true }))
    })
}

fn default_low() -> u8 {
    0
}
fn default_high() -> u8 {
    127
}
fn default_velocities() -> Vec<u8> {
    vec![40, 100]
}
fn default_note_ms() -> u32 {
    300
}
fn default_gap_ms() -> u32 {
    300
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Probe {
    /// The catalogue entry to probe; or give `channel`.
    #[serde(default)]
    entry: Option<String>,
    /// A channel playing a plugin instrument (its current state is probed).
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Lowest key of the chromatic sweep. Default 0.
    #[serde(default = "default_low")]
    low: u8,
    /// Highest key of the sweep. Default 127.
    #[serde(default = "default_high")]
    high: u8,
    /// MIDI velocities (1-127) each key is played at. Default [40, 100].
    #[serde(default = "default_velocities")]
    velocities: Vec<u8>,
    /// Length of each probe note, ms (50-5000). Default 300.
    #[serde(default = "default_note_ms")]
    note_ms: u32,
    /// Silence after each note for its release, ms (0-10000). Default 300; raise it for
    /// instruments with long releases.
    #[serde(default = "default_gap_ms")]
    gap_ms: u32,
    /// Candidate keyswitch keys: each is played, then `test_key`, to hear whether it changes
    /// the sound. Default none.
    #[serde(default)]
    keyswitches: Vec<u8>,
    /// The note played after each candidate keyswitch; defaults to the middle of the
    /// sounding range (or 60).
    #[serde(default)]
    test_key: Option<u8>,
    /// Include every probe note's measurement (`report.sweep`). Default false.
    #[serde(default)]
    detail: bool,
}

fn probe(ctx: &mut CommandCtx<'_>, p: Probe) -> CmdResult<CommandOutput> {
    if p.low > p.high || p.high > 127 {
        return invalid("low and high must be keys with low <= high <= 127");
    }
    if p.velocities.is_empty() || p.velocities.iter().any(|v| !(1..=127).contains(v)) {
        return invalid("velocities are 1-127, at least one");
    }
    if !(50..=5000).contains(&p.note_ms) || p.gap_ms > 10_000 {
        return invalid("note_ms is 50-5000 and gap_ms 0-10000");
    }
    if p.keyswitches
        .iter()
        .chain(p.test_key.iter())
        .any(|k| *k > 127)
    {
        return invalid("keys are 0-127");
    }
    let keys = (p.high - p.low) as usize + 1;
    let slots = keys * p.velocities.len() + p.keyswitches.len() * 2 + 1;
    let seconds = slots as f64 * (p.note_ms + p.gap_ms) as f64 / 1000.0;
    if seconds > 900.0 {
        return invalid(format!(
            "that probe would render {seconds:.0} s; narrow the keys or use fewer velocities"
        ));
    }
    let (format, plugin_id, state, label, guide) = match (&p.entry, p.channel) {
        (Some(id), None) => {
            let cat = store(&*ctx.host)?;
            let e = cat.get(id)?;
            let (status, _, warning) = availability(&*ctx.host, &e);
            if status != "ok" && status != "unknown" {
                return invalid(format!(
                    "cannot probe {}: {}",
                    e.name,
                    warning.unwrap_or_default()
                ));
            }
            (
                e.plugin.format,
                e.plugin.id.clone(),
                cat.state(id)?,
                e.name.clone(),
                Some(e.guide),
            )
        }
        (None, Some(c)) => {
            let name = channel_name(ctx.project, c)?;
            let st = ctx
                .host
                .plugin_state(ctx.project, PluginTarget::Channel(c))?;
            (st.format, st.plugin_id, st.file, name, None)
        }
        _ => return invalid("give `entry` or `channel`"),
    };
    // a test key inside the sweep's sounding range is chosen after the sweep, so the host
    // runs the keyswitch part with the given key or the guide's middle, else 60
    let test_key = p.test_key.unwrap_or_else(|| {
        guide
            .as_ref()
            .and_then(|g| g.range)
            .map_or(60, |[lo, hi]| ((lo as u16 + hi as u16) / 2) as u8)
    });
    let req = ProbeRequest {
        format,
        plugin_id,
        state,
        keys: (p.low..=p.high).collect(),
        velocities: p.velocities.iter().map(|v| *v as f32 / 127.0).collect(),
        note_seconds: p.note_ms as f64 / 1000.0,
        gap_seconds: p.gap_ms as f64 / 1000.0,
        keyswitches: p.keyswitches.clone(),
        test_key,
    };
    let mut report = ctx.host.plugin_probe(&req)?;
    let silent: Vec<u8> = req
        .keys
        .iter()
        .copied()
        .filter(|k| !report.sounding.contains(k))
        .collect();
    // a keyswitch is silent itself and changes the sound of the note after it; a silent
    // candidate that changed nothing may select the articulation already playing (listen)
    let silent_ks = |changes: bool| -> Vec<&KeyswitchProbe> {
        report
            .keyswitches
            .iter()
            .filter(|k| !k.sounds && k.after.sounds && k.changes_sound == changes)
            .collect()
    };
    let keyswitches: Vec<Value> = silent_ks(true)
        .iter()
        .enumerate()
        .map(|(i, k)| {
            json!({
                "key": k.key,
                "articulation": format!("articulation {} (name it)", i + 1),
            })
        })
        .collect();
    let unconfirmed: Vec<u8> = silent_ks(false).iter().map(|k| k.key).collect();
    let range_text = report
        .range
        .map(|[lo, hi]| format!("{}-{} ({lo}-{hi})", note_name(lo), note_name(hi)));
    let summary = match &range_text {
        Some(r) => format!("probed {label}: sounds on {r}"),
        None => format!("probed {label}: no key made a sound"),
    };
    let silent_runs = runs(&silent);
    if !p.detail {
        report.sweep.clear();
    }
    Ok(CommandOutput {
        summary: Some(summary),
        ..CommandOutput::value(json!({
            "proposal": {
                "range": report.range,
                "range_text": range_text,
                "silent_keys": silent_runs,
                "keyswitches": keyswitches,
                "silent_candidates_without_change": unconfirmed,
            },
            "next": "this is a proposal: check it (render a few notes and listen), name the articulations, then save it with catalogue.update_guide",
            "test_key": test_key,
            "report": report,
        }))
    })
}

/// `[[lo, hi], ...]` runs of consecutive keys.
fn runs(keys: &[u8]) -> Vec<[u8; 2]> {
    let mut out: Vec<[u8; 2]> = Vec::new();
    for &k in keys {
        match out.last_mut() {
            Some(r) if r[1] as u16 + 1 == k as u16 => r[1] = k,
            _ => out.push([k, k]),
        }
    }
    out
}
