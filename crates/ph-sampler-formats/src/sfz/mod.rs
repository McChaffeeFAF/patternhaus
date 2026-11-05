//! The SFZ importer (M3/T3), written from `docs/formats/sfz.md` only (clean room).
//!
//! Pipeline: bytes → text (UTF-8, else Latin-1) → comments stripped, `#include`s expanded
//! (paths relative to the main file's folder, never leaving it) → `#define`s substituted in
//! stream order → tokens → headers and inheritance ([`tree`]) → the Haus Sampler model
//! ([`map`]). Every opcode is either mapped, reported as approximated, skipped or kept, or
//! listed as not understood; nothing is dropped silently. The importer never panics.

pub(crate) mod lex;
pub(crate) mod map;
pub(crate) mod opcodes;
pub(crate) mod tree;

use crate::notes::Notes;
use crate::{ImportError, ImportInput, ImportReport, Imported, Importer};
use ph_core::InstrumentSource;
use std::path::{Path, PathBuf};

pub use map::note_name;
pub use opcodes::parse_key;

/// Bumped whenever the importer changes what it maps.
pub const SFZ_IMPORTER_VERSION: u32 = 1;

/// Largest `.sfz` file read (includes count separately): 64 MiB.
const MAX_FILE: u64 = 64 << 20;

/// SFZ (`docs/formats/sfz.md`): headers, `#define`/`#include`, v1 and common ARIA/v2
/// opcodes; unknown opcodes listed in the report.
#[derive(Copy, Clone, Debug, Default)]
pub struct Sfz;

impl Importer for Sfz {
    fn format(&self) -> &'static str {
        "sfz"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["sfz"]
    }
    fn version(&self) -> u32 {
        SFZ_IMPORTER_VERSION
    }
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        let file = input.name();
        let (bytes, dir): (Vec<u8>, Option<PathBuf>) = match input {
            ImportInput::Bytes { bytes, .. } => (bytes.to_vec(), None),
            ImportInput::Path(p) => {
                let io = |e: std::io::Error| ImportError::Io {
                    path: p.display().to_string(),
                    message: e.to_string(),
                };
                let meta = std::fs::metadata(p).map_err(io)?;
                if meta.len() > MAX_FILE {
                    return Err(ImportError::Malformed {
                        format: "SFZ".into(),
                        file,
                        reason: "the file is larger than 64 MiB".into(),
                    });
                }
                let bytes = std::fs::read(p).map_err(io)?;
                let dir = p.parent().map(|d| {
                    if d.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        d.to_path_buf()
                    }
                });
                (bytes, dir)
            }
        };
        import_text(&bytes, &file, dir.as_deref())
    }
}

/// Imports SFZ bytes; `dir` is the main file's folder (includes and samples resolve there).
pub fn import_text(bytes: &[u8], file: &str, dir: Option<&Path>) -> Result<Imported, ImportError> {
    let mut notes = Notes::default();
    let text = lex::decode_text(bytes, file, &mut notes);
    let main = dir.map(|d| d.join(Path::new(file).file_name().unwrap_or_default()));
    let tokens = lex::tokens(&text, &lex::Includes { root: dir }, main, &mut notes);
    let parsed = tree::build(tokens, &mut notes);
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "SFZ instrument".into());
    let mapped = map::map(&parsed, &stem, dir, &mut notes);
    let mut instrument = mapped.instrument;
    let (notes, unknown) = notes.finish();
    instrument.import_notes = notes.clone();
    instrument.source = Some(InstrumentSource {
        format: "sfz".into(),
        file: file.to_string(),
        importer_version: SFZ_IMPORTER_VERSION,
    });
    let report = ImportReport {
        format: "sfz".into(),
        file: file.to_string(),
        groups: instrument.groups.len(),
        zones: instrument.zones.len(),
        notes,
        unknown,
        presets: Vec::new(),
    };
    Ok(Imported {
        instrument,
        samples: mapped.samples,
        report,
    })
}
