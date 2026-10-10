//! `history.*`: undo, redo and the history log.
//!
//! These act on the [`Session`] (they read and extend its history), which a command's
//! `CommandCtx` cannot reach. They are registered here like every other command, so they have
//! a schema, a scope and appear in `list_commands`, and the session runs them itself through
//! the `session_cmd` path.

use crate::session_cmd::{SessionOut, parse, stub};
use crate::*;
use ph_core::ObjectPath;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn register(r: &mut Registry) {
    r.register(typed(
        "history.undo",
        "Undo your own most recent change (other authors' work is left alone).",
        Scope::Compose,
        READ,
        stub::<ModeOnly>,
    ));
    r.register(typed(
        "history.redo",
        "Redo your own most recent undo.",
        Scope::Compose,
        READ,
        stub::<ModeOnly>,
    ));
    r.register(typed(
        "history.undo_author",
        "Revert all live changes by one author, newest first (e.g. everything an agent did).",
        Scope::Full,
        READ,
        stub::<UndoAuthor>,
    ));
    r.register(typed(
        "history.undo_txn",
        "Revert one transaction by number, wherever it sits in the history.",
        Scope::Full,
        READ,
        stub::<UndoTxn>,
    ));
    r.register(typed(
        "history.list",
        "List recent history entries: who did what, and whether it has been undone.",
        Scope::Read,
        READ,
        stub::<List>,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModeOnly {
    /// What to do if someone else changed the same thing since: `stop` (default) cancels and
    /// reports the conflicts, `skip` reverts everything else, `force` overwrites their change.
    #[serde(default)]
    mode: UndoMode,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoAuthor {
    /// Whose work to revert: `human` or `agent:<name>`.
    author: Author,
    /// Conflict policy: `stop` (default), `skip` or `force`.
    #[serde(default)]
    mode: UndoMode,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoTxn {
    /// Transaction number from `history.list`. Undoing an undo is redo.
    txn: u64,
    /// Conflict policy: `stop` (default), `skip` or `force`.
    #[serde(default)]
    mode: UndoMode,
}

fn default_limit() -> usize {
    50
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct List {
    /// Only entries by this author.
    #[serde(default)]
    author: Option<Author>,
    /// At most this many (the most recent), default 50.
    #[serde(default = "default_limit")]
    limit: usize,
}

fn paths_of(s: &Session, txn: u64) -> Vec<ObjectPath> {
    s.history()
        .iter()
        .find(|e| e.txn == txn)
        .map(|e| e.patches.paths())
        .unwrap_or_default()
}

fn describe(s: &Session, what: &str, reports: &[UndoReport]) -> SessionOut {
    let mut changed = Vec::new();
    for r in reports {
        for p in paths_of(s, r.txn) {
            if !changed.contains(&p) {
                changed.push(p);
            }
        }
    }
    let skipped: usize = reports.iter().map(|r| r.conflicts.len()).sum();
    let mut summary = match reports.len() {
        0 => format!("nothing to {what}"),
        1 => format!("{what} 1 change"),
        n => format!("{what} {n} changes"),
    };
    if skipped > 0 {
        summary.push_str(&format!(
            " ({skipped} conflicting object(s) handled as asked)"
        ));
    }
    SessionOut {
        result: json!({
            "reverted": reports.len(),
            "txns": reports.iter().map(|r| r.txn).collect::<Vec<_>>(),
            "conflicts": reports.iter().flat_map(|r| r.conflicts.clone()).collect::<Vec<_>>(),
        }),
        summary: Some(summary),
        changed,
        txn: reports.last().map(|r| r.txn),
    }
}

pub(crate) fn undo(s: &mut Session, caller: &Caller, params: Value) -> CmdResult<SessionOut> {
    let p: ModeOnly = parse("history.undo", params)?;
    let reports: Vec<_> = s.undo_with(&caller.author, p.mode)?.into_iter().collect();
    Ok(describe(s, "undid", &reports))
}

pub(crate) fn redo(s: &mut Session, caller: &Caller, params: Value) -> CmdResult<SessionOut> {
    let p: ModeOnly = parse("history.redo", params)?;
    let reports: Vec<_> = s.redo_with(&caller.author, p.mode)?.into_iter().collect();
    Ok(describe(s, "redid", &reports))
}

pub(crate) fn undo_author(
    s: &mut Session,
    caller: &Caller,
    params: Value,
) -> CmdResult<SessionOut> {
    let p: UndoAuthor = parse("history.undo_author", params)?;
    let reports = s.undo_author(&p.author, &caller.author, p.mode)?;
    Ok(describe(
        s,
        &format!("reverted {}'s work:", p.author),
        &reports,
    ))
}

pub(crate) fn undo_txn(s: &mut Session, caller: &Caller, params: Value) -> CmdResult<SessionOut> {
    let p: UndoTxn = parse("history.undo_txn", params)?;
    let report = s.undo_txn_with(p.txn, &caller.author, p.mode)?;
    Ok(describe(
        s,
        &format!("reverted transaction {}:", p.txn),
        &[report],
    ))
}

pub(crate) fn list(s: &mut Session, params: Value) -> CmdResult<SessionOut> {
    let p: List = parse("history.list", params)?;
    let rows: Vec<Value> = s
        .history()
        .iter()
        .filter(|e| p.author.as_ref().is_none_or(|a| a == &e.author))
        .map(|e| {
            let live = matches!(e.kind, EntryKind::Do) && !s.undone.contains(&e.txn);
            json!({
                "txn": e.txn,
                "author": e.author,
                "kind": e.kind,
                "summary": e.summary,
                "commands": e.commands.iter().map(|c| c.command.as_str()).collect::<Vec<_>>(),
                "paths": e.patches.paths(),
                "undone": s.undone.contains(&e.txn),
                "live": live,
            })
        })
        .collect();
    let skip = rows.len().saturating_sub(p.limit);
    let rows: Vec<Value> = rows.into_iter().skip(skip).collect();
    Ok(SessionOut {
        result: json!({ "entries": rows }),
        ..Default::default()
    })
}
