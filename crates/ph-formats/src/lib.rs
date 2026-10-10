//! Patternhaus file formats.
//!
//! - the native project folder (DESIGN §10): save/load with migrations;
//! - MIDI file import/export;
//! - [`decode`]: audio decoding to [`ph_node::SampleData`] (WAV, AIFF, FLAC, MP3, Ogg Vorbis)
//!   via `symphonia`, plus WAV `smpl` loop points and root key.

pub mod decode;
mod folder;
mod midi;

pub use folder::*;
pub use midi::*;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("project.json is not valid: {0}")]
    Json(String),
    #[error(
        "project format version {found} is newer than this build understands ({supported}); update Patternhaus"
    )]
    TooNew { found: u64, supported: u32 },
    #[error("project format version {0} is not valid")]
    BadVersion(u64),
    #[error("project is inconsistent: {0}")]
    Invalid(String),
    #[error("midi: {0}")]
    Midi(String),
    #[error(transparent)]
    Model(#[from] ph_core::ModelError),
}

pub type FormatResult<T> = Result<T, FormatError>;
