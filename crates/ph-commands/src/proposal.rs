//! Agent preview mode (DESIGN §5, deferred from M1): an agent submits a batch as a
//! *proposal* instead of applying it.
//!
//! The session runs the batch on a copy of the project (the **preview**) and keeps it with a
//! [`Ghost`]: the notes and clips the batch would add, change or remove. The UI draws the
//! ghost in the agent's colour and offers Accept and Reject. Nothing in the real project
//! changes until a person (or another author with the `full` scope) accepts:
//!
//! - **accepted:** the requests run again on the *current* project as one transaction,
//!   attributed to the agent that proposed them, so one undo reverts it;
//! - **rejected:** nothing changes; the agent reads the decision back;
//! - **stale:** another edit touched an object the proposal changes (or the project was
//!   replaced, or the requests no longer apply). The proposal is reported stale and never
//!   force-applied.
//!
//! Objects the proposal *creates* are not compared: ids are never reused, so the accepted
//! run simply allocates fresh ones.

use crate::*;
use ph_core::{ClipId, NoteId, ObjectPath, PatternId, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Pending proposals one author may have at the same time.
pub const MAX_PENDING_PER_AUTHOR: usize = 8;
/// Decided proposals kept so agents can read the decision back.
const KEEP_DECIDED: usize = 40;
/// Ghost notes kept per proposal (the rest are counted in [`Ghost::more_notes`]).
const MAX_GHOST_NOTES: usize = 4000;

/// Where a proposal stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Waiting for a decision.
    Pending,
    /// Applied as one transaction attributed to the proposing agent.
    Accepted { txn: u64, by: Author },
    /// Declined (or withdrawn by its author); the project is unchanged.
    Rejected { by: Author, reason: Option<String> },
    /// Not applied: something it changes was edited since, or it no longer applies.
    Stale { reason: String },
}

impl ProposalStatus {
    /// `pending`, `accepted`, `rejected` or `stale`.
    pub fn name(&self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Accepted { .. } => "accepted",
            ProposalStatus::Rejected { .. } => "rejected",
            ProposalStatus::Stale { .. } => "stale",
        }
    }
}

/// One note a proposal would add or change.
#[derive(Clone, Debug, PartialEq)]
pub struct GhostNote {
    pub pattern: PatternId,
    pub channel: ph_core::ChannelId,
    pub note: ph_core::Note,
}

/// What a proposal would change on screen: the piano roll and playlist draw these.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ghost {
    /// Notes added or changed (their new form).
    pub notes: Vec<GhostNote>,
    /// Notes beyond [`MAX_GHOST_NOTES`] that are not listed.
    pub more_notes: usize,
    /// Notes removed (or replaced by a changed form listed in `notes`).
    pub removed_notes: Vec<(PatternId, ph_core::ChannelId, NoteId)>,
    /// Playlist clips of any kind added or changed (their new form).
    pub clips: Vec<ph_core::Clip>,
    /// Clips removed.
    pub removed_clips: Vec<ClipId>,
}

impl Ghost {
    fn between(before: &Project, after: &Project) -> Ghost {
        let mut g = Ghost::default();
        for (pid, pat) in after.patterns.iter() {
            let old = before.patterns.get(pid);
            if old.is_some_and(|o| Arc::ptr_eq(o, pat)) {
                continue;
            }
            for (ch, notes) in &pat.notes {
                let old_notes = old.and_then(|o| o.notes.get(ch));
                for (nid, n) in notes {
                    if old_notes.and_then(|m| m.get(nid)) == Some(n) {
                        continue;
                    }
                    if g.notes.len() < MAX_GHOST_NOTES {
                        g.notes.push(GhostNote {
                            pattern: *pid,
                            channel: *ch,
                            note: n.clone(),
                        });
                    } else {
                        g.more_notes += 1;
                    }
                }
            }
        }
        for (pid, pat) in before.patterns.iter() {
            let new = after.patterns.get(pid);
            for (ch, notes) in &pat.notes {
                for nid in notes.keys() {
                    let kept = new
                        .and_then(|n| n.notes.get(ch))
                        .is_some_and(|m| m.contains_key(nid));
                    if !kept {
                        g.removed_notes.push((*pid, *ch, *nid));
                    }
                }
            }
        }
        for (id, c) in after.clips.iter() {
            if before.clips.get(id) != Some(c) {
                g.clips.push(c.clone());
            }
        }
        for id in before.clips.keys() {
            if !after.clips.contains_key(id) {
                g.removed_clips.push(*id);
            }
        }
        g
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.more_notes == 0
            && self.removed_notes.is_empty()
            && self.clips.is_empty()
            && self.removed_clips.is_empty()
    }
}

pub(crate) struct Proposal {
    id: u64,
    caller: Caller,
    title: String,
    requests: Vec<Request>,
    summary: Option<String>,
    status: ProposalStatus,
    /// Last transaction when it was submitted; later ones may make it stale.
    base_txn: u64,
    epoch: u64,
    /// Objects that existed then and that it changes.
    touched: Vec<ObjectPath>,
    base: Arc<Project>,
    preview: Arc<Project>,
    ghost: Arc<Ghost>,
    /// Counts decisions, so a client can ask for the ones it has not seen.
    decided_seq: u64,
}

/// A change to the set of proposals, for subscribers (the UI, the event hub).
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalNote {
    pub id: u64,
    pub author: Author,
    pub title: String,
    /// `pending` (just submitted), `accepted`, `rejected` or `stale`.
    pub state: &'static str,
    pub reason: Option<String>,
}

#[derive(Default)]
pub(crate) struct ProposalBook {
    next_id: u64,
    epoch: u64,
    decisions: u64,
    items: BTreeMap<u64, Proposal>,
    notes: Vec<ProposalNote>,
}

impl ProposalBook {
    /// The project was replaced: pending proposals describe a project that is gone.
    pub(crate) fn project_replaced(&mut self) {
        self.epoch += 1;
        let mut gone = Vec::new();
        for p in self.items.values_mut() {
            if p.status == ProposalStatus::Pending {
                p.status = ProposalStatus::Stale {
                    reason: "the project was replaced".into(),
                };
                gone.push(ProposalNote {
                    id: p.id,
                    author: p.caller.author.clone(),
                    title: p.title.clone(),
                    state: "stale",
                    reason: Some("the project was replaced".into()),
                });
            }
        }
        self.notes.extend(gone);
    }

    fn prune(&mut self) {
        let decided: Vec<u64> = self
            .items
            .values()
            .filter(|p| p.status != ProposalStatus::Pending)
            .map(|p| p.id)
            .collect();
        for id in decided
            .iter()
            .take(decided.len().saturating_sub(KEEP_DECIDED))
        {
            self.items.remove(id);
        }
    }
}

/// A proposal as clients see it.
#[derive(Clone, Debug)]
pub struct ProposalInfo {
    pub id: u64,
    pub author: Author,
    pub title: String,
    /// Command ids, in order.
    pub commands: Vec<String>,
    /// What the commands report, joined ("added 4 note(s) to Bass; ...").
    pub summary: Option<String>,
    pub status: ProposalStatus,
    /// For a pending proposal that can no longer be applied: why. Accepting it would fail.
    pub stale: Option<String>,
    /// The project the proposal was made against.
    pub base: Arc<Project>,
    /// That project with the proposal applied.
    pub preview: Arc<Project>,
    pub ghost: Arc<Ghost>,
    /// Higher for later decisions; 0 while pending.
    pub decided_seq: u64,
}

impl ProposalInfo {
    pub fn pending(&self) -> bool {
        self.status == ProposalStatus::Pending
    }

    /// The JSON agents read (no project data).
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "author": self.author,
            "title": self.title,
            "commands": self.commands,
            "summary": self.summary,
            "status": self.status,
            "ghost": {
                "notes": self.ghost.notes.len() + self.ghost.more_notes,
                "removed_notes": self.ghost.removed_notes.len(),
                "clips": self.ghost.clips.len(),
                "removed_clips": self.ghost.removed_clips.len(),
            },
        });
        if let Some(s) = &self.stale {
            v["stale"] = json!(s);
        }
        v
    }
}

/// Commands that act on something besides the project (the transport, files, the UI, the
/// session itself, hardware): a preview cannot run them without doing them for real.
fn not_previewable(id: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "transport.",
        "record.",
        "midi.",
        "file.",
        "history.",
        "proposal.",
        "ui.",
        "query.",
    ];
    const EXACT: &[&str] = &[
        "channel.solo",
        "mixer.solo",
        "plugin.scan",
        "plugin.open_editor",
        "plugin.close_editor",
        "editor.open",
    ];
    PREFIXES.iter().any(|p| id.starts_with(p)) || EXACT.contains(&id)
}

fn pending_of(book: &ProposalBook, a: &Author) -> usize {
    book.items
        .values()
        .filter(|p| p.status == ProposalStatus::Pending && &p.caller.author == a)
        .count()
}

impl Session {
    /// Runs `reqs` on a copy of the project and keeps the result as a pending proposal of
    /// `caller`. Nothing in the real project changes. Fails (and keeps nothing) if the
    /// requests would fail, change nothing, or include a command a preview cannot run.
    pub fn submit_proposal(
        &mut self,
        caller: &Caller,
        title: Option<String>,
        reqs: Vec<Request>,
        host: &mut dyn Host,
    ) -> CmdResult<u64> {
        if reqs.is_empty() {
            return Err(CmdError::Other(
                "a proposal needs at least one command".into(),
            ));
        }
        for r in &reqs {
            let spec = self.registry().get(&r.command)?.spec();
            if !spec.mutating {
                return Err(CmdError::Other(format!(
                    "{} does not change the project, so it cannot be part of a proposal",
                    spec.id
                )));
            }
            if not_previewable(spec.id) {
                return Err(CmdError::Other(format!(
                    "{} acts outside the project (transport, files, UI, hardware) and cannot be previewed; run it directly",
                    spec.id
                )));
            }
        }
        if pending_of(&self.proposals, &caller.author) >= MAX_PENDING_PER_AUTHOR {
            return Err(CmdError::Other(format!(
                "{} already has {MAX_PENDING_PER_AUTHOR} pending proposals; wait for a decision or withdraw one (proposal.reject)",
                caller.author
            )));
        }
        let dry = self.dry_batch(caller, &reqs, host)?;
        if dry.patches.is_empty() {
            return Err(CmdError::Other(
                "the proposal would change nothing, so there is nothing to preview".into(),
            ));
        }
        let base = self.project().clone();
        let touched: Vec<ObjectPath> = dry
            .patches
            .paths()
            .into_iter()
            .filter(|p| !p.is_order_list() && ph_core::get(&base, *p).is_some())
            .collect();
        let preview = Arc::new(dry.project);
        let ghost = Arc::new(Ghost::between(&base, &preview));
        let summary = (!dry.summaries.is_empty()).then(|| dry.summaries.join("; "));
        let book = &mut self.proposals;
        book.next_id += 1;
        let id = book.next_id;
        let title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| summary.clone())
            .unwrap_or_else(|| format!("proposal {id}"));
        let title: String = title.chars().take(120).collect();
        book.notes.push(ProposalNote {
            id,
            author: caller.author.clone(),
            title: title.clone(),
            state: "pending",
            reason: None,
        });
        let epoch = book.epoch;
        book.items.insert(
            id,
            Proposal {
                id,
                caller: caller.clone(),
                title,
                requests: reqs,
                summary,
                status: ProposalStatus::Pending,
                base_txn: 0,
                epoch,
                touched,
                base,
                preview,
                ghost,
                decided_seq: 0,
            },
        );
        let last = self.last_txn();
        if let Some(p) = self.proposals.items.get_mut(&id) {
            p.base_txn = last;
        }
        Ok(id)
    }

    /// Why a pending proposal cannot be applied now, if it cannot.
    fn staleness(&self, p: &Proposal) -> Option<String> {
        if p.epoch != self.proposals.epoch {
            return Some("the project was replaced since".into());
        }
        for e in self.history().iter().filter(|e| e.txn > p.base_txn) {
            if let Some(hit) = e
                .patches
                .paths()
                .into_iter()
                .find(|path| p.touched.contains(path))
            {
                return Some(format!(
                    "{hit} was changed by {} (transaction {}) after the proposal was made",
                    e.author, e.txn
                ));
            }
        }
        None
    }

    fn info(&self, p: &Proposal) -> ProposalInfo {
        ProposalInfo {
            id: p.id,
            author: p.caller.author.clone(),
            title: p.title.clone(),
            commands: p.requests.iter().map(|r| r.command.clone()).collect(),
            summary: p.summary.clone(),
            status: p.status.clone(),
            stale: if p.status == ProposalStatus::Pending {
                self.staleness(p)
            } else {
                None
            },
            base: p.base.clone(),
            preview: p.preview.clone(),
            ghost: p.ghost.clone(),
            decided_seq: p.decided_seq,
        }
    }

    /// Every proposal kept, oldest first: the pending ones and the recent decisions.
    pub fn proposals(&self) -> Vec<ProposalInfo> {
        self.proposals
            .items
            .values()
            .map(|p| self.info(p))
            .collect()
    }

    pub fn proposal(&self, id: u64) -> Option<ProposalInfo> {
        self.proposals.items.get(&id).map(|p| self.info(p))
    }

    /// Changes to the set of proposals since the last call (the control thread forwards them).
    pub fn take_proposal_notes(&mut self) -> Vec<ProposalNote> {
        std::mem::take(&mut self.proposals.notes)
    }

    fn decide(&mut self, id: u64, status: ProposalStatus) {
        let book = &mut self.proposals;
        book.decisions += 1;
        let seq = book.decisions;
        if let Some(p) = book.items.get_mut(&id) {
            let reason = match &status {
                ProposalStatus::Rejected { reason, .. } => reason.clone(),
                ProposalStatus::Stale { reason } => Some(reason.clone()),
                _ => None,
            };
            book.notes.push(ProposalNote {
                id,
                author: p.caller.author.clone(),
                title: p.title.clone(),
                state: status.name(),
                reason,
            });
            p.status = status;
            p.decided_seq = seq;
        }
        book.prune();
    }

    fn pending(&self, id: u64) -> CmdResult<&Proposal> {
        let p = self.proposals.items.get(&id).ok_or_else(|| {
            CmdError::Other(format!("no proposal {id} (proposal.list shows them)"))
        })?;
        if p.status != ProposalStatus::Pending {
            return Err(CmdError::Other(format!(
                "proposal {id} was already decided: {}",
                p.status.name()
            )));
        }
        Ok(p)
    }

    /// Accepts a pending proposal: its requests run on the current project as one
    /// transaction by the proposing author. Returns the results; a stale proposal is marked
    /// so and refused.
    pub fn accept_proposal(
        &mut self,
        id: u64,
        by: &Caller,
        host: &mut dyn Host,
    ) -> CmdResult<Vec<Executed>> {
        let p = self.pending(id)?;
        if p.caller.author == by.author && by.author != Author::Human {
            return Err(CmdError::Other(
                "an agent cannot accept its own proposal; the person (or another author) decides"
                    .into(),
            ));
        }
        let (proposer, reqs) = (p.caller.clone(), p.requests.clone());
        if let Some(reason) = self.staleness(p) {
            self.decide(
                id,
                ProposalStatus::Stale {
                    reason: reason.clone(),
                },
            );
            return Err(CmdError::Other(format!(
                "proposal {id} is stale and was not applied: {reason}. Ask the agent to propose again"
            )));
        }
        match self.batch(&proposer, reqs, host) {
            Ok(done) => {
                let txn = done.iter().filter_map(|e| e.txn).max().unwrap_or(0);
                self.decide(
                    id,
                    ProposalStatus::Accepted {
                        txn,
                        by: by.author.clone(),
                    },
                );
                Ok(done)
            }
            Err(e) => {
                let reason = format!("it no longer applies: {e}");
                self.decide(
                    id,
                    ProposalStatus::Stale {
                        reason: reason.clone(),
                    },
                );
                Err(CmdError::Other(format!(
                    "proposal {id} was not applied: {reason}"
                )))
            }
        }
    }

    /// Rejects (or, for its author, withdraws) a pending proposal. The project is untouched.
    pub fn reject_proposal(
        &mut self,
        id: u64,
        by: &Caller,
        reason: Option<String>,
    ) -> CmdResult<()> {
        let p = self.pending(id)?;
        if p.caller.author != by.author && !by.grant.allows(Scope::Full) {
            return Err(CmdError::Scope {
                command: "proposal.reject".into(),
                needed: Scope::Full,
            });
        }
        self.decide(
            id,
            ProposalStatus::Rejected {
                by: by.author.clone(),
                reason: reason.filter(|r| !r.trim().is_empty()),
            },
        );
        Ok(())
    }
}
