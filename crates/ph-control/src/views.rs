//! Readable views of a project for agents (DESIGN §5): note names and `bar.beat.tick`
//! positions, chord progressions, the section map and a mixer summary. Pure functions of a
//! `Project` snapshot.

use ph_analysis::Timing;
use ph_analysis::music::{
    ChordGrain, SongNote, chords_from_notes, key_from_notes, note_name, pattern_notes, progression,
    song_notes,
};
use ph_core::{Channel, ChannelId, EffectSpec, InstrumentSpec, PatternId, Project, Tick};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn timing(p: &Project) -> Timing {
    Timing::new(p.settings.bpm, p.settings.time_signature)
}

/// Resolves a channel by id (`"3"`, `3`) or case-insensitive name.
pub fn find_channel(p: &Project, v: &Value) -> Result<ChannelId, String> {
    if let Some(n) = v.as_u64() {
        return p
            .channels
            .contains_key(&ChannelId(n as u32))
            .then_some(ChannelId(n as u32))
            .ok_or_else(|| format!("no channel with id {n}"));
    }
    let s = v.as_str().ok_or("channel must be an id or a name")?;
    if let Ok(n) = s.parse::<u32>()
        && p.channels.contains_key(&ChannelId(n))
    {
        return Ok(ChannelId(n));
    }
    p.channels
        .values()
        .find(|c| c.name.eq_ignore_ascii_case(s))
        .map(|c| c.id)
        .ok_or_else(|| {
            format!(
                "no channel named {s:?} (channels: {})",
                p.channels_in_order()
                    .map(|c| format!("{} = {}", c.id, c.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

pub fn find_pattern(p: &Project, v: &Value) -> Result<PatternId, String> {
    if let Some(n) = v.as_u64() {
        return p
            .patterns
            .contains_key(&PatternId(n as u32))
            .then_some(PatternId(n as u32))
            .ok_or_else(|| format!("no pattern with id {n}"));
    }
    let s = v.as_str().ok_or("pattern must be an id or a name")?;
    if let Ok(n) = s.parse::<u32>()
        && p.patterns.contains_key(&PatternId(n))
    {
        return Ok(PatternId(n));
    }
    p.patterns
        .values()
        .find(|c| c.name.eq_ignore_ascii_case(s))
        .map(|c| c.id)
        .ok_or_else(|| format!("no pattern named {s:?}"))
}

/// Channels that play pitches (as opposed to one-shot drum hits).
pub fn pitched_channels(p: &Project, notes: &[SongNote]) -> BTreeSet<ChannelId> {
    let mut keys: BTreeMap<ChannelId, BTreeSet<u8>> = BTreeMap::new();
    for n in notes {
        keys.entry(n.channel).or_default().insert(n.key);
    }
    p.channels
        .values()
        .filter(|c| match c.instrument {
            InstrumentSpec::Synth { .. }
            | InstrumentSpec::MultiSampler { .. }
            | InstrumentSpec::Plugin { .. } => true,
            InstrumentSpec::Sampler { .. } => keys.get(&c.id).is_some_and(|k| k.len() >= 3),
        })
        .map(|c| c.id)
        .collect()
}

pub fn pitched_only(p: &Project, notes: Vec<SongNote>) -> Vec<SongNote> {
    let set = pitched_channels(p, &notes);
    notes
        .into_iter()
        .filter(|n| set.contains(&n.channel))
        .collect()
}

fn len_text(t: &Timing, len: Tick) -> String {
    let beat = t.ticks_per_beat();
    if len.is_multiple_of(beat) {
        let b = len / beat;
        format!("{b} beat{}", if b == 1 { "" } else { "s" })
    } else {
        format!("{len}t")
    }
}

/// Bars covered by `[from, to)` ticks as "bars a-b".
pub fn bar_span(t: &Timing, from: Tick, to: Tick) -> String {
    let a = from / t.ticks_per_bar() + 1;
    let b = to.saturating_sub(1) / t.ticks_per_bar() + 1;
    if a == b {
        format!("bar {a}")
    } else {
        format!("bars {a}-{b}")
    }
}

// ---------------------------------------------------------------------------------------

/// `notes` view: names and `bar.beat.tick`. Args: `pattern`, `channel`, `from_bar`,
/// `to_bar`, `limit`. Without `pattern`, the song as the playlist arranges it.
pub fn notes_view(p: &Project, args: &Value) -> Result<Value, String> {
    let t = timing(p);
    let (mut notes, scope) = match args.get("pattern").filter(|v| !v.is_null()) {
        Some(v) => {
            let id = find_pattern(p, v)?;
            (
                pattern_notes(p, id),
                format!("pattern {:?}", p.patterns[&id].name),
            )
        }
        None => (song_notes(p), "song".to_string()),
    };
    if let Some(v) = args.get("channel").filter(|v| !v.is_null()) {
        let ch = find_channel(p, v)?;
        notes.retain(|n| n.channel == ch);
    }
    let bpb = t.ticks_per_bar();
    if let Some(a) = args.get("from_bar").and_then(Value::as_u64) {
        notes.retain(|n| n.start + n.length > (a.max(1) - 1) * bpb);
    }
    if let Some(b) = args.get("to_bar").and_then(Value::as_u64) {
        notes.retain(|n| n.start < b * bpb);
    }
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(400) as usize;
    let total = notes.len();
    let mut by_channel: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in notes.iter().take(limit) {
        let name = p
            .channels
            .get(&n.channel)
            .map_or("?".into(), |c| c.name.clone());
        by_channel.entry(name).or_default().push(format!(
            "{} @{} len {} v{:.2}",
            note_name(n.key),
            t.position(n.start),
            len_text(&t, n.length),
            n.velocity
        ));
    }
    Ok(json!({
        "scope": scope,
        "format": "NOTE @bar.beat.tick len <length> v<velocity>; C4 = MIDI 60; 960 ticks per beat",
        "total_notes": total,
        "shown": total.min(limit),
        "channels": by_channel,
    }))
}

fn range_text(notes: &[&SongNote]) -> Option<String> {
    let lo = notes.iter().map(|n| n.key).min()?;
    let hi = notes.iter().map(|n| n.key).max()?;
    Some(if lo == hi {
        note_name(lo)
    } else {
        format!("{}-{}", note_name(lo), note_name(hi))
    })
}

/// `pattern` view: channels, note ranges and the chords per bar. `args.pattern` is an id or
/// name; the first pattern by default.
pub fn pattern_view(p: &Project, args: &Value) -> Result<Value, String> {
    let t = timing(p);
    let id = match args.get("pattern").filter(|v| !v.is_null()) {
        Some(v) => find_pattern(p, v)?,
        None => *p
            .pattern_order
            .first()
            .ok_or("the project has no patterns")?,
    };
    let pat = &p.patterns[&id];
    let len = p.pattern_length(id).map_err(|e| e.to_string())?;
    let all = pattern_notes(p, id);
    let bars = len.div_ceil(t.ticks_per_bar());
    let pitched = pitched_channels(p, &all);
    let mut channels = Vec::new();
    for c in p.channels_in_order() {
        let ns: Vec<&SongNote> = all.iter().filter(|n| n.channel == c.id).collect();
        if ns.is_empty() {
            continue;
        }
        let first = ns.iter().map(|n| n.start).min().unwrap_or(0);
        let last = ns.iter().map(|n| n.start + n.length).max().unwrap_or(0);
        channels.push(json!({
            "channel": c.name,
            "id": c.id,
            "notes": ns.len(),
            "range": range_text(&ns),
            "spans": bar_span(&t, first, last),
            "pitched": pitched.contains(&c.id),
        }));
    }
    let pn: Vec<SongNote> = all
        .iter()
        .filter(|n| pitched.contains(&n.channel))
        .cloned()
        .collect();
    let spans = chords_from_notes(&pn, &t, 1, bars as u32, ChordGrain::Auto);
    let key = key_from_notes(&pn, &t);
    Ok(json!({
        "pattern": pat.name,
        "id": id,
        "bars": bars,
        "channels": channels,
        "key": key.map(|k| k.key),
        "chords": progression(&spans),
        "chords_by_bar": spans.iter().map(|s| format!("{}.{}: {}", s.bar, s.beat, s.symbol)).collect::<Vec<_>>(),
    }))
}

const SECTION_WORDS: &[&str] = &[
    "intro",
    "verse",
    "chorus",
    "drop",
    "bridge",
    "outro",
    "build",
    "breakdown",
    "hook",
    "pre-chorus",
];

/// `sections` view: the arrangement read from playlist clips and pattern names.
pub fn sections_view(p: &Project) -> Value {
    let t = timing(p);
    if p.clips.is_empty() {
        return json!({
            "sections": [],
            "text": "the playlist is empty (add clips with playlist.add_clip to arrange patterns)",
        });
    }
    let bar = t.ticks_per_bar();
    let end = p.song_length();
    let mut cuts: BTreeSet<Tick> = BTreeSet::new();
    cuts.insert(0);
    cuts.insert(end);
    for c in p.clips.values() {
        cuts.insert(c.start);
        cuts.insert(c.start + c.length);
    }
    let cuts: Vec<Tick> = cuts.into_iter().collect();
    // (from, to, set of pattern ids)
    let mut segs: Vec<(Tick, Tick, BTreeSet<PatternId>)> = Vec::new();
    for w in cuts.windows(2) {
        let active: BTreeSet<PatternId> = p
            .clips
            .values()
            .filter(|c| c.start <= w[0] && c.start + c.length >= w[1])
            .filter(|c| !p.tracks.get(c.track as usize).is_some_and(|t| t.muted))
            .filter_map(|c| c.pattern())
            .collect();
        match segs.last_mut() {
            Some(last) if last.2 == active => last.1 = w[1],
            _ => segs.push((w[0], w[1], active)),
        }
    }
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for (i, (a, b, set)) in segs.iter().enumerate() {
        let names: Vec<String> = set.iter().map(|id| p.patterns[id].name.clone()).collect();
        let label = names
            .iter()
            .find_map(|n| {
                let l = n.to_lowercase();
                SECTION_WORDS
                    .iter()
                    .find(|w| l.contains(*w))
                    .map(|w| w.to_string())
            })
            .unwrap_or_else(|| format!("section {}", i + 1));
        let (fa, fb) = (a / bar + 1, (b - 1) / bar + 1);
        lines.push(format!(
            "{label} {fa}-{fb}: {}",
            if names.is_empty() {
                "(silence)".into()
            } else {
                names.join(", ")
            }
        ));
        out.push(json!({ "label": label, "from_bar": fa, "to_bar": fb, "patterns": names }));
    }
    json!({ "song_bars": end.div_ceil(bar), "sections": out, "text": lines.join("; ") })
}

fn effect_name(e: &EffectSpec) -> String {
    match e {
        EffectSpec::Builtin { name, .. } => name.clone(),
        EffectSpec::Plugin { plugin_id, .. } => plugin_id.clone(),
    }
}

fn channel_line(c: &Channel) -> String {
    let inst = match &c.instrument {
        InstrumentSpec::Sampler {
            sample,
            slices,
            root_key,
            ..
        } => {
            let file = sample.as_deref().unwrap_or("(no sample)");
            if slices.is_empty() {
                format!("sampler {file}")
            } else {
                format!(
                    "sliced sampler {file} ({} slices on keys {}-{})",
                    slices.len(),
                    note_name(*root_key),
                    note_name(root_key.saturating_add(slices.len() as u8 - 1))
                )
            }
        }
        InstrumentSpec::Synth { preset, .. } => {
            format!("synth {}", preset.as_deref().unwrap_or("(init)"))
        }
        InstrumentSpec::MultiSampler { library, .. } => {
            format!("multisampler {}", library.as_deref().unwrap_or("(none)"))
        }
        InstrumentSpec::Plugin { plugin_id, .. } => format!("plugin {plugin_id}"),
    };
    format!(
        "{} (id {}): {inst}, vol {:.2}, pan {:+.2}{}, insert {}",
        c.name,
        c.id,
        c.volume,
        c.pan,
        if c.muted { ", MUTED" } else { "" },
        c.insert
    )
}

/// `channels` view.
pub fn channels_view(p: &Project) -> Value {
    json!({ "channels": p.channels_in_order().map(channel_line).collect::<Vec<_>>() })
}

/// `mixer` view: routing, effects and levels of every insert that is in use.
pub fn mixer_view(p: &Project) -> Value {
    let mut by_insert: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for c in p.channels_in_order() {
        by_insert
            .entry(c.insert.0)
            .or_default()
            .push(c.name.clone());
    }
    let mut out = Vec::new();
    for (i, ins) in p.mixer.iter().enumerate() {
        let used = i == 0
            || by_insert.contains_key(&(i as u32))
            || !ins.effects.is_empty()
            || !ins.sends.is_empty()
            || ins.sidechain_from.is_some()
            || ins.muted;
        if !used {
            continue;
        }
        let mut routes = Vec::new();
        if i != 0 && ins.route_to_master {
            routes.push("master".to_string());
        }
        for s in &ins.sends {
            routes.push(format!(
                "send to {} ({}) at {:.2}",
                s.to,
                p.mixer
                    .get(s.to.0 as usize)
                    .map_or("?", |x| x.name.as_str()),
                s.level
            ));
        }
        out.push(json!({
            "insert": i,
            "name": ins.name,
            "volume": round2(ins.volume as f64),
            "pan": round2(ins.pan as f64),
            "muted": ins.muted,
            "channels": by_insert.get(&(i as u32)).cloned().unwrap_or_default(),
            "effects": ins.effects.iter().enumerate().map(|(s, e)| format!(
                "{s}: {}{}{}",
                effect_name(&e.effect),
                if e.bypass { " (bypassed)" } else { "" },
                if (e.mix - 1.0).abs() > 1e-6 { format!(" mix {:.2}", e.mix) } else { String::new() }
            )).collect::<Vec<_>>(),
            "outputs": routes,
            "sidechain_from": ins.sidechain_from,
        }));
    }
    json!({ "inserts": out })
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// `overview` view: everything an agent needs to orient itself, in a few lines.
pub fn overview(p: &Project, warnings: &[String]) -> Value {
    let t = timing(p);
    let all = song_notes(p);
    let pn = pitched_only(p, all.clone());
    let bars = (p
        .song_length()
        .max(all.iter().map(|n| n.start + n.length).max().unwrap_or(0)))
    .div_ceil(t.ticks_per_bar());
    let key = key_from_notes(&pn, &t);
    let spans = chords_from_notes(&pn, &t, 1, bars.min(64) as u32, ChordGrain::Auto);
    let mut v = json!({
        "title": p.settings.title,
        "bpm": p.settings.bpm,
        "time_signature": format!("{}/{}", p.settings.time_signature.numerator, p.settings.time_signature.denominator),
        "channels": p.channels_in_order().map(|c| format!("{} (id {})", c.name, c.id)).collect::<Vec<_>>(),
        "patterns": p.patterns_in_order().map(|x| format!("{} (id {}, {} notes)", x.name, x.id, x.note_count())).collect::<Vec<_>>(),
        "clips": p.clips.len(),
        "song_bars": bars,
        "key": key.map(|k| format!("{} (confidence {})", k.key, k.confidence)),
        "chords": progression(&spans),
        "sections": sections_view(p)["text"],
        "warnings": warnings,
    });
    if let (Some(o), Some(extra)) = (
        v.as_object_mut(),
        crate::views_m2::overview_extra(p).as_object().cloned(),
    ) {
        o.extend(extra);
    }
    v
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ph_commands::{Caller, Registry, Request, Session};
    use std::sync::Arc;

    /// A tiny project through the real commands: two channels, a chord pattern, a clip.
    pub fn demo_project() -> Project {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("demo"));
        let mut host = ph_commands::NullHost::default();
        let who = Caller::human();
        let mut run = |cmd: &str, params: Value| -> Value {
            s.execute(&who, Request::new(cmd, params), &mut host)
                .unwrap_or_else(|e| panic!("{cmd}: {e}"))
                .result
        };
        let pat = run("pattern.add", json!({ "name": "Verse A" }));
        let pid = pat["pattern"].as_u64().unwrap();
        let bass = run(
            "channel.add",
            json!({ "name": "Bass", "instrument": { "kind": "synth", "preset": "bass/sub" } }),
        );
        let bid = bass["channel"].as_u64().unwrap();
        let kick = run(
            "channel.add",
            json!({ "name": "Kick", "instrument": { "kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60 } }),
        );
        let kid = kick["channel"].as_u64().unwrap();
        // Am F C G, root notes, one per bar
        let roots = [33u64, 29, 36, 31];
        let notes: Vec<Value> = roots
            .iter()
            .enumerate()
            .map(|(i, k)| json!({ "key": k, "start": i as u64 * 3840, "length": 3600 }))
            .collect();
        run(
            "note.add",
            json!({ "pattern": pid, "channel": bid, "notes": notes }),
        );
        let hits: Vec<Value> = (0..16)
            .map(|i| json!({ "key": 60, "start": i as u64 * 960, "length": 100 }))
            .collect();
        run(
            "note.add",
            json!({ "pattern": pid, "channel": kid, "notes": hits }),
        );
        run(
            "playlist.add_clip",
            json!({ "pattern": pid, "track": 0, "start": 0 }),
        );
        s.project().as_ref().clone()
    }

    #[test]
    fn views_read_like_a_musician() {
        let p = demo_project();
        let n = notes_view(&p, &json!({ "pattern": "Verse A", "channel": "bass" })).unwrap();
        let bass = n["channels"]["Bass"].as_array().unwrap();
        assert_eq!(bass.len(), 4);
        assert!(
            bass[0].as_str().unwrap().starts_with("A1 @1.1.0 len"),
            "{bass:?}"
        );
        assert!(bass[1].as_str().unwrap().starts_with("F1 @2.1.0"));

        let pv = pattern_view(&p, &json!({})).unwrap();
        assert_eq!(pv["bars"], 4);
        assert_eq!(pv["chords"], "A5 - F5 - C5 - G5");
        let kick = pv["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["channel"] == "Kick")
            .unwrap();
        assert_eq!(kick["pitched"], false);

        let s = sections_view(&p);
        assert_eq!(s["text"], "verse 1-4: Verse A");

        let m = mixer_view(&p);
        assert!(m["inserts"][0]["name"] == "Master");
        let o = overview(&p, &[]);
        assert_eq!(o["song_bars"], 4);
        assert!(
            o["key"].as_str().unwrap().starts_with("A minor")
                || o["key"].as_str().unwrap().contains("major")
        );
    }
}
