//! Patches: the only way a [`Project`] changes.
//!
//! A [`Patch`] replaces the value of one object (see [`ObjectPath`]) and remembers what was
//! there before. `None` means "absent": `before: None` creates an object, `after: None`
//! deletes it. Values are JSON so patches can be logged, sent to agents, diffed field by
//! field during undo, and replayed.
//!
//! Commands never mutate a project directly; they use [`Edit`], which mutates a working
//! copy *and* records the patches. Undo, redo, replay and preview all go through [`apply`].

use crate::*;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Patch {
    pub path: ObjectPath,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl Patch {
    pub fn inverse(&self) -> Patch {
        Patch {
            path: self.path,
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }
}

/// An ordered list of patches applied as a unit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct PatchSet(pub Vec<Patch>);

impl PatchSet {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Patch> {
        self.0.iter()
    }

    /// The set that exactly undoes this one (reverse order, before/after swapped). Applies
    /// cleanly only to the project state this set produced; for undo on a project that has
    /// moved on since, see the conflict-aware revert in `ph-commands`.
    pub fn inverse(&self) -> PatchSet {
        PatchSet(self.0.iter().rev().map(Patch::inverse).collect())
    }

    pub fn extend(&mut self, other: PatchSet) {
        self.0.extend(other.0);
    }

    /// Distinct object paths touched, in first-touch order.
    pub fn paths(&self) -> Vec<ObjectPath> {
        let mut seen = Vec::new();
        for p in &self.0 {
            if !seen.contains(&p.path) {
                seen.push(p.path);
            }
        }
        seen
    }
}

fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("model types always serialize")
}

fn from_value<T: DeserializeOwned>(path: ObjectPath, v: &Value) -> ModelResult<T> {
    serde_json::from_value(v.clone()).map_err(|e| ModelError::InvalidPatch(format!("{path}: {e}")))
}

/// Current value of an object, `None` if it does not exist.
pub fn get(project: &Project, path: ObjectPath) -> Option<Value> {
    match path {
        ObjectPath::Settings => Some(to_value(&project.settings)),
        ObjectPath::Channel(id) => project.channels.get(&id).map(to_value),
        ObjectPath::ChannelOrder => Some(to_value(&*project.channel_order)),
        ObjectPath::Pattern(id) => project.patterns.get(&id).map(|p| to_value(&p.header())),
        ObjectPath::PatternOrder => Some(to_value(&*project.pattern_order)),
        ObjectPath::Note(p, c, n) => project
            .patterns
            .get(&p)
            .and_then(|pat| pat.notes.get(&c))
            .and_then(|m| m.get(&n))
            .map(to_value),
        ObjectPath::Insert(i) => project.mixer.get(i.0 as usize).map(to_value),
        ObjectPath::Track(t) => project.tracks.get(t as usize).map(to_value),
        ObjectPath::Clip(id) => project.clips.get(&id).map(to_value),
    }
}

/// Set an object's value (`None` deletes), enforcing model invariants. Does not record
/// anything; use [`Edit`] or [`apply`].
pub fn set(project: &mut Project, path: ObjectPath, value: Option<&Value>) -> ModelResult<()> {
    let invalid = |msg: String| Err(ModelError::InvalidPatch(format!("{path}: {msg}")));
    if path.is_fixed() && value.is_none() {
        return invalid("object cannot be deleted".into());
    }
    match path {
        ObjectPath::Settings => {
            let s: ProjectSettings = from_value(path, value.unwrap())?;
            if !(10.0..=999.0).contains(&s.bpm) {
                return invalid(format!("bpm {} outside 10..999", s.bpm));
            }
            if s.time_signature.numerator == 0 || !s.time_signature.denominator.is_power_of_two() {
                return invalid("bad time signature".into());
            }
            project.settings = s;
        }
        ObjectPath::Channel(id) => match value {
            Some(v) => {
                let ch: Channel = from_value(path, v)?;
                if ch.id != id {
                    return invalid(format!("value has id {}", ch.id));
                }
                if ch.insert.0 as usize > MIXER_INSERTS {
                    return invalid(format!("no insert {}", ch.insert));
                }
                Arc::make_mut(&mut project.channels).insert(id, ch);
                project.next_ids.channel = project.next_ids.channel.max(id.0 + 1);
            }
            None => {
                if project.channel_order.contains(&id) {
                    return invalid("remove it from channel_order first".into());
                }
                if project
                    .patterns
                    .values()
                    .any(|p| p.notes.get(&id).is_some_and(|m| !m.is_empty()))
                {
                    return invalid("channel still has notes".into());
                }
                for pat in Arc::make_mut(&mut project.patterns).values_mut() {
                    if pat.notes.contains_key(&id) {
                        Arc::make_mut(pat).notes.remove(&id);
                    }
                }
                Arc::make_mut(&mut project.channels).remove(&id);
            }
        },
        ObjectPath::ChannelOrder => {
            let order: Vec<ChannelId> = from_value(path, value.unwrap())?;
            check_order(&order, |id| project.channels.contains_key(id)).or_else(invalid)?;
            project.channel_order = Arc::new(order);
        }
        ObjectPath::Pattern(id) => match value {
            Some(v) => {
                let h: PatternHeader = from_value(path, v)?;
                if h.id != id {
                    return invalid(format!("value has id {}", h.id));
                }
                if h.length == Some(0) {
                    return invalid("length must be > 0".into());
                }
                let pats = Arc::make_mut(&mut project.patterns);
                match pats.get_mut(&id) {
                    Some(p) => {
                        let p = Arc::make_mut(p);
                        p.name = h.name;
                        p.color = h.color;
                        p.length = h.length;
                    }
                    None => {
                        pats.insert(
                            id,
                            Arc::new(Pattern {
                                id,
                                name: h.name,
                                color: h.color,
                                length: h.length,
                                notes: Default::default(),
                            }),
                        );
                    }
                }
                project.next_ids.pattern = project.next_ids.pattern.max(id.0 + 1);
            }
            None => {
                let Some(p) = project.patterns.get(&id) else {
                    return Ok(());
                };
                if p.note_count() > 0 {
                    return invalid("pattern still has notes".into());
                }
                if project.pattern_order.contains(&id) {
                    return invalid("remove it from pattern_order first".into());
                }
                if project.clips.values().any(|c| c.pattern == id) {
                    return invalid("pattern still has playlist clips".into());
                }
                Arc::make_mut(&mut project.patterns).remove(&id);
            }
        },
        ObjectPath::PatternOrder => {
            let order: Vec<PatternId> = from_value(path, value.unwrap())?;
            check_order(&order, |id| project.patterns.contains_key(id)).or_else(invalid)?;
            project.pattern_order = Arc::new(order);
        }
        ObjectPath::Note(pid, cid, nid) => {
            if !project.channels.contains_key(&cid) {
                return Err(ModelError::NoChannel(cid));
            }
            let pats = Arc::make_mut(&mut project.patterns);
            let pat = Arc::make_mut(pats.get_mut(&pid).ok_or(ModelError::NoPattern(pid))?);
            match value {
                Some(v) => {
                    let n: Note = from_value(path, v)?;
                    if n.id != nid {
                        return invalid(format!("value has id {}", n.id));
                    }
                    validate_note(&n)?;
                    pat.notes.entry(cid).or_default().insert(nid, n);
                    project.next_ids.note = project.next_ids.note.max(nid.0 + 1);
                }
                None => {
                    if let Some(m) = pat.notes.get_mut(&cid) {
                        m.remove(&nid);
                        if m.is_empty() {
                            pat.notes.remove(&cid);
                        }
                    }
                }
            }
        }
        ObjectPath::Insert(i) => {
            let ins: MixerInsert = from_value(path, value.unwrap())?;
            let n = project.mixer.len();
            if i.0 as usize >= n {
                return Err(ModelError::NoInsert(i));
            }
            if ins.sends.iter().any(|s| s.to.0 as usize >= n || s.to == i) {
                return invalid("send to a missing insert or to itself".into());
            }
            if ins
                .sidechain_from
                .is_some_and(|s| s.0 as usize >= n || s == i)
            {
                return invalid("bad sidechain source".into());
            }
            Arc::make_mut(&mut project.mixer)[i.0 as usize] = ins;
        }
        ObjectPath::Track(t) => {
            let tr: PlaylistTrack = from_value(path, value.unwrap())?;
            let tracks = Arc::make_mut(&mut project.tracks);
            *tracks
                .get_mut(t as usize)
                .ok_or(ModelError::NoTrack(t as usize))? = tr;
        }
        ObjectPath::Clip(id) => match value {
            Some(v) => {
                let c: Clip = from_value(path, v)?;
                if c.id != id {
                    return invalid(format!("value has id {}", c.id));
                }
                if !project.patterns.contains_key(&c.pattern) {
                    return Err(ModelError::NoPattern(c.pattern));
                }
                if c.track as usize >= project.tracks.len() {
                    return Err(ModelError::NoTrack(c.track as usize));
                }
                if c.length == 0 {
                    return invalid("clip length must be > 0".into());
                }
                Arc::make_mut(&mut project.clips).insert(id, c);
                project.next_ids.clip = project.next_ids.clip.max(id.0 + 1);
            }
            None => {
                Arc::make_mut(&mut project.clips).remove(&id);
            }
        },
    }
    Ok(())
}

fn check_order<T: Ord + Copy + std::fmt::Display>(
    order: &[T],
    exists: impl Fn(&T) -> bool,
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for id in order {
        if !exists(id) {
            return Err(format!("order lists missing object {id}"));
        }
        if !seen.insert(*id) {
            return Err(format!("order lists {id} twice"));
        }
    }
    Ok(())
}

/// Applies a patch set to a copy of `project`, checking that every patch's `before` matches
/// the current value. All-or-nothing: on error the original is untouched.
pub fn apply(project: &Project, set: &PatchSet) -> ModelResult<Project> {
    let mut work = project.clone();
    for p in set.iter() {
        let current = get(&work, p.path);
        if current != p.before {
            return Err(ModelError::Conflict { path: p.path });
        }
        self::set(&mut work, p.path, p.after.as_ref())?;
    }
    Ok(work)
}

/// Records edits as patches while applying them to a working copy.
///
/// ```
/// use ph_core::*;
/// let project = Project::empty("demo");
/// let mut ed = Edit::new(&project);
/// let pat = ed.add_pattern("Beat").unwrap();
/// let kick = ed.add_channel("Kick", InstrumentSpec::Sampler {
///     sample: Some("factory:kick".into()), root_key: 60, params: Default::default() }).unwrap();
/// ed.add_note(pat, kick, NoteSpec::new(36, 0, STEP)).unwrap();
/// let (after, patches) = ed.finish();
/// assert_eq!(apply(&project, &patches).unwrap(), after);
/// let undone = apply(&after, &patches.inverse()).unwrap();
/// assert!(undone.channels.is_empty() && undone.patterns.is_empty());
/// assert_eq!(undone.next_ids, after.next_ids); // undo never frees ids
/// ```
pub struct Edit {
    work: Project,
    patches: Vec<Patch>,
}

/// A note to add; the id is assigned by [`Edit::add_note`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteSpec {
    pub key: u8,
    pub start: Tick,
    pub length: Tick,
    #[serde(default = "default_velocity")]
    pub velocity: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub fine_pitch: f32,
    /// Added muted (not played).
    #[serde(default)]
    pub muted: bool,
}

fn default_velocity() -> f32 {
    0.78
}

impl NoteSpec {
    pub fn new(key: u8, start: Tick, length: Tick) -> Self {
        Self {
            key,
            start,
            length,
            velocity: default_velocity(),
            pan: 0.0,
            fine_pitch: 0.0,
            muted: false,
        }
    }
}

impl Edit {
    pub fn new(project: &Project) -> Self {
        Self {
            work: project.clone(),
            patches: Vec::new(),
        }
    }

    /// The working copy, including every edit made so far.
    pub fn project(&self) -> &Project {
        &self.work
    }

    pub fn finish(self) -> (Project, PatchSet) {
        (self.work, PatchSet(self.patches))
    }

    /// Low-level: set any object's value. No-op when the value is unchanged.
    pub fn put(&mut self, path: ObjectPath, after: Option<Value>) -> ModelResult<()> {
        let before = get(&self.work, path);
        if before == after {
            return Ok(());
        }
        set(&mut self.work, path, after.as_ref())?;
        self.patches.push(Patch {
            path,
            before,
            after,
        });
        Ok(())
    }

    fn put_typed<T: Serialize>(&mut self, path: ObjectPath, after: Option<&T>) -> ModelResult<()> {
        self.put(path, after.map(to_value))
    }

    // ---------------------------------------------------------------- settings

    pub fn update_settings(&mut self, f: impl FnOnce(&mut ProjectSettings)) -> ModelResult<()> {
        let mut s = self.work.settings.clone();
        f(&mut s);
        self.put_typed(ObjectPath::Settings, Some(&s))
    }

    // ---------------------------------------------------------------- channels

    pub fn add_channel(
        &mut self,
        name: &str,
        instrument: InstrumentSpec,
    ) -> ModelResult<ChannelId> {
        let id = ChannelId(self.work.next_ids.channel);
        let ch = Channel {
            id,
            name: name.into(),
            color: default_color(id.0),
            instrument,
            volume: 0.78,
            pan: 0.0,
            muted: false,
            insert: InsertId(0),
        };
        self.put_typed(ObjectPath::Channel(id), Some(&ch))?;
        let mut order = (*self.work.channel_order).clone();
        order.push(id);
        self.put_typed(ObjectPath::ChannelOrder, Some(&order))?;
        Ok(id)
    }

    pub fn update_channel(
        &mut self,
        id: ChannelId,
        f: impl FnOnce(&mut Channel),
    ) -> ModelResult<()> {
        let mut ch = self.work.channel(id)?.clone();
        f(&mut ch);
        ch.id = id;
        self.put_typed(ObjectPath::Channel(id), Some(&ch))
    }

    /// Removes the channel and all of its notes in every pattern.
    pub fn remove_channel(&mut self, id: ChannelId) -> ModelResult<()> {
        self.work.channel(id)?;
        let notes: Vec<(PatternId, NoteId)> = self
            .work
            .patterns
            .values()
            .flat_map(|p| {
                p.notes
                    .get(&id)
                    .into_iter()
                    .flat_map(move |m| m.keys().map(move |n| (p.id, *n)))
            })
            .collect();
        for (p, n) in notes {
            self.put(ObjectPath::Note(p, id, n), None)?;
        }
        let order: Vec<ChannelId> = self
            .work
            .channel_order
            .iter()
            .copied()
            .filter(|c| *c != id)
            .collect();
        self.put_typed(ObjectPath::ChannelOrder, Some(&order))?;
        self.put(ObjectPath::Channel(id), None)
    }

    /// Moves a channel to `index` in the rack order (clamped).
    pub fn move_channel(&mut self, id: ChannelId, index: usize) -> ModelResult<()> {
        self.work.channel(id)?;
        let mut order: Vec<ChannelId> = self
            .work
            .channel_order
            .iter()
            .copied()
            .filter(|c| *c != id)
            .collect();
        order.insert(index.min(order.len()), id);
        self.put_typed(ObjectPath::ChannelOrder, Some(&order))
    }

    // ---------------------------------------------------------------- patterns

    pub fn add_pattern(&mut self, name: &str) -> ModelResult<PatternId> {
        let id = PatternId(self.work.next_ids.pattern);
        let h = PatternHeader {
            id,
            name: name.into(),
            color: default_color(id.0),
            length: None,
        };
        self.put_typed(ObjectPath::Pattern(id), Some(&h))?;
        let mut order = (*self.work.pattern_order).clone();
        order.push(id);
        self.put_typed(ObjectPath::PatternOrder, Some(&order))?;
        Ok(id)
    }

    pub fn update_pattern(
        &mut self,
        id: PatternId,
        f: impl FnOnce(&mut PatternHeader),
    ) -> ModelResult<()> {
        let mut h = self.work.pattern(id)?.header();
        f(&mut h);
        h.id = id;
        self.put_typed(ObjectPath::Pattern(id), Some(&h))
    }

    /// Removes the pattern, its notes and every playlist clip that uses it.
    pub fn remove_pattern(&mut self, id: PatternId) -> ModelResult<()> {
        let pat = self.work.pattern(id)?;
        let notes: Vec<(ChannelId, NoteId)> = pat
            .notes
            .iter()
            .flat_map(|(c, m)| m.keys().map(move |n| (*c, *n)))
            .collect();
        let clips: Vec<ClipId> = self
            .work
            .clips
            .values()
            .filter(|c| c.pattern == id)
            .map(|c| c.id)
            .collect();
        for (c, n) in notes {
            self.put(ObjectPath::Note(id, c, n), None)?;
        }
        for c in clips {
            self.put(ObjectPath::Clip(c), None)?;
        }
        let order: Vec<PatternId> = self
            .work
            .pattern_order
            .iter()
            .copied()
            .filter(|p| *p != id)
            .collect();
        self.put_typed(ObjectPath::PatternOrder, Some(&order))?;
        self.put(ObjectPath::Pattern(id), None)
    }

    pub fn move_pattern(&mut self, id: PatternId, index: usize) -> ModelResult<()> {
        self.work.pattern(id)?;
        let mut order: Vec<PatternId> = self
            .work
            .pattern_order
            .iter()
            .copied()
            .filter(|p| *p != id)
            .collect();
        order.insert(index.min(order.len()), id);
        self.put_typed(ObjectPath::PatternOrder, Some(&order))
    }

    // ---------------------------------------------------------------- notes

    pub fn add_note(
        &mut self,
        pattern: PatternId,
        channel: ChannelId,
        spec: NoteSpec,
    ) -> ModelResult<NoteId> {
        let id = NoteId(self.work.next_ids.note);
        let note = Note {
            id,
            key: spec.key,
            start: spec.start,
            length: spec.length,
            velocity: spec.velocity,
            pan: spec.pan,
            fine_pitch: spec.fine_pitch,
            muted: spec.muted,
        };
        self.put_typed(ObjectPath::Note(pattern, channel, id), Some(&note))?;
        Ok(id)
    }

    pub fn note(&self, pattern: PatternId, channel: ChannelId, id: NoteId) -> ModelResult<&Note> {
        self.work
            .pattern(pattern)?
            .notes
            .get(&channel)
            .and_then(|m| m.get(&id))
            .ok_or(ModelError::NoNote(id))
    }

    pub fn update_note(
        &mut self,
        pattern: PatternId,
        channel: ChannelId,
        id: NoteId,
        f: impl FnOnce(&mut Note),
    ) -> ModelResult<()> {
        let mut n = self.note(pattern, channel, id)?.clone();
        f(&mut n);
        n.id = id;
        self.put_typed(ObjectPath::Note(pattern, channel, id), Some(&n))
    }

    pub fn remove_note(
        &mut self,
        pattern: PatternId,
        channel: ChannelId,
        id: NoteId,
    ) -> ModelResult<()> {
        self.note(pattern, channel, id)?;
        self.put(ObjectPath::Note(pattern, channel, id), None)
    }

    // ---------------------------------------------------------------- mixer & playlist

    pub fn update_insert(
        &mut self,
        id: InsertId,
        f: impl FnOnce(&mut MixerInsert),
    ) -> ModelResult<()> {
        let mut ins = self.work.insert(id)?.clone();
        f(&mut ins);
        self.put_typed(ObjectPath::Insert(id), Some(&ins))
    }

    pub fn update_track(
        &mut self,
        track: u32,
        f: impl FnOnce(&mut PlaylistTrack),
    ) -> ModelResult<()> {
        let mut t = self
            .work
            .tracks
            .get(track as usize)
            .ok_or(ModelError::NoTrack(track as usize))?
            .clone();
        f(&mut t);
        self.put_typed(ObjectPath::Track(track), Some(&t))
    }

    /// Places a pattern clip; its length defaults to the pattern's current length.
    pub fn add_clip(&mut self, track: u32, pattern: PatternId, start: Tick) -> ModelResult<ClipId> {
        let length = self.work.pattern_length(pattern)?;
        let id = ClipId(self.work.next_ids.clip);
        let clip = Clip {
            id,
            track,
            pattern,
            start,
            length,
            offset: 0,
        };
        self.put_typed(ObjectPath::Clip(id), Some(&clip))?;
        Ok(id)
    }

    pub fn update_clip(&mut self, id: ClipId, f: impl FnOnce(&mut Clip)) -> ModelResult<()> {
        let mut c = self.work.clip(id)?.clone();
        f(&mut c);
        c.id = id;
        self.put_typed(ObjectPath::Clip(id), Some(&c))
    }

    pub fn remove_clip(&mut self, id: ClipId) -> ModelResult<()> {
        self.work.clip(id)?;
        self.put(ObjectPath::Clip(id), None)
    }
}
