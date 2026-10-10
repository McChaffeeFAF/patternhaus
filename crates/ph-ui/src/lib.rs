//! Patternhaus desktop UI (DESIGN §9): egui panels over the `Studio` command interface.
//!
//! **Commands only.** Nothing here mutates a `Project`; every action is built in
//! [`mapping`] and submitted through `studio.session` as `Caller::human()`.

pub mod mapping;
pub mod piano_roll;
pub mod theme;
