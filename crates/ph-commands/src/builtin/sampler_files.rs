//! `sampler.load`, `sampler.save_native`, `sampler.collect` and `sampler.import` (M3/T3):
//! the commands that move Haus Sampler instruments between files and the project
//! (`docs/formats/haus-instrument.md` §2).
//!
//! - The zone map always lives in the project; samples are **linked** (the instrument's
//!   `folder` is the library folder, read in place) or **collected** (copied into
//!   `<project>/instruments/<name>/`, `folder` = `instruments/<name>`).
//! - Library folders are never written: linking reads them, collecting and saving copy
//!   *from* them, and `save_native` refuses to write over the folder an instrument links.
//! - Model changes are one undoable edit each. Files copied into the project stay when the
//!   edit is undone (harmless, like `sample.import`).
//! - SF2/SF3 instruments have no sample files: their samples (cut out of the SoundFont,
//!   decoded and stereo-joined by the importer, `SampleSource::Audio`) are written as float
//!   WAV files into `instruments/<name>/samples/`, so those imports are always collected
//!   (M3/T4).

use super::sampler::{Import, InstrumentOnly, Load, SaveNative};
use super::util::*;
use crate::*;
use ph_core::*;
use ph_formats::instrument::{
    INSTRUMENT_FILE, INSTRUMENT_SAMPLES_DIR, load_instrument, save_instrument,
};
use ph_sampler_formats::{
    ImportError, ImportInput, Imported, PresetChoice, SampleSource, audio_wav, importer_for,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Project folder that collected instruments live in.
pub const INSTRUMENTS_DIR: &str = "instruments";

fn host_err(path: &Path, e: impl std::fmt::Display) -> CmdError {
    CmdError::Host(format!("{}: {e}", path.display()))
}

/// A path to read: the human may name any absolute path (the drop is the intent); agents
/// and relative paths go through the host's confinement (DESIGN §5).
fn read_path(ctx: &CommandCtx<'_>, path: &str) -> CmdResult<PathBuf> {
    if *ctx.author == Author::Human && Path::new(path).is_absolute() {
        return Ok(path.into());
    }
    ctx.host.resolve_path(path, FileAccess::Read)
}

/// The file a zone's sample plays, on disk.
fn sample_file(
    ctx: &CommandCtx<'_>,
    inst: &HausInstrument,
    sample: &SampleRef,
) -> CmdResult<PathBuf> {
    let reference = inst.sample_reference(ctx.project, sample).ok_or_else(|| {
        CmdError::Model(ModelError::Invalid(format!(
            "{} plays a sample that is not in the project",
            inst.name
        )))
    })?;
    read_path(ctx, &reference)
}

/// A folder name safe on every file system.
fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "Instrument".into() } else { s }
}

/// `instruments/<name>`, numbered ` (2)`, ` (3)` when the folder exists or another
/// instrument of the project already uses it. Returns (project-relative, absolute).
fn new_instrument_dir(ctx: &CommandCtx<'_>, name: &str) -> CmdResult<(String, PathBuf)> {
    let base = safe_name(name);
    for n in 1.. {
        let rel = if n == 1 {
            format!("{INSTRUMENTS_DIR}/{base}")
        } else {
            format!("{INSTRUMENTS_DIR}/{base} ({n})")
        };
        let used = ctx
            .project
            .instruments
            .values()
            .any(|i| i.folder.as_deref() == Some(rel.as_str()));
        let abs = ctx.host.resolve_path(&rel, FileAccess::Write)?;
        if !used && !abs.exists() {
            return Ok((rel, abs));
        }
    }
    unreachable!("an unbounded range always yields a free name")
}

/// Copies files into `dir`, by the relative names given; identical bytes already there are
/// reused, a different file under the same name gets ` (2)`, ` (3)`. Returns the final
/// relative name of each source.
struct Copier {
    dir: PathBuf,
    /// source -> name
    done: BTreeMap<PathBuf, String>,
    /// names taken in this run
    taken: BTreeMap<String, PathBuf>,
    copied: usize,
}

impl Copier {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            done: BTreeMap::new(),
            taken: BTreeMap::new(),
            copied: 0,
        }
    }

    fn numbered(name: &str, n: u32) -> String {
        if n == 1 {
            return name.to_string();
        }
        let p = Path::new(name);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let file = match p.extension() {
            Some(e) => format!("{stem} ({n}).{}", e.to_string_lossy()),
            None => format!("{stem} ({n})"),
        };
        match name.rfind('/') {
            Some(i) => format!("{}/{file}", &name[..i]),
            None => file,
        }
    }

    /// Places `bytes` (from `source`, a key for reuse) under `name`.
    fn place(
        &mut self,
        source: &Path,
        name: &str,
        bytes: impl Fn() -> CmdResult<Vec<u8>>,
    ) -> CmdResult<String> {
        if let Some(n) = self.done.get(source) {
            return Ok(n.clone());
        }
        let data = bytes()?;
        for n in 1.. {
            let cand = Self::numbered(name, n);
            if self.taken.contains_key(&cand) {
                continue;
            }
            let target = join_rel(&self.dir, &cand);
            if target.exists() {
                let same = std::fs::read(&target).map(|b| b == data).unwrap_or(false);
                if !same {
                    continue;
                }
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| host_err(parent, e))?;
                }
                std::fs::write(&target, &data).map_err(|e| host_err(&target, e))?;
                self.copied += 1;
            }
            self.taken.insert(cand.clone(), source.to_path_buf());
            self.done.insert(source.to_path_buf(), cand.clone());
            return Ok(cand);
        }
        unreachable!()
    }

    fn copy(&mut self, source: &Path, name: &str) -> CmdResult<String> {
        self.place(source, name, || {
            std::fs::read(source).map_err(|e| host_err(source, e))
        })
    }
}

/// `dir` joined with a `/`-separated relative name one component at a time, so the result
/// has one kind of separator (under a `\?\` folder, `/` is no separator at all).
fn join_rel(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|c| !c.is_empty())
        .fold(dir.to_path_buf(), |p, c| p.join(c))
}

fn file_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

fn wav_bytes(g: ph_sampler_formats::Generated) -> CmdResult<Vec<u8>> {
    ph_formats::encode::encode_wav(
        &[g.render()],
        ph_sampler_formats::Generated::RATE,
        ph_formats::encode::WavFormat::Float32,
        ph_formats::encode::DITHER_SEED,
    )
    .map_err(|e| CmdError::Host(e.to_string()))
}

/// Adds `inst` to the project and puts it on a channel (new, or `channel` replaced).
fn place_on_channel(
    ctx: &CommandCtx<'_>,
    inst: HausInstrument,
    channel: Option<ChannelId>,
) -> CmdResult<(Edit, InstrumentId, ChannelId)> {
    let name = inst.name.clone();
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_instrument(inst)?;
    let spec = InstrumentSpec::MultiSampler {
        library: None,
        instrument: Some(id),
        params: Default::default(),
    };
    let ch = match channel {
        Some(c) => {
            ed.update_channel(c, |ch| ch.instrument = spec)?;
            c
        }
        None => ed.add_channel(&name, spec)?,
    };
    Ok((ed, id, ch))
}

// ------------------------------------------------------------------------- load

pub(crate) fn load(ctx: &mut CommandCtx<'_>, p: Load) -> CmdResult<CommandOutput> {
    let mut dir = read_path(ctx, &p.path)?;
    let mut rel = p.path.replace('\\', "/");
    if dir.file_name().is_some_and(|n| n == INSTRUMENT_FILE) {
        dir.pop();
        rel = rel
            .trim_end_matches(INSTRUMENT_FILE)
            .trim_end_matches('/')
            .to_string();
    }
    let mut inst = load_instrument(&dir).map_err(|e| CmdError::Host(e.to_string()))?;
    let mut copied = 0;
    if p.collect {
        let (folder, abs) = new_instrument_dir(ctx, &inst.name)?;
        let mut c = Copier::new(abs);
        let zones: Vec<ZoneId> = inst.zones.keys().copied().collect();
        for z in zones {
            if let SampleRef::File(path) = &inst.zones[&z].sample {
                let src = dir.join(path);
                let name = c.copy(&src, path)?;
                inst.zones.get_mut(&z).expect("listed").sample = SampleRef::File(name);
            }
        }
        copied = c.copied;
        inst.folder = Some(folder);
    } else {
        inst.folder = Some(if Path::new(&rel).is_absolute() {
            dir.to_string_lossy().replace('\\', "/")
        } else {
            rel
        });
    }
    let (zones, groups, name) = (inst.zones.len(), inst.groups.len(), inst.name.clone());
    let folder = inst.folder.clone();
    let (ed, id, ch) = place_on_channel(ctx, inst, p.channel)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "instrument": id, "channel": ch, "zones": zones, "groups": groups,
                "folder": folder, "collected": p.collect, "copied": copied }),
        format!(
            "loaded {name} ({}){}",
            plural(zones, "zone", "zones"),
            if p.collect {
                ", samples collected into the project"
            } else {
                ""
            }
        ),
    ))
}

// ------------------------------------------------------------------------- collect

pub(crate) fn collect(ctx: &mut CommandCtx<'_>, p: InstrumentOnly) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?.clone();
    let files: Vec<&Zone> = inst
        .zones
        .values()
        .filter(|z| matches!(z.sample, SampleRef::File(_)))
        .collect();
    let already = inst
        .folder
        .as_deref()
        .is_some_and(|f| f.starts_with(&format!("{INSTRUMENTS_DIR}/")));
    if already || files.is_empty() {
        return Ok(CommandOutput::value(json!({
            "instrument": p.instrument, "copied": 0, "folder": inst.folder,
            "note": "nothing to collect: the instrument already lives in the project",
        })));
    }
    let (folder, abs) = new_instrument_dir(ctx, &inst.name)?;
    let mut c = Copier::new(abs);
    let mut renamed: Vec<(ZoneId, String)> = Vec::new();
    for z in files {
        let SampleRef::File(path) = &z.sample else {
            continue;
        };
        let src = sample_file(ctx, &inst, &z.sample)?;
        let rel = if path.starts_with(&format!("{INSTRUMENT_SAMPLES_DIR}/")) {
            path.clone()
        } else {
            format!("{INSTRUMENT_SAMPLES_DIR}/{}", file_name(path))
        };
        let name = c.copy(&src, &rel)?;
        renamed.push((z.id, name));
    }
    let mut ed = Edit::new(ctx.project);
    for (z, name) in &renamed {
        ed.update_zone(p.instrument, *z, |zone| {
            zone.sample = SampleRef::File(name.clone())
        })?;
    }
    let f = folder.clone();
    ed.update_instrument(p.instrument, |i| i.folder = Some(f))?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "instrument": p.instrument, "copied": c.copied, "folder": folder }),
        format!(
            "collected {} of {} into the project",
            plural(c.copied, "sample", "samples"),
            inst.name
        ),
    ))
}

// ------------------------------------------------------------------------- save_native

pub(crate) fn save_native(ctx: &mut CommandCtx<'_>, p: SaveNative) -> CmdResult<CommandOutput> {
    let inst = ctx.project.instrument(p.instrument)?.clone();
    let target = ctx.host.resolve_path(&p.path, FileAccess::Write)?;
    if let Some(f) = inst.folder.as_deref() {
        let linked = read_path(ctx, f).ok();
        let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        };
        if !f.starts_with(&format!("{INSTRUMENTS_DIR}/"))
            && linked.as_deref().is_some_and(|l| same(l, &target))
        {
            return invalid(format!(
                "{} is the library folder {} plays from; library folders are never written. Save it somewhere else.",
                p.path, inst.name
            ));
        }
    }
    let mut out = inst.clone();
    out.id = InstrumentId(0);
    out.folder = None;
    let mut c = Copier::new(target.clone());
    for z in out.zones.values_mut() {
        let (src, rel) = match &z.sample {
            SampleRef::File(path) => (
                sample_file(ctx, &inst, &z.sample)?,
                if path.starts_with(&format!("{INSTRUMENT_SAMPLES_DIR}/")) {
                    path.clone()
                } else {
                    format!("{INSTRUMENT_SAMPLES_DIR}/{}", file_name(path))
                },
            ),
            SampleRef::Registry(s) => {
                let e = ctx
                    .project
                    .samples
                    .get(s)
                    .ok_or(CmdError::Model(ModelError::NoSample(*s)))?;
                (
                    read_path(ctx, &e.path)?,
                    format!("{INSTRUMENT_SAMPLES_DIR}/{}", file_name(&e.path)),
                )
            }
        };
        let same_file = join_rel(&target, &rel) == src;
        let name = if same_file { rel } else { c.copy(&src, &rel)? };
        z.sample = SampleRef::File(name);
    }
    let file = save_instrument(&out, &target).map_err(|e| CmdError::Host(e.to_string()))?;
    Ok(CommandOutput::value(json!({
        "instrument": p.instrument,
        "path": file.to_string_lossy(),
        "samples": c.copied,
        "zones": out.zones.len(),
    })))
}

// ------------------------------------------------------------------------- import

fn unavailable(feature: &str, track: &str) -> CmdError {
    CmdError::Unavailable {
        command: "sampler.import".into(),
        feature: feature.into(),
        track: track.into(),
    }
}

fn report_json(imported: &Imported) -> Value {
    let r = &imported.report;
    json!({
        "format": r.format,
        "file": r.file,
        "groups": r.groups,
        "zones": r.zones,
        "notes": r.notes,
        "unknown": r.unknown.iter().map(|(w, n)| json!({ "what": w, "count": n })).collect::<Vec<_>>(),
        "presets": r.presets.iter().map(|p| json!({
            "bank": p.bank, "program": p.program, "name": p.name, "imported": p.imported,
        })).collect::<Vec<_>>(),
    })
}

/// The folder every imported file sample lives under, when the samples can be linked in
/// place: each source path ends with its zone name.
fn link_folder(imported: &Imported) -> Option<PathBuf> {
    let mut folder: Option<PathBuf> = None;
    for s in &imported.samples {
        let SampleSource::File(src) = &s.source else {
            return None;
        };
        let mut f = src.clone();
        for _ in s.name.split('/') {
            f.pop();
        }
        if f.join(&s.name) != *src && f.join(s.name.replace('/', "\\")) != *src {
            return None;
        }
        match &folder {
            None => folder = Some(f),
            Some(x) if *x == f => {}
            Some(_) => return None,
        }
    }
    folder
}

pub(crate) fn import(ctx: &mut CommandCtx<'_>, p: Import) -> CmdResult<CommandOutput> {
    // a folder is a folder of samples, unless it is an instrument container (`.dsbundle`)
    let path_is_dir = read_path(ctx, &p.path).is_ok_and(|d| d.is_dir())
        && importer_for(Path::new(&p.path)).is_none();
    if p.preset.is_some() && path_is_dir {
        return invalid("preset applies to SF2/SF3 files only, not to a folder of samples");
    }
    let imported = if path_is_dir {
        let dir = read_path(ctx, &p.path)?;
        ph_sampler_formats::automap::map_folder(&dir).map_err(|e| CmdError::Host(e.to_string()))?
    } else {
        let ext = Path::new(&p.path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let Some(importer) = importer_for(Path::new(&p.path)) else {
            return invalid(format!(
                "{} is not an instrument format the sampler reads ({}), nor a folder of samples",
                p.path,
                ph_sampler_formats::extensions().join(", ")
            ));
        };
        // formats whose importer is a later track answer before any file is touched
        if let Err(ImportError::Unavailable { track, .. }) = importer.import(ImportInput::Bytes {
            name: &p.path,
            bytes: &[],
        }) {
            return Err(unavailable(&format!(".{ext} import"), &track));
        }
        let file = read_path(ctx, &p.path)?;
        let input = ImportInput::Path(&file);
        let result = match &p.preset {
            Some(choice) if importer.format() == "sf2" => {
                ph_sampler_formats::sf2::import_preset(input, &PresetChoice::parse(choice))
            }
            Some(_) => {
                return invalid(format!(
                    "preset applies to SF2/SF3 files only; {} has one instrument",
                    p.path
                ));
            }
            None => importer.import(input),
        };
        result.map_err(|e| CmdError::Host(e.to_string()))?
    };

    // agents read samples only from inside their allowed roots
    if *ctx.author != Author::Human {
        for s in &imported.samples {
            if let SampleSource::File(src) = &s.source {
                ctx.host
                    .resolve_path(&src.to_string_lossy(), FileAccess::Read)?;
            }
        }
    }
    let mut inst = imported.instrument.clone();
    let link = if p.collect {
        None
    } else {
        link_folder(&imported)
    };
    let mut copied = 0;
    let collected = link.is_none();
    match link {
        Some(f) => inst.folder = Some(f.to_string_lossy().replace('\\', "/")),
        None => {
            let (folder, abs) = new_instrument_dir(ctx, &inst.name)?;
            let mut c = Copier::new(abs.clone());
            let mut names = BTreeMap::new();
            for s in &imported.samples {
                let name = match &s.source {
                    SampleSource::File(src) => c.copy(src, &s.name)?,
                    SampleSource::Generated(g) => {
                        let g = *g;
                        c.place(Path::new(g.name()), &s.name, || wav_bytes(g))?
                    }
                    SampleSource::Audio(audio) => {
                        let key = PathBuf::from(format!("audio:{}", s.name));
                        c.place(&key, &s.name, || audio_wav(audio).map_err(CmdError::Host))?
                    }
                    SampleSource::Chunk { .. } => {
                        return Err(unavailable("samples inside containers", "M3/T6"));
                    }
                };
                names.insert(s.name.clone(), name);
            }
            for z in inst.zones.values_mut() {
                if let SampleRef::File(n) = &z.sample
                    && let Some(new) = names.get(n)
                {
                    z.sample = SampleRef::File(new.clone());
                }
            }
            let md = abs.join("import-report.md");
            // an import whose samples were all missing (reported) copies nothing, so the
            // instrument folder may not exist yet
            std::fs::create_dir_all(&abs).map_err(|e| host_err(&abs, e))?;
            std::fs::write(&md, imported.report.to_markdown()).map_err(|e| host_err(&md, e))?;
            copied = c.copied;
            inst.folder = Some(folder);
        }
    }
    let (zones, name, folder) = (inst.zones.len(), inst.name.clone(), inst.folder.clone());
    let notes = imported.report.notes.len() + imported.report.unknown.len();
    let (ed, id, ch) = place_on_channel(ctx, inst, p.channel)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({
            "instrument": id, "channel": ch, "zones": zones, "folder": folder,
            "collected": collected, "copied": copied, "report": report_json(&imported),
        }),
        format!(
            "imported {name} ({}{})",
            plural(zones, "zone", "zones"),
            if notes > 0 {
                format!("; {} in the import report", plural(notes, "note", "notes"))
            } else {
                String::new()
            }
        ),
    ))
}
