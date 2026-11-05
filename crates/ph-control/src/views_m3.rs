//! Readable views of the M3 model for agents: `get_state { view: "zones" }`, the Haus Sampler
//! zone maps, with a playing guide per instrument in the catalogue's shape (M3/T9), so an agent
//! plays a Haus instrument and a catalogue plugin the same way. M3/T0 fixed the shape; M3/T7
//! completed it (note names, layers, round robins, gaps, the guide, paging) without renaming
//! any T0 key.
//!
//! Also here: what the analysis needs to know about Haus instruments ([`is_kit`],
//! [`keyswitch_keys`]): a kit's hits and an instrument's keyswitches are not pitches.

use ph_commands::catalogue::{CcUse, Keyswitch, PlayingGuide, guide_text, note_name};
use ph_core::{
    ArticulationId, ArticulationTrigger, ChannelId, CrossfadeSource, Group, HausInstrument,
    InstrumentId, InstrumentSpec, ModSource, ModTarget, Polyphony, Project, SampleRef, Zone,
    ZoneTrigger,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Zones listed per `zones` view page unless `args.limit` says otherwise.
pub const ZONES_PAGE: usize = 256;
/// The most zones one page lists.
pub const ZONES_PAGE_MAX: usize = 2000;

/// `C4` for one key, `C4-D4` for a range.
pub fn key_span(lo: u8, hi: u8) -> String {
    if lo == hi {
        note_name(lo)
    } else {
        format!("{}-{}", note_name(lo), note_name(hi))
    }
}

/// A zone's sample as a person names it: the file name, or the registry entry's name.
pub fn sample_label(p: &Project, s: &SampleRef) -> String {
    match s {
        SampleRef::File(f) => f.rsplit(['/', '\\']).next().unwrap_or(f).to_string(),
        SampleRef::Registry(id) => p
            .samples
            .get(id)
            .map_or_else(|| format!("sample {id}"), |e| e.name.clone()),
    }
}

/// `C4-D4 vel 81-127` (the velocity part only when it is not the full range).
pub fn zone_place(z: &Zone) -> String {
    let mut s = key_span(z.keys.lo, z.keys.hi);
    if (z.velocity.lo, z.velocity.hi) != (0, 127) {
        s.push_str(&format!(" vel {}-{}", z.velocity.lo, z.velocity.hi));
    }
    s
}

fn attack(i: &HausInstrument) -> impl Iterator<Item = &Zone> {
    i.zones
        .values()
        .filter(|z| z.trigger == ZoneTrigger::Attack)
}

/// The Haus instrument a channel plays, if any.
pub fn haus_of(p: &Project, ch: ChannelId) -> Option<&HausInstrument> {
    match &p.channels.get(&ch)?.instrument {
        InstrumentSpec::MultiSampler {
            instrument: Some(i),
            ..
        } => p.instruments.get(i),
        _ => None,
    }
}

/// Keys that select an articulation: they make no sound.
pub fn keyswitch_keys(i: &HausInstrument) -> BTreeSet<u8> {
    i.articulations
        .iter()
        .filter_map(|a| match a.trigger {
            ArticulationTrigger::Key { key } => Some(key),
            ArticulationTrigger::Cc { .. } => None,
        })
        .collect()
}

/// True for a drum kit: every attack zone sits on one key, and the key does not set the
/// pitch (the zone's root is another key, or key tracking is cancelled by a key-to-pitch
/// route, as SF2 `scaleTuning 0` imports), or the keys are scattered rather than a chromatic
/// run (a sampled piano mapped one key per zone covers a run of keys). Kits are left out of
/// the key and chord analysis, like one-shot samplers.
pub fn is_kit(i: &HausInstrument) -> bool {
    let zones: Vec<&Zone> = attack(i).collect();
    if zones.is_empty() || zones.iter().any(|z| z.keys.lo != z.keys.hi) {
        return false;
    }
    let untracked = i
        .modulation
        .iter()
        .any(|r| r.source == ModSource::Key && r.target == ModTarget::Pitch && r.depth <= -6000.0);
    if untracked || zones.iter().all(|z| z.root_key != z.keys.lo) {
        return true;
    }
    let keys: BTreeSet<u8> = zones.iter().map(|z| z.keys.lo).collect();
    let (lo, hi) = (keys.first().copied(), keys.last().copied());
    match (lo, hi) {
        (Some(lo), Some(hi)) => (hi - lo) as usize + 1 != keys.len(),
        _ => false,
    }
}

/// The groups that sound while `a` is the articulation: its members, and groups that belong
/// to no articulation (they play every note).
fn groups_playing(i: &HausInstrument, a: ArticulationId) -> BTreeSet<ph_core::GroupId> {
    i.groups
        .values()
        .filter(|g| g.articulations.is_empty() || g.articulations.contains(&a))
        .map(|g| g.id)
        .collect()
}

fn span_of<'a>(zones: impl Iterator<Item = &'a Zone>) -> Option<(u8, u8)> {
    zones.fold(None, |acc, z| match acc {
        None => Some((z.keys.lo, z.keys.hi)),
        Some((a, b)) => Some((a.min(z.keys.lo), b.max(z.keys.hi))),
    })
}

/// Key spans inside `lo..=hi` that none of `zones` covers, as `C5-C6`.
fn gaps<'a>(zones: impl Iterator<Item = &'a Zone>, lo: u8, hi: u8) -> Vec<String> {
    let mut covered = [false; 128];
    for z in zones {
        for k in z.keys.lo..=z.keys.hi {
            covered[k as usize] = true;
        }
    }
    let mut out = Vec::new();
    let mut k = lo as usize;
    while k <= hi as usize {
        if covered[k] {
            k += 1;
            continue;
        }
        let start = k;
        while k <= hi as usize && !covered[k] {
            k += 1;
        }
        out.push(key_span(start as u8, (k - 1) as u8));
    }
    out
}

/// The distinct velocity ranges of `zones`, sorted.
fn layers<'a>(zones: impl Iterator<Item = &'a Zone>) -> Vec<(u8, u8)> {
    let set: BTreeSet<(u8, u8)> = zones.map(|z| (z.velocity.lo, z.velocity.hi)).collect();
    set.into_iter().collect()
}

fn layer_text(l: &[(u8, u8)]) -> Vec<String> {
    l.iter().map(|(a, b)| format!("{a}-{b}")).collect()
}

/// Keys inside a group's own key span that one of its velocity layers misses (a layer that
/// stops short of the others): `vel 81-127: no zone on C5-C6`. Kits have none: their keys are
/// separate sounds.
fn group_gaps(i: &HausInstrument, g: ph_core::GroupId) -> Vec<String> {
    let att: Vec<&Zone> = attack(i).filter(|z| z.group == g).collect();
    let Some((lo, hi)) = span_of(att.iter().copied()) else {
        return Vec::new();
    };
    if is_kit(i) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (a, b) in layers(att.iter().copied()) {
        let miss = gaps(
            att.iter()
                .copied()
                .filter(|z| (z.velocity.lo, z.velocity.hi) == (a, b)),
            lo,
            hi,
        );
        if !miss.is_empty() {
            let what = if (a, b) != (0, 127) {
                format!("vel {a}-{b}: ")
            } else {
                String::new()
            };
            out.push(format!("{what}no zone on {}", miss.join(", ")));
        }
    }
    out
}

/// A group as a player knows it: its name, with the articulations it plays.
fn group_label(i: &HausInstrument, g: &Group) -> String {
    let arts: Vec<&str> = g
        .articulations
        .iter()
        .filter_map(|a| i.articulation(*a).map(|x| x.name.as_str()))
        .collect();
    if arts.is_empty() {
        g.name.clone()
    } else {
        format!("{} ({})", g.name, arts.join(", "))
    }
}

fn target_name(t: ModTarget) -> &'static str {
    match t {
        ModTarget::Pitch => "pitch",
        ModTarget::Volume => "volume",
        ModTarget::Pan => "pan",
        ModTarget::Cutoff => "filter cutoff",
        ModTarget::Resonance => "filter resonance",
        ModTarget::SampleStart => "sample start",
    }
}

fn add_use(ccs: &mut BTreeMap<u8, Vec<String>>, cc: u8, meaning: String) {
    let e = ccs.entry(cc).or_default();
    if !e.contains(&meaning) {
        e.push(meaning);
    }
}

fn stem(s: &str) -> &str {
    s.rsplit_once('.').map_or(s, |(a, _)| a)
}

/// The playing guide of a Haus instrument, derived from its zone map, in the catalogue's
/// shape: playable range, keyswitches with names, controllers that do something, velocity
/// layers, and notes (round robins, release samples, per-articulation ranges, a kit's keys).
pub fn guide(p: &Project, i: &HausInstrument) -> PlayingGuide {
    let range = span_of(attack(i));
    // velocity
    let ls = layers(attack(i));
    let xfade = i.zones.values().any(|z| {
        z.crossfades
            .iter()
            .any(|c| c.source == CrossfadeSource::Velocity)
    });
    let velocity = if ls.len() <= 1 {
        "one layer: velocity sets the level".to_string()
    } else {
        format!(
            "{} layers: {}{}",
            ls.len(),
            layer_text(&ls).join(", "),
            if xfade { " (crossfaded)" } else { "" }
        )
    };
    let sounding: Vec<&Group> = i
        .groups
        .values()
        .filter(|g| attack(i).any(|z| z.group == g.id))
        .collect();
    let mono = !sounding.is_empty() && sounding.iter().all(|g| g.polyphony != Polyphony::Poly);
    let legato = !sounding.is_empty() && sounding.iter().all(|g| g.polyphony == Polyphony::Legato);
    // keyswitches and controllers
    let mut keyswitches = Vec::new();
    let mut ccs: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    for a in &i.articulations {
        match a.trigger {
            ArticulationTrigger::Key { key } => keyswitches.push(Keyswitch {
                key,
                articulation: a.name.clone(),
            }),
            ArticulationTrigger::Cc { controller, lo, hi } => add_use(
                &mut ccs,
                controller,
                format!("selects {} at {lo:.2}-{hi:.2}", a.name),
            ),
        }
    }
    for z in i.zones.values() {
        for c in &z.cc_conditions {
            add_use(&mut ccs, c.controller, "switches zones".into());
        }
        for c in &z.crossfades {
            if let CrossfadeSource::Cc { controller } = c.source {
                add_use(&mut ccs, controller, "crossfades layers".into());
            }
        }
    }
    for r in &i.modulation {
        if let ModSource::Cc { controller } = r.source {
            add_use(
                &mut ccs,
                controller,
                format!("moves {}", target_name(r.target)),
            );
        }
    }
    // notes
    let mut notes: Vec<String> = Vec::new();
    if is_kit(i) {
        let hits: Vec<String> = attack(i)
            .map(|z| (z.keys.lo, sample_label(p, &z.sample)))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .take(24)
            .map(|(k, s)| format!("{} {}", note_name(k), stem(&s)))
            .collect();
        notes.push(format!(
            "a kit: each key plays its own sound ({})",
            hits.join(", ")
        ));
    }
    if let Some(d) = i
        .settings
        .default_articulation
        .and_then(|a| i.articulation(a))
    {
        notes.push(format!("starts as {}", d.name));
    }
    if i.articulations.len() > 1 {
        let spans: Vec<(String, Option<(u8, u8)>)> = i
            .articulations
            .iter()
            .map(|a| {
                let gs = groups_playing(i, a.id);
                (
                    a.name.clone(),
                    span_of(attack(i).filter(|z| gs.contains(&z.group))),
                )
            })
            .collect();
        if spans.iter().any(|s| s.1 != range) {
            notes.push(format!(
                "ranges per articulation: {}",
                spans
                    .iter()
                    .map(|(n, s)| match s {
                        Some((a, b)) => format!("{n} {}", key_span(*a, *b)),
                        None => format!("{n} silent"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let holes: Vec<String> = i
        .groups
        .values()
        .flat_map(|g| {
            let label = group_label(i, g);
            group_gaps(i, g.id)
                .into_iter()
                .map(move |t| format!("{label} {t}"))
        })
        .collect();
    if !holes.is_empty() {
        notes.push(format!("gaps: {}", holes.join("; ")));
    }
    let rr = i
        .zones
        .values()
        .filter_map(|z| z.round_robin)
        .max()
        .map(|m| m + 1);
    if let Some(n) = rr.filter(|n| *n > 1) {
        notes.push(format!(
            "round robins: up to {n} takes per note, so repeated notes vary"
        ));
    }
    if i.zones.values().any(|z| z.trigger == ZoneTrigger::Release) {
        notes.push("release samples sound on note-off".into());
    }
    PlayingGuide {
        range: range.map(|(a, b)| [a, b]),
        velocity,
        mono,
        legato,
        notes: notes.join("; "),
        keyswitches,
        ccs: ccs
            .into_iter()
            .map(|(cc, m)| CcUse {
                cc,
                meaning: m.join("; "),
            })
            .collect(),
    }
}

struct Page {
    offset: usize,
    limit: usize,
    zones: bool,
    /// Zones counted so far, in the view's order.
    seen: usize,
    shown: usize,
}

impl Page {
    fn take(&mut self) -> bool {
        let i = self.seen;
        self.seen += 1;
        let on = self.zones && i >= self.offset && i < self.offset + self.limit;
        if on {
            self.shown += 1;
        }
        on
    }
}

/// `{ instruments: [...], zones_total }`, or one instrument with `args.instrument`.
///
/// Each instrument: `instrument` (id), `name`, `channels` (ids playing it), `folder`, `source`,
/// `settings`, `articulations` ([{id, name, trigger, switch, groups}]), `default_articulation`,
/// `modulation_routes` (count), `import_notes` (count), `range` ([lo, hi] of attack zones) and
/// `range_text`, `zone_count`, `kit`, `guide` (the catalogue's playing-guide shape) and
/// `guide_text`, and `groups`, each with its `id`, `name`, `output`, `polyphony`,
/// `round_robin`, `articulations` (ids) and `articulation_names`, `zone_count`, `keys_text`,
/// `layers` (velocity ranges), `rr` (round-robin positions), `release_zones`, `gaps` (keys of
/// the group's own span that one of its velocity layers misses) and its `zones`,
/// compact: `{ id, keys: [lo, hi], velocity: [lo, hi], root, sample, rr?, release?, text }`
/// (`sample` is a registry id or a path relative to the folder; `rr` the round-robin position;
/// `release` true for release-trigger zones; `text` like `C4-D4 vel 81-127 c4_f.wav`).
///
/// Big libraries page: zones are counted in instrument, group, zone-id order and only
/// `args.offset` (default 0) to `offset + args.limit` (default [`ZONES_PAGE`], at most
/// [`ZONES_PAGE_MAX`]) are listed; `zones_total` counts them all and `page` (present when not
/// every zone is listed) says where the next page starts. `args.zones: false` lists no zones
/// (the summaries and guides only).
pub fn zones_view(p: &Project, args: &Value) -> Result<Value, String> {
    let only = match args.get("instrument") {
        None | Some(Value::Null) => None,
        Some(v) => Some(InstrumentId(
            v.as_u64()
                .ok_or("args.instrument is an instrument id (a number)")? as u32,
        )),
    };
    if let Some(i) = only {
        p.instrument(i).map_err(|e| e.to_string())?;
    }
    let num = |k: &str, d: usize| -> Result<usize, String> {
        match args.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v
                .as_u64()
                .map(|n| n as usize)
                .ok_or(format!("args.{k} is a whole number")),
        }
    };
    let mut page = Page {
        offset: num("offset", 0)?,
        limit: num("limit", ZONES_PAGE)?.clamp(1, ZONES_PAGE_MAX),
        zones: match args.get("zones") {
            None | Some(Value::Null) => true,
            Some(v) => v.as_bool().ok_or("args.zones is true or false")?,
        },
        seen: 0,
        shown: 0,
    };
    let list: Vec<Value> = p
        .instruments
        .values()
        .filter(|i| only.is_none_or(|o| o == i.id))
        .map(|i| instrument(p, i, &mut page))
        .collect();
    let mut v = json!({ "instruments": list, "zones_total": page.seen });
    if page.shown < page.seen {
        let end = page.offset + page.shown;
        v["page"] = json!({
            "offset": page.offset,
            "limit": page.limit,
            "shown": page.shown,
            "next_offset": if page.zones && end < page.seen { json!(end) } else { Value::Null },
        });
    }
    Ok(v)
}

fn instrument(p: &Project, i: &HausInstrument, page: &mut Page) -> Value {
    let range = span_of(attack(i));
    let art_name = |a: &ArticulationId| i.articulation(*a).map(|x| x.name.clone());
    let groups: Vec<Value> = i
        .groups
        .values()
        .map(|g| {
            let all: Vec<&Zone> = i.zones_of(g.id).collect();
            let att: Vec<&Zone> = all
                .iter()
                .copied()
                .filter(|z| z.trigger == ZoneTrigger::Attack)
                .collect();
            let mut zones: Vec<Value> = Vec::new();
            for z in &all {
                if !page.take() {
                    continue;
                }
                let mut v = json!({
                    "id": z.id,
                    "keys": z.keys,
                    "velocity": z.velocity,
                    "root": z.root_key,
                    "sample": match &z.sample {
                        SampleRef::Registry(s) => json!(s),
                        SampleRef::File(f) => json!(f),
                    },
                    "text": format!("{} {}", zone_place(z), sample_label(p, &z.sample)),
                });
                if let Some(rr) = z.round_robin {
                    v["rr"] = json!(rr);
                }
                if z.trigger == ZoneTrigger::Release {
                    v["release"] = json!(true);
                }
                zones.push(v);
            }
            let ls = layers(att.iter().copied());
            let gap_text = group_gaps(i, g.id);
            let rr: BTreeSet<u32> = all.iter().filter_map(|z| z.round_robin).collect();
            let mut v = json!({
                "id": g.id,
                "name": g.name,
                "output": g.output,
                "polyphony": g.polyphony,
                "round_robin": g.round_robin,
                "articulations": g.articulations,
                "articulation_names": g.articulations.iter().filter_map(art_name).collect::<Vec<_>>(),
                "zone_count": all.len(),
                "keys_text": span_of(att.iter().copied()).map(|(a, b)| key_span(a, b)),
                "layers": layer_text(&ls),
                "rr": rr.len(),
                "release_zones": all.len() - att.len(),
                "gaps": gap_text,
            });
            if page.zones {
                v["zones"] = json!(zones);
            }
            v
        })
        .collect();
    let articulations: Vec<Value> = i
        .articulations
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "name": a.name,
                "trigger": a.trigger,
                "switch": match a.trigger {
                    ArticulationTrigger::Key { key } => format!("key {} ({key})", note_name(key)),
                    ArticulationTrigger::Cc { controller, lo, hi } =>
                        format!("CC{controller} {lo:.2}-{hi:.2}"),
                },
                "groups": i
                    .groups
                    .values()
                    .filter(|g| g.articulations.contains(&a.id))
                    .map(|g| g.id)
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let g = guide(p, i);
    json!({
        "instrument": i.id,
        "name": i.name,
        "channels": p.channels_playing(i.id).map(|c| c.id).collect::<Vec<_>>(),
        "folder": i.folder,
        "source": i.source,
        "settings": i.settings,
        "articulations": articulations,
        "default_articulation": i.settings.default_articulation,
        "modulation_routes": i.modulation.len(),
        "import_notes": i.import_notes.len(),
        "range": range.map(|(a, b)| [a, b]),
        "range_text": range.map(|(a, b)| key_span(a, b)),
        "zone_count": i.zones.len(),
        "kit": is_kit(i),
        "guide_text": guide_text(&g),
        "guide": g,
        "groups": groups,
    })
}

/// The colours new objects get in the default theme (Neon Slate's pattern palette, less the
/// hue next to the agent colour: `ph_ui::Theme::object_colors`). The sampler window colours a
/// group without its own colour by its place in this list, and so does the zone-map snapshot.
pub const OBJECT_COLOURS: [u32; 4] = [0x38c6ff, 0x2dd4bf, 0x94a3b8, 0xf472b6];

/// A group's colour on the zone map: its own, else the default theme's for its place.
pub fn group_colour(i: &HausInstrument, g: ph_core::GroupId) -> u32 {
    match i.groups.get(&g) {
        Some(gr) if gr.color != 0 => gr.color,
        _ => {
            let n = i.groups.keys().position(|k| *k == g).unwrap_or(0);
            OBJECT_COLOURS[n % OBJECT_COLOURS.len()]
        }
    }
}

/// An articulation's keyswitch colour: its own, else the default theme's, two places on.
pub fn articulation_colour(i: &HausInstrument, a: ArticulationId) -> u32 {
    let n = i.articulations.iter().position(|x| x.id == a).unwrap_or(0);
    match i.articulations.get(n).and_then(|x| x.color) {
        Some(c) => c,
        None => OBJECT_COLOURS[(n + 2) % OBJECT_COLOURS.len()],
    }
}

/// The `snapshot { kind: zone_map }` image of one instrument, and its caption.
pub fn zone_map_png(p: &Project, i: &HausInstrument, width: u32) -> (Vec<u8>, String) {
    use ph_analysis::image::{ZoneBox, ZoneMapSpec, ZonePanel, zone_map};
    type Stack = (ph_core::GroupId, u8, u8, u8, u8, bool);
    let key = |z: &Zone| -> Stack {
        (
            z.group,
            z.keys.lo,
            z.keys.hi,
            z.velocity.lo,
            z.velocity.hi,
            z.trigger == ZoneTrigger::Release,
        )
    };
    let mut stacks: BTreeMap<Stack, Vec<&Zone>> = BTreeMap::new();
    for z in i.zones.values().filter(|z| z.round_robin.is_some()) {
        stacks.entry(key(z)).or_default().push(z);
    }
    for s in stacks.values_mut() {
        s.sort_by_key(|z| (z.round_robin, z.id));
    }
    let boxed = |z: &Zone| {
        let label = stem(&sample_label(p, &z.sample)).to_string();
        let (band, label) = match z.round_robin {
            Some(pos) => {
                let s = &stacks[&key(z)];
                let at = s.iter().position(|o| o.id == z.id).unwrap_or(0);
                (
                    (at as u32, s.len() as u32),
                    format!("RR {} {label}", pos + 1),
                )
            }
            None => ((0, 1), label),
        };
        ZoneBox {
            keys: (z.keys.lo, z.keys.hi),
            velocity: (z.velocity.lo, z.velocity.hi),
            color: group_colour(i, z.group),
            release: z.trigger == ZoneTrigger::Release,
            band,
            label,
        }
    };
    // one panel per articulation (the zones that sound under it), so they never hide each
    // other; groups in no articulation play in every panel
    let panels: Vec<ZonePanel> = if i.articulations.is_empty() {
        vec![ZonePanel {
            title: "all zones".into(),
            zones: i.zones.values().map(boxed).collect(),
        }]
    } else {
        i.articulations
            .iter()
            .map(|a| {
                let gs = groups_playing(i, a.id);
                ZonePanel {
                    title: format!(
                        "{} ({})",
                        a.name,
                        match a.trigger {
                            ArticulationTrigger::Key { key } => format!("key {}", note_name(key)),
                            ArticulationTrigger::Cc { controller, lo, hi } =>
                                format!("CC{controller} {lo:.2}-{hi:.2}"),
                        }
                    ),
                    zones: i
                        .zones
                        .values()
                        .filter(|z| gs.contains(&z.group))
                        .map(boxed)
                        .collect(),
                }
            })
            .collect()
    };
    let legend: Vec<(String, u32)> = i
        .groups
        .values()
        .map(|g| (group_label(i, g), group_colour(i, g.id)))
        .collect();
    let keyswitches: Vec<(u8, String, u32)> = i
        .articulations
        .iter()
        .filter_map(|a| match a.trigger {
            ArticulationTrigger::Key { key } => {
                Some((key, a.name.clone(), articulation_colour(i, a.id)))
            }
            ArticulationTrigger::Cc { .. } => None,
        })
        .collect();
    let range = span_of(i.zones.values());
    let png = zone_map(&ZoneMapSpec {
        title: format!(
            "ZONE MAP  {}  {} ZONES  {}",
            i.name.to_uppercase(),
            i.zones.len(),
            range.map_or(String::new(), |(a, b)| key_span(a, b))
        ),
        panels: &panels,
        legend: &legend,
        keyswitches: &keyswitches,
        width,
    });
    let mut caption = format!(
        "zone map of {}: {} in {}",
        i.name,
        if i.zones.len() == 1 {
            "1 zone".to_string()
        } else {
            format!("{} zones", i.zones.len())
        },
        if i.groups.len() == 1 {
            "1 group".to_string()
        } else {
            format!("{} groups", i.groups.len())
        }
    );
    if let Some((a, b)) = range {
        caption.push_str(&format!(" over {}", key_span(a, b)));
    }
    caption.push_str("; keys across, velocity up; colours per group: ");
    caption.push_str(
        &legend
            .iter()
            .map(|l| l.0.clone())
            .collect::<Vec<_>>()
            .join(", "),
    );
    if !keyswitches.is_empty() {
        caption.push_str("; keyswitches ");
        caption.push_str(
            &keyswitches
                .iter()
                .map(|(k, n, _)| format!("{} {n}", note_name(*k)))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !i.articulations.is_empty() {
        caption.push_str("; one panel per articulation");
    }
    caption.push_str("; stacked bands are round robins, outlines release zones");
    (png, caption)
}

/// Adds the playing guide to every plugin instrument of a `plugins` view that was added from
/// the plugin catalogue (M3/T9): the project keeps a copy beside the state,
/// `plugins/<file>.catalogue.toml`, read here from `project_dir`.
pub fn attach_guides(view: &mut Value, p: &Project, project_dir: &std::path::Path) {
    let Some(rows) = view["plugins"].as_array_mut() else {
        return;
    };
    for row in rows {
        let Some(c) = row["channel"].as_u64() else {
            continue;
        };
        let Ok(ch) = p.channel(ph_core::ChannelId(c as u32)) else {
            continue;
        };
        let ph_core::InstrumentSpec::Plugin { state: Some(s), .. } = &ch.instrument else {
            continue;
        };
        let rel = ph_commands::catalogue::project_guide_ref(s);
        let Ok(text) = std::fs::read_to_string(project_dir.join(&rel)) else {
            continue;
        };
        match ph_commands::catalogue::ProjectGuide::from_toml(&text) {
            Ok(g) => {
                row["catalogue_entry"] = json!(g.entry);
                row["guide_text"] = json!(ph_commands::catalogue::guide_text(&g.guide));
                row["guide"] = json!(g.guide);
            }
            Err(e) => row["guide_error"] = json!(format!("{rel}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::*;

    #[test]
    fn zones_view_has_the_t0_shape() {
        let mut e = Edit::new(&Project::empty("z"));
        let i = e.add_instrument(HausInstrument::new("Piano")).unwrap();
        let g = e.add_group(i, Group::new("Main")).unwrap();
        let mut z = Zone::new(
            g,
            SampleRef::File("samples/C4.wav".into()),
            KeyRange { lo: 55, hi: 66 },
            60,
        );
        z.round_robin = Some(1);
        e.add_zone(i, z).unwrap();
        let mut r = Zone::new(
            g,
            SampleRef::File("samples/rel.wav".into()),
            KeyRange::ALL,
            60,
        );
        r.trigger = ZoneTrigger::Release;
        e.add_zone(i, r).unwrap();
        let p = e.finish().0;
        let v = zones_view(&p, &json!({})).unwrap();
        let inst = &v["instruments"][0];
        assert_eq!(inst["instrument"], 0);
        assert_eq!(inst["name"], "Piano");
        let zones = &inst["groups"][0]["zones"];
        assert_eq!(
            zones[0],
            json!({ "id": 0, "keys": [55, 66], "velocity": [0, 127], "root": 60,
                    "sample": "samples/C4.wav", "rr": 1, "text": "G3-F#4 C4.wav" })
        );
        assert_eq!(zones[1]["release"], true);
        assert_eq!(zones_view(&p, &json!({ "instrument": 0 })).unwrap(), v);
        assert!(zones_view(&p, &json!({ "instrument": 4 })).is_err());
        assert!(zones_view(&p, &json!({ "instrument": "x" })).is_err());
    }

    /// A strings patch like the acceptance test's: two keyswitched articulations, velocity
    /// layers with round robins, a release group, and a staccato forte zone that stops short.
    fn strings() -> Project {
        let mut e = Edit::new(&Project::empty("s"));
        let i = e.add_instrument(HausInstrument::new("Strings")).unwrap();
        let sus = e.add_group(i, Group::new("Sustain")).unwrap();
        let rel = e.add_group(i, Group::new("Release")).unwrap();
        let stac = e.add_group(i, Group::new("Staccato")).unwrap();
        let k = KeyRange { lo: 36, hi: 84 };
        for (n, vel, rr) in [
            ("sus_p_rr1.wav", (0, 80), 0),
            ("sus_p_rr2.wav", (0, 80), 1),
            ("sus_f_rr1.wav", (81, 127), 0),
            ("sus_f_rr2.wav", (81, 127), 1),
        ] {
            let mut z = Zone::new(sus, SampleRef::File(n.into()), k, 60);
            z.velocity = VelRange {
                lo: vel.0,
                hi: vel.1,
            };
            z.round_robin = Some(rr);
            e.add_zone(i, z).unwrap();
        }
        let mut r = Zone::new(rel, SampleRef::File("rel.wav".into()), k, 60);
        r.trigger = ZoneTrigger::Release;
        e.add_zone(i, r).unwrap();
        let mut p = Zone::new(stac, SampleRef::File("stac_p.wav".into()), k, 60);
        p.velocity = VelRange { lo: 0, hi: 80 };
        e.add_zone(i, p).unwrap();
        let mut f = Zone::new(
            stac,
            SampleRef::File("stac_f.wav".into()),
            KeyRange { lo: 36, hi: 71 },
            60,
        );
        f.velocity = VelRange { lo: 81, hi: 127 };
        e.add_zone(i, f).unwrap();
        e.update_instrument(i, |x| {
            x.articulations = vec![
                Articulation {
                    id: ArticulationId(0),
                    name: "Sustain".into(),
                    trigger: ArticulationTrigger::Key { key: 24 },
                    color: None,
                },
                Articulation {
                    id: ArticulationId(1),
                    name: "Staccato".into(),
                    trigger: ArticulationTrigger::Key { key: 25 },
                    color: None,
                },
            ];
            x.settings.default_articulation = Some(ArticulationId(0));
            x.modulation = vec![ModRoute {
                source: ModSource::Cc { controller: 1 },
                target: ModTarget::Volume,
                depth: 6.0,
                curve: ModCurve::Linear,
                group: None,
            }];
        })
        .unwrap();
        e.update_group(i, sus, |g| g.articulations = vec![ArticulationId(0)])
            .unwrap();
        e.update_group(i, rel, |g| g.articulations = vec![ArticulationId(0)])
            .unwrap();
        e.update_group(i, stac, |g| g.articulations = vec![ArticulationId(1)])
            .unwrap();
        e.finish().0
    }

    #[test]
    fn zones_view_summarises_groups_and_guides_the_player() {
        let p = strings();
        let v = zones_view(&p, &json!({})).unwrap();
        let inst = &v["instruments"][0];
        assert_eq!(inst["range"], json!([36, 84]));
        assert_eq!(inst["range_text"], "C2-C6");
        assert_eq!(inst["zone_count"], 7);
        assert_eq!(inst["kit"], false);
        assert_eq!(v["zones_total"], 7);
        assert!(v.get("page").is_none(), "every zone is listed: {v}");
        assert_eq!(inst["articulations"][1]["switch"], "key C#1 (25)");
        assert_eq!(inst["articulations"][1]["groups"], json!([2]));
        let g = &inst["groups"];
        assert_eq!(g[0]["layers"], json!(["0-80", "81-127"]));
        assert_eq!(g[0]["rr"], 2);
        assert_eq!(g[0]["articulation_names"], json!(["Sustain"]));
        assert_eq!(g[0]["gaps"], json!([]));
        assert_eq!(g[1]["release_zones"], 1);
        assert_eq!(g[1]["keys_text"], Value::Null);
        assert_eq!(g[2]["gaps"], json!(["vel 81-127: no zone on C5-C6"]));
        assert_eq!(g[2]["zones"][1]["text"], "C2-B4 vel 81-127 stac_f.wav");
        // the guide: the catalogue's shape, so it reads like a plugin's
        let guide: PlayingGuide = serde_json::from_value(inst["guide"].clone()).unwrap();
        assert_eq!(guide.range, Some([36, 84]));
        assert_eq!(
            guide.keyswitches,
            vec![
                Keyswitch {
                    key: 24,
                    articulation: "Sustain".into()
                },
                Keyswitch {
                    key: 25,
                    articulation: "Staccato".into()
                }
            ]
        );
        assert_eq!(
            guide.ccs,
            vec![CcUse {
                cc: 1,
                meaning: "moves volume".into()
            }]
        );
        assert_eq!(guide.velocity, "2 layers: 0-80, 81-127");
        assert!(!guide.mono && !guide.legato);
        for part in [
            "starts as Sustain",
            "gaps: Staccato (Staccato) vel 81-127: no zone on C5-C6",
            "round robins: up to 2 takes per note",
            "release samples sound on note-off",
        ] {
            assert!(guide.notes.contains(part), "{part}: {}", guide.notes);
        }
        let text = inst["guide_text"].as_str().unwrap();
        assert!(
            text.starts_with("plays C2-C6 (36-84)") && text.contains("C1 (24) Sustain"),
            "{text}"
        );
    }

    #[test]
    fn zones_view_pages_big_libraries() {
        let mut e = Edit::new(&Project::empty("big"));
        let i = e.add_instrument(HausInstrument::new("Big")).unwrap();
        let g = e.add_group(i, Group::new("All")).unwrap();
        for k in 0..100u8 {
            e.add_zone(
                i,
                Zone::new(
                    g,
                    SampleRef::File(format!("k{k}.wav")),
                    KeyRange { lo: k, hi: k },
                    k,
                ),
            )
            .unwrap();
        }
        let p = e.finish().0;
        let v = zones_view(&p, &json!({ "limit": 40 })).unwrap();
        assert_eq!(v["zones_total"], 100);
        assert_eq!(
            v["page"],
            json!({ "offset": 0, "limit": 40, "shown": 40, "next_offset": 40 })
        );
        let zones = v["instruments"][0]["groups"][0]["zones"]
            .as_array()
            .unwrap();
        assert_eq!((zones.len(), &zones[0]["id"]), (40, &json!(0)));
        let last = zones_view(&p, &json!({ "limit": 40, "offset": 80 })).unwrap();
        assert_eq!(last["page"]["next_offset"], Value::Null);
        assert_eq!(last["page"]["shown"], 20);
        assert_eq!(
            last["instruments"][0]["groups"][0]["zones"][0]["id"],
            json!(80)
        );
        // the default page and the summary-only form
        let d = zones_view(&p, &json!({})).unwrap();
        assert!(d.get("page").is_none());
        let s = zones_view(&p, &json!({ "zones": false })).unwrap();
        assert!(s["instruments"][0]["groups"][0].get("zones").is_none());
        assert_eq!(s["instruments"][0]["groups"][0]["zone_count"], 100);
        assert!(zones_view(&p, &json!({ "limit": "x" })).is_err());
        // one sample per key over a run of keys is a piano, not a kit
        assert!(!is_kit(&p.instruments[&InstrumentId(0)]));
    }

    #[test]
    fn kits_and_keyswitches_are_not_pitches() {
        let mut e = Edit::new(&Project::empty("kit"));
        let i = e.add_instrument(HausInstrument::new("Kit")).unwrap();
        let g = e.add_group(i, Group::new("Hits")).unwrap();
        for (k, n) in [(36, "kick.wav"), (38, "snare.wav"), (42, "hat.wav")] {
            e.add_zone(
                i,
                Zone::new(g, SampleRef::File(n.into()), KeyRange { lo: k, hi: k }, k),
            )
            .unwrap();
        }
        let p = e.finish().0;
        let kit = &p.instruments[&InstrumentId(0)];
        assert!(is_kit(kit), "scattered single keys");
        let gd = guide(&p, kit);
        assert!(
            gd.notes
                .contains("a kit: each key plays its own sound (C2 kick, D2 snare, F#2 hat)"),
            "{}",
            gd.notes
        );
        // the same keys with their pitch fixed by a cancelled key track (SF2 scaleTuning 0)
        let mut tracked = kit.clone();
        for z in tracked.zones.values_mut() {
            z.keys = KeyRange {
                lo: z.keys.lo - 36 + 60,
                hi: z.keys.lo - 36 + 60,
            };
            z.root_key = z.keys.lo;
        }
        tracked.zones.get_mut(&ZoneId(1)).unwrap().keys = KeyRange { lo: 61, hi: 61 };
        tracked.zones.get_mut(&ZoneId(1)).unwrap().root_key = 61;
        tracked.zones.get_mut(&ZoneId(2)).unwrap().keys = KeyRange { lo: 62, hi: 62 };
        tracked.zones.get_mut(&ZoneId(2)).unwrap().root_key = 62;
        assert!(!is_kit(&tracked), "a chromatic run of keys plays pitches");
        tracked.modulation.push(ModRoute {
            source: ModSource::Key,
            target: ModTarget::Pitch,
            depth: -6400.0,
            curve: ModCurve::Linear,
            group: None,
        });
        assert!(is_kit(&tracked), "key tracking cancelled");
        let s = strings();
        assert!(!is_kit(&s.instruments[&InstrumentId(0)]));
        assert_eq!(
            keyswitch_keys(&s.instruments[&InstrumentId(0)]),
            BTreeSet::from([24, 25])
        );
    }
}
