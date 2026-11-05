//! The musical diff for Haus Sampler edits (M3/T7): instruments, groups, zones, articulations
//! and the modulation matrix, in words, e.g. "Strings: moved zone c4_f.wav to C4-D4 vel
//! 81-127" or "Strings: added articulation Staccato on C0 (12)". The same lines reach other
//! agents through `while_you_worked` blocks and the events resource, which diff the project
//! the same way.

use crate::views_m3::{key_span, sample_label, zone_place};
use ph_commands::catalogue::note_name;
use ph_core::{
    Articulation, ArticulationTrigger, Group, HausInstrument, InstrumentSpec, Project, Zone, ZoneId,
};
use std::collections::BTreeSet;

/// Zone changes of one kind listed one per line up to this many, then summarised.
const ZONE_LINES: usize = 4;

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn trigger_text(t: &ArticulationTrigger) -> String {
    match t {
        ArticulationTrigger::Key { key } => format!("on {} ({key})", note_name(*key)),
        ArticulationTrigger::Cc { controller, lo, hi } => {
            format!("on CC{controller} {lo:.2}-{hi:.2}")
        }
    }
}

/// A channel that now plays a Haus instrument, named; `None` for other instrument changes.
pub fn channel_instrument_line(
    after: &Project,
    old: &InstrumentSpec,
    new: &InstrumentSpec,
    channel: &str,
) -> Option<String> {
    let InstrumentSpec::MultiSampler {
        instrument: Some(i),
        ..
    } = new
    else {
        return None;
    };
    if matches!(old, InstrumentSpec::MultiSampler { instrument: Some(o), .. } if o == i) {
        return None;
    }
    let name = after
        .instruments
        .get(i)
        .map_or_else(|| format!("instrument {i}"), |x| x.name.clone());
    Some(format!(
        "{channel}: now plays Haus instrument {name} (id {i})"
    ))
}

fn span(i: &HausInstrument) -> Option<String> {
    let lo = i.zones.values().map(|z| z.keys.lo).min()?;
    let hi = i.zones.values().map(|z| z.keys.hi).max()?;
    Some(key_span(lo, hi))
}

/// Appends the lines for Haus Sampler instruments.
pub fn m3_lines(before: &Project, after: &Project, lines: &mut Vec<String>) {
    for (id, n) in after.instruments.iter() {
        match before.instruments.get(id) {
            None => lines.push(format!(
                "added instrument {} (id {id}): {} in {}{}",
                n.name,
                plural(n.zones.len(), "zone", "zones"),
                plural(n.groups.len(), "group", "groups"),
                span(n).map_or(String::new(), |s| format!(", {s}"))
            )),
            Some(o) if o != n => instrument_lines(before, after, o, n, lines),
            Some(_) => {}
        }
    }
    for (id, o) in before.instruments.iter() {
        if !after.instruments.contains_key(id) {
            lines.push(format!("removed instrument {} (id {id})", o.name));
        }
    }
}

fn instrument_lines(
    before: &Project,
    after: &Project,
    o: &HausInstrument,
    n: &HausInstrument,
    lines: &mut Vec<String>,
) {
    let name = &n.name;
    if o.name != n.name {
        lines.push(format!("renamed instrument {} -> {}", o.name, n.name));
    }
    if o.folder != n.folder {
        lines.push(format!(
            "{name}: samples now {}",
            n.folder
                .as_deref()
                .map_or("beside the project".to_string(), |f| format!("in {f}"))
        ));
    }
    settings_lines(o, n, lines);
    articulation_lines(o, n, lines);
    if o.modulation != n.modulation {
        lines.push(format!(
            "{name}: modulation matrix now {}",
            plural(n.modulation.len(), "route", "routes")
        ));
    }
    for (gid, g) in n.groups.iter() {
        match o.groups.get(gid) {
            None => lines.push(format!("{name}: added group {}", g.name)),
            Some(og) if og != g => group_lines(n, og, g, lines),
            Some(_) => {}
        }
    }
    for (gid, g) in o.groups.iter() {
        if !n.groups.contains_key(gid) {
            lines.push(format!("{name}: removed group {}", g.name));
        }
    }
    zone_lines(before, after, o, n, lines);
}

fn settings_lines(o: &HausInstrument, n: &HausInstrument, lines: &mut Vec<String>) {
    let (a, b, name) = (&o.settings, &n.settings, &n.name);
    if a == b {
        return;
    }
    let mut parts = Vec::new();
    if a.volume_db != b.volume_db {
        parts.push(format!(
            "volume {:.1} -> {:.1} dB",
            a.volume_db, b.volume_db
        ));
    }
    if a.pan != b.pan {
        parts.push(format!("pan {:+.2} -> {:+.2}", a.pan, b.pan));
    }
    if a.transpose != b.transpose {
        parts.push(format!("transpose {:+} -> {:+}", a.transpose, b.transpose));
    }
    if a.tune_cents != b.tune_cents {
        parts.push(format!(
            "tune {:+.1} -> {:+.1} cents",
            a.tune_cents, b.tune_cents
        ));
    }
    if a.voice_limit != b.voice_limit {
        parts.push(format!("voices {} -> {}", a.voice_limit, b.voice_limit));
    }
    if (a.bend_up, a.bend_down) != (b.bend_up, b.bend_down) {
        parts.push(format!("bend +{}/-{} semitones", b.bend_up, b.bend_down));
    }
    if a.default_articulation != b.default_articulation {
        parts.push(
            match b.default_articulation.and_then(|x| n.articulation(x)) {
                Some(x) => format!("starts as {}", x.name),
                None => "no default articulation".into(),
            },
        );
    }
    if parts.is_empty() {
        parts.push("settings changed".into());
    }
    lines.push(format!("{name}: {}", parts.join(", ")));
}

fn articulation_lines(o: &HausInstrument, n: &HausInstrument, lines: &mut Vec<String>) {
    let name = &n.name;
    let find = |list: &[Articulation], id| list.iter().find(|a| a.id == id).cloned();
    for a in &n.articulations {
        match find(&o.articulations, a.id) {
            None => lines.push(format!(
                "{name}: added articulation {} {}",
                a.name,
                trigger_text(&a.trigger)
            )),
            Some(x) if x != *a => {
                let mut parts = Vec::new();
                if x.name != a.name {
                    parts.push(format!("renamed {} -> {}", x.name, a.name));
                }
                if x.trigger != a.trigger {
                    parts.push(format!("now {}", trigger_text(&a.trigger)));
                }
                if x.color != a.color {
                    parts.push("recoloured".into());
                }
                lines.push(format!(
                    "{name}: articulation {} {}",
                    a.name,
                    parts.join(", ")
                ));
            }
            Some(_) => {}
        }
    }
    for a in &o.articulations {
        if find(&n.articulations, a.id).is_none() {
            lines.push(format!("{name}: removed articulation {}", a.name));
        }
    }
}

fn group_lines(inst: &HausInstrument, o: &Group, g: &Group, lines: &mut Vec<String>) {
    let name = &inst.name;
    if o.name != g.name {
        lines.push(format!("{name}: renamed group {} -> {}", o.name, g.name));
    }
    if o.articulations != g.articulations {
        let names: Vec<String> = g
            .articulations
            .iter()
            .filter_map(|a| inst.articulation(*a).map(|x| x.name.clone()))
            .collect();
        lines.push(format!(
            "{name}: group {} now plays {}",
            g.name,
            if names.is_empty() {
                "every articulation".to_string()
            } else {
                names.join(", ")
            }
        ));
    }
    let mut parts: Vec<&str> = Vec::new();
    let mut check = |changed: bool, what: &'static str| {
        if changed {
            parts.push(what);
        }
    };
    check(o.amp_env != g.amp_env, "amp envelope");
    check(o.filter_env != g.filter_env, "filter envelope");
    check(o.filter != g.filter, "filter");
    check(o.lfos != g.lfos, "LFOs");
    check(
        o.voice_limit != g.voice_limit || o.stealing != g.stealing,
        "voices",
    );
    check(
        o.polyphony != g.polyphony || o.portamento != g.portamento,
        "polyphony",
    );
    check(o.round_robin != g.round_robin, "round robin");
    check(o.output != g.output, "output");
    check(o.effects != g.effects, "effects");
    check(o.volume_db != g.volume_db, "volume");
    check(o.color != g.color, "colour");
    check(o.extra != g.extra, "kept data");
    if !parts.is_empty() {
        lines.push(format!(
            "{name}: group {} {} changed",
            g.name,
            parts.join(", ")
        ));
    }
}

/// Fields of a zone other than its place, that changed.
fn zone_fields(a: &Zone, b: &Zone) -> Vec<&'static str> {
    let mut v = Vec::new();
    let mut check = |changed: bool, what: &'static str| {
        if changed {
            v.push(what);
        }
    };
    check(a.sample != b.sample, "sample");
    check(a.root_key != b.root_key && a.keys == b.keys, "root key");
    check(a.tune != b.tune || a.fine != b.fine, "tuning");
    check(a.volume_db != b.volume_db, "volume");
    check(a.pan != b.pan, "pan");
    check(a.start != b.start || a.end != b.end, "start/end");
    check(a.looping != b.looping, "loop");
    check(a.reverse != b.reverse, "reverse");
    check(a.trigger != b.trigger, "trigger");
    check(a.one_shot != b.one_shot, "one-shot");
    check(a.release_decay_db != b.release_decay_db, "release decay");
    check(a.round_robin != b.round_robin, "round robin");
    check(a.cc_conditions != b.cc_conditions, "CC conditions");
    check(a.crossfades != b.crossfades, "crossfades");
    v
}

fn zone_lines(
    before: &Project,
    after: &Project,
    o: &HausInstrument,
    n: &HausInstrument,
    lines: &mut Vec<String>,
) {
    let name = &n.name;
    let label = |p: &Project, z: &Zone| sample_label(p, &z.sample);
    let group_name = |i: &HausInstrument, z: &Zone| {
        i.groups
            .get(&z.group)
            .map_or("?", |g| g.name.as_str())
            .to_string()
    };
    let added: Vec<&Zone> = n
        .zones
        .values()
        .filter(|z| !o.zones.contains_key(&z.id))
        .collect();
    let removed: Vec<&Zone> = o
        .zones
        .values()
        .filter(|z| !n.zones.contains_key(&z.id))
        .collect();
    let mut moved: Vec<(&Zone, &Zone)> = Vec::new();
    let mut changed: Vec<(&Zone, &Zone)> = Vec::new();
    for z in n.zones.values() {
        let Some(old) = o.zones.get(&z.id) else {
            continue;
        };
        if old == z {
            continue;
        }
        if old.keys != z.keys || old.velocity != z.velocity || old.group != z.group {
            moved.push((old, z));
        }
        if !zone_fields(old, z).is_empty() {
            changed.push((old, z));
        }
    }
    // added
    if added.len() <= ZONE_LINES {
        for z in &added {
            lines.push(format!(
                "{name}: added zone {} at {}",
                label(after, z),
                zone_place(z)
            ));
        }
    } else {
        let lo = added.iter().map(|z| z.keys.lo).min().unwrap_or(0);
        let hi = added.iter().map(|z| z.keys.hi).max().unwrap_or(0);
        lines.push(format!(
            "{name}: added {} over {}",
            plural(added.len(), "zone", "zones"),
            key_span(lo, hi)
        ));
    }
    // removed
    if removed.len() <= ZONE_LINES {
        for z in &removed {
            lines.push(format!(
                "{name}: removed zone {} ({})",
                label(before, z),
                zone_place(z)
            ));
        }
    } else {
        lines.push(format!(
            "{name}: removed {}",
            plural(removed.len(), "zone", "zones")
        ));
    }
    // moved
    if moved.len() <= ZONE_LINES {
        for (a, b) in &moved {
            let into = if a.group != b.group {
                format!(" in group {}", group_name(n, b))
            } else {
                String::new()
            };
            lines.push(format!(
                "{name}: moved zone {} to {}{into}",
                label(after, b),
                zone_place(b)
            ));
        }
    } else {
        let places: BTreeSet<String> = moved.iter().map(|(_, b)| zone_place(b)).collect();
        let shown: Vec<&String> = places.iter().take(4).collect();
        lines.push(format!(
            "{name}: moved {} (now {}{})",
            plural(moved.len(), "zone", "zones"),
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("; "),
            if places.len() > 4 { "; ..." } else { "" }
        ));
    }
    // other settings
    let ids: Vec<ZoneId> = changed.iter().map(|(_, b)| b.id).collect();
    if changed.len() <= ZONE_LINES {
        for (a, b) in &changed {
            lines.push(format!(
                "{name}: changed zone {} ({})",
                label(after, b),
                zone_fields(a, b).join(", ")
            ));
        }
    } else {
        let mut what: Vec<&str> = changed
            .iter()
            .flat_map(|(a, b)| zone_fields(a, b))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        what.sort_unstable();
        lines.push(format!(
            "{name}: changed {} ({})",
            plural(ids.len(), "zone", "zones"),
            what.join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::diff::diff;
    use ph_commands::{Author, Caller, NullHost, Registry, Request, Session};
    use ph_core::Project;
    use serde_json::json;
    use std::sync::Arc;

    fn run(s: &mut Session, id: &str, params: serde_json::Value) -> serde_json::Value {
        let mut h = NullHost::default();
        s.execute(&Caller::human(), Request::new(id, params), &mut h)
            .unwrap_or_else(|e| panic!("{id}: {e}"))
            .result
    }

    #[test]
    fn sampler_edits_read_like_a_session_log() {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("d"));
        let b0 = s.project().clone();
        run(
            &mut s,
            "sampler.new",
            json!({ "name": "Strings", "group": "Sustain" }),
        );
        let d = diff(&b0, s.project()).lines.join(" | ");
        assert!(
            d.contains("added instrument Strings (id 0): 0 zones in 1 group"),
            "{d}"
        );
        assert!(
            d.contains("now plays Haus instrument Strings (id 0)"),
            "{d}"
        );

        let b1 = s.project().clone();
        run(
            &mut s,
            "sampler.zone_add",
            json!({ "instrument": 0, "zones": [
                { "group": 0, "sample": "s/c4_p.wav", "root_key": 60, "keys": [48, 72], "velocity": [0, 80] },
                { "group": 0, "sample": "s/c4_f.wav", "root_key": 60, "keys": [48, 72], "velocity": [81, 127] },
            ] }),
        );
        let d = diff(&b1, s.project()).lines.join(" | ");
        assert!(
            d.contains("Strings: added zone c4_f.wav at C3-C5 vel 81-127"),
            "{d}"
        );

        let b2 = s.project().clone();
        run(
            &mut s,
            "sampler.zone_move",
            json!({ "instrument": 0, "zones": [1], "keys": [60, 62] }),
        );
        let d = diff(&b2, s.project()).lines.join(" | ");
        assert_eq!(d, "Strings: moved zone c4_f.wav to C4-D4 vel 81-127");

        let b3 = s.project().clone();
        run(
            &mut s,
            "sampler.keyswitch_set",
            json!({ "instrument": 0, "name": "Staccato", "trigger": { "kind": "key", "key": 12 }, "groups": [0] }),
        );
        let d = diff(&b3, s.project()).lines.join(" | ");
        assert!(
            d.contains("Strings: added articulation Staccato on C0 (12)"),
            "{d}"
        );
        assert!(d.contains("group Sustain now plays Staccato"), "{d}");

        let b4 = s.project().clone();
        run(
            &mut s,
            "sampler.zone_set",
            json!({ "instrument": 0, "zones": [0], "set": { "tune": 2, "volume_db": -3 } }),
        );
        run(
            &mut s,
            "sampler.group_set",
            json!({ "instrument": 0, "group": 0, "set": { "polyphony": "mono" } }),
        );
        run(
            &mut s,
            "sampler.set",
            json!({ "instrument": 0, "set": { "volume_db": -6 } }),
        );
        let d = diff(&b4, s.project()).lines.join(" | ");
        assert!(
            d.contains("Strings: changed zone c4_p.wav (tuning, volume)"),
            "{d}"
        );
        assert!(
            d.contains("Strings: group Sustain polyphony changed"),
            "{d}"
        );
        assert!(d.contains("Strings: volume 0.0 -> -6.0 dB"), "{d}");

        let b5 = s.project().clone();
        run(
            &mut s,
            "sampler.zone_remove",
            json!({ "instrument": 0, "zones": [0] }),
        );
        let d = diff(&b5, s.project()).lines.join(" | ");
        assert_eq!(d, "Strings: removed zone c4_p.wav (C3-C5 vel 0-80)");

        // many zones at once are summarised
        let b6 = s.project().clone();
        let many: Vec<_> = (0..12)
            .map(|k| json!({ "group": 0, "sample": format!("k{k}.wav"), "root_key": 40 + k, "keys": [40 + k, 40 + k] }))
            .collect();
        run(
            &mut s,
            "sampler.zone_add",
            json!({ "instrument": 0, "zones": many }),
        );
        let d = diff(&b6, s.project()).lines.join(" | ");
        assert_eq!(d, "Strings: added 12 zones over E2-D#3");

        let b7 = s.project().clone();
        s.undo(&Author::Human).unwrap();
        s.undo(&Author::Human).unwrap();
        let d = diff(&b7, s.project()).lines.join(" | ");
        assert!(d.contains("Strings: removed 12 zones"), "{d}");
        assert!(d.contains("Strings: added zone c4_p.wav"), "{d}");
    }
}
