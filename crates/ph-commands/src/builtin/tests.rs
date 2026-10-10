use crate::*;
use ph_core::*;
use serde_json::json;
use std::sync::Arc;

fn session() -> (Session, NullHost) {
    (
        Session::new(Arc::new(Registry::with_builtins()), Project::empty("t")),
        NullHost::default(),
    )
}

fn run(
    s: &mut Session,
    h: &mut NullHost,
    who: &Caller,
    cmd: &str,
    params: serde_json::Value,
) -> Executed {
    s.execute(who, Request::new(cmd, params), h).unwrap()
}

fn beat(s: &mut Session, h: &mut NullHost, who: &Caller) -> (u32, u32) {
    let pat = run(s, h, who, "pattern.add", json!({}));
    let ch = run(
        s,
        h,
        who,
        "channel.add",
        json!({ "name": "Kick", "instrument": { "kind": "sampler", "sample": "factory:kick", "root_key": 60 } }),
    );
    (
        pat.result["pattern"].as_u64().unwrap() as u32,
        ch.result["channel"].as_u64().unwrap() as u32,
    )
}

#[test]
fn every_command_has_an_object_schema() {
    for spec in Registry::with_builtins().list("") {
        assert_eq!(spec.params["type"], "object", "{}", spec.id);
    }
}

#[test]
fn batch_is_one_transaction_and_reports_changes() {
    let (mut s, mut h) = session();
    let human = Caller::human();
    let (pat, ch) = beat(&mut s, &mut h, &human);
    let notes = (0..4)
        .map(|i| json!({ "key": 36, "start": i * 960, "length": 240 }))
        .collect::<Vec<_>>();
    let out = s
        .batch(
            &human,
            vec![
                Request::new(
                    "note.add",
                    json!({ "pattern": pat, "channel": ch, "notes": notes }),
                ),
                Request::new("project.set_tempo", json!({ "bpm": 92 })),
            ],
            &mut h,
        )
        .unwrap();
    assert_eq!(out[0].txn, out[1].txn);
    assert_eq!(out[0].summary.as_deref(), Some("added 4 note(s) to Kick"));
    assert_eq!(s.project().settings.bpm, 92.0);
    assert_eq!(s.history().len(), 3);
}

#[test]
fn failed_batch_changes_nothing() {
    let (mut s, mut h) = session();
    let human = Caller::human();
    let before = s.project().clone();
    let err = s.batch(
        &human,
        vec![
            Request::new("project.set_tempo", json!({ "bpm": 100 })),
            Request::new("project.set_tempo", json!({ "bpm": 5000 })),
        ],
        &mut h,
    );
    assert!(err.is_err());
    assert_eq!(s.project(), &before);
    assert!(s.history().is_empty());
}

#[test]
fn scopes_are_enforced() {
    let (mut s, mut h) = session();
    let reader = Caller::agent("viewer", Grant(vec![Scope::Read]));
    assert!(matches!(
        s.execute(
            &reader,
            Request::new("project.set_tempo", json!({ "bpm": 100 })),
            &mut h
        ),
        Err(CmdError::Scope { .. })
    ));
    assert!(
        s.execute(
            &reader,
            Request::new("project.get_settings", json!({})),
            &mut h
        )
        .is_ok()
    );
}

#[test]
fn bad_params_are_reported_not_panicked() {
    let (mut s, mut h) = session();
    let r = s.execute(
        &Caller::human(),
        Request::new("project.set_tempo", json!({ "tempo": 100 })),
        &mut h,
    );
    assert!(matches!(r, Err(CmdError::BadParams { .. })));
}

#[test]
fn per_author_undo_leaves_other_authors_alone() {
    let (mut s, mut h) = session();
    let human = Caller::human();
    let claude = Caller::agent("claude", Grant(vec![Scope::Compose]));
    let (pat, ch) = beat(&mut s, &mut h, &human);
    run(
        &mut s,
        &mut h,
        &claude,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": [{ "key": 36, "start": 0, "length": 240 }] }),
    );
    run(
        &mut s,
        &mut h,
        &human,
        "project.set_tempo",
        json!({ "bpm": 150 }),
    );

    s.undo(&claude.author).unwrap().unwrap();
    assert_eq!(
        s.project().pattern(PatternId(pat)).unwrap().note_count(),
        0,
        "agent's note undone"
    );
    assert_eq!(s.project().settings.bpm, 150.0, "human's later edit kept");

    s.redo(&claude.author).unwrap().unwrap();
    assert_eq!(s.project().pattern(PatternId(pat)).unwrap().note_count(), 1);
    // Ctrl+Z after redo undoes the same work again, not something older.
    s.undo(&claude.author).unwrap().unwrap();
    assert_eq!(s.project().pattern(PatternId(pat)).unwrap().note_count(), 0);
    assert_eq!(s.project().channels.len(), 1, "human's channel untouched");
}

#[test]
fn conflicting_undo_stops_and_names_the_object() {
    let (mut s, mut h) = session();
    let human = Caller::human();
    let claude = Caller::agent("claude", Grant(vec![Scope::Compose]));
    run(
        &mut s,
        &mut h,
        &claude,
        "project.set_tempo",
        json!({ "bpm": 100 }),
    );
    run(
        &mut s,
        &mut h,
        &human,
        "project.set_tempo",
        json!({ "bpm": 120 }),
    );
    match s.undo(&claude.author) {
        Err(CmdError::UndoConflict(c)) => {
            assert_eq!(c[0].path, ObjectPath::Settings);
            assert_eq!(c[0].changed_by, vec![Author::Human]);
        }
        other => panic!("expected conflict, got {other:?}"),
    }
    assert_eq!(s.project().settings.bpm, 120.0);
}
