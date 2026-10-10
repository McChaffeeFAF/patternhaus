//! Commands: the single way anything happens in Patternhaus.
//!
//! Menus, shortcuts, tools, the CLI, JSON-RPC and MCP all call commands from one
//! [`Registry`] (DESIGN §3). A mutating command returns [`ph_core::PatchSet`]s built with
//! [`ph_core::Edit`]; the [`Session`] applies them, stamps them with an [`Author`], and keeps
//! the history used for per-author undo. Side effects outside the project (transport,
//! rendering, files, UI) go through the [`Host`] port.

mod builtin;
mod host;
mod registry;
mod session;

pub use builtin::register_builtin;
pub use host::*;
pub use registry::*;
pub use session::*;

use ph_core::{ModelError, ObjectPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Who made a change. String form: `human`, `agent:<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Author {
    Human,
    Agent(String),
}

impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Author::Human => write!(f, "human"),
            Author::Agent(n) => write!(f, "agent:{n}"),
        }
    }
}

impl FromStr for Author {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "human" => Ok(Author::Human),
            _ => match s.strip_prefix("agent:") {
                Some(n) if !n.is_empty() => Ok(Author::Agent(n.into())),
                _ => Err(format!("bad author {s:?}: use human or agent:<name>")),
            },
        }
    }
}

impl Serialize for Author {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Author {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// What a client may do (DESIGN §5). A command declares the scope it needs; a client is
/// granted a set of scopes when it connects. `Full` implies all others; every scope
/// implies `Read`.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Query state, render, analyse.
    Read,
    /// Patterns, notes, channels, playlist, transport.
    Compose,
    /// Mixer, effects, automation.
    Mix,
    /// Everything, including files, plugins and project-level operations.
    Full,
}

/// The scopes granted to one client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Grant(pub Vec<Scope>);

impl Grant {
    pub fn full() -> Self {
        Grant(vec![Scope::Full])
    }

    pub fn allows(&self, needed: Scope) -> bool {
        needed == Scope::Read || self.0.contains(&Scope::Full) || self.0.contains(&needed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CmdError {
    #[error("unknown command {0:?}")]
    UnknownCommand(String),
    #[error("bad parameters for {command}: {message}")]
    BadParams { command: String, message: String },
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("{command} needs scope {needed:?}, which this client was not granted")]
    Scope { command: String, needed: Scope },
    #[error("{0} is destructive; repeat with confirm: true")]
    NeedsConfirm(String),
    #[error("undo stopped: {} object(s) were changed since by someone else", .0.len())]
    UndoConflict(Vec<UndoConflict>),
    #[error("{0}")]
    Host(String),
    #[error("{0}")]
    Other(String),
}

/// One object an undo could not revert without overwriting another author's later change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UndoConflict {
    pub path: ObjectPath,
    /// Fields (JSON pointer-ish, `/velocity`) in conflict; empty = the whole object.
    pub fields: Vec<String>,
    /// Who changed it afterwards.
    pub changed_by: Vec<Author>,
}

impl JsonSchema for Author {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Author".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "description": "human | agent:<name>" })
    }
}

pub type CmdResult<T> = Result<T, CmdError>;
