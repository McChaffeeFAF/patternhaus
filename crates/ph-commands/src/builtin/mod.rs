//! Built-in commands, one module per area.
//!
//! Conventions every command follows:
//! - Parameters are a `#[derive(Deserialize, JsonSchema)]` struct with doc comments on every
//!   field (they become the schema descriptions agents read). Use `#[serde(deny_unknown_fields)]`.
//! - Mutations go through `ph_core::Edit`, returned with `CommandOutput::from_edit` and a
//!   one-line summary written for a musician ("added 4 notes to Kick").
//! - Ids created are returned in `result` so agents can refer to them next.
//! - Commands that must act on the [`crate::Session`] itself (history, solo, opening a
//!   project) are registered here like any other, so they have a schema and a scope, but the
//!   session runs them; see `session_cmd`.

mod audio;
mod automation;
#[cfg(test)]
mod behaviour;
mod channel;
mod file;
pub(crate) mod history;
#[cfg(test)]
mod m2_behaviour;
mod m2_stubs;
mod mixer;
mod note;
mod pattern;
mod playlist;
mod project;
mod query;
mod sample;
mod step;
#[cfg(test)]
mod tests;
mod transport;
mod ui;
mod util;

pub(crate) use channel::solo_channel;
pub(crate) use file::open_project;
pub(crate) use mixer::solo_insert;

use crate::*;

pub fn register_builtin(r: &mut Registry) {
    project::register(r);
    channel::register(r);
    pattern::register(r);
    note::register(r);
    step::register(r);
    playlist::register(r);
    mixer::register(r);
    transport::register(r);
    file::register(r);
    ui::register(r);
    history::register(r);
    query::register(r);
    sample::register(r);
    audio::register(r);
    automation::register(r);
    m2_stubs::register(r);
}
