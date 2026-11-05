//! The EXS24 importer (M3/T4), written from `docs/formats/exs.md` only (clean room).
//!
//! **The layout is unconfirmed on real files** (exs.md §9): the reader follows our spec, and
//! its fixtures test agreement with the spec, not compatibility with Logic. Disputed facts
//! follow the spec's [PH] choices; where it has none, both readings are accepted when the
//! file makes the choice unambiguous, and the import report says so otherwise.
//!
//! Pipeline: blocks and byte order ([`blocks`]) → zones, groups, samples, parameters →
//! sample lookup (§5) → the Haus Sampler model (§7).

pub(crate) mod blocks;

use crate::assemble::{Assembler, GroupDraft, clampf, split_cents};
use crate::notes::Notes;
use crate::probe::probe;
use crate::{
    ImportError, ImportInput, ImportReport, Imported, ImportedSample, Importer, SampleSource,
};
use blocks::{GroupRec, Order, SampleRec, ZoneRec};
use ph_core::*;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Bumped whenever the importer changes what it maps.
pub const EXS_IMPORTER_VERSION: u32 = 1;

/// How many folder levels above the `.exs` file the sample search climbs (§5 step 3).
pub const SEARCH_LEVELS_UP: usize = 2;

/// Largest `.exs` file read: 64 MiB.
const MAX_FILE: u64 = 64 << 20;

/// Most directories the recursive sample search visits under one root.
const MAX_SEARCH_DIRS: usize = 20_000;

/// Logic EXS24 / Sampler instruments (`docs/formats/exs.md`).
#[derive(Copy, Clone, Debug, Default)]
pub struct Exs;

impl Importer for Exs {
    fn format(&self) -> &'static str {
        "exs"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["exs"]
    }
    fn version(&self) -> u32 {
        EXS_IMPORTER_VERSION
    }
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        let file = input.name();
        let (bytes, dir) = match input {
            ImportInput::Bytes { bytes, .. } => (bytes.to_vec(), None),
            ImportInput::Path(p) => {
                let io = |e: std::io::Error| ImportError::Io {
                    path: p.display().to_string(),
                    message: e.to_string(),
                };
                if std::fs::metadata(p).map_err(io)?.len() > MAX_FILE {
                    return Err(malformed(&file, "the file is larger than 64 MiB"));
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
        import_bytes(&bytes, &file, dir.as_deref())
    }
}

fn malformed(file: &str, reason: impl Into<String>) -> ImportError {
    ImportError::Malformed {
        format: "EXS24".into(),
        file: file.to_string(),
        reason: reason.into(),
    }
}

// ------------------------------------------------------------------------- sample lookup

/// Finds sample files by the rules of §5, caching folder listings.
struct Lookup {
    dir: Option<PathBuf>,
    /// Per search root: lower-case file name → first path (sorted walk, so deterministic).
    index: BTreeMap<PathBuf, BTreeMap<String, PathBuf>>,
}

fn walk(root: &Path, into: &mut BTreeMap<String, PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        seen += 1;
        if seen > MAX_SEARCH_DIRS {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for e in entries {
            // never follow links out of the searched folders
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                subdirs.push(e.path());
            } else if t.is_file() {
                into.entry(e.file_name().to_string_lossy().to_lowercase())
                    .or_insert_with(|| e.path());
            }
        }
        // depth-first, in name order
        stack.extend(subdirs.into_iter().rev());
    }
}

impl Lookup {
    fn find(&mut self, s: &SampleRec, notes: &mut Notes) -> Result<PathBuf, String> {
        let name = if s.file_name.is_empty() {
            s.name.clone()
        } else {
            s.file_name.clone()
        };
        // SPEC-GAP: exs.md §5: whether the path field is a folder or the full path is
        // disputed; both are tried.
        let p = s.path.trim();
        if !p.is_empty() {
            let full = Path::new(p);
            for cand in [full.join(&name), full.to_path_buf()] {
                if cand.is_file() {
                    return Ok(cand);
                }
            }
        }
        let Some(dir) = self.dir.clone() else {
            return Err(format!("{name}: not found (no folder to search)"));
        };
        if name.is_empty()
            || name.contains(['/', '\\'])
            || Path::new(&name)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("{name:?}: not a plain file name"));
        }
        let direct = dir.join(&name);
        if direct.is_file() {
            return Ok(direct);
        }
        let want = name.to_lowercase();
        let mut root = dir.clone();
        for level in 0..=SEARCH_LEVELS_UP {
            let idx = self.index.entry(root.clone()).or_insert_with(|| {
                let mut m = BTreeMap::new();
                walk(&root, &mut m);
                m
            });
            if let Some(hit) = idx.get(&want) {
                if level > 0 || hit.parent() != Some(dir.as_path()) {
                    notes.approx(
                        "exs:sample-search",
                        "samples not at their stored path were found by name under the instrument's folder or its parents",
                    );
                }
                return Ok(hit.clone());
            }
            match root.parent() {
                Some(p) if !p.as_os_str().is_empty() => root = p.to_path_buf(),
                _ => break,
            }
        }
        Err(format!(
            "{name}: not found next to the instrument or in the folders around it"
        ))
    }
}

fn common(a: &Path, b: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for (x, y) in a.components().zip(b.components()) {
        if x != y {
            break;
        }
        out.push(x.as_os_str());
    }
    out
}

// ------------------------------------------------------------------------- parameters

/// Envelope time from a 0–127 parameter: `10 × (p/127)⁴` seconds (§6, approximate).
fn env_time(p: i32) -> f32 {
    let x = (p.clamp(0, 127) as f64) / 127.0;
    (10.0 * x.powi(4)) as f32
}

/// Names of the parameter ids the importer knows but does not map (kept as data).
fn param_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x08 => "volume key scale",
        0x116 | 0x11A => "MIDI mono mode",
        0x14 => "glide",
        0x48 | 0x49 => "portamento",
        0xA6 => "coarse tune remote",
        0x4B => "filter drive",
        0xAA => "filter fat",
        0x174 | 0x186 | 0x177 | 0x178 | 0x179 | 0x173 | 0x17A => "filter 2",
        0x3C..=0x40 | 0x14C..=0x15D | 0x187..=0x189 | 0xA7 => "LFOs",
        0x195 | 0x192 => "envelope attack curve",
        0x59 => "env 1 volume high",
        0x5B => "env 2 time curve",
        0x183 => "velocity curve",
        0x5F => "velocity offset",
        0xA4 => "random velocity",
        0x62 => "random pitch",
        0x61 | 0xA5 => "crossfade",
        0xAB => "unison",
        0xAC => "hold via control",
        0xAD..=0xFD | 0x19B..=0x1A2 => "modulation matrix",
        0x17D | 0x5A => "envelope velocity sensitivity",
        0x6B | 0x16A => "envelope type",
        _ => return None,
    })
}

const MAPPED: [u16; 25] = [
    0x07, 0x160, 0x03, 0x04, 0x05, 0x0A, 0x2D, 0x0E, 0x0F, 0x2C, 0xF3, 0x1E, 0x1D, 0x2E, 0x52,
    0x53, 0x58, 0x54, 0x51, 0x55, 0x16D, 0xA3, 0x4C, 0x38, 0x4E,
];

// ------------------------------------------------------------------------- mapping

pub fn import_bytes(bytes: &[u8], file: &str, dir: Option<&Path>) -> Result<Imported, ImportError> {
    let mut notes = Notes::default();
    let (order, blocks) = blocks::blocks(bytes, &mut notes).map_err(|r| malformed(file, r))?;
    if order == Order::Big {
        notes.approx(
            "exs:big-endian",
            "a big-endian (older) instrument: its layout is read as the little-endian one",
        );
    }
    let head = &blocks[0];
    let rd = blocks::Rd {
        b: head.content,
        order,
    };
    let counts = [rd.u32(4), rd.u32(8), rd.u32(12), rd.u32(16)];
    let mut zones: Vec<ZoneRec> = Vec::new();
    let mut groups: Vec<GroupRec> = Vec::new();
    let mut samples: Vec<SampleRec> = Vec::new();
    let mut params: Vec<(u16, i32)> = Vec::new();
    let mut n_params = 0u32;
    let mut extra = BTreeMap::new();
    for (bi, b) in blocks.iter().enumerate().skip(1) {
        if b.flags & 0x40 != 0 {
            notes.kept(
                "exs:variant",
                "blocks of the newer (0x40) variant were read as standard",
            );
        }
        match b.kind {
            0 => notes.skip(
                "exs:block:instrument",
                "a second instrument header was ignored",
            ),
            1 => match blocks::zone(b, order) {
                Some(z) => zones.push(z),
                None => {
                    zones.push(ZoneRec {
                        group: -1,
                        sample: -1,
                        ..Default::default()
                    });
                    notes.skip(
                        "exs:block:zone",
                        "a zone block shorter than 96 bytes was skipped",
                    );
                }
            },
            2 => match blocks::group(b, order) {
                Some(g) => groups.push(g),
                None => {
                    groups.push(GroupRec {
                        name: b.name.clone(),
                        vel_hi: 127,
                        key_hi: 127,
                        enable_cc_hi: 127,
                        rr_position: -1,
                        ..Default::default()
                    });
                    notes.skip(
                        "exs:block:group",
                        "a group block shorter than 76 bytes was read as a plain group",
                    );
                }
            },
            3 => match blocks::sample(b, order) {
                Some(s) => samples.push(s),
                None => {
                    samples.push(SampleRec::default());
                    notes.skip(
                        "exs:block:sample",
                        "a sample block shorter than 336 bytes was skipped",
                    );
                }
            },
            4 => {
                n_params += 1;
                params.extend(blocks::params(b, order, &mut notes));
            }
            5 => notes.skip(
                "exs:block:5",
                "tagged float records (GAMETSPP) are not part of the mapping",
            ),
            7 | 8 => notes.skip(
                format!("exs:block:{}", b.kind),
                "a block of unknown meaning was skipped",
            ),
            0x0A | 0x0B => {
                let what = if b.kind == 0x0A {
                    "sampler layout"
                } else {
                    "sample-file list"
                };
                let hex: String = b
                    .content
                    .iter()
                    .take(65_536)
                    .map(|c| format!("{c:02x}"))
                    .collect();
                extra.insert(format!("exs.block.{:#04x}.{bi}", b.kind), hex);
                notes.kept(
                    format!("exs:block:{:#04x}", b.kind),
                    format!("the {what} property list is kept as data (not read yet)"),
                );
            }
            k => notes.unknown(format!("exs:block:{k:#04x}")),
        }
    }
    let found = [
        zones.len() as u32,
        groups.len() as u32,
        samples.len() as u32,
        n_params,
    ];
    if counts != found && counts != [0; 4] {
        notes.approx(
            "exs:counts",
            format!(
                "the header lists {}/{}/{}/{} zone/group/sample/parameter blocks, the file holds {}/{}/{}/{}",
                counts[0], counts[1], counts[2], counts[3], found[0], found[1], found[2], found[3]
            ),
        );
    }

    // zone → sample references (§3): the object-id fallback when no zone names a sample
    let any_sample = zones.iter().any(|z| z.sample >= 0);
    if !any_sample && !zones.is_empty() {
        notes.approx(
            "exs:zone-sample-ids",
            "no zone names a sample; each zone's object id was used as its sample index",
        );
        for (i, z) in zones.iter_mut().enumerate() {
            z.sample = if z.id as i32 <= 0 {
                i as i32
            } else {
                z.id as i32
            };
        }
    }

    // pan scale (§3 [D?]): ±50 unless a value beyond ±50 shows the ±64 reading
    let wide = zones.iter().any(|z| z.pan.unsigned_abs() > 50)
        || groups.iter().any(|g| g.pan.unsigned_abs() > 50);
    let pan_scale = if wide { 64.0 } else { 50.0 };
    if wide {
        notes.approx(
            "exs:pan-scale",
            "pan values beyond ±50 were read on the −64…63 scale",
        );
    } else if zones.iter().any(|z| z.pan != 0) || groups.iter().any(|g| g.pan != 0) {
        notes.approx(
            "exs:pan-scale",
            "the pan scale is disputed (±50 or −64…63); read as ±50 = full left/right",
        );
    }

    // parameters (§6)
    let pm: BTreeMap<u16, i32> = params.iter().copied().collect();
    let get = |id: u16| pm.get(&id).copied();
    let mut unknown_params = Vec::new();
    for &(id, v) in &params {
        if MAPPED.contains(&id) {
            continue;
        }
        match param_name(id) {
            Some(n) => {
                if v != 0 {
                    notes.kept(
                        format!("exs:param:{n}"),
                        "kept as data (exs.params); not mapped yet",
                    );
                }
            }
            None => unknown_params.push(id),
        }
    }
    for id in unknown_params {
        notes.unknown(format!("exs:param:{id:#05x}"));
    }
    if !params.is_empty() {
        extra.insert(
            "exs.params".into(),
            params
                .iter()
                .map(|(i, v)| format!("{i:#x}={v}"))
                .collect::<Vec<_>>()
                .join(","),
        );
    }

    let name = if head.name.is_empty() {
        Path::new(file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "EXS instrument".into())
    } else {
        head.name.clone()
    };
    let mut inst = HausInstrument::new(&name);
    inst.extra = extra;
    let s = &mut inst.settings;
    if let Some(v) = get(0x07) {
        s.volume_db = clampf(v as f64, -144.0, 24.0) as f32;
        if v != 0 {
            notes.approx(
                "exs:param:master volume",
                "read as decibels (unit unconfirmed)",
            );
        }
    }
    // SPEC-GAP: exs.md §6 gives no scale for master pan (0x160); read on the zone pan scale.
    if let Some(v) = get(0x160) {
        s.pan = clampf(v as f64 / pan_scale, -1.0, 1.0) as f32;
    }
    if let Some(up) = get(0x03) {
        let up = clampf(up.unsigned_abs() as f64, 0.0, 48.0) as f32;
        s.bend_up = up;
        s.bend_down = match get(0x04) {
            None | Some(-1) => up,
            Some(d) => clampf(d.unsigned_abs() as f64, 0.0, 48.0) as f32,
        };
    }
    if let Some(v) = get(0x05).filter(|v| *v > 0) {
        s.voice_limit = v.clamp(1, 1024) as u32;
    }
    let transpose = get(0x2D).unwrap_or(0) + get(0x0E).unwrap_or(0);
    s.transpose = transpose.clamp(-48, 48);
    s.tune_cents = clampf(get(0x0F).unwrap_or(0) as f64, -100.0, 100.0) as f32;
    let legato = get(0x0A).is_some_and(|v| v > 0);
    let random_global = get(0xA3).is_some_and(|v| v > 0);

    // amp envelope (env 1) and filter
    let mut amp = Envelope::default();
    let has_env = [0x52, 0x53, 0x58, 0x54, 0x51, 0x55, 0x16D]
        .iter()
        .any(|i| pm.contains_key(i));
    if has_env {
        let kind = get(0x6B).unwrap_or(5);
        amp = Envelope {
            delay: if matches!(kind, 4 | 5) {
                env_time(get(0x16D).unwrap_or(0))
            } else {
                0.0
            },
            attack: env_time(get(0x52).unwrap_or(0)),
            hold: if matches!(kind, 3 | 5) {
                env_time(get(0x58).unwrap_or(0))
            } else {
                0.0
            },
            decay: if kind == 1 {
                0.0
            } else {
                env_time(get(0x54).unwrap_or(0))
            },
            sustain: match kind {
                0 => 0.0,
                1 => 1.0,
                _ => (get(0x51).unwrap_or(127).clamp(0, 127) as f32) / 127.0,
            },
            release: env_time(get(0x55).unwrap_or(0)).max(0.001),
        };
        notes.approx(
            "exs:param:envelope",
            "envelope times use 10 × (p/127)⁴ s (a reader's mapping); the attack at high velocity is used",
        );
    }
    let filter = if get(0x2C).is_some_and(|v| v != 0) {
        let x = get(0x1E).unwrap_or(1000).clamp(0, 1000) as f64 / 1000.0;
        let kind = match get(0xF3).unwrap_or(2) {
            0 | 1 => FilterKind::LowPass24,
            4 => FilterKind::HighPass12,
            5 => FilterKind::BandPass,
            _ => FilterKind::LowPass12,
        };
        // SPEC-GAP: exs.md §6: the cutoff's 0…1 range is not given in Hz; read as
        // 20 Hz × 1000^x (20 Hz–20 kHz, exponential).
        notes.approx(
            "exs:param:filter",
            "filter cutoff and resonance scales are unconfirmed (cutoff 20 Hz–20 kHz exponential, resonance 0–24 dB)",
        );
        Some(Filter {
            kind,
            cutoff: clampf(20.0 * 1000f64.powf(x), 10.0, 22_000.0) as f32,
            resonance: clampf(get(0x1D).unwrap_or(0) as f64 / 1000.0 * 24.0, 0.0, 40.0) as f32,
            key_track: clampf(get(0x2E).unwrap_or(0) as f64 / 1000.0 * 100.0, 0.0, 1200.0) as f32,
            env_depth: 0.0,
        })
    } else {
        None
    };
    let filter_env = (filter.is_some() && [0x4C, 0x38, 0x4E, 0x4F, 0x50].iter().any(|i| pm.contains_key(i)))
        .then(|| {
            notes.skip(
                "exs:param:filter envelope depth",
                "the filter envelope's depth comes from the modulation matrix, whose enumerations are undocumented; depth 0",
            );
            Envelope {
                delay: env_time(get(0x16C).unwrap_or(0)),
                attack: env_time(get(0x4C).unwrap_or(0)),
                hold: env_time(get(0x38).unwrap_or(0)),
                decay: env_time(get(0x4E).unwrap_or(0)),
                sustain: (get(0x4F).unwrap_or(127).clamp(0, 127) as f32) / 127.0,
                release: env_time(get(0x50).unwrap_or(0)).max(0.001),
            }
        });

    // samples: found once each
    let mut look = Lookup {
        dir: dir.map(Path::to_path_buf),
        index: BTreeMap::new(),
    };
    let mut found: BTreeMap<usize, Result<(PathBuf, u64, u32), String>> = BTreeMap::new();

    // round-robin groups (§4 [PH]): sorted by stored position, numbered 0…n−1
    let mut rr: Vec<usize> = (0..groups.len())
        .filter(|&g| groups[g].enable_by == 2)
        .collect();
    rr.sort_by_key(|&g| (groups[g].rr_position, g));
    let rr_pos: BTreeMap<usize, u32> = rr.iter().enumerate().map(|(i, &g)| (g, i as u32)).collect();
    if !rr.is_empty() {
        notes.approx(
            "exs:round-robin",
            "round-robin groups are numbered by their stored position into one cycle (encoding unconfirmed)",
        );
    }
    // keyswitch articulations (enable by note)
    let mut art_of_key: BTreeMap<u8, ArticulationId> = BTreeMap::new();
    for g in &groups {
        if g.enable_by == 1 && g.enable_note <= 127 && !art_of_key.contains_key(&g.enable_note) {
            let id = ArticulationId(art_of_key.len() as u32);
            art_of_key.insert(g.enable_note, id);
            inst.articulations.push(Articulation {
                id,
                name: if g.name.is_empty() {
                    format!("Key {}", g.enable_note)
                } else {
                    g.name.clone()
                },
                trigger: ArticulationTrigger::Key { key: g.enable_note },
                color: None,
            });
        }
    }
    // the first group of each exclusive id names the choke group
    let mut choke_name: BTreeMap<u8, String> = BTreeMap::new();
    for (gi, g) in groups.iter().enumerate() {
        if g.exclusive != 0 {
            choke_name
                .entry(g.exclusive)
                .or_insert_with(|| group_name(g, gi));
        }
    }

    let mut asm = Assembler::default();
    // the sample index of each zone added, in order (= zone ids)
    let mut placed: Vec<usize> = Vec::new();
    for zr in &zones {
        if zr.sample < 0 {
            notes.skip("exs:zone", "zones without a sample were skipped");
            continue;
        }
        let si = zr.sample as usize;
        if si >= samples.len() {
            notes.skip(
                "exs:zone",
                format!("a zone names sample {si} of {}; skipped", samples.len()),
            );
            continue;
        }
        let g = if zr.group >= 0 && (zr.group as usize) < groups.len() {
            Some((zr.group as usize, &groups[zr.group as usize]))
        } else {
            if zr.group >= 0 {
                notes.approx(
                    "exs:zone-group",
                    "zones naming a missing group were put in \"Ungrouped\"",
                );
            }
            None
        };
        let r = found.entry(si).or_insert_with(|| {
            let s = &samples[si];
            if s.compressed || &s.file_type == b"caff" || &s.file_type == b"ffac" {
                return Err(format!(
                    "{}: compressed sample (CAF/AAC) not supported",
                    s.file_name
                ));
            }
            let path = look.find(s, &mut notes)?;
            let info = probe(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if s.frames != 0 && s.frames as u64 != info.frames {
                notes.approx(
                    "exs:sample-length",
                    "a sample file's length differs from the instrument's record; the file's is used",
                );
            }
            Ok((path, info.frames, info.sample_rate))
        });
        let (frames, rate) = match r {
            Ok((_, f, r)) => (*f, *r),
            Err(why) => {
                notes.skip("exs:sample", why.clone());
                continue;
            }
        };

        // ---- the group draft
        let mut d = match g {
            Some((_, gr)) if gr.exclusive != 0 => GroupDraft::new(&choke_name[&gr.exclusive]),
            Some((gi, gr)) => GroupDraft::new(&group_name(gr, gi)),
            None => GroupDraft::new("Ungrouped"),
        };
        d.group.amp_env = amp.clone();
        d.group.filter = filter.clone();
        d.group.filter_env = filter_env.clone();
        if legato {
            d.group.polyphony = Polyphony::Legato;
        }
        let mut z = Zone::new(
            GroupId(0),
            SampleRef::File(String::new()),
            KeyRange {
                lo: zr.key_lo.min(127).min(zr.key_hi.min(127)),
                hi: zr.key_hi.min(127).max(zr.key_lo.min(127)),
            },
            zr.root.min(127),
        );
        if zr.key_lo > zr.key_hi {
            notes.approx("exs:zone-keys", "reversed key ranges were put in order");
        }
        if zr.options & 0x08 != 0 {
            z.velocity = VelRange {
                lo: zr.vel_lo.min(zr.vel_hi).min(127),
                hi: zr.vel_hi.max(zr.vel_lo).min(127),
            };
        }
        let mut vol = zr.volume as f64;
        let mut pan = zr.pan as f64;
        let mut muted = false;
        if let Some((gi, gr)) = g {
            vol += gr.volume as f64;
            pan += gr.pan as f64;
            if gr.polyphony > 0 {
                d.group.voice_limit = Some(gr.polyphony as u32);
            }
            if gr.exclusive != 0 {
                d.group.voice_limit = Some(1);
                d.group
                    .extra
                    .insert("exs.exclusive".into(), gr.exclusive.to_string());
            }
            if gr.options & 0x10 != 0 {
                muted = true;
                notes.approx(
                    "exs:group-mute",
                    "muted groups are imported silent (−144 dB)",
                );
            }
            if gr.options & 0x80 != 0 {
                notes.skip(
                    "exs:group-fixed-select",
                    "fixed sample select is not supported",
                );
            }
            // velocity and key limits clip the zone; 0 = no limit
            let vhi = if gr.vel_hi == 0 {
                127
            } else {
                gr.vel_hi.min(127)
            };
            let khi = if gr.extended && gr.key_hi != 0 {
                gr.key_hi.min(127)
            } else {
                127
            };
            let klo = if gr.extended { gr.key_lo.min(127) } else { 0 };
            let (vlo2, vhi2) = (
                z.velocity.lo.max(gr.vel_lo.min(127)),
                z.velocity.hi.min(vhi),
            );
            let (klo2, khi2) = (z.keys.lo.max(klo), z.keys.hi.min(khi));
            if vlo2 > vhi2 || klo2 > khi2 {
                notes.approx(
                    "exs:group-limits",
                    "zones entirely outside their group's key or velocity limits were dropped",
                );
                continue;
            }
            z.velocity = VelRange { lo: vlo2, hi: vhi2 };
            z.keys = KeyRange { lo: klo2, hi: khi2 };
            if gr.release_trigger != 0 {
                z.trigger = ZoneTrigger::Release;
                if gr.options & 0x40 != 0 {
                    z.release_decay_db = 3.0;
                    notes.approx(
                        "exs:release-decay",
                        "release-trigger decay is played at 3 dB per second held",
                    );
                }
            }
            if gr.release_time != 0 {
                notes.skip(
                    "exs:group-release-time",
                    "the group's release trigger time (unit unknown) is not used",
                );
            }
            d.group.output = (gr.output as u32).min(MAX_OUTPUT_BUSES - 1);
            if gr.vel_xfade != 0 || gr.key_xfade != 0 {
                d.group.extra.insert(
                    "exs.crossfade".into(),
                    format!(
                        "vel={},{} key={},{}",
                        gr.vel_xfade, gr.vel_xfade_type, gr.key_xfade, gr.key_xfade_type
                    ),
                );
                notes.skip("exs:group-crossfade", "group crossfades (amount and type encodings unknown) are kept as data, not played");
            }
            if gr.cutoff != 0 || gr.resonance != 0 || gr.env.iter().any(|v| *v != 0) {
                d.group.extra.insert(
                    "exs.offsets".into(),
                    format!(
                        "cutoff={} resonance={} env={:?}",
                        gr.cutoff, gr.resonance, gr.env
                    ),
                );
                notes.skip(
                    "exs:group-offsets",
                    "group filter and envelope offsets (units unknown) are kept as data",
                );
            }
            if gr.random_offset > 0 || random_global {
                d.group.round_robin = RoundRobinMode::Random;
            }
            match gr.enable_by {
                0 => {}
                1 => {
                    if let Some(a) = art_of_key.get(&gr.enable_note) {
                        d.group.articulations = vec![*a];
                    }
                }
                2 => {
                    z.round_robin = rr_pos.get(&gi).copied();
                    d.group.name = rr
                        .first()
                        .map(|&f| group_name(&groups[f], f))
                        .unwrap_or_default();
                }
                3 => {
                    let (lo, hi) = (gr.enable_cc_lo.min(127), gr.enable_cc_hi.min(127));
                    z.cc_conditions.push(CcCondition {
                        controller: gr.enable_cc.min(127),
                        lo: lo.min(hi) as f32 / 127.0,
                        hi: hi.max(lo) as f32 / 127.0,
                    });
                }
                4 | 5 | 7 => {
                    let (what, data) = match gr.enable_by {
                        4 => ("pitch bend", "pitch bend".to_string()),
                        5 => (
                            "MIDI channel",
                            format!("MIDI channel {}", gr.enable_channel),
                        ),
                        _ => ("tempo", format!("tempo {}-{}", gr.tempo_lo, gr.tempo_hi)),
                    };
                    d.group.extra.insert("exs.enable_by".into(), data);
                    notes.skip(
                        format!("exs:enable-by:{what}"),
                        "groups enabled by this are kept as data and play always",
                    );
                }
                6 => {
                    d.group.extra.insert(
                        "exs.enable_by".into(),
                        format!("articulation {}", gr.enable_articulation),
                    );
                    notes.skip("exs:enable-by:articulation", "Logic articulation-set ids are kept as data; such groups play in every articulation");
                }
                n => notes.unknown(format!("exs:enable-by:{n}")),
            }
        } else if random_global {
            d.group.round_robin = RoundRobinMode::Random;
        }
        if muted {
            vol = -144.0;
        }
        z.volume_db = clampf(vol, -144.0, 24.0) as f32;
        z.pan = clampf(pan / pan_scale, -1.0, 1.0) as f32;
        // pitch (§7)
        let cents = zr.coarse as f64 * 100.0 + zr.fine as f64;
        if zr.options & 0x02 != 0 {
            // pitch tracking off: every key plays the recorded pitch
            d.route(ModSource::Key, ModTarget::Pitch, -6400.0, ModCurve::Linear);
            z.root_key = 60;
        }
        let (tune, fine, clamped) = split_cents(cents);
        if clamped {
            notes.approx(
                "exs:zone-tune",
                "pitch offsets beyond ±48 semitones were cut",
            );
        }
        z.tune = tune;
        z.fine = fine;
        z.one_shot = zr.options & 0x01 != 0;
        z.reverse = zr.options & 0x04 != 0;
        if zr.options & 0x40 != 0 && zr.output != 0 {
            d.group.output = (zr.output as u32).min(MAX_OUTPUT_BUSES - 1);
        }
        // play range: SPEC-GAP: exs.md §3 does not say whether the sample end is inclusive;
        // read as exclusive (like the loop end), 0 = the whole file
        let len = frames.max(1);
        let start = (zr.start as u64).min(len - 1);
        let end = if zr.end == 0 {
            len
        } else {
            (zr.end as u64).clamp(start + 1, len)
        };
        z.start = start;
        z.end = (end < len).then_some(end);
        // SPEC-GAP: exs.md §7 says to subtract 1 from the loop end "for our inclusive model";
        // the Haus model's loop end is exclusive too, so the stored end is kept as is.
        if zr.loop_options & 0x01 != 0 {
            let (ls, le) = (
                (zr.loop_start as u64).clamp(start, end),
                (zr.loop_end as u64).clamp(start, end),
            );
            if le > ls {
                let mode = match zr.loop_dir {
                    2 => LoopMode::PingPong,
                    1 => {
                        notes.approx("exs:loop-backward", "backward loops play forward");
                        LoopMode::Forward
                    }
                    _ => LoopMode::Forward,
                };
                let xf = (zr.loop_xfade_ms as u64 * rate.max(1) as u64 / 1000).min(le - ls);
                if xf > 0 && zr.loop_options & 0x02 == 0 {
                    notes.approx(
                        "exs:loop-crossfade",
                        "linear loop crossfades play with the sampler's own crossfade curve",
                    );
                }
                z.looping = Some(ZoneLoop {
                    mode,
                    start: ls,
                    end: le,
                    crossfade: xf,
                    on_release: if zr.loop_options & 0x04 != 0 {
                        LoopRelease::Exit
                    } else {
                        LoopRelease::Continue
                    },
                });
                if zr.loop_tune != 0 {
                    notes.skip("exs:loop-tune", "loop tuning is not supported");
                }
            } else {
                notes.approx("exs:loop", "empty loops play unlooped");
            }
        }
        if zr.fade_out != 0 || zr.sample_offset != 0 {
            notes.skip(
                "exs:zone-extended",
                "zone fade-out and sample offset (extended zone fields) are not used",
            );
        }
        let gid = asm.group(&d);
        placed.push(si);
        asm.zone(z, gid);
    }
    if random_global {
        notes.approx("exs:random-select", "random sample selection plays the sampler's random round robin among zones on the same keys");
    }

    // sample names relative to the folder the found samples share
    let paths: Vec<PathBuf> = found
        .values()
        .filter_map(|r| r.as_ref().ok().map(|x| x.0.clone()))
        .collect();
    let mut root: Option<PathBuf> = None;
    for p in &paths {
        let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
        root = Some(match root {
            None => parent,
            Some(r) => common(&r, &parent),
        });
    }
    let root = root.unwrap_or_default();
    let mut imported: Vec<ImportedSample> = Vec::new();
    let mut name_of: BTreeMap<usize, String> = BTreeMap::new();
    for (si, r) in &found {
        if let Ok((p, _, _)) = r {
            let rel = p
                .strip_prefix(&root)
                .map(|x| x.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
            name_of.insert(*si, rel.clone());
            if !imported.iter().any(|s| s.name == rel) {
                imported.push(ImportedSample {
                    name: rel,
                    source: SampleSource::File(p.clone()),
                });
            }
        }
    }
    asm.finish(&mut inst);
    // sample names, in the order zones were added (zone ids follow `placed`)
    for (zid, si) in placed.iter().enumerate() {
        if let Some(z) = inst.zones.get_mut(&ZoneId(zid as u32)) {
            z.sample = SampleRef::File(name_of[si].clone());
        }
    }
    random_positions(&mut inst);

    let (notes, unknown) = notes.finish();
    inst.import_notes = notes.clone();
    inst.source = Some(InstrumentSource {
        format: "exs".into(),
        file: file.to_string(),
        importer_version: EXS_IMPORTER_VERSION,
    });
    let report = ImportReport {
        format: "exs".into(),
        file: file.to_string(),
        groups: inst.groups.len(),
        zones: inst.zones.len(),
        notes,
        unknown,
        presets: Vec::new(),
    };
    Ok(Imported {
        instrument: inst,
        samples: imported,
        report,
    })
}

fn group_name(g: &GroupRec, i: usize) -> String {
    if g.name.is_empty() {
        format!("Group {}", i + 1)
    } else {
        g.name.clone()
    }
}

/// In random-select groups, zones sharing a key and velocity range become positions of the
/// group's random cycle.
fn random_positions(inst: &mut HausInstrument) {
    let random: Vec<GroupId> = inst
        .groups
        .values()
        .filter(|g| g.round_robin == RoundRobinMode::Random)
        .map(|g| g.id)
        .collect();
    for g in random {
        let mut by_range: BTreeMap<(u8, u8, u8, u8), Vec<ZoneId>> = BTreeMap::new();
        for z in inst.zones_of(g) {
            if z.round_robin.is_none() {
                by_range
                    .entry((z.keys.lo, z.keys.hi, z.velocity.lo, z.velocity.hi))
                    .or_default()
                    .push(z.id);
            }
        }
        for ids in by_range.values().filter(|v| v.len() > 1) {
            for (n, id) in ids.iter().enumerate() {
                if let Some(z) = inst.zones.get_mut(id) {
                    z.round_robin = Some(n as u32);
                }
            }
        }
    }
}
