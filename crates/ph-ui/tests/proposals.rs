//! Agent preview mode in the UI (M2/T9): an agent's proposal shows as ghost notes and clips and
//! an Accept / Reject strip; Accept lands it as one undo step attributed to the agent, Reject
//! changes nothing, and a proposal made stale by an edit can only be dismissed. Real egui
//! events offscreen; every wait settles on the session's own state, never on frame counts.

mod common;

use common::*;
use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_commands::{Author, Caller, Grant, ProposalStatus, Request};
use ph_ui::PhApp;
use ph_ui::mapping;
use ph_ui::proposals;
use serde_json::json;
use std::time::Duration;

type H = Harness<'static, PhApp>;

fn harness(app: PhApp) -> H {
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    for _ in 0..4 {
        h.step();
    }
    h
}

fn start() -> (H, Ids, tempfile::TempDir) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.bassline);
    app.core.sel.channel = Some(ids.bass);
    app.core.fix_selection();
    (harness(app), ids, fx.dir)
}

/// Steps until `done` holds and every command the UI sent has been answered.
fn until(h: &mut H, what: &str, mut done: impl FnMut(&PhApp) -> bool) {
    let t0 = std::time::Instant::now();
    loop {
        h.step();
        let app = h.state_mut();
        app.core.update(app.core.now + 0.01);
        if app.core.dispatch.in_flight() == 0 && done(app) {
            return;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}; toasts: {:?}; proposals: {:?}",
            app.core
                .toasts
                .iter()
                .map(|t| t.text.clone())
                .collect::<Vec<_>>(),
            app.core
                .proposals
                .iter()
                .map(|p| (p.id, p.status.clone(), p.stale.clone()))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(6));
    }
}

fn agent() -> Caller {
    Caller::agent("muse", Grant::full())
}

fn notes_in(app: &PhApp, ids: &Ids) -> usize {
    app.core
        .project
        .pattern(ids.bassline)
        .map_or(0, |p| p.notes.get(&ids.bass).map_or(0, |m| m.len()))
}

/// The agent proposes four bass notes in the bassline and a clip of the beat in the playlist.
fn propose(app: &PhApp, ids: &Ids) -> u64 {
    let notes: Vec<_> = (0..4)
        .map(|i| json!({ "key": 41 + i * 2, "start": 960 + i * 960, "length": 480 }))
        .collect();
    let out = app
        .core
        .studio
        .session
        .execute(
            &agent(),
            Request::new(
                "proposal.submit",
                json!({ "title": "Fill the bridge", "commands": [
                    { "id": "note.add", "params": { "pattern": ids.bassline.0, "channel": ids.bass.0, "notes": notes } },
                    { "id": "playlist.add_clip", "params": { "track": 3, "pattern": ids.beat.0, "start": 15360 } } ] }),
            ),
        )
        .unwrap();
    out.result["proposal"].as_u64().unwrap()
}

fn pending(app: &PhApp, id: u64) -> bool {
    app.core.proposals.iter().any(|p| p.id == id && p.pending())
}

#[test]
fn a_proposal_shows_as_ghosts_and_accept_lands_it_as_the_agents_undo_step() {
    let (mut h, ids, _dir) = start();
    only(&mut h, &[ph_ui::workspace::WindowId::PianoRoll]);
    let before_notes = notes_in(h.state(), &ids);
    let before_clips = h.state().core.project.clips.len();
    let before_hist = h.state().core.history.len();

    let plain = h.render().ok();
    let id = propose(h.state(), &ids);
    until(&mut h, "the proposal reaches the UI", |a| pending(a, id));
    // nothing changed; the ghosts are there
    assert_eq!(notes_in(h.state(), &ids), before_notes);
    assert_eq!(h.state().core.project.clips.len(), before_clips);
    let roll = proposals::roll_previews(&h.state().core.proposals, ids.bassline);
    assert_eq!(roll.len(), 4);
    assert!(roll.iter().all(|n| n.channel == ids.bass && !n.removed));
    assert!(
        proposals::roll_previews(&h.state().core.proposals, ids.beat).is_empty(),
        "other patterns have no ghosts"
    );
    let clips = proposals::playlist_previews(&h.state().core.proposals);
    assert_eq!(clips.len(), 1);
    assert_eq!((clips[0].track, clips[0].start), (3, 15360));
    assert_eq!(clips[0].author, "agent:muse");
    // a toast announced it
    assert!(
        h.state()
            .core
            .toasts
            .iter()
            .any(|t| t.text.contains("agent:muse proposes: Fill the bridge")),
        "{:?}",
        h.state().core.toasts
    );
    // the strip offers both choices, with the title
    for _ in 0..3 {
        h.step();
    }
    assert!(h.get_all_by_label("Accept").next().is_some());
    assert!(h.get_all_by_label("Reject").next().is_some());
    assert!(
        h.get_all_by_label_contains("Fill the bridge")
            .next()
            .is_some()
    );

    // the ghosts are painted: the frame differs from the same frame before the proposal
    // (skipped where there is no GPU adapter for offscreen rendering)
    if let (Some(plain), Ok(with)) = (plain, h.render()) {
        let changed = plain
            .pixels()
            .zip(with.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 300, "only {changed} pixels differ");
        if std::env::var_os("PH_SCREENSHOTS").is_some_and(|v| v == "1") {
            let dir =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/M2");
            std::fs::create_dir_all(&dir).unwrap();
            with.save(dir.join("agent-proposal-preview.png")).unwrap();
            with.save(dir.join("agent-proposal-roll.png")).unwrap();
            // and the playlist alone, where the ghost clip sits
            only(&mut h, &[]);
            if let Ok(pl) = h.render() {
                pl.save(dir.join("agent-proposal-playlist.png")).unwrap();
            }
            only(&mut h, &[ph_ui::workspace::WindowId::PianoRoll]);
        }
    }

    // Accept, with the mouse
    h.get_all_by_label("Accept").next().unwrap().click();
    until(&mut h, "accepted and applied", |a| {
        !pending(a, id) && notes_in(a, &ids) == before_notes + 4
    });
    assert_eq!(h.state().core.project.clips.len(), before_clips + 1);
    assert!(proposals::roll_previews(&h.state().core.proposals, ids.bassline).is_empty());
    until(&mut h, "history shows it", |a| {
        a.core.history.len() == before_hist + 1
    });
    let e = h.state().core.history.last().unwrap().clone();
    assert_eq!(
        e.author,
        Author::Agent("muse".into()),
        "attributed to the agent"
    );
    assert_eq!(e.commands.len(), 2, "one transaction holds both commands");
    let info = h
        .state()
        .core
        .proposals
        .iter()
        .find(|p| p.id == id)
        .unwrap()
        .clone();
    assert_eq!(
        info.status,
        ProposalStatus::Accepted {
            txn: e.txn,
            by: Author::Human
        }
    );
    // the agent (not the person) undoes it, in one step
    h.state_mut()
        .core
        .studio
        .session
        .undo(&Author::Agent("muse".into()), ph_commands::UndoMode::Stop)
        .unwrap();
    until(&mut h, "undone", |a| {
        notes_in(a, &ids) == before_notes && a.core.project.clips.len() == before_clips
    });
}

#[test]
fn reject_changes_nothing_and_the_agent_can_read_the_decision() {
    let (mut h, ids, _dir) = start();
    only(&mut h, &[]);
    let before = h.state().core.project.clone();
    let id = propose(h.state(), &ids);
    until(&mut h, "the proposal reaches the UI", |a| pending(a, id));
    for _ in 0..3 {
        h.step();
    }
    h.get_all_by_label("Reject").next().unwrap().click();
    until(&mut h, "rejected", |a| !pending(a, id));
    assert_eq!(*h.state().core.project, *before, "nothing changed");
    assert!(proposals::roll_previews(&h.state().core.proposals, ids.bassline).is_empty());
    assert!(proposals::playlist_previews(&h.state().core.proposals).is_empty());
    // what the agent reads back
    let seen = h
        .state()
        .core
        .studio
        .session
        .execute(&agent(), Request::new("proposal.get", json!({ "id": id })))
        .unwrap()
        .result;
    assert_eq!(seen["status"]["state"], "rejected");
}

#[test]
fn an_edit_makes_a_proposal_stale_and_it_can_only_be_dismissed() {
    let (mut h, ids, _dir) = start();
    only(&mut h, &[]);
    // the agent proposes to change a bass note that is already there
    let note = h.state().core.project.pattern(ids.bassline).unwrap().notes[&ids.bass]
        .values()
        .next()
        .unwrap()
        .id;
    let out = h
        .state()
        .core
        .studio
        .session
        .execute(
            &agent(),
            Request::new(
                "proposal.submit",
                json!({ "commands": [{ "id": "note.set_velocity", "params": {
                    "pattern": ids.bassline.0, "channel": ids.bass.0, "ids": [note.0], "velocity": 0.2 } }] }),
            ),
        )
        .unwrap();
    let id = out.result["proposal"].as_u64().unwrap();
    until(&mut h, "the proposal reaches the UI", |a| pending(a, id));
    for _ in 0..3 {
        h.step();
    }
    assert!(h.get_all_by_label("Accept").next().is_some());

    // the person edits that very note
    h.state_mut()
        .core
        .send(mapping::note_velocity(ids.bassline, ids.bass, &[note], 0.9));
    until(&mut h, "the proposal is stale", |a| {
        a.core
            .proposals
            .iter()
            .any(|p| p.id == id && p.stale.is_some())
    });
    for _ in 0..3 {
        h.step();
    }
    assert!(
        h.query_all_by_label("Accept").next().is_none(),
        "a stale proposal cannot be accepted"
    );
    assert!(h.query_all_by_label_contains("Stale").next().is_some());
    assert!(proposals::roll_previews(&h.state().core.proposals, ids.bassline).is_empty());
    let v = h.state().core.project.pattern(ids.bassline).unwrap().notes[&ids.bass][&note].velocity;
    assert_eq!(v, 0.9);
    // dismiss clears it
    h.get_all_by_label("Dismiss").next().unwrap().click();
    until(&mut h, "dismissed", |a| !pending(a, id));
}

/// Hides every window except the playlist and `keep`, so nothing covers the strip.
fn only(h: &mut H, keep: &[ph_ui::workspace::WindowId]) {
    use ph_ui::workspace::WindowId;
    let ws = &mut h.state_mut().ws;
    for id in ws.ids().collect::<Vec<_>>() {
        if id != WindowId::Playlist {
            ws.hide(id);
        }
    }
    for id in keep {
        ws.show(*id);
    }
    for _ in 0..3 {
        h.step();
    }
}
