//! `proposal.*`: agent preview mode. A batch submitted as a proposal runs on a copy of the
//! project and waits for a decision; see [`crate::proposal`]. These act on the [`Session`]
//! (which keeps the proposals), so the session runs them through the `session_cmd` path.

use crate::session_cmd::{SessionOut, parse, stub};
use crate::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn register(r: &mut Registry) {
    r.register(typed(
        "proposal.submit",
        "Propose a batch of commands for the person to preview and accept or reject; nothing changes until they accept.",
        Scope::Compose,
        MUTATES,
        stub::<Submit>,
    ));
    r.register(typed(
        "proposal.list",
        "List proposals: pending ones, and recent decisions (accepted, rejected or stale) with the reason.",
        Scope::Read,
        READ,
        stub::<NoParams>,
    ));
    r.register(typed(
        "proposal.get",
        "Show one proposal: its status, the decision and why, what it would change.",
        Scope::Read,
        READ,
        stub::<Id>,
    ));
    r.register(typed(
        "proposal.accept",
        "Accept a pending proposal: it is applied as one undo step attributed to the agent that proposed it. A stale proposal is refused.",
        Scope::Full,
        MUTATES,
        stub::<Id>,
    ));
    r.register(typed(
        "proposal.reject",
        "Reject a pending proposal (or withdraw your own); nothing changes. The agent sees the decision.",
        Scope::Read,
        READ,
        stub::<Reject>,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Id {
    /// Proposal number from proposal.submit or proposal.list.
    id: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reject {
    /// Proposal number from proposal.submit or proposal.list.
    id: u64,
    /// Why, in a sentence; the agent reads it (optional).
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposedCommand {
    /// Command id, e.g. note.add.
    id: String,
    /// The command's parameters.
    #[serde(default)]
    params: Value,
    /// Required for destructive commands.
    #[serde(default)]
    confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Submit {
    /// A short name the person sees on the Accept / Reject card, e.g. "Add a bass line".
    #[serde(default)]
    title: Option<String>,
    /// The commands, run in order on a copy of the project. Transport, file, record, MIDI and
    /// UI commands cannot be previewed.
    commands: Vec<ProposedCommand>,
}

pub(crate) fn submit(
    s: &mut Session,
    caller: &Caller,
    host: &mut dyn Host,
    params: Value,
) -> CmdResult<SessionOut> {
    let p: Submit = parse("proposal.submit", params)?;
    let reqs: Vec<Request> = p
        .commands
        .into_iter()
        .map(|c| Request {
            command: c.id,
            params: c.params,
            confirm: c.confirm,
        })
        .collect();
    let id = s.submit_proposal(caller, p.title, reqs, host)?;
    let info = s.proposal(id).expect("just submitted");
    Ok(SessionOut {
        result: json!({
            "proposal": id,
            "status": "pending",
            "summary": info.summary,
            "ghost": info.to_json()["ghost"],
            "note": "nothing has changed yet: the person previews it and accepts or rejects. Read the decision with proposal.get (or the proposal_status tool).",
        }),
        summary: Some(format!("proposed \"{}\" (pending)", info.title)),
        ..Default::default()
    })
}

pub(crate) fn accept(
    s: &mut Session,
    caller: &Caller,
    host: &mut dyn Host,
    params: Value,
) -> CmdResult<SessionOut> {
    let p: Id = parse("proposal.accept", params)?;
    let done = s.accept_proposal(p.id, caller, host)?;
    let mut changed = Vec::new();
    for e in &done {
        for c in &e.changed {
            if !changed.contains(c) {
                changed.push(*c);
            }
        }
    }
    let info = s.proposal(p.id).expect("decided proposals are kept");
    Ok(SessionOut {
        result: json!({ "proposal": p.id, "status": "accepted", "applied_as": info.status }),
        summary: Some(format!("accepted \"{}\"", info.title)),
        changed,
        txn: done.iter().filter_map(|e| e.txn).max(),
    })
}

pub(crate) fn reject(s: &mut Session, caller: &Caller, params: Value) -> CmdResult<SessionOut> {
    let p: Reject = parse("proposal.reject", params)?;
    s.reject_proposal(p.id, caller, p.reason)?;
    let info = s.proposal(p.id).expect("decided proposals are kept");
    Ok(SessionOut {
        result: json!({ "proposal": p.id, "status": "rejected" }),
        summary: Some(format!("rejected \"{}\"", info.title)),
        ..Default::default()
    })
}

pub(crate) fn list(s: &mut Session, params: Value) -> CmdResult<SessionOut> {
    let _: NoParams = parse("proposal.list", params)?;
    let rows: Vec<Value> = s.proposals().iter().map(|p| p.to_json()).collect();
    Ok(SessionOut {
        result: json!({ "proposals": rows }),
        ..Default::default()
    })
}

pub(crate) fn get(s: &mut Session, params: Value) -> CmdResult<SessionOut> {
    let p: Id = parse("proposal.get", params)?;
    match s.proposal(p.id) {
        Some(info) => Ok(SessionOut {
            result: info.to_json(),
            ..Default::default()
        }),
        None => Err(CmdError::Other(format!(
            "no proposal {} (it may have been pruned; proposal.list shows the kept ones)",
            p.id
        ))),
    }
}
