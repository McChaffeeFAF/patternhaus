use crate::*;
use ph_core::{Edit, PatchSet, Project};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;

/// Static description of a command. This is what `list_commands` / `describe_command`
/// return to agents and what the keymap and menus refer to.
#[derive(Clone, Debug, Serialize)]
pub struct CommandSpec {
    /// Dotted id: `<area>.<verb>[_<object>]`, e.g. `pattern.add_note`, `mixer.set_volume`.
    pub id: &'static str,
    /// One sentence, imperative, shown in menus, the command palette and to agents.
    pub summary: &'static str,
    pub scope: Scope,
    /// Changes the project (produces patches).
    pub mutating: bool,
    /// Needs `confirm: true` (deletes data, overwrites files, discards changes).
    pub destructive: bool,
    /// JSON Schema of the parameters object.
    pub params: Value,
}

/// Everything a command can see and touch while it runs.
pub struct CommandCtx<'a> {
    /// The project as of this command (including earlier commands in the same batch).
    pub project: &'a Project,
    pub author: &'a Author,
    pub host: &'a mut dyn Host,
}

/// What a command returns.
#[derive(Debug, Default)]
pub struct CommandOutput {
    /// JSON result for the caller (ids created, values read, ...).
    pub result: Value,
    /// Changes to the project. Empty for read-only commands.
    pub patches: PatchSet,
    /// The project with `patches` applied, when the command already has it (from
    /// [`Edit::finish`]); saves the session re-applying them.
    pub project: Option<Project>,
    /// Short human/agent-readable description of what changed ("added 16 notes to Kick").
    pub summary: Option<String>,
}

impl CommandOutput {
    pub fn value(result: impl Serialize) -> Self {
        Self {
            result: serde_json::to_value(result).unwrap_or(Value::Null),
            ..Default::default()
        }
    }

    pub fn from_edit(edit: Edit, result: impl Serialize, summary: impl Into<String>) -> Self {
        let (project, patches) = edit.finish();
        Self {
            result: serde_json::to_value(result).unwrap_or(Value::Null),
            patches,
            project: Some(project),
            summary: Some(summary.into()),
        }
    }
}

pub trait Command: Send + Sync {
    fn spec(&self) -> &CommandSpec;
    fn run(&self, ctx: &mut CommandCtx<'_>, params: Value) -> CmdResult<CommandOutput>;
}

/// Flags for [`typed`].
#[derive(Copy, Clone, Debug, Default)]
pub struct Flags {
    pub mutating: bool,
    pub destructive: bool,
}

pub const READ: Flags = Flags {
    mutating: false,
    destructive: false,
};
pub const MUTATES: Flags = Flags {
    mutating: true,
    destructive: false,
};
pub const DESTRUCTIVE: Flags = Flags {
    mutating: true,
    destructive: true,
};

struct Typed<P> {
    spec: CommandSpec,
    f: fn(&mut CommandCtx<'_>, P) -> CmdResult<CommandOutput>,
}

impl<P: DeserializeOwned + Send + Sync + 'static> Command for Typed<P> {
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    fn run(&self, ctx: &mut CommandCtx<'_>, params: Value) -> CmdResult<CommandOutput> {
        let params = if params.is_null() {
            Value::Object(Default::default())
        } else {
            params
        };
        let p: P = serde_json::from_value(params).map_err(|e| CmdError::BadParams {
            command: self.spec.id.into(),
            message: e.to_string(),
        })?;
        (self.f)(ctx, p)
    }
}

/// Defines a command from a typed parameter struct and a plain function. The parameter
/// schema is derived from `P`, so docs, MCP schemas and validation can never drift apart.
pub fn typed<P: DeserializeOwned + JsonSchema + Send + Sync + 'static>(
    id: &'static str,
    summary: &'static str,
    scope: Scope,
    flags: Flags,
    f: fn(&mut CommandCtx<'_>, P) -> CmdResult<CommandOutput>,
) -> Box<dyn Command> {
    let params = serde_json::to_value(schemars::schema_for!(P)).expect("schema serializes");
    Box::new(Typed {
        spec: CommandSpec {
            id,
            summary,
            scope,
            mutating: flags.mutating,
            destructive: flags.destructive,
            params,
        },
        f,
    })
}

/// All commands, by id.
#[derive(Default)]
pub struct Registry {
    commands: BTreeMap<&'static str, Box<dyn Command>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with every built-in command.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        register_builtin(&mut r);
        r
    }

    /// Panics on a duplicate id: that is a programming error caught by tests.
    pub fn register(&mut self, cmd: Box<dyn Command>) {
        let id = cmd.spec().id;
        assert!(
            valid_id(id),
            "command id {id:?} must be <area>.<verb>, lowercase snake_case"
        );
        assert!(
            self.commands.insert(id, cmd).is_none(),
            "duplicate command id {id}"
        );
    }

    pub fn get(&self, id: &str) -> CmdResult<&dyn Command> {
        self.commands
            .get(id)
            .map(|c| &**c)
            .ok_or_else(|| CmdError::UnknownCommand(id.into()))
    }

    /// Specs whose id or summary contains `filter` (case-insensitive); all when empty.
    pub fn list(&self, filter: &str) -> Vec<&CommandSpec> {
        let f = filter.to_lowercase();
        self.commands
            .values()
            .map(|c| c.spec())
            .filter(|s| f.is_empty() || s.id.contains(&f) || s.summary.to_lowercase().contains(&f))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

fn valid_id(id: &str) -> bool {
    let mut parts = id.split('.');
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if ok(a) && ok(b))
}
