//! Change summaries: a musical diff between two project snapshots, in words an agent (or a
//! musician) can read, e.g. "Bass: +64 notes, bars 9-16; Verse chords now Am - F - C - G".

use crate::views::{bar_span, pitched_only, timing};
use ph_analysis::music::{ChordGrain, SongNote, chords_from_notes, pattern_notes, progression};
use ph_core::{ChannelId, EffectSpec, MixerInsert, Note, PatternId, Project, Tick};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diff {
    pub lines: Vec<String>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn to_json(&self) -> Value {
        json!(self.lines)
    }
}

const MAX_LINES: usize = 24;

fn effect_name(e: &EffectSpec) -> String {
    match e {
        EffectSpec::Builtin { name, .. } => name.clone(),
        EffectSpec::Plugin { plugin_id, .. } => plugin_id.clone(),
    }
}

fn ch_name(p: &Project, id: ChannelId) -> String {
    p.channels
        .get(&id)
        .map_or(format!("channel {id}"), |c| c.name.clone())
}

fn note_extent(notes: &[&Note]) -> Option<(Tick, Tick)> {
    Some((
        notes.iter().map(|n| n.start).min()?,
        notes.iter().map(|n| n.start + n.length).max()?,
    ))
}

fn pattern_chords(p: &Project, id: PatternId) -> Option<String> {
    let t = timing(p);
    let len = p.pattern_length(id).ok()?;
    let notes: Vec<SongNote> = pitched_only(p, pattern_notes(p, id));
    if notes.is_empty() {
        return None;
    }
    let bars = len.div_ceil(t.ticks_per_bar()) as u32;
    Some(progression(&chords_from_notes(
        &notes,
        &t,
        1,
        bars,
        ChordGrain::Auto,
    )))
}

pub fn diff(before: &Project, after: &Project) -> Diff {
    let mut lines: Vec<String> = Vec::new();
    let t = timing(after);

    // settings
    let (a, b) = (&before.settings, &after.settings);
    if a.title != b.title {
        lines.push(format!("title: {:?} -> {:?}", a.title, b.title));
    }
    if a.bpm != b.bpm {
        lines.push(format!("tempo: {} -> {} bpm", a.bpm, b.bpm));
    }
    if a.time_signature != b.time_signature {
        lines.push(format!(
            "time signature: {}/{} -> {}/{}",
            a.time_signature.numerator,
            a.time_signature.denominator,
            b.time_signature.numerator,
            b.time_signature.denominator
        ));
    }
    if a.swing != b.swing {
        lines.push(format!("swing: {} -> {}", a.swing, b.swing));
    }

    // channels
    for (id, c) in after.channels.iter() {
        match before.channels.get(id) {
            None => lines.push(format!("added channel {} (id {id})", c.name)),
            Some(o) => {
                if o.name != c.name {
                    lines.push(format!("renamed channel {} -> {}", o.name, c.name));
                }
                if o.instrument != c.instrument {
                    lines.push(crate::diff_m2::instrument_line(
                        &o.instrument,
                        &c.instrument,
                        &c.name,
                    ));
                }
                if o.volume != c.volume {
                    lines.push(format!(
                        "{}: volume {:.2} -> {:.2}",
                        c.name, o.volume, c.volume
                    ));
                }
                if o.pan != c.pan {
                    lines.push(format!("{}: pan {:+.2} -> {:+.2}", c.name, o.pan, c.pan));
                }
                if o.muted != c.muted {
                    lines.push(format!(
                        "{}: {}",
                        c.name,
                        if c.muted { "muted" } else { "unmuted" }
                    ));
                }
                if o.insert != c.insert {
                    lines.push(format!("{}: routed to insert {}", c.name, c.insert));
                }
            }
        }
    }
    for (id, c) in before.channels.iter() {
        if !after.channels.contains_key(id) {
            lines.push(format!("removed channel {}", c.name));
        }
    }

    // patterns and notes
    for (pid, pat) in after.patterns.iter() {
        let old = before.patterns.get(pid);
        match old {
            None => lines.push(format!("added pattern {} (id {pid})", pat.name)),
            Some(o) if o.name != pat.name => {
                lines.push(format!("renamed pattern {} -> {}", o.name, pat.name));
            }
            Some(_) => {}
        }
        let mut notes_changed = false;
        let chans: BTreeSet<ChannelId> = pat
            .notes
            .keys()
            .chain(old.into_iter().flat_map(|o| o.notes.keys()))
            .copied()
            .collect();
        for ch in chans {
            let new_map = pat.notes.get(&ch);
            let old_map = old.and_then(|o| o.notes.get(&ch));
            let empty = Default::default();
            let (nm, om) = (new_map.unwrap_or(&empty), old_map.unwrap_or(&empty));
            let added: Vec<&Note> = nm
                .iter()
                .filter(|(k, _)| !om.contains_key(k))
                .map(|(_, n)| n)
                .collect();
            let removed: Vec<&Note> = om
                .iter()
                .filter(|(k, _)| !nm.contains_key(k))
                .map(|(_, n)| n)
                .collect();
            let changed: Vec<&Note> = nm
                .iter()
                .filter(|(k, n)| om.get(k).is_some_and(|o| o != *n))
                .map(|(_, n)| n)
                .collect();
            if added.is_empty() && removed.is_empty() && changed.is_empty() {
                continue;
            }
            notes_changed = true;
            let name = ch_name(after, ch);
            let mut parts = Vec::new();
            if let Some((s, e)) = note_extent(&added) {
                parts.push(format!("+{} notes, {}", added.len(), bar_span(&t, s, e)));
            }
            if let Some((s, e)) = note_extent(&removed) {
                parts.push(format!("-{} notes, {}", removed.len(), bar_span(&t, s, e)));
            }
            if let Some((s, e)) = note_extent(&changed) {
                parts.push(format!("~{} edited, {}", changed.len(), bar_span(&t, s, e)));
            }
            lines.push(format!("{name} in {}: {}", pat.name, parts.join("; ")));
        }
        if notes_changed {
            let (b, a) = (pattern_chords(before, *pid), pattern_chords(after, *pid));
            if b != a
                && let Some(a) = a
            {
                lines.push(format!("{} chords now: {a}", pat.name));
            }
        }
    }
    for (pid, pat) in before.patterns.iter() {
        if !after.patterns.contains_key(pid) {
            lines.push(format!("removed pattern {}", pat.name));
        }
    }

    // playlist
    let clip_line = |p: &Project, c: &ph_core::Clip| {
        format!(
            "{} {} on track {}",
            p.clip_label(c),
            bar_span(&t, c.start, c.start + c.length),
            c.track + 1
        )
    };
    let added: Vec<String> = after
        .clips
        .iter()
        .filter(|(k, c)| c.pattern().is_some() && !before.clips.contains_key(k))
        .map(|(_, c)| clip_line(after, c))
        .collect();
    let removed: Vec<String> = before
        .clips
        .iter()
        .filter(|(k, c)| c.pattern().is_some() && !after.clips.contains_key(k))
        .map(|(_, c)| clip_line(before, c))
        .collect();
    let moved = after
        .clips
        .iter()
        .filter(|(k, c)| c.pattern().is_some() && before.clips.get(k).is_some_and(|o| o != *c))
        .count();
    if !added.is_empty() {
        lines.push(format!("playlist: added {}", join_some(&added)));
    }
    if !removed.is_empty() {
        lines.push(format!("playlist: removed {}", join_some(&removed)));
    }
    if moved > 0 {
        lines.push(format!("playlist: moved or resized {moved} clip(s)"));
    }
    for (i, (o, n)) in before.tracks.iter().zip(after.tracks.iter()).enumerate() {
        if o != n {
            lines.push(format!(
                "playlist track {}: {}",
                i + 1,
                if o.muted != n.muted {
                    if n.muted { "muted" } else { "unmuted" }.to_string()
                } else {
                    format!("renamed {} -> {}", o.name, n.name)
                }
            ));
        }
    }

    crate::diff_m2::m2_lines(before, after, &t, &mut lines);

    // mixer
    for (i, (o, n)) in before.mixer.iter().zip(after.mixer.iter()).enumerate() {
        mixer_lines(i, o, n, &mut lines);
    }

    let extra = lines.len().saturating_sub(MAX_LINES);
    lines.truncate(MAX_LINES);
    if extra > 0 {
        lines.push(format!("...and {extra} more change(s)"));
    }
    Diff { lines }
}

fn join_some(v: &[String]) -> String {
    if v.len() <= 4 {
        v.join("; ")
    } else {
        format!("{}; ...and {} more", v[..4].join("; "), v.len() - 4)
    }
}

fn mixer_lines(i: usize, o: &MixerInsert, n: &MixerInsert, lines: &mut Vec<String>) {
    let name = &n.name;
    let label = if i == 0 {
        "Master".to_string()
    } else {
        format!("insert {i} ({name})")
    };
    if o.name != n.name {
        lines.push(format!("insert {i}: renamed {} -> {}", o.name, n.name));
    }
    if o.volume != n.volume {
        lines.push(format!(
            "{label}: volume {:.2} -> {:.2}",
            o.volume, n.volume
        ));
    }
    if o.pan != n.pan {
        lines.push(format!("{label}: pan {:+.2} -> {:+.2}", o.pan, n.pan));
    }
    if o.muted != n.muted {
        lines.push(format!(
            "{label}: {}",
            if n.muted { "muted" } else { "unmuted" }
        ));
    }
    if o.route_to_master != n.route_to_master {
        lines.push(format!(
            "{label}: route to master {}",
            if n.route_to_master { "on" } else { "off" }
        ));
    }
    if o.sidechain_from != n.sidechain_from {
        lines.push(match n.sidechain_from {
            Some(s) => format!("{label}: sidechain from insert {s}"),
            None => format!("{label}: sidechain cleared"),
        });
    }
    for s in &n.sends {
        match o.sends.iter().find(|x| x.to == s.to) {
            None => lines.push(format!(
                "{label}: send to insert {} at {:.2}",
                s.to, s.level
            )),
            Some(x) if x.level != s.level => lines.push(format!(
                "{label}: send to insert {} {:.2} -> {:.2}",
                s.to, x.level, s.level
            )),
            _ => {}
        }
    }
    for s in &o.sends {
        if !n.sends.iter().any(|x| x.to == s.to) {
            lines.push(format!("{label}: removed send to insert {}", s.to));
        }
    }
    let names = |m: &MixerInsert| -> Vec<String> {
        m.effects.iter().map(|e| effect_name(&e.effect)).collect()
    };
    let (on, nn) = (names(o), names(n));
    if on != nn {
        lines.push(format!("{label}: effects now [{}]", nn.join(", ")));
    } else {
        for (s, (a, b)) in o.effects.iter().zip(&n.effects).enumerate() {
            if a != b {
                lines.push(format!(
                    "{label}: effect {s} ({}) settings changed",
                    effect_name(&b.effect)
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, Registry, Request, Session};
    use std::sync::Arc;

    #[test]
    fn diff_reads_like_a_session_log() {
        let before = crate::views::tests::demo_project();
        let mut s = Session::new(Arc::new(Registry::with_builtins()), before.clone());
        let mut host = ph_commands::NullHost::default();
        let who = Caller::human();
        let reqs = vec![
            Request::new(
                "note.add",
                json!({ "pattern": 0, "channel": 0, "notes": [
                    { "key": 45, "start": 15360, "length": 960 },
                    { "key": 48, "start": 16320, "length": 960 }] }),
            ),
            Request::new("project.set_tempo", json!({ "bpm": 100.0 })),
            Request::new("mixer.set_volume", json!({ "insert": 1, "volume": 0.5 })),
            Request::new(
                "mixer.add_effect",
                json!({ "insert": 1, "effect": { "kind": "builtin", "name": "eq" } }),
            ),
        ];
        s.batch(&who, reqs, &mut host).unwrap();
        let d = diff(&before, s.project());
        let text = d.lines.join(" | ");
        assert!(text.contains("Bass in Verse A: +2 notes, bar 5"), "{text}");
        assert!(text.contains("tempo: 130 -> 100 bpm"), "{text}");
        assert!(text.contains("effects now [eq]"), "{text}");
        assert!(text.contains("volume"), "{text}");
        assert!(diff(&before, &before).is_empty());
    }
}
