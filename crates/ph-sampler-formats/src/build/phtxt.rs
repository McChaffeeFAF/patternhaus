//! `.phtxt`, a made-up text instrument format for tests of the importer hook (M3/T6): an
//! importer that is not built in, registered at runtime (`Importers::register`). Never
//! registered by Patternhaus itself.

use crate::{
    ImportError, ImportInput, ImportReport, Imported, ImportedSample, Importer, SampleSource,
};
use ph_core::{
    Group, GroupId, HausInstrument, ImportNote, ImportNoteKind, InstrumentSource, KeyRange,
    SampleRef, VelRange, Zone, ZoneId,
};
use std::path::Path;

/// `.phtxt`: one statement per line, `#` comments.
/// - `name <text>`;
/// - `group <name> [volume=<dB>]` starts a group;
/// - `zone sample=<file> lo=<key> hi=<key> root=<key> [vlo=..] [vhi=..] [tune=<cents>]` adds a
///   zone to the last group (sample paths relative to the file);
/// - `fx <anything>` is understood but not carried over (reported as skipped);
/// - a first line `LOCKED` is this format's protection marker: refused, never read further.
#[derive(Copy, Clone, Debug, Default)]
pub struct PhTxt;

fn malformed(file: &str, reason: impl Into<String>) -> ImportError {
    ImportError::Malformed {
        format: "phtxt".into(),
        file: file.into(),
        reason: reason.into(),
    }
}

impl Importer for PhTxt {
    fn format(&self) -> &'static str {
        "phtxt"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["phtxt"]
    }
    fn version(&self) -> u32 {
        1
    }
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        let file = input.name();
        let (text, dir) = match input {
            ImportInput::Bytes { bytes, .. } => (bytes.to_vec(), None),
            ImportInput::Path(p) => (
                std::fs::read(p).map_err(|e| ImportError::Io {
                    path: file.clone(),
                    message: e.to_string(),
                })?,
                p.parent().map(Path::to_path_buf),
            ),
        };
        let text = String::from_utf8(text).map_err(|_| malformed(&file, "not UTF-8 text"))?;
        if text.lines().next().map(str::trim) == Some("LOCKED") {
            return Err(ImportError::Encrypted {
                format: "PhTxt".into(),
                file,
                detail: "it starts with the LOCKED marker".into(),
            });
        }
        let mut inst = HausInstrument::new("Untitled");
        let mut samples: Vec<ImportedSample> = Vec::new();
        let mut unknown: Vec<(String, u32)> = Vec::new();
        let mut skipped_fx = 0u32;
        let mut group: Option<GroupId> = None;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
            let kv = |key: &str| {
                rest.split_whitespace()
                    .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
            };
            let num = |key: &str, default: i64| -> Result<i64, ImportError> {
                kv(key).map_or(Ok(default), |v| {
                    v.parse()
                        .map_err(|_| malformed(&file, format!("{key}={v} is not a number")))
                })
            };
            match head {
                "name" => inst.name = rest.to_string(),
                "group" => {
                    let id = GroupId(inst.groups.len() as u32);
                    let mut g = Group::new(rest.split_whitespace().next().unwrap_or("Group"));
                    g.id = id;
                    g.amp_env.release = 0.05;
                    if let Some(v) = kv("volume") {
                        g.volume_db = v.parse().map_err(|_| malformed(&file, "bad volume"))?;
                    }
                    inst.groups.insert(id, g);
                    group = Some(id);
                }
                "zone" => {
                    let g = group.ok_or_else(|| malformed(&file, "a zone before any group"))?;
                    let sample =
                        kv("sample").ok_or_else(|| malformed(&file, "zone without sample"))?;
                    let key = |k: &str, d: i64| -> Result<u8, ImportError> {
                        u8::try_from(num(k, d)?)
                            .ok()
                            .filter(|v| *v <= 127)
                            .ok_or_else(|| malformed(&file, format!("{k} outside 0..127")))
                    };
                    let name = format!("samples/{sample}");
                    let mut z = Zone::new(
                        g,
                        SampleRef::File(name.clone()),
                        KeyRange {
                            lo: key("lo", 0)?,
                            hi: key("hi", 127)?,
                        },
                        key("root", 60)?,
                    );
                    z.id = ZoneId(inst.zones.len() as u32);
                    z.velocity = VelRange {
                        lo: key("vlo", 1)?,
                        hi: key("vhi", 127)?,
                    };
                    z.fine = num("tune", 0)?.clamp(-100, 100) as f32;
                    inst.zones.insert(z.id, z);
                    if !samples.iter().any(|s| s.name == name) {
                        let dir = dir
                            .clone()
                            .ok_or_else(|| malformed(&file, "samples need a folder"))?;
                        samples.push(ImportedSample {
                            name,
                            source: SampleSource::File(dir.join("samples").join(sample)),
                        });
                    }
                }
                "fx" => skipped_fx += 1,
                other => match unknown.iter_mut().find(|(w, _)| w == other) {
                    Some((_, n)) => *n += 1,
                    None => unknown.push((other.to_string(), 1)),
                },
            }
        }
        let mut notes = Vec::new();
        if skipped_fx > 0 {
            notes.push(ImportNote {
                kind: ImportNoteKind::Skipped,
                what: "fx".into(),
                detail: "effects are not carried over".into(),
                count: skipped_fx,
            });
        }
        inst.import_notes = notes.clone();
        inst.source = Some(InstrumentSource {
            format: "phtxt".into(),
            file: file.clone(),
            importer_version: 1,
        });
        Ok(Imported {
            report: ImportReport {
                format: "phtxt".into(),
                file,
                groups: inst.groups.len(),
                zones: inst.zones.len(),
                notes,
                unknown,
                presets: vec![],
            },
            instrument: inst,
            samples,
        })
    }
}

/// A `.phtxt` file with two zones over `samples/low.wav` and `samples/high.wav` (which the
/// caller writes beside it), one skipped `fx` line and one unknown statement.
pub const EXAMPLE: &str = "# a test instrument
name Toy Keys
group Main volume=-3
zone sample=low.wav lo=36 hi=59 root=57
zone sample=high.wav lo=60 hi=84 root=69 tune=5
fx reverb
shimmer 1
";
