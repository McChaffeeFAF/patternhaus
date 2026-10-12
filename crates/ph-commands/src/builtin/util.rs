//! Helpers shared by the command modules.

use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Parameters of a command that takes none.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// A value the model would refuse, reported to the caller as a bad request.
pub fn invalid<T>(msg: impl Into<String>) -> CmdResult<T> {
    Err(CmdError::Model(ModelError::Invalid(msg.into())))
}

/// Checks `lo <= v <= hi`.
pub fn range(name: &str, v: f32, lo: f32, hi: f32) -> CmdResult<()> {
    if (lo..=hi).contains(&v) {
        Ok(())
    } else {
        invalid(format!("{name} {v} is outside {lo}..{hi}"))
    }
}

/// The mixer insert `i`, which must exist.
pub fn insert(p: &Project, i: u32) -> CmdResult<InsertId> {
    p.insert(InsertId(i))?;
    Ok(InsertId(i))
}

/// A channel's name, for summaries.
pub fn channel_name(p: &Project, id: ChannelId) -> CmdResult<String> {
    Ok(p.channel(id)?.name.clone())
}

/// What a clip plays, for summaries (see [`Project::clip_label`]).
pub fn clip_label(p: &Project, clip: &Clip) -> String {
    p.clip_label(clip)
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Notes of one channel in a pattern, in id order.
pub fn notes_of(p: &Project, pat: PatternId, ch: ChannelId) -> CmdResult<Vec<Note>> {
    Ok(p.pattern(pat)?
        .notes
        .get(&ch)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default())
}

/// Resolves `ids` (or every note of the channel when `None`) to notes, failing on unknown ids.
pub fn pick_notes(
    p: &Project,
    pat: PatternId,
    ch: ChannelId,
    ids: Option<&[NoteId]>,
) -> CmdResult<Vec<Note>> {
    p.channel(ch)?;
    let all = notes_of(p, pat, ch)?;
    match ids {
        None => Ok(all),
        Some(ids) => ids
            .iter()
            .map(|id| {
                all.iter()
                    .find(|n| n.id == *id)
                    .cloned()
                    .ok_or(CmdError::Model(ModelError::NoNote(*id)))
            })
            .collect(),
    }
}

/// Changes `project` into `target` with the fewest recorded edits, in an order the model
/// accepts (notes and clips go before the things that hold them, orders are trimmed before
/// removal and set last). Used by `project.new`.
pub fn replace_project(ed: &mut Edit, target: &Project) -> ModelResult<()> {
    let cur = ed.project().clone();
    for pat in cur.patterns.values() {
        for (c, notes) in &pat.notes {
            for n in notes.keys() {
                let path = ObjectPath::Note(pat.id, *c, *n);
                if get(target, path).is_none() {
                    ed.put(path, None)?;
                }
            }
        }
    }
    for (cid, pid) in points_of(&cur) {
        let path = ObjectPath::Point(cid, pid);
        if get(target, path).is_none() {
            ed.put(path, None)?;
        }
    }
    for id in cur.clips.keys() {
        if !target.clips.contains_key(id) {
            ed.put(ObjectPath::Clip(*id), None)?;
        }
    }
    for id in cur.samples.keys() {
        if !target.samples.contains_key(id) {
            ed.put(ObjectPath::Sample(*id), None)?;
        }
    }
    let keep_c: Vec<ChannelId> = cur
        .channel_order
        .iter()
        .copied()
        .filter(|c| target.channels.contains_key(c))
        .collect();
    ed.put(ObjectPath::ChannelOrder, Some(json!(keep_c)))?;
    let keep_p: Vec<PatternId> = cur
        .pattern_order
        .iter()
        .copied()
        .filter(|p| target.patterns.contains_key(p))
        .collect();
    ed.put(ObjectPath::PatternOrder, Some(json!(keep_p)))?;
    for id in cur.channels.keys() {
        if !target.channels.contains_key(id) {
            ed.put(ObjectPath::Channel(*id), None)?;
        }
    }
    for id in cur.patterns.keys() {
        if !target.patterns.contains_key(id) {
            ed.put(ObjectPath::Pattern(*id), None)?;
        }
    }
    for id in target.channels.keys() {
        let path = ObjectPath::Channel(*id);
        ed.put(path, get(target, path))?;
    }
    for id in target.patterns.keys() {
        let path = ObjectPath::Pattern(*id);
        ed.put(path, get(target, path))?;
    }
    ed.put(
        ObjectPath::ChannelOrder,
        get(target, ObjectPath::ChannelOrder),
    )?;
    ed.put(
        ObjectPath::PatternOrder,
        get(target, ObjectPath::PatternOrder),
    )?;
    ed.put(ObjectPath::Settings, get(target, ObjectPath::Settings))?;
    for i in 0..target.mixer.len() as u32 {
        let path = ObjectPath::Insert(InsertId(i));
        ed.put(path, get(target, path))?;
    }
    for t in 0..target.tracks.len() as u32 {
        let path = ObjectPath::Track(t);
        ed.put(path, get(target, path))?;
    }
    for pat in target.patterns.values() {
        for (c, notes) in &pat.notes {
            for n in notes.keys() {
                let path = ObjectPath::Note(pat.id, *c, *n);
                ed.put(path, get(target, path))?;
            }
        }
    }
    for id in target.samples.keys() {
        let path = ObjectPath::Sample(*id);
        ed.put(path, get(target, path))?;
    }
    for id in target.clips.keys() {
        let path = ObjectPath::Clip(*id);
        ed.put(path, get(target, path))?;
    }
    for (cid, pid) in points_of(target) {
        let path = ObjectPath::Point(cid, pid);
        ed.put(path, get(target, path))?;
    }
    Ok(())
}

/// Every automation point, as (clip, point).
fn points_of(p: &Project) -> Vec<(ClipId, PointId)> {
    p.clips
        .values()
        .filter_map(|c| c.automation().map(|a| (c.id, a)))
        .flat_map(|(c, a)| a.points.keys().map(move |p| (c, *p)))
        .collect()
}
