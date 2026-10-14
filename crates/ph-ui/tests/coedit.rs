//! M1 criterion 3, seen from the UI: a scripted agent edits while the human does, per-author
//! undo reverts only the agent's work, and a conflicting undo stops with the dialog.

mod common;

use common::*;
use ph_core::ObjectPath;
use ph_ui::core::ConflictChoice;
use ph_ui::demo_agent;
use ph_ui::mapping;

fn note_count(app: &ph_ui::PhApp, ids: &Ids) -> usize {
    app.core
        .project
        .pattern(ids.beat)
        .map(|p| p.notes.get(&ids.kick).map_or(0, |m| m.len()))
        .unwrap_or(0)
}

#[test]
fn undo_all_by_agent_leaves_my_work() {
    let (mut fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let app = &mut fx.app;
    pump_until(app, 5000, |a| a.core.project.channels.len() == 4);
    let base = note_count(app, &ids);
    assert_eq!(base, 4, "kick row from the beat");

    // human adds a note through the UI path (mapping -> dispatcher -> session)
    app.core.send(mapping::note_add(
        ids.beat,
        ids.kick,
        &[mapping::NewNote {
            key: 60,
            start: 3600,
            length: 240,
            velocity: 0.9,
            muted: false,
        }],
    ));
    settle(app);
    assert_eq!(note_count(app, &ids), base + 1);

    // the agent edits too (first pattern, first channel = beat/kick)
    for k in 0..3 {
        assert!(demo_agent::step(&app.core.studio.session, k).contains(&"note.add".to_string()));
    }
    settle(app);
    assert_eq!(note_count(app, &ids), base + 1 + 3);
    // its notes flash and carry a badge; its ui.select highlights the note
    assert!(!app.core.flashes.is_empty());
    assert!(app.core.flashes.iter().all(|f| f.author == "agent:demo"));
    assert!(
        app.core
            .touches
            .keys()
            .any(|p| matches!(p, ObjectPath::Note(..)))
    );
    assert!(
        app.core
            .agent_select
            .iter()
            .any(|p| matches!(p, ObjectPath::Note(..)))
    );
    // history shows both authors
    pump_until(app, 500, |a| {
        a.core.authors().contains(&"agent:demo".to_string())
    });
    assert_eq!(app.core.authors(), vec!["agent:demo".to_string()]);

    // "Undo all by agent:demo": only the agent's notes disappear
    app.core.send(mapping::undo_author("agent:demo", "stop"));
    settle(app);
    assert_eq!(note_count(app, &ids), base + 1, "my note stays");
    assert!(app.core.conflict.is_none());

    // plain Undo reverts MY last change, not the agent's undo entries
    app.core.send(mapping::undo_mine());
    settle(app);
    assert_eq!(note_count(app, &ids), base);
}

/// Agent adds a note, the human then edits that note, and the human asks to undo the agent.
fn conflicted() -> (Fixture, Ids, ph_core::NoteId) {
    let (mut fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let app = &mut fx.app;
    pump_until(app, 5000, |a| a.core.project.channels.len() == 4);
    demo_agent::step(&app.core.studio.session, 0);
    settle(app);
    let note = app.core.project.pattern(ids.beat).unwrap().notes[&ids.kick]
        .values()
        .find(|n| n.key == 48)
        .expect("agent's note")
        .id;
    app.core
        .send(mapping::note_velocity(ids.beat, ids.kick, &[note], 0.25));
    settle(app);
    app.core.send(mapping::undo_author("agent:demo", "stop"));
    settle(app);
    (fx, ids, note)
}

fn has(app: &ph_ui::PhApp, ids: &Ids, n: ph_core::NoteId) -> bool {
    app.core.project.pattern(ids.beat).unwrap().notes[&ids.kick].contains_key(&n)
}

#[test]
fn a_conflicting_undo_stops_and_cancel_changes_nothing() {
    let (mut fx, ids, note) = conflicted();
    let app = &mut fx.app;
    // undoing the agent would overwrite the human's velocity edit: undo stopped
    let dialog = app.core.conflict.clone().expect("conflict dialog is open");
    assert_eq!(dialog.request.command, "history.undo_author");
    assert!(
        dialog
            .conflicts
            .iter()
            .any(|c| c.path == ObjectPath::Note(ids.beat, ids.kick, note))
    );
    assert!(
        dialog
            .conflicts
            .iter()
            .any(|c| c.changed_by.iter().any(|a| a.to_string() == "human"))
    );
    assert!(has(app, &ids, note), "nothing changed yet");
    app.core.resolve_conflict(ConflictChoice::Cancel);
    settle(app);
    assert!(app.core.conflict.is_none());
    assert!(has(app, &ids, note));
    // the human's edit survived
    assert!(
        (app.core.project.pattern(ids.beat).unwrap().notes[&ids.kick][&note].velocity - 0.25).abs()
            < 1e-6
    );
}

#[test]
fn skip_leaves_the_conflicting_object_alone() {
    let (mut fx, ids, note) = conflicted();
    let app = &mut fx.app;
    assert!(app.core.conflict.is_some());
    app.core.resolve_conflict(ConflictChoice::Skip);
    settle(app);
    assert!(app.core.conflict.is_none());
    assert!(has(app, &ids, note), "skip keeps it");
}

#[test]
fn force_overwrites_the_conflicting_object() {
    let (mut fx, ids, note) = conflicted();
    let app = &mut fx.app;
    assert!(app.core.conflict.is_some());
    app.core.resolve_conflict(ConflictChoice::Force);
    settle(app);
    assert!(app.core.conflict.is_none());
    assert!(!has(app, &ids, note), "force removes it");
}

#[test]
fn undoing_a_single_transaction_from_the_history() {
    let (mut fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let app = &mut fx.app;
    pump_until(app, 5000, |a| a.core.project.channels.len() == 4);
    demo_agent::step(&app.core.studio.session, 1);
    demo_agent::step(&app.core.studio.session, 2);
    settle(app);
    pump_until(app, 500, |a| {
        a.core
            .history
            .iter()
            .any(|e| e.author.to_string() == "agent:demo")
    });
    let before = note_count(app, &ids);
    let agent_txns: Vec<u64> = app
        .core
        .history
        .iter()
        .filter(|e| {
            e.author.to_string() == "agent:demo"
                && e.commands.iter().any(|c| c.command == "note.add")
        })
        .map(|e| e.txn)
        .collect();
    assert_eq!(agent_txns.len(), 2);
    app.core.send(mapping::undo_txn(agent_txns[0], "stop"));
    settle(app);
    assert_eq!(note_count(app, &ids), before - 1);
}
