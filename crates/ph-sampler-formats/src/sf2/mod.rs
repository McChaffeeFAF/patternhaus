//! The SoundFont 2 / SF3 importer (M3/T4), written from `docs/formats/sf2.md` only (clean
//! room).
//!
//! Pipeline: RIFF and hydra records with the structural rejection rules ([`riff`]) →
//! zones with the generator ordering rules, summing and the modulator hierarchy ([`gens`])
//! → one preset to the Haus Sampler model ([`map`]). A file holds many presets; an import
//! reads **one** ([`PresetChoice`], default the first by bank then program) and the report
//! lists them all ([`crate::ImportReport::presets`]).
//!
//! **Where samples go:** an SF2 has no sample files. Each sample a zone plays is cut out of
//! `smpl` (24-bit with `sm24`; SF3 Ogg Vorbis streams decoded by `ph-formats`; stereo pairs
//! joined) and handed over as [`crate::SampleSource::Audio`]; `sampler.import` writes them
//! as float WAV files into `<project>/instruments/<name>/samples/` (always collected).

pub(crate) mod gens;
pub(crate) mod map;
pub(crate) mod riff;

use crate::notes::Notes;
use crate::{ImportError, ImportInput, ImportReport, Imported, Importer, PresetListing};
use ph_core::InstrumentSource;
use std::path::Path;

/// Bumped whenever the importer changes what it maps.
pub const SF2_IMPORTER_VERSION: u32 = 1;

/// Largest SoundFont read: 2 GiB (the RIFF size field's reach).
const MAX_FILE: u64 = 2 << 30;

/// Which preset of a SoundFont to import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PresetChoice {
    /// The first preset by bank, then program (presets without zones are passed over).
    #[default]
    First,
    /// A MIDI bank (128 = percussion) and program; the first such preset in the file.
    BankProgram { bank: u16, program: u16 },
    /// A preset name (exact, then ignoring case and surrounding spaces).
    Name(String),
}

impl PresetChoice {
    /// Parses `bank:program` (`128:0`) or else a name.
    pub fn parse(s: &str) -> Self {
        let t = s.trim();
        if let Some((b, p)) = t.split_once(':')
            && let (Ok(bank), Ok(program)) = (b.trim().parse(), p.trim().parse())
        {
            return PresetChoice::BankProgram { bank, program };
        }
        if t.is_empty() {
            PresetChoice::First
        } else {
            PresetChoice::Name(t.to_string())
        }
    }
}

/// SoundFont 2.04 and SF3 (`docs/formats/sf2.md`).
#[derive(Copy, Clone, Debug, Default)]
pub struct SoundFont;

impl Importer for SoundFont {
    fn format(&self) -> &'static str {
        "sf2"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["sf2", "sf3"]
    }
    fn version(&self) -> u32 {
        SF2_IMPORTER_VERSION
    }
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        import_preset(input, &PresetChoice::First)
    }
}

fn malformed(file: &str, reason: impl Into<String>) -> ImportError {
    ImportError::Malformed {
        format: "SoundFont".into(),
        file: file.to_string(),
        reason: reason.into(),
    }
}

/// Imports one preset of a SoundFont.
pub fn import_preset(
    input: ImportInput<'_>,
    choice: &PresetChoice,
) -> Result<Imported, ImportError> {
    let file = input.name();
    let owned;
    let bytes: &[u8] = match input {
        ImportInput::Bytes { bytes, .. } => bytes,
        ImportInput::Path(p) => {
            let io = |e: std::io::Error| ImportError::Io {
                path: p.display().to_string(),
                message: e.to_string(),
            };
            if std::fs::metadata(p).map_err(io)?.len() > MAX_FILE {
                return Err(malformed(&file, "the file is larger than 2 GiB"));
            }
            owned = std::fs::read(p).map_err(io)?;
            &owned
        }
    };
    import_bytes(bytes, &file, choice)
}

/// The presets of a SoundFont, in file order (bank, program, name).
pub fn list_presets(bytes: &[u8]) -> Result<Vec<(u16, u16, String)>, String> {
    let mut notes = Notes::default();
    let sf = riff::read(bytes, &mut notes)?;
    Ok(sf
        .presets
        .iter()
        .map(|p| (p.bank, p.program, p.name.clone()))
        .collect())
}

fn import_bytes(bytes: &[u8], file: &str, choice: &PresetChoice) -> Result<Imported, ImportError> {
    let mut notes = Notes::default();
    let sf = riff::read(bytes, &mut notes).map_err(|r| malformed(file, r))?;
    gens::check_indices(&sf).map_err(|r| malformed(file, r))?;
    if sf.info.rom.is_none() && sf.samples.iter().any(|s| s.kind & 0x8000 != 0) {
        notes.skip(
            "chunk:irom",
            "ROM samples without a named sound ROM (irom): the file is defective; those zones are skipped",
        );
    }
    let has_zones = |i: usize| sf.presets[i].bag_end > sf.presets[i].bag;
    let chosen = match choice {
        PresetChoice::First => {
            let mut order: Vec<usize> = (0..sf.presets.len()).filter(|&i| has_zones(i)).collect();
            order.sort_by_key(|&i| (sf.presets[i].bank, sf.presets[i].program));
            order.first().copied()
        }
        PresetChoice::BankProgram { bank, program } => (0..sf.presets.len())
            .find(|&i| sf.presets[i].bank == *bank && sf.presets[i].program == *program),
        PresetChoice::Name(n) => (0..sf.presets.len())
            .find(|&i| sf.presets[i].name == *n)
            .or_else(|| {
                (0..sf.presets.len())
                    .find(|&i| sf.presets[i].name.trim().eq_ignore_ascii_case(n.trim()))
            }),
    };
    let listing = |imported: Option<usize>| -> Vec<PresetListing> {
        sf.presets
            .iter()
            .enumerate()
            .map(|(i, p)| PresetListing {
                bank: p.bank,
                program: p.program,
                name: p.name.clone(),
                imported: Some(i) == imported,
            })
            .collect()
    };
    let Some(chosen) = chosen else {
        let names: Vec<String> = listing(None)
            .iter()
            .map(|p| format!("{}:{} {}", p.bank, p.program, p.name))
            .collect();
        return Err(ImportError::Unsupported {
            file: file.to_string(),
            reason: match choice {
                PresetChoice::First => "the SoundFont has no preset with zones".into(),
                _ => format!(
                    "no preset {choice:?} in the SoundFont; it has: {}",
                    names.join(", ")
                ),
            },
        });
    };
    let dupes = sf
        .presets
        .iter()
        .filter(|p| p.bank == sf.presets[chosen].bank && p.program == sf.presets[chosen].program)
        .count();
    if dupes > 1 {
        notes.approx(
            "preset:duplicate",
            "several presets share this bank and program; the first is the active one",
        );
    }
    let mapped = map::map(&sf, chosen, &mut notes);
    let format = if sf.is_sf3() { "sf3" } else { "sf2" };
    let mut instrument = mapped.instrument;
    if instrument.name.trim().is_empty() {
        instrument.name = Path::new(file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SoundFont".into());
    }
    let (notes, unknown) = notes.finish();
    instrument.import_notes = notes.clone();
    instrument.source = Some(InstrumentSource {
        format: format.into(),
        file: file.to_string(),
        importer_version: SF2_IMPORTER_VERSION,
    });
    let report = ImportReport {
        format: format.into(),
        file: file.to_string(),
        groups: instrument.groups.len(),
        zones: instrument.zones.len(),
        notes,
        unknown,
        presets: listing(Some(chosen)),
    };
    Ok(Imported {
        instrument,
        samples: mapped.samples,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_choices_parse() {
        assert_eq!(
            PresetChoice::parse("128:0"),
            PresetChoice::BankProgram {
                bank: 128,
                program: 0
            }
        );
        assert_eq!(PresetChoice::parse(" "), PresetChoice::First);
        assert_eq!(
            PresetChoice::parse("Grand Piano"),
            PresetChoice::Name("Grand Piano".into())
        );
        assert_eq!(PresetChoice::parse("a:b"), PresetChoice::Name("a:b".into()));
    }
}
