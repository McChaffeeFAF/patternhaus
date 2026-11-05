//! Haus Sampler importers (M3): other samplers' instrument files to a [`HausInstrument`].
//!
//! Clean room (AGENTS.md, M3 plan): importers are written from the specs in `docs/formats/`
//! only, never from third-party sampler code.
//!
//! Every importer implements [`Importer`]: bytes or a path in; the instrument, the list of
//! samples it plays and an [`ImportReport`] out. Importers **never panic** on any input (each
//! has a fuzz target, `tests/fuzz.rs`), and refuse encrypted or licence-locked content with
//! [`ImportError::Encrypted`]: they detect it and say so, and never try to decrypt it.
//!
//! | extension | format | track |
//! |---|---|---|
//! | `.sfz` | SFZ | M3/T3 |
//! | `.sf2`, `.sf3` | SoundFont 2 (SF3: Ogg Vorbis samples) | M3/T4 |
//! | `.dspreset` | DecentSampler | M3/T4 |
//! | `.exs` | EXS24 | M3/T4 |
//! | `.nki`, `.nkm`, `.nkx`, `.nkc`, `.ncw` | Kontakt (unencrypted only) | M3/T6 |
//!
//! Every importer is a stub in M3/T0 ([`ImportError::Unavailable`]).
//!
//! # What an import produces
//!
//! - The instrument's zones play [`SampleRef::File`](ph_core::SampleRef::File) paths, each
//!   the `name` of one [`ImportedSample`] (`samples/<file>`, relative to the instrument
//!   folder). The caller (`sampler.import`, M3/T3) decides where they live: linked in place
//!   (`folder` = the library folder, when every sample is a plain file there) or written into
//!   the project (`instruments/<name>/samples/`).
//! - [`ImportedSample::source`] says how to get the audio: a file on disk, or a byte range of
//!   a container (SF2 sample data, Kontakt monoliths, NCW).
//! - The report lists what was approximated, skipped or kept for later (scripts and UI data
//!   are kept in `HausInstrument::extra` and reported as not run yet), and every opcode,
//!   chunk or field the importer did not understand.

use ph_core::{HausInstrument, ImportNote, ImportNoteKind};
use std::path::{Path, PathBuf};

mod stubs;

pub use stubs::{DecentSampler, Exs, Kontakt, Sfz, SoundFont};

/// What an importer reads.
#[derive(Copy, Clone, Debug)]
pub enum ImportInput<'a> {
    /// The file's bytes, with its name (for the extension, the report and relative paths;
    /// samples referenced by relative path cannot be found without a folder).
    Bytes { name: &'a str, bytes: &'a [u8] },
    /// A file on disk: the importer reads it (and may read files beside it: `#include`,
    /// samples, monolith sections).
    Path(&'a Path),
}

impl ImportInput<'_> {
    /// The file name, for messages and extension lookup.
    pub fn name(&self) -> String {
        match self {
            ImportInput::Bytes { name, .. } => name.to_string(),
            ImportInput::Path(p) => p.to_string_lossy().into_owned(),
        }
    }
}

/// A successful import.
#[derive(Clone, Debug)]
pub struct Imported {
    /// The instrument (id 0, `folder` unset; `source` and `import_notes` filled).
    pub instrument: HausInstrument,
    /// Every sample its zones play, by the `SampleRef::File` name the zones use.
    pub samples: Vec<ImportedSample>,
    pub report: ImportReport,
}

/// One sample an imported instrument plays.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedSample {
    /// The path the zones use, relative to the instrument folder (`samples/C4.wav`).
    pub name: String,
    pub source: SampleSource,
}

/// Where an imported sample's audio is.
#[derive(Clone, Debug, PartialEq)]
pub enum SampleSource {
    /// An audio file `ph-formats` decodes (WAV, AIFF, FLAC, Ogg, ...).
    File(PathBuf),
    /// A byte range of a container file, in a raw encoding.
    Chunk {
        container: PathBuf,
        offset: u64,
        length: u64,
        encoding: SampleEncoding,
        sample_rate: u32,
        channels: u16,
    },
}

/// Raw sample encodings found inside containers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SampleEncoding {
    /// Signed 16-bit little-endian PCM (SF2 `smpl`).
    Pcm16Le,
    /// Signed 24-bit little-endian PCM (SF2 `smpl` + `sm24`, monoliths).
    Pcm24Le,
    /// 32-bit float little-endian.
    F32Le,
    /// An Ogg Vorbis stream (SF3).
    OggVorbis,
    /// A whole embedded audio file (WAV/AIFF inside a monolith).
    EmbeddedFile,
    /// Native Instruments' compressed audio (NCW), decoded by the Kontakt importer (M3/T6).
    Ncw,
}

/// What an import did, for the person and for agents (`sampler.report`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    /// `sfz`, `sf2`, ...
    pub format: String,
    pub file: String,
    pub groups: usize,
    pub zones: usize,
    /// Approximated, skipped and kept items (also stored on the instrument).
    pub notes: Vec<ImportNote>,
    /// Opcodes, chunks or fields not understood at all, with counts.
    pub unknown: Vec<(String, u32)>,
}

impl ImportReport {
    /// A Markdown summary (the `import-report.md` written beside an imported instrument).
    pub fn to_markdown(&self) -> String {
        let mut s = format!(
            "# Import report: {}\n\nFormat: {}. Groups: {}. Zones: {}.\n",
            self.file, self.format, self.groups, self.zones
        );
        for (title, kind) in [
            ("Approximated", ImportNoteKind::Approximated),
            ("Skipped", ImportNoteKind::Skipped),
            ("Kept, not used yet", ImportNoteKind::Kept),
        ] {
            let items: Vec<_> = self.notes.iter().filter(|n| n.kind == kind).collect();
            if !items.is_empty() {
                s.push_str(&format!("\n## {title}\n\n"));
                for n in items {
                    s.push_str(&format!("- {} ({}×)", n.what, n.count));
                    if !n.detail.is_empty() {
                        s.push_str(&format!(": {}", n.detail));
                    }
                    s.push('\n');
                }
            }
        }
        if !self.unknown.is_empty() {
            s.push_str("\n## Not understood\n\n");
            for (what, n) in &self.unknown {
                s.push_str(&format!("- {what} ({n}×)\n"));
            }
        }
        s
    }
}

/// Why an import failed. Every importer returns one of these instead of panicking.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ImportError {
    /// Encrypted or licence-locked content. Never decrypted (DESIGN §8).
    #[error(
        "{file} is an encrypted or licence-locked {format} instrument ({detail}); Patternhaus does not open protected libraries"
    )]
    Encrypted {
        format: String,
        file: String,
        detail: String,
    },
    /// The importer for this format is a later track.
    #[error("{format} import is not available until {track}")]
    Unavailable { format: String, track: String },
    /// Not a valid file of this format.
    #[error("{file}: not a valid {format} file: {reason}")]
    Malformed {
        format: String,
        file: String,
        reason: String,
    },
    /// A valid file using something the importer cannot represent at all.
    #[error("{file}: {reason}")]
    Unsupported { file: String, reason: String },
    #[error("{path}: {message}")]
    Io { path: String, message: String },
}

/// An instrument file reader.
pub trait Importer: Send + Sync {
    /// Short format id: `sfz`, `sf2`, `dspreset`, `exs`, `nki`.
    fn format(&self) -> &'static str;
    /// Lower-case extensions (without the dot) this importer reads.
    fn extensions(&self) -> &'static [&'static str];
    /// Bumped whenever the importer changes what it maps; stored in
    /// `HausInstrument::source.importer_version`.
    fn version(&self) -> u32;
    /// Reads an instrument. Must not panic on any input.
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError>;
}

/// Every importer, in DESIGN §8 priority order.
pub fn importers() -> &'static [&'static dyn Importer] {
    &[&Sfz, &SoundFont, &DecentSampler, &Exs, &Kontakt]
}

/// The importer for a file name's extension (case-insensitive).
pub fn importer_for(path: &Path) -> Option<&'static dyn Importer> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    importers()
        .iter()
        .copied()
        .find(|i| i.extensions().contains(&ext.as_str()))
}

/// Every extension some importer reads.
pub fn extensions() -> Vec<&'static str> {
    importers()
        .iter()
        .flat_map(|i| i.extensions().iter().copied())
        .collect()
}

/// Imports a file by its extension.
pub fn import_path(path: &Path) -> Result<Imported, ImportError> {
    let importer = importer_for(path).ok_or_else(|| ImportError::Unsupported {
        file: path.to_string_lossy().into_owned(),
        reason: format!(
            "not an instrument format the sampler reads ({})",
            extensions().join(", ")
        ),
    })?;
    importer.import(ImportInput::Path(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_covers_every_extension_once() {
        let all = extensions();
        assert_eq!(
            all,
            [
                "sfz", "sf2", "sf3", "dspreset", "exs", "nki", "nkm", "nkx", "nkc", "ncw"
            ]
        );
        for (ext, format) in [
            ("piano.SFZ", "sfz"),
            ("kit.sf3", "sf2"),
            ("a.dspreset", "dspreset"),
            ("b.exs", "exs"),
            ("c.nkm", "nki"),
            ("d.ncw", "nki"),
        ] {
            assert_eq!(importer_for(Path::new(ext)).unwrap().format(), format);
        }
        assert!(importer_for(Path::new("x.wav")).is_none());
        assert!(importer_for(Path::new("noext")).is_none());
    }

    #[test]
    fn stubs_name_their_track_and_unknown_files_are_refused() {
        for (file, track) in [
            ("a.sfz", "M3/T3"),
            ("a.sf2", "M3/T4"),
            ("a.dspreset", "M3/T4"),
            ("a.exs", "M3/T4"),
            ("a.nki", "M3/T6"),
        ] {
            match import_path(Path::new(file)) {
                Err(ImportError::Unavailable { track: t, .. }) => assert_eq!(t, track),
                other => panic!("{file}: {other:?}"),
            }
        }
        assert!(matches!(
            import_path(Path::new("a.txt")),
            Err(ImportError::Unsupported { .. })
        ));
        let e = ImportError::Encrypted {
            format: "Kontakt".into(),
            file: "x.nki".into(),
            detail: "NI Player library".into(),
        };
        assert!(e.to_string().contains("does not open protected"));
    }

    #[test]
    fn the_report_reads_as_markdown() {
        let r = ImportReport {
            format: "sfz".into(),
            file: "piano.sfz".into(),
            groups: 2,
            zones: 88,
            notes: vec![
                ImportNote {
                    kind: ImportNoteKind::Approximated,
                    what: "opcode:fil_type=lpf_6p".into(),
                    detail: "played as 4-pole".into(),
                    count: 2,
                },
                ImportNote {
                    kind: ImportNoteKind::Kept,
                    what: "script".into(),
                    detail: String::new(),
                    count: 1,
                },
            ],
            unknown: vec![("opcode:foo".into(), 3)],
        };
        let md = r.to_markdown();
        assert!(md.contains("## Approximated\n\n- opcode:fil_type=lpf_6p (2×): played as 4-pole"));
        assert!(md.contains("## Kept, not used yet"));
        assert!(md.contains("- opcode:foo (3×)"));
        assert!(!md.contains("## Skipped"));
    }
}
