//! Drop to create (M3/T3, command side): a folder of samples becomes an instrument, mapped by
//! the note names in the file names.
//!
//! # The mapping rule
//!
//! 1. **Files:** the audio files directly in the folder (`wav`, `wave`, `aif`, `aiff`, `flac`,
//!    `ogg`, `mp3`, any case), in file-name order. Sub-folders are not read.
//! 2. **Root key** from the file name (without extension):
//!    - the **last note name** in it: a letter `A`–`G` (either case), an optional `#` or `b`,
//!      then an octave `-1`…`9` (C4 = 60, so `C-1` = 0 and `G9` = 127). The letter must not
//!      follow another letter, and the octave must not be followed by a letter or digit:
//!      `Piano_C#4_f` is C#4 (61), `Bass Eb1` is 27, but `Snare_v1` and `Cb12` hold none;
//!    - otherwise the **last stand-alone number** 0…127 (digits with no letter or digit on
//!      either side): `piano_060_v2` is 60;
//!    - otherwise the file is not mapped (reported as skipped).
//! 3. **Velocity layers:** files with the same root key split the velocity range evenly, in
//!    file-name order: of `n` files, file `i` (0-based) plays velocities
//!    `⌊128·i/n⌋ … ⌊128·(i+1)/n⌋ − 1` (so `soft` before `loud` when named that way).
//! 4. **Key ranges:** with the distinct roots `r₀ < r₁ < … < rₘ`, root `rⱼ` plays from
//!    `⌊(rⱼ₋₁ + rⱼ)/2⌋ + 1` (0 for the lowest) to `⌊(rⱼ + rⱼ₊₁)/2⌋` (127 for the highest):
//!    every key plays the nearest sample, a tie going to the lower one.
//! 5. One group, `Samples`; the instrument is named after the folder.

use crate::notes::Notes;
use crate::{ImportError, ImportReport, Imported, ImportedSample, SampleSource};
use ph_core::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Extensions the auto-mapper picks up.
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "wave", "aif", "aiff", "flac", "ogg", "mp3"];

/// The root key a file name names (rule 2), from its stem.
pub fn root_from_name(stem: &str) -> Option<u8> {
    let b = stem.as_bytes();
    let mut best: Option<u8> = None;
    for i in 0..b.len() {
        let c = b[i].to_ascii_lowercase();
        if !(b'a'..=b'g').contains(&c) || (i > 0 && b[i - 1].is_ascii_alphabetic()) {
            continue;
        }
        let mut j = i + 1;
        let mut acc = 0i32;
        if j < b.len() && (b[j] == b'#' || b[j] == b'b') {
            // `b` is a flat only when an octave follows
            let k = j + 1;
            let octave_follows = k < b.len()
                && (b[k].is_ascii_digit() || (b[k] == b'-' && b.get(k + 1) == Some(&b'1')));
            if octave_follows {
                acc = if b[j] == b'#' { 1 } else { -1 };
                j += 1;
            }
        }
        let (octave, end) = if j + 1 < b.len() && b[j] == b'-' && b[j + 1] == b'1' {
            (-1, j + 2)
        } else if j < b.len() && b[j].is_ascii_digit() {
            ((b[j] - b'0') as i32, j + 1)
        } else {
            continue;
        };
        if end < b.len() && b[end].is_ascii_alphanumeric() {
            continue;
        }
        let base = match c {
            b'c' => 0,
            b'd' => 2,
            b'e' => 4,
            b'f' => 5,
            b'g' => 7,
            b'a' => 9,
            _ => 11,
        };
        let k = 12 * (octave + 1) + base + acc;
        if (0..=127).contains(&k) {
            best = Some(k as u8);
        }
    }
    if best.is_some() {
        return best;
    }
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if (i == b.len() || !b[i].is_ascii_alphabetic())
                && let Ok(n) = stem[s..i].parse::<u32>()
                && n <= 127
            {
                best = Some(n as u8);
            }
        } else {
            i += 1;
        }
    }
    best
}

/// Maps named files (rules 2–5). `files` are (file name, path), in any order.
pub fn map_files(folder_name: &str, files: &[(String, PathBuf)]) -> Imported {
    let mut notes = Notes::default();
    let mut files: Vec<&(String, PathBuf)> = files.iter().collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut by_root: BTreeMap<u8, Vec<&(String, PathBuf)>> = BTreeMap::new();
    for f in files {
        let stem = Path::new(&f.0)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match root_from_name(&stem) {
            Some(k) => by_root.entry(k).or_default().push(f),
            None => notes.skip(
                format!("file:{}", f.0),
                "no note name or key number in the file name; not mapped",
            ),
        }
    }
    let mut inst = HausInstrument::new(folder_name);
    let gid = GroupId(0);
    inst.groups.insert(
        gid,
        Group {
            id: gid,
            ..Group::new("Samples")
        },
    );
    let roots: Vec<u8> = by_root.keys().copied().collect();
    let mut samples = Vec::new();
    let mut zid = 0;
    for (j, r) in roots.iter().enumerate() {
        let lo = if j == 0 {
            0
        } else {
            ((roots[j - 1] as u32 + *r as u32) / 2 + 1) as u8
        };
        let hi = if j + 1 == roots.len() {
            127
        } else {
            ((*r as u32 + roots[j + 1] as u32) / 2) as u8
        };
        let layer = &by_root[r];
        let n = layer.len() as u32;
        for (i, f) in layer.iter().enumerate() {
            let i = i as u32;
            let mut z = Zone::new(gid, SampleRef::File(f.0.clone()), KeyRange { lo, hi }, *r);
            z.id = ZoneId(zid);
            zid += 1;
            z.velocity = VelRange {
                lo: (128 * i / n) as u8,
                hi: (128 * (i + 1) / n - 1) as u8,
            };
            inst.zones.insert(z.id, z);
            samples.push(ImportedSample {
                name: f.0.clone(),
                source: SampleSource::File(f.1.clone()),
            });
        }
    }
    let (notes, unknown) = notes.finish();
    inst.import_notes = notes.clone();
    inst.source = Some(InstrumentSource {
        format: "folder".into(),
        file: folder_name.into(),
        importer_version: 1,
    });
    Imported {
        report: ImportReport {
            format: "folder".into(),
            file: folder_name.into(),
            groups: 1,
            zones: inst.zones.len(),
            notes,
            unknown,
        },
        instrument: inst,
        samples,
    }
}

/// Maps the audio files directly in `dir` (rule 1).
pub fn map_folder(dir: &Path) -> Result<Imported, ImportError> {
    let io = |e: std::io::Error| ImportError::Io {
        path: dir.display().to_string(),
        message: e.to_string(),
    };
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).map_err(io)? {
        let e = e.map_err(io)?;
        let p = e.path();
        let audio = p.extension().is_some_and(|x| {
            let x = x.to_string_lossy().to_lowercase();
            AUDIO_EXTENSIONS.contains(&x.as_str())
        });
        if audio && p.is_file() {
            files.push((e.file_name().to_string_lossy().into_owned(), p));
        }
    }
    if files.is_empty() {
        return Err(ImportError::Unsupported {
            file: dir.display().to_string(),
            reason: "the folder holds no audio files to map".into(),
        });
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Samples".into());
    Ok(map_files(&name, &files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_keys_from_names() {
        for (s, k) in [
            ("Piano_C#4_f", Some(61)),
            ("Bass Eb1", Some(27)),
            ("pad-c-1", Some(0)),
            ("lead_G9", Some(127)),
            ("Grand_A0_v2", Some(21)),
            ("piano_060_v2", Some(60)),
            ("Snare_v1", None),
            ("Cb12", None),
            ("kick", None),
            ("Strings C3 A3", Some(57)),
            ("b4", Some(71)),
            ("bb4", Some(70)),
            ("200", None),
        ] {
            assert_eq!(root_from_name(s), k, "{s}");
        }
    }

    #[test]
    fn ranges_split_between_roots_and_layers_split_velocity() {
        let f = |n: &str| (n.to_string(), PathBuf::from(n));
        let out = map_files(
            "Keys",
            &[
                f("k_C4_2.wav"),
                f("k_C4_1.wav"),
                f("k_E4.wav"),
                f("k_C3.wav"),
                f("readme.wav"),
            ],
        );
        let z: Vec<_> = out
            .instrument
            .zones
            .values()
            .map(|z| {
                let SampleRef::File(n) = &z.sample else {
                    unreachable!()
                };
                (
                    n.as_str(),
                    z.root_key,
                    z.keys.lo,
                    z.keys.hi,
                    z.velocity.lo,
                    z.velocity.hi,
                )
            })
            .collect();
        assert_eq!(
            z,
            vec![
                ("k_C3.wav", 48, 0, 54, 0, 127),
                ("k_C4_1.wav", 60, 55, 62, 0, 63),
                ("k_C4_2.wav", 60, 55, 62, 64, 127),
                ("k_E4.wav", 64, 63, 127, 0, 127),
            ]
        );
        assert_eq!(out.samples.len(), 4);
        assert!(out.report.notes.iter().any(|n| n.what == "file:readme.wav"));
        let three = map_files("x", &[f("a_C4.wav"), f("b_C4.wav"), f("c_C4.wav")]);
        let v: Vec<_> = three
            .instrument
            .zones
            .values()
            .map(|z| (z.velocity.lo, z.velocity.hi))
            .collect();
        assert_eq!(v, vec![(0, 41), (42, 84), (85, 127)]);
    }
}
