//! Behaviour tests for `proposal.*` (M2/T9): an agent's batch is previewed and then accepted,
//! rejected or found stale; the project changes only on accept, as one transaction by the agent.

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::sync::Arc;

struct Rig {
    s: Session,
    h: NullHost,
    human: Caller,
    agent: Caller,
    pat: u32,
    ch: u32,
}

impl Rig {
    fn new() -> Rig {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("p"));
        let mut h = NullHost::default();
        let human = Caller::human();
        let pat = s
            .execute(&human, Request::new("pattern.add", json!({})), &mut h)
            .unwrap()
            .result["pattern"]
            .as_u64()
            .unwrap() as u32;
        let ch = s
            .execute(
                &human,
                Request::new(
                    "channel.add",
                    json!({ "name": "Bass", "instrument": { "kind": "synth", "preset": "bass/sub" } }),
                ),
                &mut h,
            )
            .unwrap()
            .result["channel"]
            .as_u64()
            .unwrap() as u32;
        Rig {
            s,
            h,
            human,
            agent: Caller::agent("muse", Grant::full()),
            pat,
            ch,
        }
    }

    fn run(&mut self, who: &Caller, cmd: &str, params: Value) -> CmdResult<Executed> {
        self.s.execute(who, Request::new(cmd, params), &mut self.h)
    }

    fn notes(&self) -> usize {
        self.s
            .project()
            .pattern(PatternId(self.pat))
            .unwrap()
            .notes
            .get(&ChannelId(self.ch))
            .map_or(0, |m| m.len())
    }

    /// Submits "add `n` notes" as the agent and returns the proposal id.
    fn propose_notes(&mut self, n: u64) -> u64 {
        let notes: Vec<Value> = (0..n)
            .map(|i| json!({ "key": 40 + i, "start": i * 960, "length": 480 }))
            .collect();
        let agent = self.agent.clone();
        self.run(
            &agent,
            "proposal.submit",
            json!({ "title": "Bass line", "commands": [
                { "id": "note.add", "params": { "pattern": self.pat, "channel": self.ch, "notes": notes } }] }),
        )
        .unwrap()
        .result["proposal"]
            .as_u64()
            .unwrap()
    }
}

#[test]
fn submit_previews_without_touching_the_project() {
    let mut r = Rig::new();
    let before = r.s.project().clone();
    let history = r.s.history().len();
    let id = r.propose_notes(4);
    assert!(Arc::ptr_eq(&before, r.s.project()), "project untouched");
    assert_eq!(r.s.history().len(), history, "no transaction");
    assert_eq!(r.notes(), 0);

    let human = r.human.clone();
    let list = r.run(&human, "proposal.list", json!({})).unwrap().result;
    assert_eq!(list["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(list["proposals"][0]["status"]["state"], "pending");
    let got = r
        .run(&human, "proposal.get", json!({ "id": id }))
        .unwrap()
        .result;
    assert_eq!(got["title"], "Bass line");
    assert_eq!(got["author"], "agent:muse");
    assert_eq!(got["ghost"]["notes"], 4);

    // the preview and the ghost describe the four notes
    let info = r.s.proposal(id).unwrap();
    assert_eq!(info.ghost.notes.len(), 4);
    assert!(info.stale.is_none());
    let pv = info.preview.pattern(PatternId(r.pat)).unwrap();
    assert_eq!(pv.notes[&ChannelId(r.ch)].len(), 4);
}

#[test]
fn accept_lands_as_one_undo_step_by_the_agent() {
    let mut r = Rig::new();
    let id = r.propose_notes(3);
    let human = r.human.clone();
    let before = r.s.history().len();
    let out = r
        .run(&human, "proposal.accept", json!({ "id": id }))
        .unwrap();
    assert_eq!(out.result["status"], "accepted");
    assert_eq!(r.notes(), 3);
    assert_eq!(r.s.history().len(), before + 1, "one transaction");
    let entry = r.s.history().last().unwrap();
    assert_eq!(entry.author, Author::Agent("muse".into()));
    assert_eq!(out.txn, Some(entry.txn));
    assert_eq!(
        r.s.proposal(id).unwrap().status,
        ProposalStatus::Accepted {
            txn: entry.txn,
            by: Author::Human
        }
    );
    // a decided proposal cannot be decided again
    let again = r.run(&human, "proposal.accept", json!({ "id": id }));
    assert!(again.unwrap_err().to_string().contains("already decided"));
    // the agent undoes it as one step
    let agent = r.agent.clone();
    r.run(&agent, "history.undo", json!({})).unwrap();
    assert_eq!(r.notes(), 0);
}

#[test]
fn reject_changes_nothing_and_the_agent_reads_the_reason() {
    let mut r = Rig::new();
    let id = r.propose_notes(2);
    let human = r.human.clone();
    let before = r.s.project().clone();
    r.run(
        &human,
        "proposal.reject",
        json!({ "id": id, "reason": "too busy" }),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&before, r.s.project()));
    let agent = r.agent.clone();
    let got = r
        .run(&agent, "proposal.get", json!({ "id": id }))
        .unwrap()
        .result;
    assert_eq!(got["status"]["state"], "rejected");
    assert_eq!(got["status"]["reason"], "too busy");
    // notes for subscribers: submitted, then rejected
    let notes = r.s.take_proposal_notes();
    let states: Vec<_> = notes.iter().map(|n| n.state).collect();
    assert_eq!(states, ["pending", "rejected"]);
    // an agent withdraws its own; it cannot reject someone else's without the full scope
    let mine = r.propose_notes(1);
    r.run(&agent, "proposal.reject", json!({ "id": mine }))
        .unwrap();
    let other = Caller::agent("other", Grant(vec![Scope::Compose]));
    let theirs = r.propose_notes(1);
    let err = r.run(&other, "proposal.reject", json!({ "id": theirs }));
    assert!(matches!(err, Err(CmdError::Scope { .. })), "{err:?}");
}

#[test]
fn an_edit_to_the_same_object_makes_it_stale_and_it_is_not_applied() {
    let mut r = Rig::new();
    let human = r.human.clone();
    let agent = r.agent.clone();
    r.run(
        &human,
        "note.add",
        json!({ "pattern": r.pat, "channel": r.ch, "notes": [{ "key": 36, "start": 0, "length": 480 }] }),
    )
    .unwrap();
    let id = r
        .run(
            &agent,
            "proposal.submit",
            json!({ "commands": [{ "id": "note.set_velocity",
                "params": { "pattern": r.pat, "channel": r.ch, "ids": [0], "velocity": 0.2 } }] }),
        )
        .unwrap()
        .result["proposal"]
        .as_u64()
        .unwrap();
    // the person edits that very note meanwhile
    r.run(
        &human,
        "note.set_velocity",
        json!({ "pattern": r.pat, "channel": r.ch, "ids": [0], "velocity": 0.9 }),
    )
    .unwrap();
    let info = r.s.proposal(id).unwrap();
    assert!(info.stale.as_deref().unwrap().contains("note/"), "{info:?}");
    let before = r.s.project().clone();
    let err = r
        .run(&human, "proposal.accept", json!({ "id": id }))
        .unwrap_err()
        .to_string();
    assert!(err.contains("stale"), "{err}");
    assert!(Arc::ptr_eq(&before, r.s.project()), "not force-applied");
    assert!(matches!(
        r.s.proposal(id).unwrap().status,
        ProposalStatus::Stale { .. }
    ));
}

#[test]
fn unrelated_edits_do_not_make_it_stale() {
    let mut r = Rig::new();
    let human = r.human.clone();
    let id = r.propose_notes(2);
    // the person adds a note and changes the tempo meanwhile: other objects, and new ids
    r.run(
        &human,
        "note.add",
        json!({ "pattern": r.pat, "channel": r.ch, "notes": [{ "key": 60, "start": 3840, "length": 480 }] }),
    )
    .unwrap();
    r.run(&human, "project.set_tempo", json!({ "bpm": 100.0 }))
        .unwrap();
    assert!(r.s.proposal(id).unwrap().stale.is_none());
    r.run(&human, "proposal.accept", json!({ "id": id }))
        .unwrap();
    assert_eq!(
        r.notes(),
        3,
        "the person's note and the agent's two, all with their own ids"
    );
}

#[test]
fn replacing_the_project_makes_pending_proposals_stale() {
    let mut r = Rig::new();
    let id = r.propose_notes(1);
    r.s.reset(Project::empty("other"));
    assert!(matches!(
        r.s.proposal(id).unwrap().status,
        ProposalStatus::Stale { .. }
    ));
    let human = r.human.clone();
    assert!(
        r.run(&human, "proposal.accept", json!({ "id": id }))
            .is_err()
    );
}

#[test]
fn guards() {
    let mut r = Rig::new();
    let agent = r.agent.clone();
    let human = r.human.clone();
    // an agent cannot accept its own proposal
    let id = r.propose_notes(1);
    let err = r
        .run(&agent, "proposal.accept", json!({ "id": id }))
        .unwrap_err();
    assert!(err.to_string().contains("own proposal"), "{err}");
    // a read-only client cannot propose edits it could not make
    let ro = Caller::agent("ro", Grant(vec![Scope::Read]));
    let cmds = json!({ "commands": [{ "id": "project.set_tempo", "params": { "bpm": 90.0 } }] });
    assert!(r.run(&ro, "proposal.submit", cmds.clone()).is_err());
    // not previewable: transport, queries, nested proposals, files
    for id in [
        "transport.play",
        "query.project",
        "proposal.accept",
        "file.save",
    ] {
        let e = r
            .run(
                &agent,
                "proposal.submit",
                json!({ "commands": [{ "id": id, "params": {} }] }),
            )
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("cannot be previewed") || e.contains("does not change the project"),
            "{id}: {e}"
        );
    }
    // a failing batch keeps nothing, an empty one is refused
    let bad = json!({ "commands": [{ "id": "note.add", "params": { "pattern": 99, "channel": 0, "notes": [] } }] });
    assert!(r.run(&agent, "proposal.submit", bad).is_err());
    let noop = r.run(&agent, "proposal.submit", json!({ "commands": [] }));
    assert!(noop.is_err());
    assert_eq!(r.s.proposals().len(), 1, "only the first is kept");
    // the cap on pending proposals
    for _ in 1..MAX_PENDING_PER_AUTHOR {
        r.propose_notes(1);
    }
    let e = r
        .run(&agent, "proposal.submit", cmds)
        .unwrap_err()
        .to_string();
    assert!(e.contains("pending proposals"), "{e}");
    // the person can still decide them
    r.run(&human, "proposal.reject", json!({ "id": id }))
        .unwrap();
}

#[test]
fn proposals_travel_through_the_handle() {
    let h = SessionHandle::spawn(
        Session::new(Arc::new(Registry::with_builtins()), Project::empty("q")),
        NullHost::default(),
    );
    let notes = h.subscribe_proposals().unwrap();
    let agent = Caller::agent("muse", Grant::full());
    let out = h
        .execute(
            &agent,
            Request::new(
                "proposal.submit",
                json!({ "commands": [{ "id": "project.set_tempo", "params": { "bpm": 90.0 } }] }),
            ),
        )
        .unwrap();
    let id = out.result["proposal"].as_u64().unwrap();
    assert_eq!(h.project().unwrap().settings.bpm, 130.0);
    assert_eq!(notes.recv().unwrap().state, "pending");
    h.execute(
        &Caller::human(),
        Request::new("proposal.accept", json!({ "id": id })),
    )
    .unwrap();
    assert_eq!(notes.recv().unwrap().state, "accepted");
    assert_eq!(h.project().unwrap().settings.bpm, 90.0);
    assert_eq!(h.proposals().unwrap().len(), 1);
}
