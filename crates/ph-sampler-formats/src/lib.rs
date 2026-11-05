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
//! | `.dspreset`, `.dsbundle` (folder), `.dslibrary` (refused: zip) | DecentSampler | M3/T4 |
//! | `.exs` | EXS24 | M3/T4 |
//! | `.nki`, `.nkm`, `.nkx`, `.nkc`, `.ncw` | Kontakt (unencrypted only) | M3/T6 |
//!
//! SFZ (M3/T3, [`Sfz`]), SF2/SF3 ([`SoundFont`]), DecentSampler ([`DecentSampler`]) and EXS24
//! ([`Exs`]) (M3/T4) are implemented; Kontakt is a stub until M3/T6
//! ([`ImportError::Unavailable`]). [`automap`] maps a folder of samples by the note names in
//! their file names (drop to create). [`build`] holds the byte-level fixture writers.
//!
//! # What an import produces
//!
//! - The instrument's zones play [`SampleRef::File`](ph_core::SampleRef::File) paths, each
//!   the `name` of one [`ImportedSample`] (`samples/<file>`, relative to the instrument
//!   folder). The caller (`sampler.import`, M3/T3) decides where they live: linked in place
//!   (`folder` = the library folder, when every sample is a plain file there) or written into
//!   the project (`instruments/<name>/samples/`).
//! - [`ImportedSample::source`] says how to get the audio: a file on disk, audio decoded
//!   from inside the instrument file (SF2/SF3 samples, written out as WAV by the caller), a
//!   synthesised sample, or a byte range of a container (Kontakt monoliths, NCW).
//! - The report lists what was approximated, skipped or kept for later (scripts and UI data
//!   are kept in `HausInstrument::extra` and reported as not run yet), and every opcode,
//!   chunk or field the importer did not understand.

use ph_core::{HausInstrument, ImportNote, ImportNoteKind};
use std::path::{Path, PathBuf};

mod assemble;
pub mod automap;
pub mod build;
pub mod dspreset;
pub mod exs;
mod notes;
pub mod probe;
pub mod sf2;
pub mod sfz;
mod stubs;

pub use dspreset::DecentSampler;
pub use exs::Exs;
pub use sf2::{PresetChoice, SoundFont};
pub use sfz::Sfz;
pub use stubs::Kontakt;

/// A float WAV file holding `audio` (for [`SampleSource::Audio`]).
pub fn audio_wav(audio: &ph_node::SampleData) -> Result<Vec<u8>, String> {
    let rate = audio.sample_rate.round().clamp(1.0, 1_000_000.0) as u32;
    ph_formats::encode::encode_wav(
        &audio.channels,
        rate,
        ph_formats::encode::WavFormat::Float32,
        ph_formats::encode::DITHER_SEED,
    )
    .map_err(|e| e.to_string())
}

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
    /// Audio the importer synthesises (SFZ `*sine`, `*silence`); the caller writes it as a
    /// WAV file ([`Generated::render`]) when it saves or collects the instrument.
    Generated(Generated),
    /// Audio the importer decoded from inside the instrument file (SF2/SF3 sample data,
    /// stereo pairs joined into one sample). The caller writes it as a float WAV
    /// ([`audio_wav`]) into the instrument's `samples/` folder: such instruments are always
    /// collected. Loop and root fields are unused (zones carry them).
    Audio(std::sync::Arc<ph_node::SampleData>),
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

/// A synthetic sample (SFZ built-in generators, `docs/formats/sfz.md` §3.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
// SPEC-GAP: §3.4 does not say what `*sine` sounds like (level, length, pitch at the root);
// a looped half-scale sine tuned so the zone's root key plays its own pitch.
pub enum Generated {
    /// A looped sine: [`Generated::SINE_HZ`] at [`Generated::RATE`], one second, whole-file
    /// loop. Zones playing it are tuned so their root key sounds at its own pitch.
    Sine,
    /// A tenth of a second of silence.
    Silence,
}

impl Generated {
    pub const RATE: u32 = 48_000;
    /// A whole number of cycles per second, so the whole-file loop is seamless.
    pub const SINE_HZ: f64 = 375.0;

    /// The zone name the importers use (`generated/sine.wav`).
    pub fn name(self) -> &'static str {
        match self {
            Generated::Sine => "generated/sine.wav",
            Generated::Silence => "generated/silence.wav",
        }
    }

    /// Cents that bring the sine from [`Self::SINE_HZ`] to middle C (C4 = 60, 261.63 Hz), so
    /// a zone with root key `k` plays key `k`'s pitch at `k`.
    pub fn sine_correction_cents() -> f64 {
        let c4 = 440.0 * 2f64.powf(-9.0 / 12.0);
        -1200.0 * (Self::SINE_HZ / c4).log2()
    }

    /// Mono audio at [`Self::RATE`].
    pub fn render(self) -> Vec<f32> {
        match self {
            Generated::Sine => (0..Self::RATE as usize)
                .map(|i| {
                    (0.5 * (std::f64::consts::TAU * Self::SINE_HZ * i as f64 / Self::RATE as f64)
                        .sin()) as f32
                })
                .collect(),
            Generated::Silence => vec![0.0; Self::RATE as usize / 10],
        }
    }

    /// The facts an importer needs (length, rate, loop).
    pub fn info(self) -> probe::SampleInfo {
        let frames = match self {
            Generated::Sine => Self::RATE as u64,
            Generated::Silence => Self::RATE as u64 / 10,
        };
        probe::SampleInfo {
            frames,
            sample_rate: Self::RATE,
            loop_range: matches!(self, Generated::Sine).then_some((0, frames)),
            root_key: None,
        }
    }
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
    /// Multi-preset files (SF2/SF3): every preset in the file, in file order. Empty for
    /// formats with one instrument per file.
    pub presets: Vec<PresetListing>,
}

/// One preset of a multi-preset file (SF2 `phdr`).
#[derive(Clone, Debug, PartialEq)]
pub struct PresetListing {
    /// MIDI bank (128 = the percussion bank).
    pub bank: u16,
    pub program: u16,
    pub name: String,
    /// The preset this import read.
    pub imported: bool,
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
        if !self.presets.is_empty() {
            s.push_str("\n## Presets in the file\n\n");
            for p in &self.presets {
                s.push_str(&format!(
                    "- {:03}:{:03} {}{}\n",
                    p.bank,
                    p.program,
                    p.name,
                    if p.imported { " (imported)" } else { "" }
                ));
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
                "sfz",
                "sf2",
                "sf3",
                "dspreset",
                "dsbundle",
                "dslibrary",
                "exs",
                "nki",
                "nkm",
                "nkx",
                "nkc",
                "ncw"
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
        for (file, track) in [("a.dspreset", "M3/T4"), ("a.nki", "M3/T6")] {
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
            presets: vec![],
        };
        let md = r.to_markdown();
        assert!(md.contains("## Approximated\n\n- opcode:fil_type=lpf_6p (2×): played as 4-pole"));
        assert!(md.contains("## Kept, not used yet"));
        assert!(md.contains("- opcode:foo (3×)"));
        assert!(!md.contains("## Skipped"));
    }
}
