//! Patternhaus project model.
//!
//! Pure data. Nothing here touches audio devices, files, threads or UI.
//!
//! - [`Project`] is the whole song. Cheap to clone: collections are `Arc`-shared and copied
//!   on write, so the history, the engine and the UI can all hold snapshots.
//! - Every change is a [`Patch`] on one [`ObjectPath`]. Commands build patches with [`Edit`];
//!   undo, redo, replay and agent preview apply them with [`apply`].

mod ids;
mod model;
mod patch;
mod path;
mod time;

pub use ids::*;
pub use model::*;
pub use patch::*;
pub use path::*;
pub use time::*;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ModelError {
    #[error("no channel with id {0}")]
    NoChannel(ChannelId),
    #[error("no pattern with id {0}")]
    NoPattern(PatternId),
    #[error("no note with id {0}")]
    NoNote(NoteId),
    #[error("no mixer insert {0}")]
    NoInsert(InsertId),
    #[error("no playlist track {0}")]
    NoTrack(usize),
    #[error("no clip with id {0}")]
    NoClip(ClipId),
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
    /// A patch's `before` did not match the object's current value.
    #[error("conflict at {path}: object changed since the patch was made")]
    Conflict { path: ObjectPath },
}

pub type ModelResult<T> = Result<T, ModelError>;

#[cfg(test)]
mod tests;
