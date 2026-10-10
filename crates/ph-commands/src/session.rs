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
    pub(crate) project: Arc<Project>,
    pub(crate) history: Vec<HistoryEntry>,
    /// Transactions already reverted by an `Undo` entry.
    pub(crate) undone: BTreeSet<u64>,
    next_txn: u64,
    pub(crate) solo: SoloState,
}

impl Session {
    pub fn new(registry: Arc<Registry>, project: Project) -> Self {
        Self {
            registry,
            project: Arc::new(project),
            history: Vec::new(),
            undone: BTreeSet::new(),
            next_txn: 1,
            solo: SoloState::default(),
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
        if reqs
            .iter()
            .any(|r| crate::session_cmd::is_session_command(&r.command))
        {
            return match <[Request; 1]>::try_from(reqs) {
                Ok([req]) => crate::session_cmd::run(self, caller, req, host).map(|e| vec![e]),
                Err(_) => Err(CmdError::Other(
                    "history, solo and file.open commands act on the session itself and cannot be batched"
                        .into(),
                )),
            };
        }
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
        Ok(self.redo_with(author, UndoMode::Stop)?.map(|r| r.txn))
    }

    /// Redo with an explicit conflict policy.
    pub fn redo_with(&mut self, author: &Author, mode: UndoMode) -> CmdResult<Option<UndoReport>> {
        match self.last_redoable(author) {
            Some(txn) => self.undo_txn_with(txn, author, mode).map(Some),
            None => Ok(None),
        }
    }

    /// Plain undo (Ctrl+Z): the caller's own most recent transaction.
    pub fn undo(&mut self, author: &Author) -> CmdResult<Option<u64>> {
        Ok(self.undo_with(author, UndoMode::Stop)?.map(|r| r.txn))
    }

    /// Plain undo with an explicit conflict policy.
    pub fn undo_with(&mut self, author: &Author, mode: UndoMode) -> CmdResult<Option<UndoReport>> {
        match self.last_undoable(author) {
            Some(txn) => self.undo_txn_with(txn, author, mode).map(Some),
            None => Ok(None),
        }
    }

    /// Reverts one transaction as a new `Undo` transaction by `by`, stopping at the first
    /// conflict ([`UndoMode::Stop`]).
    pub fn undo_txn(&mut self, txn: u64, by: &Author) -> CmdResult<u64> {
        self.undo_txn_with(txn, by, UndoMode::Stop).map(|r| r.txn)
    }

    /// Reverts one transaction with the field-level three-way merge of DESIGN §3 (see the
    /// `undo` module). Undoing an `Undo` entry is redo. The new entry is stamped with `by`.
    pub fn undo_txn_with(
        &mut self,
        txn: u64,
        by: &Author,
        mode: UndoMode,
    ) -> CmdResult<UndoReport> {
        let pos = self
            .history
            .iter()
            .position(|e| e.txn == txn)
            .ok_or_else(|| CmdError::Other(format!("no transaction {txn}")))?;
        if self.undone.contains(&txn) {
            return Err(CmdError::Other(format!(
                "transaction {txn} is already undone"
            )));
        }
        let entry = &self.history[pos];
        let revert =
            crate::undo::plan_revert(&self.project, entry, &self.history[pos + 1..], mode)?;
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
        let target = apply(&self.project, &revert.patches)?;
        self.undone.insert(txn);
        let new = self.commit(
            by.clone(),
            kind,
            vec![],
            revert.patches,
            summary.into_iter().collect(),
        );
        self.project = Arc::new(target);
        Ok(UndoReport {
            txn: new,
            conflicts: revert.conflicts,
        })
    }

    /// Reverts every live transaction `target` made, newest first, as `by`. All-or-nothing:
    /// if any step stops on a conflict, nothing changes.
    pub fn undo_author(
        &mut self,
        target: &Author,
        by: &Author,
        mode: UndoMode,
    ) -> CmdResult<Vec<UndoReport>> {
        let saved = (
            self.project.clone(),
            self.history.len(),
            self.undone.clone(),
            self.next_txn,
        );
        let mut reports = Vec::new();
        while let Some(txn) = self.last_undoable(target) {
            match self.undo_txn_with(txn, by, mode) {
                Ok(r) => reports.push(r),
                Err(e) => {
                    self.project = saved.0;
                    self.history.truncate(saved.1);
                    self.undone = saved.2;
                    self.next_txn = saved.3;
                    return Err(e);
                }
            }
        }
        Ok(reports)
    }

    /// Replaces the whole project and forgets the history: opening a different document.
    /// Transaction numbers keep counting up, so they stay unique for the process.
    pub fn reset(&mut self, project: Project) {
        self.project = Arc::new(project);
        self.history.clear();
        self.undone.clear();
        self.solo = SoloState::default();
    }

    /// Solo state (a view of the mix, not part of the saved project).
    pub fn solo(&self) -> &SoloState {
        &self.solo
    }
}

/// Which channels and mixer inserts are soloed. Session-local view state: it is not a model
/// field, so it is not saved, not patched and not undone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SoloState {
    pub channels: BTreeSet<ph_core::ChannelId>,
    pub inserts: BTreeSet<u32>,
}

/// Result of one undo or redo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UndoReport {
    /// The new history entry.
    pub txn: u64,
    /// Conflicts that were skipped (`Skip`) or overridden (`Force`); empty for a clean undo.
    pub conflicts: Vec<UndoConflict>,
}
