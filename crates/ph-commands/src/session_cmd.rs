//! Commands the [`Session`] runs itself.
//!
//! A normal command sees only `CommandCtx` (a project snapshot, the author, the host) and
//! returns patches. A few commands must act on the session instead:
//!
//! - `history.*` read and extend the session's history log;
//! - `channel.solo` and `mixer.solo` change session-local view state (solo is not a model
//!   field, so it has no patches);
//! - `file.open` and `file.import_flp` replace the whole project and start a new history.
//!
//! They are registered in the [`Registry`] like every other command, with a params schema,
//! a scope and the destructive flag, so listing, describing, scope checks and `confirm` all
//! work the same way. Only the `run` of the registered entry is a stub; `Session::batch`
//! sees the command id in [`is_session_command`] and calls [`run`] instead. Because they
//! carry their own transaction semantics they cannot be mixed into a batch.

use crate::builtin::{self, history};
use crate::*;
use ph_core::ObjectPath;
use serde::de::DeserializeOwned;
use serde_json::Value;

const SESSION_COMMANDS: &[&str] = &[
    "history.undo",
    "history.redo",
    "history.undo_author",
    "history.undo_txn",
    "history.list",
    "channel.solo",
    "mixer.solo",
    "file.open",
    "file.import_flp",
];

/// Every session-handled command id (the registry must have an entry for each).
#[cfg(test)]
pub(crate) fn session_command_ids() -> &'static [&'static str] {
    SESSION_COMMANDS
}

pub(crate) fn is_session_command(id: &str) -> bool {
    SESSION_COMMANDS.contains(&id)
}

/// What a session command reports.
#[derive(Default)]
pub(crate) struct SessionOut {
    pub result: Value,
    pub summary: Option<String>,
    pub changed: Vec<ObjectPath>,
    pub txn: Option<u64>,
}

/// The registered `run` of a session command. Reaching it means someone called the entry
/// through the registry directly instead of through a [`Session`].
pub(crate) fn stub<P>(_: &mut CommandCtx<'_>, _: P) -> CmdResult<CommandOutput> {
    Err(CmdError::Other(
        "this command acts on the session; run it with Session::execute".into(),
    ))
}

/// Parses a params value (null = no params) into `P`.
pub(crate) fn parse<P: DeserializeOwned>(command: &str, params: Value) -> CmdResult<P> {
    let params = if params.is_null() {
        Value::Object(Default::default())
    } else {
        params
    };
    serde_json::from_value(params).map_err(|e| CmdError::BadParams {
        command: command.into(),
        message: e.to_string(),
    })
}

pub(crate) fn run(
    s: &mut Session,
    caller: &Caller,
    req: Request,
    host: &mut dyn Host,
) -> CmdResult<Executed> {
    let registry = s.registry().clone();
    let spec = registry.get(&req.command)?.spec();
    if !caller.grant.allows(spec.scope) {
        return Err(CmdError::Scope {
            command: spec.id.into(),
            needed: spec.scope,
        });
    }
    if spec.destructive && !req.confirm {
        return Err(CmdError::NeedsConfirm(spec.id.into()));
    }
    let params = req.params;
    let out = match spec.id {
        "history.undo" => history::undo(s, caller, params),
        "history.redo" => history::redo(s, caller, params),
        "history.undo_author" => history::undo_author(s, caller, params),
        "history.undo_txn" => history::undo_txn(s, caller, params),
        "history.list" => history::list(s, params),
        "channel.solo" => builtin::solo_channel(s, params),
        "mixer.solo" => builtin::solo_insert(s, params),
        "file.open" => builtin::open_project(s, host, &caller.author, params),
        "file.import_flp" => builtin::import_flp_project(s, host, &caller.author, params),
        other => Err(CmdError::Other(format!("{other} is not a session command"))),
    }?;
    Ok(Executed {
        command: spec.id.into(),
        result: out.result,
        summary: out.summary,
        changed: out.changed,
        txn: out.txn,
    })
}
