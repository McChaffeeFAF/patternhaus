use crate::*;
use ph_core::{ObjectPath, PatchSet, Project, apply};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// One command invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub command: String,
    #[serde(default)]
    pub params: Value,
    /// Required for destructive commands.
    #[serde(default)]
    pub confirm: bool,
}

impl Request {
    pub fn new(command: &str, params: Value) -> Self {
        Self {
            command: command.into(),
            params,
            confirm: false,
        }
    }
}

/// Who is calling, and what they may do.
#[derive(Clone, Debug, PartialEq)]
pub struct Caller {
    pub author: Author,
    pub grant: Grant,
}

impl Caller {
    pub fn human() -> Self {
        Self {
            author: Author::Human,
            grant: Grant::full(),
        }
    }

    pub fn agent(name: &str, grant: Grant) -> Self {
        Self {
            author: Author::Agent(name.into()),
            grant,
        }
    }
}

/// Result of one executed command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Executed {
    pub command: String,
    pub result: Value,
    pub summary: Option<String>,
    /// Objects this command changed.
    pub changed: Vec<ObjectPath>,
    /// History transaction the changes belong to (`None` for read-only commands).
    pub txn: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryKind {
    /// A normal transaction of one or more commands.
    Do,
    /// Reverts the listed transactions.
    Undo { of: Vec<u64> },
    /// Reverts the listed `Undo` entries, restoring what they undid.
    Redo { of: Vec<u64> },
}

/// One transaction in the history log (DESIGN §3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntry {
    /// Monotonic transaction id.
    pub txn: u64,
    pub author: Author,
    pub kind: EntryKind,
    /// Commands in the transaction, with their parameters, for replay and display.
    pub commands: Vec<Request>,
    pub patches: PatchSet,
    pub summary: Option<String>,
}

/// The authoritative project plus its history.
///
/// Single-threaded by design: the co-editing queue (T1) owns a `Session` on the control
/// thread and feeds it requests from the UI and every connected agent in arrival order.
/// Readers (engine, UI, agents) get `Arc<Project>` snapshots via [`Session::project`].
pub struct Session {
    registry: Arc<Registry>,
    project: Arc<Project>,
    history: Vec<HistoryEntry>,
    /// Transactions already reverted by an `Undo` entry.
    undone: BTreeSet<u64>,
    next_txn: u64,
}

impl Session {
    pub fn new(registry: Arc<Registry>, project: Project) -> Self {
        Self {
            registry,
            project: Arc::new(project),
            history: Vec::new(),
            undone: BTreeSet::new(),
            next_txn: 1,
        }
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Current snapshot.
    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    /// Runs one command as its own transaction.
    pub fn execute(
        &mut self,
        caller: &Caller,
        req: Request,
        host: &mut dyn Host,
    ) -> CmdResult<Executed> {
        let mut out = self.batch(caller, vec![req], host)?;
        Ok(out.pop().expect("one request, one result"))
    }

    /// Runs commands in order as **one** transaction (one undo step). All-or-nothing: if any
    /// command fails, nothing is applied and the error is returned.
    pub fn batch(
        &mut self,
        caller: &Caller,
        reqs: Vec<Request>,
        host: &mut dyn Host,
    ) -> CmdResult<Vec<Executed>> {
        let registry = self.registry.clone();
        let mut work: Project = (*self.project).clone();
        let mut all = PatchSet::default();
        let mut results = Vec::with_capacity(reqs.len());
        let mut summaries = Vec::new();
        for req in &reqs {
            let cmd = registry.get(&req.command)?;
            let spec = cmd.spec();
            if !caller.grant.allows(spec.scope) {
                return Err(CmdError::Scope {
                    command: spec.id.into(),
                    needed: spec.scope,
                });
            }
            if spec.destructive && !req.confirm {
                return Err(CmdError::NeedsConfirm(spec.id.into()));
            }
            let mut ctx = CommandCtx {
                project: &work,
                author: &caller.author,
                host: &mut *host,
            };
            let out = cmd.run(&mut ctx, req.params.clone())?;
            if !spec.mutating && !out.patches.is_empty() {
                return Err(CmdError::Other(format!(
                    "{} is read-only but produced patches",
                    spec.id
                )));
            }
            if !out.patches.is_empty() {
                work = match out.project {
                    Some(p) => {
                        debug_assert_eq!(
                            apply(&work, &out.patches).as_ref(),
                            Ok(&p),
                            "{} output disagrees with its patches",
                            spec.id
                        );
                        p
                    }
                    None => apply(&work, &out.patches)?,
                };
            }
            if let Some(s) = &out.summary {
                summaries.push(s.clone());
            }
            results.push(Executed {
                command: spec.id.into(),
                result: out.result,
                summary: out.summary,
                changed: out.patches.paths(),
                txn: None,
            });
            all.extend(out.patches);
        }
        if !all.is_empty() {
            let txn = self.commit(caller.author.clone(), EntryKind::Do, reqs, all, summaries);
            for r in results.iter_mut().filter(|r| !r.changed.is_empty()) {
                r.txn = Some(txn);
            }
            self.project = Arc::new(work);
        }
        Ok(results)
    }

    fn commit(
        &mut self,
        author: Author,
        kind: EntryKind,
        commands: Vec<Request>,
        patches: PatchSet,
        summaries: Vec<String>,
    ) -> u64 {
        let txn = self.next_txn;
        self.next_txn += 1;
        let summary = if summaries.is_empty() {
            None
        } else {
            Some(summaries.join("; "))
        };
        self.history.push(HistoryEntry {
            txn,
            author,
            kind,
            commands,
            patches,
            summary,
        });
        txn
    }

    /// The author's most recent normal transaction that has not been undone.
    pub fn last_undoable(&self, author: &Author) -> Option<u64> {
        self.history
            .iter()
            .rev()
            .find(|e| {
                &e.author == author && e.kind == EntryKind::Do && !self.undone.contains(&e.txn)
            })
            .map(|e| e.txn)
    }

    /// The author's most recent undo that can be redone: a not-yet-reverted `Undo` entry with
    /// no newer `Do` by the same author (a new edit clears the redo stack, as usual).
    pub fn last_redoable(&self, author: &Author) -> Option<u64> {
        for e in self.history.iter().rev().filter(|e| &e.author == author) {
            match e.kind {
                EntryKind::Do => return None,
                EntryKind::Undo { .. } if !self.undone.contains(&e.txn) => return Some(e.txn),
                EntryKind::Undo { .. } | EntryKind::Redo { .. } => continue,
            }
        }
        None
    }

    /// Redo (Ctrl+Y): reverts the caller's most recent undo.
    pub fn redo(&mut self, author: &Author) -> CmdResult<Option<u64>> {
        match self.last_redoable(author) {
            Some(txn) => self.undo_txn(txn, author).map(Some),
            None => Ok(None),
        }
    }

    /// Reverts one transaction as a new `Undo` transaction by `by`.
    ///
    /// **T0 contract implementation (strict):** succeeds only when nobody changed the
    /// affected objects since; otherwise returns [`CmdError::UndoConflict`]. T1 replaces the
    /// body with the field-level three-way revert from DESIGN §3 (skip / force / cancel),
    /// keeping this signature.
    pub fn undo_txn(&mut self, txn: u64, by: &Author) -> CmdResult<u64> {
        let entry = self
            .history
            .iter()
            .find(|e| e.txn == txn)
            .ok_or_else(|| CmdError::Other(format!("no transaction {txn}")))?;
        if self.undone.contains(&txn) {
            return Err(CmdError::Other(format!(
                "transaction {txn} is already undone"
            )));
        }
        let inverse = entry.patches.inverse();
        let target = apply(&self.project, &inverse).map_err(|e| match e {
            ph_core::ModelError::Conflict { path } => {
                let changed_by = self
                    .history
                    .iter()
                    .filter(|h| h.txn > txn && h.patches.iter().any(|p| p.path == path))
                    .map(|h| h.author.clone())
                    .collect();
                CmdError::UndoConflict(vec![UndoConflict {
                    path,
                    fields: vec![],
                    changed_by,
                }])
            }
            other => CmdError::Model(other),
        })?;
        let (kind, summary) = match &entry.kind {
            // Redoing: the transactions that undo reverted are live again.
            EntryKind::Undo { of } => {
                for t in of.clone() {
                    self.undone.remove(&t);
                }
                (
                    EntryKind::Redo { of: vec![txn] },
                    entry.summary.as_ref().map(|s| format!("redo: {s}")),
                )
            }
            _ => (
                EntryKind::Undo { of: vec![txn] },
                entry.summary.as_ref().map(|s| format!("undo: {s}")),
            ),
        };
        self.undone.insert(txn);
        let new = self.commit(
            by.clone(),
            kind,
            vec![],
            inverse,
            summary.into_iter().collect(),
        );
        self.project = Arc::new(target);
        Ok(new)
    }

    /// Plain undo (Ctrl+Z): the caller's own most recent transaction.
    pub fn undo(&mut self, author: &Author) -> CmdResult<Option<u64>> {
        match self.last_undoable(author) {
            Some(txn) => self.undo_txn(txn, author).map(Some),
            None => Ok(None),
        }
    }
}
