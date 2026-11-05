//! The DecentSampler importer (M3/T4), written from `docs/formats/dspreset.md` only (clean
//! room).
//!
//! Pipeline: bytes → our own XML reader ([`xml`]) → groups and samples with the inheritance
//! rules (§3.1: most specific wins; volumes multiply; pan and tuning offsets add) → the
//! Haus Sampler model. Group-level settings (envelope, round robin, choke, output, effects,
//! LFOs, keyswitch articulations) decide the Haus group (`crate::assemble`). The `<ui>`,
//! unmapped bindings, oscillators, note sequences and the arpeggiator are kept as data in
//! `extra` for M3b and reported. Containers: `.dspreset` files and `.dsbundle` folders;
//! `.dslibrary` zip archives are refused with a message (no zip reader in the tree).

pub(crate) mod xml;

use crate::assemble::{Assembler, GroupDraft, clampf};
use crate::notes::Notes;
use crate::probe::{SampleInfo, probe};
use crate::{
    ImportError, ImportInput, ImportReport, Imported, ImportedSample, Importer, PresetListing,
    SampleSource,
};
use ph_core::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use xml::Element;

/// Bumped whenever the importer changes what it maps.
pub const DS_IMPORTER_VERSION: u32 = 1;

/// Largest preset file read: 32 MiB.
const MAX_FILE: u64 = 32 << 20;

/// Tempo used for `delayUnit="beats"` (the importer does not know the project's).
const BEATS_BPM: f64 = 120.0;

/// DecentSampler presets (`docs/formats/dspreset.md`).
#[derive(Copy, Clone, Debug, Default)]
pub struct DecentSampler;

impl Importer for DecentSampler {
    fn format(&self) -> &'static str {
        "dspreset"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["dspreset", "dsbundle", "dslibrary"]
    }
    fn version(&self) -> u32 {
        DS_IMPORTER_VERSION
    }
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        let file = input.name();
        match input {
            ImportInput::Bytes { bytes, .. } => import_bytes(bytes, &file, None),
            ImportInput::Path(p) => {
                let ext = p
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if ext == "dslibrary" {
                    return Err(ImportError::Unsupported {
                        file,
                        reason: "DecentSampler .dslibrary archives are zip files, which the sampler does not open yet; unzip it and import the .dspreset or .dsbundle inside".into(),
                    });
                }
                if p.is_dir() {
                    return import_bundle(p, &file);
                }
                let io = |e: std::io::Error| ImportError::Io {
                    path: p.display().to_string(),
                    message: e.to_string(),
                };
                if std::fs::metadata(p).map_err(io)?.len() > MAX_FILE {
                    return Err(malformed(&file, "the file is larger than 32 MiB"));
                }
                let bytes = std::fs::read(p).map_err(io)?;
                let dir = p.parent().map(|d| {
                    if d.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        d.to_path_buf()
                    }
                });
                import_bytes(&bytes, &file, dir.as_deref())
            }
        }
    }
}

fn malformed(file: &str, reason: impl Into<String>) -> ImportError {
    ImportError::Malformed {
        format: "DecentSampler".into(),
        file: file.to_string(),
        reason: reason.into(),
    }
}

/// A `.dsbundle` folder (§1): the first preset of `DSLibraryInfo.xml`'s menu, else the first
/// `.dspreset` at the bundle's root by name; every preset is listed in the report.
fn import_bundle(dir: &Path, file: &str) -> Result<Imported, ImportError> {
    let mut presets: Vec<String> = Vec::new();
    let mut library = None;
    let info = dir.join("DSLibraryInfo.xml");
    if let Ok(b) = std::fs::read(&info)
        && let Ok(root) = xml::parse(&xml::decode(&b))
    {
        fn walk(e: &Element, out: &mut Vec<String>) {
            for c in e.elements() {
                if c.name == "preset"
                    && let Some(f) = c.attr("file")
                {
                    out.push(f.replace('\\', "/"));
                }
                walk(c, out);
            }
        }
        walk(&root, &mut presets);
        library = Some(root);
    }
    if presets.is_empty() {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map_err(|e| ImportError::Io {
                path: dir.display().to_string(),
                message: e.to_string(),
            })?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_lowercase().ends_with(".dspreset"))
            .collect();
        names.sort();
        presets = names;
    }
    let first = presets.first().ok_or_else(|| ImportError::Unsupported {
        file: file.to_string(),
        reason: "the bundle holds no .dspreset".into(),
    })?;
    let path = resolve_rel(dir, first).ok_or_else(|| ImportError::Io {
        path: dir.join(first).display().to_string(),
        message: "the preset named by the library menu is missing".into(),
    })?;
    let mut out = DecentSampler.import(ImportInput::Path(&path))?;
    if let Some(lib) = library {
        out.instrument
            .extra
            .insert("dspreset.library".into(), lib.to_xml());
        let note = ImportNote {
            kind: ImportNoteKind::Kept,
            what: "file:DSLibraryInfo.xml".into(),
            detail: "the library's name and preset menu are kept as data".into(),
            count: 1,
        };
        out.instrument.import_notes.push(note.clone());
        out.report.notes.push(note);
    }
    out.report.presets = presets
        .iter()
        .enumerate()
        .map(|(i, p)| PresetListing {
            bank: 0,
            program: i as u16,
            name: p.clone(),
            imported: i == 0,
        })
        .collect();
    out.report.file = file.to_string();
    if let Some(s) = out.instrument.source.as_mut() {
        s.file = file.to_string();
    }
    Ok(out)
}

// ------------------------------------------------------------------------- values

fn num(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

fn boolean(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// A volume (§3.2): linear, or decibels with a `dB` suffix. Returns decibels.
fn volume_db(v: &str) -> Option<f64> {
    let t = v.trim();
    let lower = t.to_ascii_lowercase();
    if let Some(n) = lower.strip_suffix("db") {
        return num(n);
    }
    let lin = num(t)?;
    Some(if lin <= 0.0 {
        -144.0
    } else {
        20.0 * lin.log10()
    })
}

fn key(v: &str) -> Option<u8> {
    match num(v) {
        Some(n) => (0.0..=127.0).contains(&n).then_some(n.round() as u8),
        None => crate::sfz::parse_key(v.trim())
            .and_then(|k| u8::try_from(k).ok())
            .filter(|k| *k <= 127),
    }
}

/// A path relative to the preset's folder (§1): `/` or `\`, exact case first, then
/// ignoring case per component. Lexical `..` is allowed; agents are confined by
/// `sampler.import`.
fn resolve_rel(dir: &Path, rel: &str) -> Option<PathBuf> {
    resolve_rel_case(dir, rel).map(|(p, _)| p)
}

/// [`resolve_rel`], also saying whether a component's case differed from the disk's
/// (decided from the folder listings, so case-insensitive file systems agree).
fn resolve_rel_case(dir: &Path, rel: &str) -> Option<(PathBuf, bool)> {
    let rel = rel.replace('\\', "/");
    let mut cur = dir.to_path_buf();
    let mut fixed = false;
    for part in rel.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            cur.pop();
            continue;
        }
        let names: Vec<std::ffi::OsString> = std::fs::read_dir(&cur)
            .ok()?
            .flatten()
            .map(|e| e.file_name())
            .collect();
        if names.iter().any(|n| n == part) {
            cur.push(part);
            continue;
        }
        let want = part.to_lowercase();
        let hit = names
            .into_iter()
            .filter(|n| n.to_string_lossy().to_lowercase() == want)
            .min()?;
        cur.push(hit);
        fixed = true;
    }
    cur.is_file().then_some((cur, fixed))
}

/// Attribute lookup through the three levels (§3.1): the most specific wins.
struct Levels<'a> {
    sample: &'a Element,
    group: &'a Element,
    groups: &'a Element,
}

impl<'a> Levels<'a> {
    fn get(&self, name: &str) -> Option<&'a str> {
        self.sample
            .attr(name)
            .or_else(|| self.group.attr(name))
            .or_else(|| self.groups.attr(name))
    }
    fn num(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(num)
    }
    fn first(&self, names: &[&str]) -> Option<f64> {
        names.iter().find_map(|n| self.num(n))
    }
    fn bool(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(boolean)
    }
}

// ------------------------------------------------------------------------- bindings

/// The sound parameters a binding can drive, with the model target and the scale from
/// the parameter's unit.
fn binding_target(param: &str) -> Option<(ModTarget, f64)> {
    Some(match param {
        "GLOBAL_TUNING" | "GROUP_TUNING" => (ModTarget::Pitch, 100.0),
        "PAN" => (ModTarget::Pan, 0.01),
        "AMP_VOLUME" | "TAG_VOLUME" => (ModTarget::Volume, 1.0),
        "FX_FILTER_FREQUENCY" => (ModTarget::Cutoff, 1.0),
        _ => return None,
    })
}

/// The depth of a binding in the target's unit (approximate: §5.1, §5.4).
fn binding_depth(b: &Element, target: ModTarget, scale: f64, amount: f64) -> Option<f64> {
    let lo = b.attr("translationOutputMin").and_then(num).unwrap_or(0.0);
    let hi = b.attr("translationOutputMax").and_then(num).unwrap_or(1.0);
    let rev = b
        .attr("translationReversed")
        .and_then(boolean)
        .unwrap_or(false);
    let (lo, hi) = if rev { (hi, lo) } else { (lo, hi) };
    let d = match target {
        ModTarget::Cutoff => {
            if lo <= 0.0 || hi <= 0.0 {
                return None;
            }
            1200.0 * (hi / lo).log2()
        }
        ModTarget::Volume => {
            // linear gain: the dB span between the two ends (floored at −48 dB)
            let db = |g: f64| {
                if g <= 0.0 {
                    -48.0
                } else {
                    (20.0 * g.log10()).max(-48.0)
                }
            };
            db(hi) - db(lo)
        }
        _ => (hi - lo) * scale,
    };
    Some(d * amount)
}

/// Groups a binding addresses (§5.1): by index or by tag; `None` = the whole instrument.
fn bound_groups(b: &Element, groups: &[&Element]) -> Option<Vec<usize>> {
    match b.attr("level") {
        Some("group") => {}
        _ => return None,
    }
    if let Some(i) = b
        .attr("groupIndex")
        .or_else(|| b.attr("position"))
        .and_then(num)
    {
        return Some(if i >= 0.0 && (i as usize) < groups.len() {
            vec![i as usize]
        } else {
            vec![]
        });
    }
    let tags: Vec<String> = b
        .attr("groupTags")
        .or_else(|| b.attr("tags"))
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some(
        (0..groups.len())
            .filter(|&g| {
                groups[g]
                    .attr("tags")
                    .is_some_and(|t| t.split(',').any(|x| tags.iter().any(|w| w == x.trim())))
            })
            .collect(),
    )
}

// ------------------------------------------------------------------------- effects

/// One `<effect>` to a built-in effect, a volume change, or nothing (reported).
enum Fx {
    Effect(GroupEffect),
    Gain(f64),
    None,
}

fn effect(e: &Element, notes: &mut Notes) -> Fx {
    let t = e.attr("type").unwrap_or("");
    let f = |n: &str, d: f64| e.attr(n).and_then(num).unwrap_or(d);
    let bypass = e.attr("enabled").and_then(boolean) == Some(false);
    let mk = |name: &str, params: Vec<(&str, f64)>, mix: f64| {
        Fx::Effect(GroupEffect {
            name: name.into(),
            params: params
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            bypass,
            mix: mix.clamp(0.0, 1.0) as f32,
        })
    };
    let what = format!("effect:{t}");
    match t {
        "lowpass" | "lowpass_4pl" | "highpass" | "bandpass" | "notch" | "lowpass_1pl" => {
            let mode = match t {
                "highpass" => 1.0,
                "bandpass" => 2.0,
                "notch" => 3.0,
                _ => 0.0,
            };
            if t == "lowpass_1pl" || t == "lowpass_4pl" {
                notes.approx(what, "played by the built-in filter's 12 dB/oct low-pass");
            }
            let res = if t == "notch" {
                f("Q", 0.7) / 18.0
            } else {
                f("resonance", 0.7) / 5.0
            };
            let def = if t == "notch" { 10_000.0 } else { 22_000.0 };
            mk(
                "filter",
                vec![
                    ("mode", mode),
                    ("cutoff", f("frequency", def).clamp(20.0, 20_000.0)),
                    ("resonance", res.clamp(0.0, 1.0)),
                ],
                1.0,
            )
        }
        "peak" | "low_shelf" | "high_shelf" => {
            let gain = e.attr("gain").and_then(num).unwrap_or(1.0);
            let db = if e.attr("gainUnit") == Some("decibels") {
                gain
            } else if gain <= 0.0 {
                -24.0
            } else {
                20.0 * gain.log10()
            };
            let (ty, def_f) = match t {
                "low_shelf" => (1.0, 200.0),
                "high_shelf" => (2.0, 5000.0),
                _ => (0.0, 10_000.0),
            };
            let q = e
                .attr("Q")
                .or_else(|| e.attr("q"))
                .and_then(num)
                .unwrap_or(0.7);
            let mut params = vec![
                ("b1_on", 0.0),
                ("b2_on", 1.0),
                ("b2_type", ty),
                ("b2_freq", f("frequency", def_f).clamp(20.0, 20_000.0)),
                ("b2_gain", db.clamp(-24.0, 24.0)),
                ("b2_q", q.clamp(0.1, 18.0)),
                ("b3_on", 0.0),
                ("b4_on", 0.0),
            ];
            params.sort_by(|a, b| a.0.cmp(b.0));
            mk("eq", params, 1.0)
        }
        "gain" => {
            let level = f("level", -6.0);
            Fx::Gain(if e.attr("levelUnit") == Some("linear") {
                if level <= 0.0 {
                    -144.0
                } else {
                    20.0 * level.log10()
                }
            } else {
                level
            })
        }
        "compressor" => {
            notes.approx(what, "played by the built-in compressor");
            mk(
                "compressor",
                vec![
                    ("threshold", f("threshold", -12.0)),
                    ("ratio", f("ratio", 4.0).clamp(1.0, 30.0)),
                    ("attack", f("attack", 5.0)),
                    ("release", f("release", 100.0)),
                    ("makeup", f("outputGain", 0.0).clamp(-12.0, 24.0)),
                ],
                1.0,
            )
        }
        "reverb" => {
            notes.approx(what, "played by the built-in reverb");
            mk(
                "reverb",
                vec![
                    ("size", f("roomSize", 0.7).clamp(0.0, 1.0)),
                    ("damping", f("damping", 0.3).clamp(0.0, 1.0)),
                    ("mix", 1.0),
                ],
                f("wetLevel", 0.0),
            )
        }
        "delay" => {
            notes.approx(
                what,
                "played by the built-in delay (musical times at 120 BPM)",
            );
            let mut t = f("delayTime", 0.7);
            if e.attr("delayTimeFormat") == Some("musical_time") {
                t *= 60.0 / BEATS_BPM;
            }
            let ms = (t * 1000.0).clamp(1.0, 2000.0);
            mk(
                "delay",
                vec![
                    ("time_l", ms),
                    ("time_r", ms),
                    ("feedback", f("feedback", 0.2).clamp(0.0, 1.0)),
                    ("mix", 1.0),
                ],
                f("wetLevel", 0.5),
            )
        }
        "chorus" => {
            notes.approx(what, "played by the built-in chorus");
            mk(
                "chorus",
                vec![("rate", f("modRate", 0.2).clamp(0.05, 10.0)), ("mix", 1.0)],
                f("mix", 0.5),
            )
        }
        "phaser" => {
            notes.approx(what, "played by the built-in phaser");
            mk(
                "phaser",
                vec![
                    ("rate", f("modRate", 0.2).clamp(0.02, 10.0)),
                    ("depth", f("modDepth", 0.2).clamp(0.0, 1.0)),
                    ("feedback", f("feedback", 0.7).clamp(-0.95, 0.95)),
                    ("centre", f("centerFrequency", 400.0).clamp(100.0, 8000.0)),
                    ("mix", 1.0),
                ],
                f("mix", 0.5),
            )
        }
        "convolution" => {
            notes.approx(
                what,
                "played by the built-in convolution with its generated hall: the preset's impulse response file is kept as data",
            );
            mk("convolution", vec![("mix", 1.0)], f("mix", 0.5))
        }
        "stereo_simulator" => {
            notes.approx(what, "played by the built-in stereo widener");
            mk(
                "stereo",
                vec![("width", (1.0 + f("width", 0.5)).clamp(0.0, 2.0))],
                1.0,
            )
        }
        "bit_crusher" => {
            notes.approx(what, "played by the built-in bit crusher");
            mk(
                "bitcrush",
                vec![("bits", f("bitDepth", 24.0).clamp(1.0, 24.0)), ("mix", 1.0)],
                f("mix", 1.0),
            )
        }
        "pitch_shift" | "wave_folder" | "wave_shaper" | "stutter" => {
            notes.skip(what, "no built-in equivalent yet; kept as data");
            Fx::None
        }
        _ => {
            notes.unknown(format!("effect:{t}"));
            Fx::None
        }
    }
}

/// The chain of an `<effects>` element: effects and the summed gain in dB.
fn chain(e: Option<&Element>, notes: &mut Notes) -> (Vec<GroupEffect>, f64) {
    let mut fx = Vec::new();
    let mut gain = 0.0;
    if let Some(e) = e {
        for x in e.children_named("effect") {
            match effect(x, notes) {
                Fx::Effect(g) => fx.push(g),
                Fx::Gain(db) => {
                    if x.attr("enabled").and_then(boolean) != Some(false) {
                        gain += db;
                    }
                }
                Fx::None => {}
            }
        }
    }
    (fx, gain)
}

// ------------------------------------------------------------------------- attributes

const SAMPLE_ATTRS: &[&str] = &[
    "path",
    "rootNote",
    "loNote",
    "hiNote",
    "loVel",
    "hiVel",
    "start",
    "end",
    "tuning",
    "volume",
    "pan",
    "pitchKeyTrack",
    "delay",
    "delayUnit",
    "playbackMode",
    "sampleMode",
    "tags",
    "trigger",
    "releaseTriggerDecay",
    "previousNotes",
    "legatoInterval",
    "seqMode",
    "seqPosition",
    "seqLength",
    "loopStart",
    "loopEnd",
    "loopEnabled",
    "loopCrossfade",
    "loopCrossfadeMode",
    "loopDuringRelease",
    "ampEnvEnabled",
    "ampEnvDelay",
    "attack",
    "ampEnvAttack",
    "ampEnvHold",
    "decay",
    "ampEnvDecay",
    "sustain",
    "ampEnvSustain",
    "release",
    "ampEnvRelease",
    "attackCurve",
    "decayCurve",
    "releaseCurve",
    "silencedByTags",
    "silencingMode",
    "silencingDecay",
    "retriggerEnabled",
    "retriggerInterval",
    "retriggerIntervalUnit",
    "beats",
    "originalTempo",
    "slices",
    "sliceGrid",
    "sliceLoopMode",
    "sliceEnvelope",
    "ampVelTrack",
    "polyphony",
];
const GROUP_ATTRS: &[&str] = &[
    "name",
    "enabled",
    "groupVolume",
    "groupPan",
    "groupTuning",
    "glideTime",
    "glideMode",
];
const GROUPS_ATTRS: &[&str] = &[
    "globalVolume",
    "globalPan",
    "globalTuning",
    "tuningIgnoresKeyTrack",
    "glideTime",
    "glideMode",
];

fn known_attr(level: &str, name: &str) -> bool {
    let dynamic = |prefixes: &[&str]| {
        prefixes.iter().any(|p| {
            name.strip_prefix(p)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        })
    };
    if dynamic(&["loCC", "hiCC", "onLoCC", "onHiCC"]) {
        return true;
    }
    if let Some(n) = name.strip_prefix("output")
        && (n.ends_with("Target") || n.ends_with("Volume"))
    {
        return true;
    }
    SAMPLE_ATTRS.contains(&name)
        || (level != "sample" && GROUP_ATTRS.contains(&name))
        || (level == "groups" && GROUPS_ATTRS.contains(&name))
}

fn check_attrs(e: &Element, level: &str, notes: &mut Notes) {
    for (k, _) in &e.attrs {
        if !known_attr(level, k) {
            notes.unknown(format!("attribute:{level}.{k}"));
        }
    }
}

// ------------------------------------------------------------------------- import

struct Keyswitch {
    key: u8,
    enabled: BTreeSet<usize>,
}

pub fn import_bytes(bytes: &[u8], file: &str, dir: Option<&Path>) -> Result<Imported, ImportError> {
    let mut notes = Notes::default();
    let text = xml::decode(bytes);
    let root = xml::parse(&text).map_err(|r| malformed(file, r))?;
    if root.name != "DecentSampler" {
        return Err(malformed(
            file,
            format!("the root element is <{}>, not <DecentSampler>", root.name),
        ));
    }
    let groups_el = root
        .child("groups")
        .ok_or_else(|| malformed(file, "there is no <groups> element"))?;
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "DecentSampler instrument".into());
    let mut inst = HausInstrument::new(&stem);
    if let Some(v) = root.attr("minVersion") {
        inst.extra
            .insert("dspreset.minVersion".into(), v.to_string());
    }
    if root.attr("limiterStyle").is_some() {
        notes.skip(
            "attribute:limiterStyle",
            "the output limiter is not applied",
        );
    }
    for c in root.elements() {
        match c.name.as_str() {
            "groups" | "effects" | "midi" | "modulators" | "tags" | "buses" => {}
            "ui" => {
                inst.extra.insert("dspreset.ui".into(), c.to_xml());
                notes.kept(
                    "element:ui",
                    "the UI (knobs, menus, buttons, keyboard colours) is kept as data for M3b; its controls are not shown yet",
                );
            }
            "noteSequences" | "arpeggiator" => {
                inst.extra
                    .insert(format!("dspreset.{}", c.name), c.to_xml());
                notes.kept(
                    format!("element:{}", c.name),
                    "kept as data; not played yet",
                );
            }
            other => notes.unknown(format!("element:{other}")),
        }
    }
    check_attrs(groups_el, "groups", &mut notes);
    let groups: Vec<&Element> = groups_el.children_named("group").collect();
    for c in groups_el.elements() {
        if c.name != "group" {
            notes.unknown(format!("element:groups.{}", c.name));
        }
    }

    // tags (§5.5)
    let mut tag_info: BTreeMap<String, (bool, f64, Option<u32>)> = BTreeMap::new();
    if let Some(t) = root.child("tags") {
        for tag in t.children_named("tag") {
            let Some(n) = tag.attr("name") else { continue };
            let enabled = tag.attr("enabled").and_then(boolean).unwrap_or(true);
            let vol = tag.attr("volume").and_then(volume_db).unwrap_or(0.0);
            let poly = tag
                .attr("polyphony")
                .and_then(num)
                .filter(|p| *p >= 1.0)
                .map(|p| p as u32);
            tag_info.insert(n.trim().to_string(), (enabled, vol, poly));
        }
    }

    // keyswitches (§5.3): <midi><note> whose bindings only set group ENABLED
    let mut keyswitches: Vec<Keyswitch> = Vec::new();
    if let Some(m) = root.child("midi") {
        let mut kept_midi = false;
        for n in m.elements() {
            let binds: Vec<&Element> = n.children_named("binding").collect();
            let is_ks = n.name == "note"
                && !binds.is_empty()
                && binds.iter().all(|b| {
                    b.attr("level") == Some("group")
                        && b.attr("parameter") == Some("ENABLED")
                        && b.attr("translation") == Some("fixed_value")
                });
            if is_ks
                && let Some(k) = n
                    .attr("note")
                    .and_then(|v| key(v.split('-').next().unwrap_or(v)))
            {
                let mut enabled = BTreeSet::new();
                for b in &binds {
                    let on = b
                        .attr("translationValue")
                        .and_then(boolean)
                        .unwrap_or(false);
                    for g in bound_groups(b, &groups).unwrap_or_default() {
                        if on {
                            enabled.insert(g);
                        }
                    }
                }
                let swallow = n.attr("swallowNotes").and_then(boolean).unwrap_or(false);
                if !swallow {
                    notes.approx(
                        "midi:keyswitch",
                        "keyswitch keys without swallowNotes would also play samples; the sampler's keyswitch keys are silent",
                    );
                }
                keyswitches.push(Keyswitch { key: k, enabled });
                continue;
            }
            if n.name == "cc" {
                // mapped below as modulation routes where it can be
                continue;
            }
            kept_midi = true;
            notes.kept(
                format!("midi:{}", n.name),
                "MIDI bindings other than keyswitches are kept as data",
            );
        }
        if kept_midi || m.children_named("cc").next().is_some() {
            inst.extra.insert("dspreset.midi".into(), m.to_xml());
        }
    }
    let initially: BTreeSet<usize> = (0..groups.len())
        .filter(|&g| groups[g].attr("enabled").and_then(boolean).unwrap_or(true))
        .collect();
    let in_keyswitch: BTreeSet<usize> = keyswitches
        .iter()
        .flat_map(|k| k.enabled.iter().copied())
        .collect();
    for (i, k) in keyswitches.iter().enumerate() {
        let name = k
            .enabled
            .iter()
            .next()
            .and_then(|&g| groups[g].attr("name"))
            .map(str::to_string)
            .unwrap_or_else(|| format!("Key {}", crate::sfz::note_name(k.key)));
        inst.articulations.push(Articulation {
            id: ArticulationId(i as u32),
            name,
            trigger: ArticulationTrigger::Key { key: k.key },
            color: None,
        });
    }
    if !keyswitches.is_empty() {
        let enabled_in: BTreeSet<usize> = initially.intersection(&in_keyswitch).copied().collect();
        inst.settings.default_articulation = keyswitches
            .iter()
            .position(|k| k.enabled == enabled_in)
            .map(|i| ArticulationId(i as u32));
    }

    // modulators (§5.4): per group (index) the LFOs/envelope and routes they add
    let mut group_lfos: Vec<Vec<Lfo>> = vec![Vec::new(); groups.len()];
    let mut group_env: Vec<Option<Envelope>> = vec![None; groups.len()];
    let mut group_routes: Vec<Vec<(ModSource, ModTarget, f64)>> = vec![Vec::new(); groups.len()];
    let mut global_routes: Vec<(ModSource, ModTarget, f64)> = Vec::new();
    if let Some(mods) = root.child("modulators") {
        inst.extra
            .insert("dspreset.modulators".into(), mods.to_xml());
        for m in mods.elements() {
            let amount = m.attr("modAmount").and_then(num).unwrap_or(1.0);
            enum Src {
                Lfo(Lfo),
                Env(Envelope),
                Plain(ModSource),
            }
            let src = match m.name.as_str() {
                "lfo" => {
                    let shape = match m.attr("shape").unwrap_or("sine") {
                        "square" => LfoShape::Square,
                        "saw" => LfoShape::SawUp,
                        "triangle" => LfoShape::Triangle,
                        _ => LfoShape::Sine,
                    };
                    let mut rate = m.attr("frequency").and_then(num).unwrap_or(1.0);
                    if m.attr("frequencyFormat") == Some("musical_time") {
                        notes.approx(
                            "modulator:lfo",
                            "LFO rates in musical time are read at 120 BPM",
                        );
                        rate = BEATS_BPM / 60.0 / rate.max(1e-3);
                    }
                    if m.attr("scope").unwrap_or("global") == "global" {
                        notes.approx("modulator:lfo", "global (free-running) LFOs run per voice");
                    }
                    Src::Lfo(Lfo {
                        shape,
                        rate: clampf(rate, 0.01, 100.0) as f32,
                        delay: clampf(m.attr("delayTime").and_then(num).unwrap_or(0.0), 0.0, 60.0)
                            as f32,
                        fade: 0.0,
                    })
                }
                "envelope" => {
                    let t = |n: &str, d: f64| {
                        clampf(m.attr(n).and_then(num).unwrap_or(d), 0.0, 60.0) as f32
                    };
                    Src::Env(Envelope {
                        delay: t("delay", 0.0),
                        attack: t("attack", 0.0),
                        hold: t("hold", 0.0),
                        decay: t("decay", 0.0),
                        sustain: clampf(m.attr("sustain").and_then(num).unwrap_or(1.0), 0.0, 1.0)
                            as f32,
                        release: t("release", 0.1).max(0.001),
                    })
                }
                "midiCC" => match m.attr("number").and_then(num) {
                    Some(n) if (0.0..=127.0).contains(&n) => Src::Plain(ModSource::Cc {
                        controller: n as u8,
                    }),
                    _ => {
                        notes.skip(
                            "modulator:midiCC",
                            "a midiCC modulator without a valid number was skipped",
                        );
                        continue;
                    }
                },
                "midiVelocity" => Src::Plain(ModSource::Velocity),
                "random" => {
                    if m.attr("mode") == Some("periodic") {
                        notes.approx(
                            "modulator:random",
                            "periodic random modulation is a new value per note",
                        );
                    }
                    Src::Plain(ModSource::Random)
                }
                other => {
                    notes.skip(
                        format!("modulator:{other}"),
                        "not supported yet; kept as data",
                    );
                    continue;
                }
            };
            for b in m.children_named("binding") {
                let param = b.attr("parameter").unwrap_or("");
                let Some((target, scale)) = binding_target(param) else {
                    notes.kept(
                        format!("binding:{param}"),
                        "a modulation target the sampler cannot modulate yet; kept as data",
                    );
                    continue;
                };
                let Some(depth) = binding_depth(b, target, scale, amount) else {
                    notes.skip(
                        format!("binding:{param}"),
                        "a binding range that cannot be converted",
                    );
                    continue;
                };
                if b.attr("modBehavior").is_some_and(|x| x != "add") {
                    notes.approx(
                        "binding:modBehavior",
                        "modulation is added to the stored value (the sampler's matrix adds)",
                    );
                }
                notes.approx(
                    format!("modulator:{}", m.name),
                    "depth taken from the binding's output range × modAmount",
                );
                let targets = bound_groups(b, &groups);
                let all: Vec<usize> = (0..groups.len()).collect();
                let gs = targets.clone().unwrap_or(all);
                match &src {
                    Src::Lfo(l) => {
                        for &g in &gs {
                            if group_lfos[g].len() >= 4 {
                                notes.skip("modulator:lfo", "more than four LFOs in a group");
                                continue;
                            }
                            let index = group_lfos[g].len() as u8;
                            group_lfos[g].push(l.clone());
                            group_routes[g].push((ModSource::Lfo { index }, target, depth));
                        }
                    }
                    Src::Env(e) => {
                        for &g in &gs {
                            group_env[g] = Some(e.clone());
                            group_routes[g].push((ModSource::Envelope { index: 1 }, target, depth));
                        }
                    }
                    Src::Plain(s) => match targets {
                        Some(gs) => {
                            for g in gs {
                                group_routes[g].push((s.clone(), target, depth));
                            }
                        }
                        None => global_routes.push((s.clone(), target, depth)),
                    },
                }
            }
        }
    }
    // <midi><cc> bindings: added routes (§5.3)
    if let Some(m) = root.child("midi") {
        for cc in m.children_named("cc") {
            let Some(n) = cc
                .attr("number")
                .and_then(num)
                .filter(|n| (0.0..=127.0).contains(n))
            else {
                continue;
            };
            for b in cc.children_named("binding") {
                let param = b.attr("parameter").unwrap_or("");
                match binding_target(param)
                    .and_then(|(t, s)| binding_depth(b, t, s, 1.0).map(|d| (t, d)))
                {
                    Some((target, depth)) => {
                        notes.approx(
                            "midi:cc",
                            "controller bindings are played as modulation added to the stored value (the controller's range sets the depth)",
                        );
                        let src = ModSource::Cc {
                            controller: n as u8,
                        };
                        match bound_groups(b, &groups) {
                            Some(gs) => gs
                                .into_iter()
                                .for_each(|g| group_routes[g].push((src.clone(), target, depth))),
                            None => global_routes.push((src.clone(), target, depth)),
                        }
                    }
                    None => notes.kept(
                        format!("binding:{param}"),
                        "a controller binding the sampler cannot map yet; kept as data",
                    ),
                }
            }
        }
    }

    // effects (§4) and buses (§5.5)
    let (inst_fx, inst_gain) = chain(root.child("effects"), &mut notes);
    if let Some(e) = root.child("effects") {
        inst.extra.insert("dspreset.effects".into(), e.to_xml());
        if !inst_fx.is_empty() {
            notes.approx(
                "effects:instrument",
                "the instrument's effect chain runs in every group's insert chain",
            );
        }
    }
    inst.settings.volume_db = clampf(inst_gain, -144.0, 24.0) as f32;
    let mut buses: Vec<(Vec<GroupEffect>, f64)> = Vec::new();
    if let Some(b) = root.child("buses") {
        inst.extra.insert("dspreset.buses".into(), b.to_xml());
        for bus in b.children_named("bus") {
            let (fx, g) = chain(bus.child("effects"), &mut notes);
            let vol = bus.attr("busVolume").and_then(volume_db).unwrap_or(0.0);
            buses.push((fx, g + vol));
        }
    }

    let mut asm = Assembler::default();
    for (s, t, d) in &global_routes {
        asm.global_routes.push(ModRoute {
            source: s.clone(),
            target: *t,
            depth: *d as f32,
            curve: ModCurve::Linear,
            group: None,
        });
    }
    let mut samples: Vec<(String, PathBuf)> = Vec::new();
    let mut probes: BTreeMap<PathBuf, Result<SampleInfo, String>> = BTreeMap::new();
    let mut zone_files: Vec<PathBuf> = Vec::new();
    let mut choke_sets: Vec<BTreeSet<String>> = Vec::new();

    for (gi, g) in groups.iter().enumerate() {
        check_attrs(g, "group", &mut notes);
        let enabled = initially.contains(&gi);
        if !enabled && !in_keyswitch.contains(&gi) {
            notes.skip(
                "group:enabled",
                "disabled groups no keyswitch enables were skipped",
            );
            continue;
        }
        let gname = g
            .attr("name")
            .map(str::to_string)
            .unwrap_or_else(|| format!("Group {}", gi + 1));
        let (gfx, ggain) = chain(g.child("effects"), &mut notes);
        for c in g.elements() {
            match c.name.as_str() {
                "sample" | "effects" => {}
                "oscillator" => {
                    let k = format!("dspreset.group{gi}.oscillators");
                    let prev = inst.extra.remove(&k).unwrap_or_default();
                    inst.extra.insert(k, prev + &c.to_xml());
                    notes.skip(
                        "element:oscillator",
                        "oscillators are kept as data; not played yet",
                    );
                }
                other => notes.unknown(format!("element:group.{other}")),
            }
        }
        if let Some(e) = g.child("effects") {
            inst.extra
                .insert(format!("dspreset.group{gi}.effects"), e.to_xml());
        }
        for s in g.children_named("sample") {
            check_attrs(s, "sample", &mut notes);
            let lv = Levels {
                sample: s,
                group: g,
                groups: groups_el,
            };
            let Some(path) = s.attr("path") else {
                notes.skip("sample:path", "samples without a path were skipped");
                continue;
            };
            let Some((file_path, fixed)) = dir.and_then(|d| resolve_rel_case(d, path)) else {
                notes.skip(
                    "sample:missing",
                    format!("{path}: not found beside the preset"),
                );
                continue;
            };
            if fixed {
                notes.approx(
                    "sample:path-case",
                    "sample paths whose case differs from the files were matched ignoring case",
                );
            }
            let info = match probes
                .entry(file_path.clone())
                .or_insert_with(|| probe(&file_path))
            {
                Ok(i) => *i,
                Err(e) => {
                    notes.skip("sample:unreadable", format!("{path}: {e}"));
                    continue;
                }
            };
            // tags (§5.5)
            let tags: Vec<String> = lv
                .get("tags")
                .map(|t| {
                    t.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if tags.iter().any(|t| tag_info.get(t).is_some_and(|i| !i.0)) {
                notes.skip("tag:enabled", "samples of disabled tags were skipped");
                continue;
            }

            let mut d = GroupDraft::new(&gname);
            // ---- mapping
            let lo = lv.get("loNote").and_then(key).unwrap_or(0);
            let hi = lv.get("hiNote").and_then(key).unwrap_or(127);
            let root_key = match lv.get("rootNote").and_then(key) {
                Some(k) => k,
                None => {
                    notes.approx(
                        "sample:rootNote",
                        "samples without a rootNote play at key 60",
                    );
                    60
                }
            };
            let mut z = Zone::new(
                GroupId(0),
                SampleRef::File(String::new()),
                KeyRange {
                    lo: lo.min(hi),
                    hi: hi.max(lo),
                },
                root_key,
            );
            let vlo = lv.num("loVel").unwrap_or(0.0).clamp(0.0, 127.0) as u8;
            let vhi = lv.num("hiVel").unwrap_or(127.0).clamp(0.0, 127.0) as u8;
            z.velocity = VelRange {
                lo: vlo.min(vhi),
                hi: vhi.max(vlo),
            };
            // volume: the three levels multiply (§3.1) — dB add
            let level_vol = |e: &Element, alias: &str| {
                e.attr(alias)
                    .or_else(|| e.attr("volume"))
                    .and_then(volume_db)
                    .unwrap_or(0.0)
            };
            let mut vol = level_vol(groups_el, "globalVolume")
                + level_vol(g, "groupVolume")
                + s.attr("volume").and_then(volume_db).unwrap_or(0.0)
                + ggain;
            for t in &tags {
                if let Some((_, tv, _)) = tag_info.get(t) {
                    vol += tv;
                    if *tv != 0.0 {
                        notes.approx(
                            "tag:volume",
                            "tag volumes are applied to their samples' levels (not a control yet)",
                        );
                    }
                }
            }
            // pan: inherits, plus the group and global offsets (§3.1)
            let pan = lv.num("pan").unwrap_or(0.0)
                + g.attr("groupPan").and_then(num).unwrap_or(0.0)
                + groups_el.attr("globalPan").and_then(num).unwrap_or(0.0);
            z.pan = (pan.clamp(-100.0, 100.0) / 100.0) as f32;
            // pitch (§3.7)
            let tuning = lv.num("tuning").unwrap_or(0.0)
                + g.attr("groupTuning").and_then(num).unwrap_or(0.0)
                + groups_el.attr("globalTuning").and_then(num).unwrap_or(0.0);
            let track = lv.num("pitchKeyTrack").unwrap_or(1.0).clamp(0.0, 1.0);
            let s100 = track * 100.0;
            let c0 = tuning * 100.0;
            let (root2, cents) = if (s100 - 100.0).abs() < 1e-9 {
                (root_key as i32, c0)
            } else {
                // SPEC-GAP: dspreset.md §3.7 / §7 #4: tunings are not scaled by key tracking
                // (read as tuningIgnoresKeyTrack in every case).
                if tuning != 0.0
                    && groups_el.attr("tuningIgnoresKeyTrack").and_then(boolean) != Some(true)
                {
                    notes.approx(
                        "sample:pitchKeyTrack",
                        "tunings are applied in full even with reduced key tracking",
                    );
                }
                d.route(
                    ModSource::Key,
                    ModTarget::Pitch,
                    (s100 - 100.0) * 64.0,
                    ModCurve::Linear,
                );
                let r = (((root_key as f64 * s100 - (s100 - 100.0) * 60.0) / 100.0).round() as i32)
                    .clamp(0, 127);
                (
                    r,
                    r as f64 * 100.0 + (s100 - 100.0) * 60.0 - root_key as f64 * s100 + c0,
                )
            };
            z.root_key = root2 as u8;
            let (tune, fine, clamped) = crate::assemble::split_cents(cents);
            if clamped {
                notes.approx("sample:tuning", "tunings beyond ±48 semitones were cut");
            }
            z.tune = tune;
            z.fine = fine;
            // play range (§3.5: end is the last frame played)
            let len = info.frames.max(1);
            let start = (lv.num("start").unwrap_or(0.0).max(0.0) as u64).min(len - 1);
            let end = lv
                .num("end")
                .map(|e| (e.max(0.0) as u64 + 1).clamp(start + 1, len))
                .unwrap_or(len);
            z.start = start;
            z.end = (end < len).then_some(end);
            // triggers (§3.5)
            match lv.get("trigger").unwrap_or("attack") {
                "release" => z.trigger = ZoneTrigger::Release,
                "attack" | "continuous" => {}
                t @ ("first" | "legato") => notes.approx(
                    format!("trigger:{t}"),
                    "played on every note-on (the sampler has no first/legato triggers yet)",
                ),
                t => notes.unknown(format!("trigger:{t}")),
            }
            if let Some(r) = lv.get("releaseTriggerDecay") {
                let lower = r.trim().to_ascii_lowercase();
                let db = if let Some(n) = lower.strip_suffix("db") {
                    num(n).map(f64::abs)
                } else {
                    notes.approx(
                        "sample:releaseTriggerDecay",
                        "a linear release decay is read as a gain factor per second",
                    );
                    num(&lower)
                        .filter(|x| *x > 0.0 && *x < 1.0)
                        .map(|x| -20.0 * x.log10())
                };
                if let Some(db) = db {
                    z.release_decay_db = db.clamp(0.0, 200.0) as f32;
                }
            }
            for attr in ["previousNotes", "legatoInterval"] {
                if lv.get(attr).is_some() {
                    notes.skip(format!("sample:{attr}"), "the sampler has no conditions on the previous note yet; the sample plays without it");
                }
            }
            // controller conditions: loCCN/hiCCN; onLoCCN/onHiCCN (CC-triggered) skipped
            let mut cc_ns = BTreeSet::new();
            for e in [groups_el, *g, s] {
                for (k, _) in &e.attrs {
                    for p in ["loCC", "hiCC"] {
                        if let Some(n) = k
                            .strip_prefix(p)
                            .and_then(|n| n.parse::<u8>().ok())
                            .filter(|n| *n <= 127)
                        {
                            cc_ns.insert(n);
                        }
                    }
                    if k.starts_with("onLoCC") || k.starts_with("onHiCC") {
                        notes.skip(
                            "sample:onLoCC",
                            "controller-triggered samples are not supported yet",
                        );
                    }
                }
            }
            for n in cc_ns {
                let lo = lv.num(&format!("loCC{n}")).unwrap_or(-1.0);
                let hi = lv.num(&format!("hiCC{n}")).unwrap_or(-1.0);
                if lo < 0.0 || hi < 0.0 {
                    continue; // both must be set (§3.5)
                }
                let (a, b) = (lo.min(hi).min(127.0) / 127.0, hi.max(lo).min(127.0) / 127.0);
                z.cc_conditions.push(CcCondition {
                    controller: n,
                    lo: a as f32,
                    hi: b as f32,
                });
            }
            // round robin (§3.5)
            match lv.get("seqMode").unwrap_or("always") {
                "round_robin" => {}
                "random" => d.group.round_robin = RoundRobinMode::Random,
                "true_random" => {
                    d.group.round_robin = RoundRobinMode::Random;
                    notes.approx(
                        "sample:seqMode",
                        "true_random plays as random (never the same position twice in a row)",
                    );
                }
                "always" => {}
                m => notes.unknown(format!("seqMode:{m}")),
            }
            if lv.get("seqMode").is_some_and(|m| m != "always") {
                let pos = lv.num("seqPosition").unwrap_or(1.0).max(1.0) as u32;
                z.round_robin = Some(pos - 1);
                if let Some(l) = lv.num("seqLength").filter(|l| *l > 0.0) {
                    d.group
                        .extra
                        .insert("dspreset.seqLength".into(), (l as u32).to_string());
                }
            }
            // loops (§3.5)
            let has_loop_attr = lv.get("loopStart").is_some() || lv.get("loopEnd").is_some();
            let loop_on = lv
                .bool("loopEnabled")
                .unwrap_or(info.loop_range.is_some() || has_loop_attr);
            if loop_on {
                let (fs, fe) = info.loop_range.unwrap_or((0, len));
                let ls = lv.num("loopStart").map(|v| v.max(0.0) as u64).unwrap_or(fs);
                let le = lv
                    .num("loopEnd")
                    .map(|v| v.max(0.0) as u64 + 1)
                    .unwrap_or(fe);
                let (ls, le) = (ls.clamp(start, end), le.clamp(start, end));
                if le > ls {
                    let xf = (lv.num("loopCrossfade").unwrap_or(0.0).max(0.0) as u64).min(le - ls);
                    if xf > 0 && lv.get("loopCrossfadeMode") == Some("linear") {
                        notes.approx(
                            "sample:loopCrossfadeMode",
                            "linear loop crossfades play with the sampler's crossfade curve",
                        );
                    }
                    z.looping = Some(ZoneLoop {
                        mode: LoopMode::Forward,
                        start: ls,
                        end: le,
                        crossfade: xf,
                        on_release: if lv.bool("loopDuringRelease") == Some(false) {
                            LoopRelease::Exit
                        } else {
                            LoopRelease::Continue
                        },
                    });
                } else {
                    notes.approx("sample:loop", "empty loops play unlooped");
                }
            }
            // envelope (§3.5); ampEnvEnabled=false = one-shot
            if lv.bool("ampEnvEnabled") == Some(false) {
                z.one_shot = true;
            }
            let t = |names: &[&str], d: f64| clampf(lv.first(names).unwrap_or(d), 0.0, 60.0) as f32;
            let mut delay = t(&["ampEnvDelay"], 0.0) as f64;
            if let Some(dl) = lv.num("delay").filter(|x| *x > 0.0) {
                let secs = match lv.get("delayUnit").unwrap_or("seconds") {
                    "beats" => {
                        notes.approx("sample:delay", "delays in beats are read at 120 BPM");
                        dl * 60.0 / BEATS_BPM
                    }
                    "samples" => dl / info.sample_rate.max(1) as f64,
                    _ => dl,
                };
                notes.approx(
                    "sample:delay",
                    "the start delay is played as the envelope's delay",
                );
                delay += secs;
            }
            d.group.amp_env = Envelope {
                delay: clampf(delay, 0.0, 60.0) as f32,
                attack: t(&["ampEnvAttack", "attack"], 0.0),
                hold: t(&["ampEnvHold"], 0.0),
                decay: t(&["ampEnvDecay", "decay"], 0.0),
                sustain: clampf(
                    lv.first(&["ampEnvSustain", "sustain"]).unwrap_or(1.0),
                    0.0,
                    1.0,
                ) as f32,
                release: t(&["ampEnvRelease", "release"], 0.1).max(0.001),
            };
            for (c, def) in [
                ("attackCurve", -100.0),
                ("decayCurve", 100.0),
                ("releaseCurve", 100.0),
            ] {
                if lv.num(c).is_some_and(|v| v != def) {
                    notes.approx(
                        format!("sample:{c}"),
                        "envelope curves play with the sampler's own envelope shape",
                    );
                }
            }
            if lv.num("ampVelTrack").is_some_and(|v| v != 1.0) {
                notes.approx(
                    "sample:ampVelTrack",
                    "velocity tracking other than 1 plays with the sampler's own velocity response",
                );
            }
            for a in ["retriggerEnabled", "beats", "slices", "originalTempo"] {
                if lv.get(a).is_some() {
                    notes.skip(format!("sample:{a}"), "retrigger and slice-stretch playback are not supported; the sample plays normally");
                }
            }
            if lv.get("sampleMode") == Some("slice_stretch") {
                notes.skip("sample:sampleMode", "slice_stretch samples play normally");
            }
            // silencing (§3.5): a sample silenced by tag T shares a one-voice group with T's
            if let Some(by) = lv.get("silencedByTags") {
                let set: BTreeSet<String> = by
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect();
                d.group.extra.insert(
                    "dspreset.silenced_by".into(),
                    set.iter().cloned().collect::<Vec<_>>().join(","),
                );
                choke_sets.push(set);
                if lv.get("silencingMode").is_some() || lv.get("silencingDecay").is_some() {
                    notes.approx(
                        "sample:silencingMode",
                        "silenced voices fade with the sampler's voice-stealing fade",
                    );
                }
            }
            d.group.extra.insert("dspreset.tags".into(), tags.join(","));
            for t in &tags {
                if let Some((_, _, Some(p))) = tag_info.get(t) {
                    d.group.voice_limit = Some(d.group.voice_limit.map_or(*p, |v| v.min(*p)));
                }
            }
            // output (§3.5)
            let mut fx = gfx.clone();
            match lv.get("output1Target").unwrap_or("MAIN_OUTPUT") {
                "MAIN_OUTPUT" => {}
                "NO_OUTPUT" => vol = -144.0,
                t if t.starts_with("AUX_STEREO_OUTPUT_") => {
                    let n: u32 = t["AUX_STEREO_OUTPUT_".len()..].parse().unwrap_or(0);
                    d.group.output = n.min(MAX_OUTPUT_BUSES - 1);
                }
                t if t.starts_with("BUS_") => {
                    let n: usize = t[4..].parse().unwrap_or(0);
                    match n.checked_sub(1).and_then(|i| buses.get(i)) {
                        Some((bfx, bg)) => {
                            fx.extend(bfx.iter().cloned());
                            vol += bg;
                            notes.approx("bus", "a bus's effects run in the insert chain of each group that feeds it");
                        }
                        None => notes.skip(
                            "bus:missing",
                            format!("{t} is not defined; played on the main output"),
                        ),
                    }
                }
                t => notes.unknown(format!("output:{t}")),
            }
            if let Some(v) = lv.get("output1Volume").and_then(num).filter(|v| *v != 1.0) {
                vol += if v <= 0.0 { -144.0 } else { 20.0 * v.log10() };
            }
            for n in 2..=8 {
                if lv
                    .get(&format!("output{n}Target"))
                    .is_some_and(|t| t != "NO_OUTPUT")
                {
                    notes.skip(
                        "output:extra",
                        "outputs 2–8 (parallel sends) are not supported; output 1 plays",
                    );
                }
            }
            z.volume_db = clampf(vol, -144.0, 24.0) as f32;
            fx.extend(inst_fx.iter().cloned());
            d.group.effects = fx;
            // group-wide settings
            if let Some(p) = g.attr("polyphony").and_then(num).filter(|p| *p >= 1.0) {
                d.group.voice_limit =
                    Some(d.group.voice_limit.map_or(p as u32, |v| v.min(p as u32)));
            }
            let glide = g
                .attr("glideTime")
                .or_else(|| groups_el.attr("glideTime"))
                .and_then(num)
                .unwrap_or(0.0);
            if glide > 0.0 {
                d.group.portamento = clampf(glide, 0.0, 10.0) as f32;
                let mode = g
                    .attr("glideMode")
                    .or_else(|| groups_el.attr("glideMode"))
                    .unwrap_or("legato");
                d.group.polyphony = match mode {
                    "always" => Polyphony::Mono,
                    "off" => Polyphony::Poly,
                    _ => Polyphony::Legato,
                };
                notes.approx("group:glide", "glide makes the group monophonic (the sampler glides in mono and legato groups)");
            }
            d.group.lfos = group_lfos[gi].clone();
            if let Some(e) = &group_env[gi] {
                d.group.filter_env = Some(e.clone());
            }
            for (src, tg, depth) in &group_routes[gi] {
                d.route(src.clone(), *tg, *depth, ModCurve::Linear);
            }
            d.group.articulations = keyswitches
                .iter()
                .enumerate()
                .filter(|(_, k)| k.enabled.contains(&gi))
                .map(|(i, _)| ArticulationId(i as u32))
                .collect();
            // the sample, by its path relative to the preset's folder
            zone_files.push(file_path.clone());
            let gid = asm.group(&d);
            asm.zone(z, gid);
        }
    }

    asm.finish(&mut inst);
    // choke (§3.5 [PH]): every zone of a silencing set (the silenced samples and the samples
    // tagged with the silencing tags) moves into one one-voice group
    if !choke_sets.is_empty() {
        notes.approx(
            "sample:silencedByTags",
            "a sample and the tags that silence it share one one-voice group (they silence each other)",
        );
        choke(&mut inst, &choke_sets);
    }
    // names relative to the folder the samples share
    let mut root_dir: Option<PathBuf> = None;
    for p in &zone_files {
        let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
        root_dir = Some(match root_dir {
            None => parent,
            Some(r) => common(&r, &parent),
        });
    }
    let root_dir = root_dir.unwrap_or_default();
    for (zid, p) in zone_files.iter().enumerate() {
        let rel = p
            .strip_prefix(&root_dir)
            .map(|x| x.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        if let Some(z) = inst.zones.get_mut(&ZoneId(zid as u32)) {
            z.sample = SampleRef::File(rel.clone());
        }
        if !samples.iter().any(|(n, _)| *n == rel) {
            samples.push((rel, p.clone()));
        }
    }
    let (notes, unknown) = notes.finish();
    inst.import_notes = notes.clone();
    inst.source = Some(InstrumentSource {
        format: "dspreset".into(),
        file: file.to_string(),
        importer_version: DS_IMPORTER_VERSION,
    });
    let report = ImportReport {
        format: "dspreset".into(),
        file: file.to_string(),
        groups: inst.groups.len(),
        zones: inst.zones.len(),
        notes,
        unknown,
        presets: Vec::new(),
    };
    Ok(Imported {
        instrument: inst,
        samples: samples
            .into_iter()
            .map(|(name, p)| ImportedSample {
                name,
                source: SampleSource::File(p),
            })
            .collect(),
        report,
    })
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

/// Moves the zones of each silencing set (the samples silenced by its tags and the samples
/// carrying them) into one shared one-voice group.
fn choke(inst: &mut HausInstrument, sets: &[BTreeSet<String>]) {
    let hits = |g: &Group, key: &str, set: &BTreeSet<String>| {
        g.extra
            .get(key)
            .is_some_and(|t| t.split(',').any(|x| set.contains(x.trim())))
    };
    let mut next_id = inst.groups.keys().map(|g| g.0 + 1).max().unwrap_or(0);
    let mut done: Vec<BTreeSet<String>> = Vec::new();
    for set in sets {
        if done.contains(set) {
            continue;
        }
        done.push(set.clone());
        let members: Vec<GroupId> = inst
            .groups
            .values()
            .filter(|g| g.voice_limit != Some(1) || !g.extra.contains_key("dspreset.choke"))
            .filter(|g| hits(g, "dspreset.tags", set) || hits(g, "dspreset.silenced_by", set))
            .map(|g| g.id)
            .collect();
        if members.len() < 2
            && members
                .iter()
                .all(|g| !hits(&inst.groups[g], "dspreset.tags", set))
        {
            continue;
        }
        let first = inst.groups[&members[0]].clone();
        let mut g = first.clone();
        g.id = GroupId(next_id);
        next_id += 1;
        g.voice_limit = Some(1);
        g.extra.insert(
            "dspreset.choke".into(),
            set.iter().cloned().collect::<Vec<_>>().join(","),
        );
        let routes: Vec<ModRoute> = inst
            .modulation
            .iter()
            .filter(|r| r.group == Some(first.id))
            .map(|r| ModRoute {
                group: Some(g.id),
                ..r.clone()
            })
            .collect();
        inst.modulation.extend(routes);
        inst.groups.insert(g.id, g.clone());
        for z in inst.zones.values_mut() {
            if members.contains(&z.group) {
                z.group = g.id;
            }
        }
        for m in members {
            inst.groups.remove(&m);
            inst.modulation.retain(|r| r.group != Some(m));
        }
    }
}
