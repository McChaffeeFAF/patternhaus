//! FL Studio project (`.flp`) import, implemented from `docs/formats/flp.md` alone (clean
//! room: no other FLP parser or write-up was consulted).
//!
//! Three layers, so each can be tested alone:
//! - [`parse`]: the container, bytes to events. Total: any input gives events or an error;
//! - [`read`]: events to typed data using the spec's context rules ([`FlpFile`]);
//! - [`import_flp`] / [`map`]: typed data to a [`ph_core::Project`], plus plugin state files
//!   to save and an [`ImportReport`] listing everything approximated or skipped.
//!
//! [`build::FlpBuilder`] writes synthetic FLP files for tests and fuzz seeds.

pub mod build;
mod import;
mod parse;
mod read;
mod report;

#[cfg(test)]
mod tests;

pub use import::*;
pub use parse::{Event, FlpError, Header, Payload, RawFlp, parse};
pub use read::*;
pub use report::*;
