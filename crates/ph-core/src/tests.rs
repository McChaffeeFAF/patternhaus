use crate::*;
use std::sync::Arc;

fn synth() -> InstrumentSpec {
    InstrumentSpec::Synth {
        preset: None,
        params: Default::default(),
    }
}

/// A small project built through `Edit`, plus the patches that built it.
fn demo() -> (Project, Project, PatchSet, PatternId, ChannelId) {
    let base = Project::empty("t");
    let mut ed = Edit::new(&base);
    let pat = ed.add_pattern("Beat").unwrap();
    let ch = ed.add_channel("Bass", synth()).unwrap();
    ed.add_note(pat, ch, NoteSpec::new(36, 0, STEP)).unwrap();
    ed.add_note(pat, ch, NoteSpec::new(43, STEP * 4, STEP))
        .unwrap();
    ed.add_clip(0, pat, 0).unwrap();
    ed.update_settings(|s| s.bpm = 140.0).unwrap();
    let (after, patches) = ed.finish();
    (base, after, patches, pat, ch)
}

#[test]
fn edit_patches_replay_and_invert() {
    let (base, after, patches, _, _) = demo();
    assert_eq!(apply(&base, &patches).unwrap(), after);
    assert_eq!(
        apply(&after, &patches.inverse()).unwrap(),
        base.with_ids_of(&after)
    );
}

impl Project {
    /// Undo never frees ids, so compare against the base with the later id counters.
    fn with_ids_of(mut self, other: &Project) -> Project {
        self.next_ids = other.next_ids.clone();
        self
    }
}

#[test]
fn snapshots_share_untouched_parts() {
    let (_, a, _, pat, ch) = demo();
    let mut ed = Edit::new(&a);
    ed.add_note(pat, ch, NoteSpec::new(48, 0, STEP)).unwrap();
    let (b, _) = ed.finish();
    assert!(Arc::ptr_eq(&a.mixer, &b.mixer));
    assert!(Arc::ptr_eq(&a.channels, &b.channels));
    assert!(!Arc::ptr_eq(&a.patterns, &b.patterns));
    assert_eq!(a.pattern(pat).unwrap().note_count(), 2);
    assert_eq!(b.pattern(pat).unwrap().note_count(), 3);
}

#[test]
fn unchanged_put_records_nothing() {
    let (_, a, _, _, _) = demo();
    let mut ed = Edit::new(&a);
    ed.update_settings(|s| s.bpm = 140.0).unwrap();
    assert!(ed.finish().1.is_empty());
}

#[test]
fn apply_detects_conflicts() {
    let (_, a, _, pat, ch) = demo();
    let note = *a.pattern(pat).unwrap().notes[&ch].keys().next().unwrap();
    let mut e1 = Edit::new(&a);
    e1.update_note(pat, ch, note, |n| n.key = 40).unwrap();
    let (_, p1) = e1.finish();
    let mut e2 = Edit::new(&a);
    e2.update_note(pat, ch, note, |n| n.velocity = 0.2).unwrap();
    let (b, _) = e2.finish();
    assert_eq!(
        apply(&b, &p1),
        Err(ModelError::Conflict {
            path: ObjectPath::Note(pat, ch, note)
        })
    );
}

#[test]
fn removing_channel_records_note_deletions_so_undo_restores_them() {
    let (_, a, _, pat, ch) = demo();
    let mut ed = Edit::new(&a);
    ed.remove_channel(ch).unwrap();
    let (b, patches) = ed.finish();
    assert!(b.patterns[&pat].notes.is_empty());
    assert!(b.channel(ch).is_err());
    assert_eq!(apply(&b, &patches.inverse()).unwrap(), a);
}

#[test]
fn removing_pattern_removes_its_clips() {
    let (_, a, _, pat, _) = demo();
    let mut ed = Edit::new(&a);
    ed.remove_pattern(pat).unwrap();
    let (b, patches) = ed.finish();
    assert!(b.clips.is_empty());
    assert_eq!(apply(&b, &patches.inverse()).unwrap(), a);
}

#[test]
fn ids_are_never_reused_after_undo() {
    let (_, a, _, pat, ch) = demo();
    let mut ed = Edit::new(&a);
    let n1 = ed.add_note(pat, ch, NoteSpec::new(60, 0, STEP)).unwrap();
    let (b, patches) = ed.finish();
    let undone = apply(&b, &patches.inverse()).unwrap();
    let mut ed = Edit::new(&undone);
    let n2 = ed.add_note(pat, ch, NoteSpec::new(60, 0, STEP)).unwrap();
    assert_ne!(n1, n2);
}

#[test]
fn invariants_are_enforced() {
    let (_, a, _, pat, ch) = demo();
    let mut ed = Edit::new(&a);
    assert!(
        ed.add_note(pat, ChannelId(99), NoteSpec::new(60, 0, STEP))
            .is_err()
    );
    assert!(ed.add_note(pat, ch, NoteSpec::new(200, 0, STEP)).is_err());
    assert!(ed.add_note(pat, ch, NoteSpec::new(60, 0, 0)).is_err());
    assert!(ed.update_settings(|s| s.bpm = 0.0).is_err());
    assert!(
        ed.update_insert(InsertId(1), |i| i.sends.push(Send {
            to: InsertId(1),
            level: 1.0
        }))
        .is_err()
    );
    assert!(ed.put(ObjectPath::Settings, None).is_err());
    // a failed edit leaves no patch behind
    assert!(ed.finish().1.is_empty());
}

#[test]
fn object_paths_round_trip_as_strings() {
    for p in [
        ObjectPath::Settings,
        ObjectPath::Channel(ChannelId(3)),
        ObjectPath::ChannelOrder,
        ObjectPath::Pattern(PatternId(2)),
        ObjectPath::PatternOrder,
        ObjectPath::Note(PatternId(1), ChannelId(2), NoteId(1532)),
        ObjectPath::Insert(InsertId(4)),
        ObjectPath::Track(0),
        ObjectPath::Clip(ClipId(7)),
    ] {
        assert_eq!(p.to_string().parse::<ObjectPath>().unwrap(), p);
        assert_eq!(
            serde_json::to_value(p).unwrap(),
            serde_json::Value::String(p.to_string())
        );
    }
    assert!("note/1/2".parse::<ObjectPath>().is_err());
}

#[test]
fn project_and_patches_round_trip_through_json() {
    let (_, a, patches, _, _) = demo();
    let back: Project = serde_json::from_str(&serde_json::to_string_pretty(&a).unwrap()).unwrap();
    assert_eq!(a, back);
    let back: PatchSet = serde_json::from_str(&serde_json::to_string(&patches).unwrap()).unwrap();
    assert_eq!(patches, back);
}

#[test]
fn pattern_length_rounds_to_bar() {
    let (_, a, _, pat, ch) = demo();
    assert_eq!(a.pattern_length(pat).unwrap(), PPQ * 4);
    let mut ed = Edit::new(&a);
    ed.add_note(pat, ch, NoteSpec::new(60, PPQ * 5, STEP))
        .unwrap();
    assert_eq!(ed.project().pattern_length(pat).unwrap(), PPQ * 8);
}

#[test]
fn note_muted_is_additive_and_migration_safe() {
    // a note written before `muted` existed loads as not muted
    let old = r#"{"id":3,"key":60,"start":0,"length":240,"velocity":0.5}"#;
    let n: Note = serde_json::from_str(old).unwrap();
    assert!(!n.muted);
    // an unmuted note serialises exactly as it did before the field existed
    let text = serde_json::to_string(&n).unwrap();
    assert!(!text.contains("muted"), "{text}");
    // a muted one round-trips
    let m = Note { muted: true, ..n };
    let back: Note = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    assert_eq!(back, m);
    // and a spec defaults to unmuted
    let spec: NoteSpec = serde_json::from_str(r#"{"key":60,"start":0,"length":240}"#).unwrap();
    assert!(!spec.muted);
    // the field survives an Edit and an undo of it
    let (_, a, _, pat, ch) = demo();
    let id = *a.pattern(pat).unwrap().notes[&ch].keys().next().unwrap();
    let mut ed = Edit::new(&a);
    ed.update_note(pat, ch, id, |n| n.muted = true).unwrap();
    let (b, patches) = ed.finish();
    assert!(b.pattern(pat).unwrap().notes[&ch][&id].muted);
    let again = crate::patch::apply(&b, &patches.inverse()).unwrap();
    assert_eq!(again, a);
}
