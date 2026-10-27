//! The musical diff for M2 edits (M2/T9): audio clips, the sample registry, automation clips,
//! plugin instruments and sliced samplers, in words.

use crate::views::bar_span;
use crate::views_m2::{
    audio_clip_line, auto_range, automation_clip_line, format_name, stretch_name, target_words,
};
use ph_analysis::Timing;
use ph_core::{AudioClip, AutomationClip, Clip, InstrumentSpec, Project};

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// How a channel's instrument changed, for the diff.
pub fn instrument_line(old: &InstrumentSpec, new: &InstrumentSpec, name: &str) -> String {
    use InstrumentSpec::*;
    match (old, new) {
        (
            Plugin {
                plugin_id: a,
                state: sa,
                params: pa,
                ..
            },
            Plugin {
                plugin_id: b,
                state: sb,
                params: pb,
                ..
            },
        ) if a == b => {
            let n = pb.iter().filter(|(k, v)| pa.get(*k) != Some(v)).count();
            if sa != sb && n == 0 {
                format!("{name}: plugin {b} state changed")
            } else if n > 0 {
                format!("{name}: plugin {b} {n} parameter{} changed", plural(n))
            } else {
                format!("{name}: plugin {b} settings changed")
            }
        }
        (
            _,
            Plugin {
                format, plugin_id, ..
            },
        ) => format!(
            "{name}: instrument is now plugin {plugin_id} ({})",
            format_name(*format)
        ),
        (
            Sampler {
                slices: a,
                sample: sa,
                ..
            },
            Sampler {
                slices: b,
                sample: sb,
                root_key,
                ..
            },
        ) if a != b => {
            if b.is_empty() {
                format!("{name}: no longer sliced")
            } else {
                format!(
                    "{name}: sliced into {} slice{} on keys {}-{} ({})",
                    b.len(),
                    plural(b.len()),
                    root_key,
                    root_key.saturating_add((b.len() - 1) as u8),
                    sb.as_deref().or(sa.as_deref()).unwrap_or("?")
                )
            }
        }
        _ => format!("{name}: instrument changed"),
    }
}

fn audio_changes(
    p: &Project,
    o: &(Clip, AudioClip),
    n: &(Clip, AudioClip),
    t: &Timing,
) -> Vec<String> {
    let ((oc, oa), (nc, na)) = (o, n);
    let mut parts = Vec::new();
    if oa.sample != na.sample {
        parts.push(format!(
            "now plays {}",
            p.sample(na.sample)
                .map_or_else(|_| format!("sample {}", na.sample), |e| e.name.clone())
        ));
    }
    if oc.track != nc.track {
        parts.push(format!("moved to track {}", nc.track + 1));
    }
    if oc.start != nc.start {
        parts.push(format!("moved to {}", t.position(nc.start)));
    }
    if oc.length != nc.length {
        parts.push(format!(
            "{} to {}",
            if nc.length < oc.length {
                "trimmed"
            } else {
                "lengthened"
            },
            bar_span(t, nc.start, nc.start + nc.length)
        ));
    }
    if oa.source_offset != na.source_offset {
        let sr = p.sample(na.sample).map_or(48_000, |e| e.sample_rate.max(1)) as f64;
        parts.push(format!(
            "starts {:+.3} s into the file",
            (na.source_offset as f64 - oa.source_offset as f64) / sr
        ));
    }
    if oa.gain_db != na.gain_db {
        parts.push(format!("gain {:+.1} -> {:+.1} dB", oa.gain_db, na.gain_db));
    }
    if oa.pitch() != na.pitch() {
        parts.push(format!("pitch {:+.2} -> {:+.2} st", oa.pitch(), na.pitch()));
    }
    if oa.reverse != na.reverse {
        parts.push(
            if na.reverse {
                "reversed"
            } else {
                "forwards again"
            }
            .into(),
        );
    }
    if oa.stretch != na.stretch {
        parts.push(format!(
            "mode {} -> {}",
            stretch_name(oa.stretch),
            stretch_name(na.stretch)
        ));
    }
    if oa.tempo_sync != na.tempo_sync {
        parts.push(
            if na.tempo_sync {
                "now follows the project tempo"
            } else {
                "no longer follows the tempo"
            }
            .into(),
        );
    }
    if oa.fade_in != na.fade_in {
        parts.push(format!(
            "fade in {} -> {} ticks",
            oa.fade_in.length, na.fade_in.length
        ));
    }
    if oa.fade_out != na.fade_out {
        parts.push(format!(
            "fade out {} -> {} ticks",
            oa.fade_out.length, na.fade_out.length
        ));
    }
    parts
}

fn auto_changes(
    p: &Project,
    o: &(Clip, AutomationClip),
    n: &(Clip, AutomationClip),
    t: &Timing,
) -> Vec<String> {
    let ((oc, oa), (nc, na)) = (o, n);
    let mut parts = Vec::new();
    if oa.target != na.target {
        parts.push(format!(
            "now controls {} (was {})",
            target_words(p, &na.target),
            target_words(p, &oa.target)
        ));
    }
    if oc.track != nc.track {
        parts.push(format!("moved to track {}", nc.track + 1));
    }
    if oc.start != nc.start {
        parts.push(format!("moved to {}", t.position(nc.start)));
    }
    if oc.length != nc.length || oc.offset != nc.offset {
        parts.push(format!(
            "now covers {}",
            bar_span(t, nc.start, nc.start + nc.length)
        ));
    }
    let added = na
        .points
        .keys()
        .filter(|k| !oa.points.contains_key(k))
        .count();
    let removed = oa
        .points
        .keys()
        .filter(|k| !na.points.contains_key(k))
        .count();
    let edited = na
        .points
        .iter()
        .filter(|(k, v)| oa.points.get(k).is_some_and(|x| x != *v))
        .count();
    if added + removed + edited > 0 {
        let mut s = Vec::new();
        if added > 0 {
            s.push(format!("+{added}"));
        }
        if removed > 0 {
            s.push(format!("-{removed}"));
        }
        if edited > 0 {
            s.push(format!("~{edited} edited"));
        }
        let range = match auto_range(na) {
            Some((lo, hi)) => format!(
                ", now {} point(s) from {} to {}",
                na.points.len(),
                r3(lo),
                r3(hi)
            ),
            None => ", now empty".into(),
        };
        parts.push(format!("points {}{range}", s.join(" ")));
    }
    parts
}

/// Appends the lines for audio clips, samples and automation clips.
pub fn m2_lines(before: &Project, after: &Project, t: &Timing, lines: &mut Vec<String>) {
    // the sample registry
    for (id, e) in after.samples.iter() {
        match before.samples.get(id) {
            None => lines.push(format!(
                "sample registered: {} ({:.1} s{})",
                e.name,
                e.seconds(),
                e.bpm()
                    .map_or(String::new(), |b| format!(", {} bpm", r1(b)))
            )),
            Some(o) if o.bpm() != e.bpm() => lines.push(format!(
                "sample {}: bpm {} -> {}",
                e.name,
                o.bpm().map_or("unknown".into(), |b| r1(b).to_string()),
                e.bpm().map_or("unknown".into(), |b| r1(b).to_string())
            )),
            Some(_) => {}
        }
    }
    for (id, e) in before.samples.iter() {
        if !after.samples.contains_key(id) {
            lines.push(format!("sample removed: {}", e.name));
        }
    }

    // audio and automation clips
    let (mut audio_added, mut audio_removed) = (Vec::new(), Vec::new());
    let (mut auto_added, mut auto_removed) = (Vec::new(), Vec::new());
    for (id, c) in after.clips.iter() {
        let old = before.clips.get(id);
        match (&c.audio(), &c.automation()) {
            (Some(a), _) => match old.and_then(|o| o.audio().map(|oa| (o.clone(), oa.clone()))) {
                None => audio_added.push(audio_clip_line(after, c, a, t)),
                Some(o) if o.0 != *c || o.1 != **a => {
                    let parts = audio_changes(after, &o, &(c.clone(), (*a).clone()), t);
                    if !parts.is_empty() {
                        lines.push(format!(
                            "audio {} (clip {id}): {}",
                            after.clip_label(c),
                            parts.join("; ")
                        ));
                    }
                }
                Some(_) => {}
            },
            (_, Some(a)) => {
                match old.and_then(|o| o.automation().map(|oa| (o.clone(), oa.clone()))) {
                    None => auto_added.push(automation_clip_line(after, c, a, t)),
                    Some(o) if o.0 != *c || o.1 != **a => {
                        let parts = auto_changes(after, &o, &(c.clone(), (*a).clone()), t);
                        if !parts.is_empty() {
                            lines.push(format!(
                                "automation of {} (clip {id}): {}",
                                target_words(after, &a.target),
                                parts.join("; ")
                            ));
                        }
                    }
                    Some(_) => {}
                }
            }
            _ => {}
        }
    }
    for (id, c) in before.clips.iter() {
        if after.clips.contains_key(id) {
            continue;
        }
        if let Some(a) = c.audio() {
            audio_removed.push(audio_clip_line(before, c, a, t));
        } else if let Some(a) = c.automation() {
            auto_removed.push(automation_clip_line(before, c, a, t));
        }
    }
    for (label, v) in [
        ("audio: added", audio_added),
        ("audio: removed", audio_removed),
        ("automation: added", auto_added),
        ("automation: removed", auto_removed),
    ] {
        match v.len() {
            0 => {}
            1..=3 => lines.push(format!("{label} {}", v.join("; "))),
            n => lines.push(format!(
                "{label} {}; ...and {} more",
                v[..3].join("; "),
                n - 3
            )),
        }
    }
}
