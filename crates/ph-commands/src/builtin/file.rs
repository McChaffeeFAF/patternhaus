//! `file.*`: project folders, audio export and MIDI. Paths go through [`Host::resolve_path`],
//! which enforces where the caller may read and write (DESIGN §5); relative paths are
//! relative to the project folder, and `.` is the project folder itself.

use super::util::*;
use crate::session_cmd::{SessionOut, parse};
use crate::*;
use ph_core::*;
use ph_formats::{MidiExport, MidiImportOptions};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

/// Refuses to replace an existing file unless `overwrite` is set, so writing new files needs no
/// `confirm` (exporting is routine for agents) and replacing one is always explicit.
fn check_overwrite(path: &Path, overwrite: bool) -> CmdResult<()> {
    if path.exists() && !overwrite {
        return Err(CmdError::Other(format!(
            "{} already exists; pass overwrite: true to replace it",
            path.display()
        )));
    }
    Ok(())
}

/// Where a read may come from. The human may open and import from any absolute path (dropping
/// a file onto the window is the intent); agents and relative paths keep the confinement of
/// [`Host::resolve_path`].
fn read_path(host: &dyn Host, author: &Author, path: &str) -> CmdResult<std::path::PathBuf> {
    if *author == Author::Human && Path::new(path).is_absolute() {
        return Ok(path.into());
    }
    host.resolve_path(path, FileAccess::Read)
}

pub fn register(r: &mut Registry) {
    r.register(typed(
        "file.save",
        "Save the project to its folder (the project folder `.`, unless a path is given).",
        Scope::Full,
        READ,
        save,
    ));
    r.register(typed(
        "file.save_as",
        "Save the project to a folder, replacing the project.json already there.",
        Scope::Full,
        READ,
        save_as,
    ));
    r.register(typed(
        "file.open",
        "Open a project folder. Discards the current song and its history, saved or not.",
        Scope::Full,
        DESTRUCTIVE,
        crate::session_cmd::stub::<Open>,
    ));
    r.register(typed(
        "file.import_flp",
        "Open an FL Studio project (.flp) as a new Patternhaus project, with an import report.",
        Scope::Full,
        DESTRUCTIVE,
        crate::session_cmd::stub::<ImportFlp>,
    ));
    r.register(typed(
        "file.export_audio",
        "Render the song (or a range) to a WAV or FLAC file.",
        Scope::Full,
        READ,
        export_audio,
    ));
    r.register(typed(
        "file.import_midi",
        "Import a MIDI file as a new pattern with one channel per track or MIDI channel.",
        Scope::Full,
        MUTATES,
        import_midi,
    ));
    r.register(typed(
        "file.export_midi",
        "Export a pattern, or the whole song, as a MIDI file.",
        Scope::Full,
        READ,
        export_midi,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Save {
    /// Project folder; defaults to the current project folder (`.`).
    #[serde(default)]
    path: Option<String>,
}

fn write_project(ctx: &mut CommandCtx<'_>, path: &str) -> CmdResult<CommandOutput> {
    let dir = ctx.host.resolve_path(path, FileAccess::Write)?;
    ph_formats::save_project(ctx.project, &dir).map_err(|e| CmdError::Host(e.to_string()))?;
    Ok(CommandOutput::value(json!({ "path": dir })))
}

fn save(ctx: &mut CommandCtx<'_>, p: Save) -> CmdResult<CommandOutput> {
    write_project(ctx, p.path.as_deref().unwrap_or("."))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveAs {
    /// Project folder to save into; created if missing.
    path: String,
    /// Replace a project already saved in that folder. Default false.
    #[serde(default)]
    overwrite: bool,
}

fn save_as(ctx: &mut CommandCtx<'_>, p: SaveAs) -> CmdResult<CommandOutput> {
    let dir = ctx.host.resolve_path(&p.path, FileAccess::Write)?;
    check_overwrite(&dir.join("project.json"), p.overwrite)?;
    write_project(ctx, &p.path)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Open {
    /// Project folder to open.
    path: String,
}

/// Session-handled: replaces the project and clears the history.
pub(crate) fn open_project(
    s: &mut Session,
    host: &mut dyn Host,
    author: &Author,
    params: Value,
) -> CmdResult<SessionOut> {
    let p: Open = parse("file.open", params)?;
    let dir = read_path(host, author, &p.path)?;
    let project = ph_formats::load_project(&dir).map_err(|e| CmdError::Host(e.to_string()))?;
    let (channels, patterns) = (project.channels.len(), project.patterns.len());
    let title = project.settings.title.clone();
    // Move the project folder first: a save after this must write the opened project, never
    // the folder the session started in.
    host.set_project_dir(&dir);
    s.reset(project);
    Ok(SessionOut {
        result: json!({ "path": dir, "channels": channels, "patterns": patterns }),
        summary: Some(format!("opened {title}")),
        ..Default::default()
    })
}

fn rate() -> u32 {
    48_000
}

fn bits() -> u16 {
    24
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportAudio {
    /// Output file; the extension picks the format (`.wav` or `.flac`).
    path: String,
    /// `[start, end]` in ticks; omit for the whole song (or the current pattern in pattern mode).
    #[serde(default)]
    range: Option<(Tick, Tick)>,
    /// Sample rate in Hz, default 48000.
    #[serde(default = "rate")]
    sample_rate: u32,
    /// Bit depth, default 24.
    #[serde(default = "bits")]
    bit_depth: u16,
    /// Use the high-quality offline mode, default true.
    #[serde(default = "yes")]
    high_quality: bool,
    /// Let reverb and delay tails ring out past the end, default true.
    #[serde(default = "yes")]
    include_tail: bool,
    /// Replace the file if it already exists. Default false.
    #[serde(default)]
    overwrite: bool,
}

fn export_audio(ctx: &mut CommandCtx<'_>, p: ExportAudio) -> CmdResult<CommandOutput> {
    if let Some((s, e)) = p.range
        && s >= e
    {
        return invalid("the range must end after it starts");
    }
    let path = ctx.host.resolve_path(&p.path, FileAccess::Write)?;
    check_overwrite(&path, p.overwrite)?;
    ensure_parent(&path)?;
    let req = RenderRequest {
        path: path.to_string_lossy().into_owned(),
        range: p.range,
        sample_rate: p.sample_rate,
        bit_depth: p.bit_depth,
        high_quality: p.high_quality,
        include_tail: p.include_tail,
    };
    let report = ctx.host.render(ctx.project, &req)?;
    Ok(CommandOutput::value(report))
}

fn ensure_parent(path: &Path) -> CmdResult<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| CmdError::Host(format!("{}: {e}", dir.display())))?;
    }
    Ok(())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImportMidi {
    /// MIDI file to import.
    path: String,
    /// Name for the new pattern; defaults to the file name.
    #[serde(default)]
    pattern_name: Option<String>,
    /// Also set the project tempo and time signature from the file. Default false.
    #[serde(default)]
    apply_tempo: bool,
}

fn import_midi(ctx: &mut CommandCtx<'_>, p: ImportMidi) -> CmdResult<CommandOutput> {
    let path = read_path(ctx.host, ctx.author, &p.path)?;
    let bytes =
        std::fs::read(&path).map_err(|e| CmdError::Host(format!("{}: {e}", path.display())))?;
    let name = p.pattern_name.unwrap_or_else(|| {
        path.file_stem().map_or_else(
            || "MIDI import".into(),
            |s| s.to_string_lossy().into_owned(),
        )
    });
    let mut ed = Edit::new(ctx.project);
    let r = ph_formats::import_midi(
        &mut ed,
        &bytes,
        &MidiImportOptions {
            pattern_name: name.clone(),
        },
    )
    .map_err(|e| CmdError::Host(e.to_string()))?;
    if p.apply_tempo {
        ed.update_settings(|s| {
            if let Some(b) = r.tempo_bpm {
                s.bpm = b;
            }
            if let Some(t) = r.time_signature {
                s.time_signature = t;
            }
        })?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({
            "pattern": r.pattern, "channels": r.channels, "notes": r.notes,
            "tempo_bpm": r.tempo_bpm, "time_signature": r.time_signature
        }),
        format!(
            "imported {} as {name}: {}, {}",
            p.path,
            plural(r.channels.len(), "channel", "channels"),
            plural(r.notes, "note", "notes")
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportMidi {
    /// Output `.mid` file.
    path: String,
    /// Pattern to export; omit to export the whole song from the playlist.
    #[serde(default)]
    pattern: Option<PatternId>,
    /// Replace the file if it already exists. Default false.
    #[serde(default)]
    overwrite: bool,
}

fn export_midi(ctx: &mut CommandCtx<'_>, p: ExportMidi) -> CmdResult<CommandOutput> {
    let path = ctx.host.resolve_path(&p.path, FileAccess::Write)?;
    check_overwrite(&path, p.overwrite)?;
    let what = p.pattern.map_or(MidiExport::Song, MidiExport::Pattern);
    let bytes =
        ph_formats::export_midi(ctx.project, what).map_err(|e| CmdError::Host(e.to_string()))?;
    ensure_parent(&path)?;
    std::fs::write(&path, &bytes)
        .map_err(|e| CmdError::Host(format!("{}: {e}", path.display())))?;
    Ok(CommandOutput::value(
        json!({ "path": path, "bytes": bytes.len() }),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFlp {
    /// The .flp file. Agents may only name files inside the allowed folders.
    path: String,
    /// Where the new project folder goes: `projects_folder` (default; `Documents/Patternhaus
    /// Projects/<name>`) or `next_to_flp` (`<name> (Patternhaus)` beside the .flp). If the
    /// chosen place cannot be written, the projects folder is used and the report says so.
    #[serde(default)]
    placement: Placement,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    #[default]
    ProjectsFolder,
    NextToFlp,
}

/// `parent/<name>`, numbered `<name> (2)`, `(3)` ... while that exists (an import never
/// overwrites anything).
fn numbered_dir(parent: &Path, name: &str) -> std::path::PathBuf {
    let mut dir = parent.join(name);
    let mut n = 2;
    while dir.exists() {
        dir = parent.join(format!("{name} ({n})"));
        n += 1;
    }
    dir
}

/// Creates the new project folder in `parent`, where the caller may write. Errors describe why
/// the place is unusable.
fn make_import_dir(
    host: &dyn Host,
    author: &Author,
    parent: &Path,
    name: &str,
) -> Result<std::path::PathBuf, String> {
    let wanted = numbered_dir(parent, name);
    let dir = if *author == Author::Human {
        wanted
    } else {
        host.resolve_path(&wanted.to_string_lossy(), FileAccess::Write)
            .map_err(|e| e.to_string())?
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// Picks and creates the folder for an FLP import, with a note when the fallback was used.
fn choose_import_dir(
    host: &dyn Host,
    author: &Author,
    src: &Path,
    placement: Placement,
) -> CmdResult<(std::path::PathBuf, Option<String>)> {
    let stem = src
        .file_stem()
        .map_or_else(|| "FL project".into(), |s| s.to_string_lossy().into_owned());
    let projects = host.projects_dir();
    let beside = || {
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        make_import_dir(host, author, parent, &format!("{stem} (Patternhaus)"))
    };
    let in_projects = |p: &Path| make_import_dir(host, author, p, &stem);
    let first = match (placement, &projects) {
        (Placement::ProjectsFolder, Some(p)) => in_projects(p),
        // No projects folder known to this host: beside the .flp is all there is.
        (Placement::ProjectsFolder, None) | (Placement::NextToFlp, _) => beside(),
    };
    match first {
        Ok(dir) => Ok((dir, None)),
        Err(why) => match &projects {
            Some(p) if placement == Placement::NextToFlp => {
                let dir = in_projects(p).map_err(|e| {
                    CmdError::Host(format!("cannot create the project folder ({why}; {e})"))
                })?;
                let note = format!(
                    "Placed in the Patternhaus projects folder because the folder next to the FL project could not be used ({why})."
                );
                Ok((dir, Some(note)))
            }
            _ => Err(CmdError::Host(format!(
                "cannot create the project folder: {why}"
            ))),
        },
    }
}

// SPEC-GAP: 4.1: the spec lists the placeholders but not how to resolve them.
/// Expands a leading `%NAME%` placeholder of a stored sample path from the environment
/// (spec 4.1 names `%USERPROFILE%`, `%FLStudioData%`, `%FLStudioFactoryData%` but not how to
/// resolve them: only environment variables are tried, so FL's own folders are not found).
fn expand_placeholder(path: &str) -> Option<std::path::PathBuf> {
    let rest = path.strip_prefix('%')?;
    let (name, tail) = rest.split_once('%')?;
    let base = std::env::var_os(name)?;
    let tail = tail.trim_start_matches(['/', '\\']).replace('\\', "/");
    Some(std::path::PathBuf::from(base).join(tail))
}

/// Finds a sample an FLP names on this machine and copies it into the new project folder
/// through the host, so the import is self-contained.
struct HostSamples<'a> {
    host: &'a mut dyn Host,
    author: &'a Author,
    flp_dir: std::path::PathBuf,
}

impl ph_formats::flp::SampleResolver for HostSamples<'_> {
    fn resolve(&mut self, flp_path: &str) -> Option<ph_formats::flp::ResolvedSample> {
        let plain = std::path::PathBuf::from(flp_path.replace('\\', "/"));
        let candidates = [
            expand_placeholder(flp_path),
            Some(plain.clone()),
            // a relative path is taken relative to the .flp
            plain.is_relative().then(|| self.flp_dir.join(&plain)),
        ];
        for cand in candidates.into_iter().flatten() {
            if !cand.is_file() {
                continue;
            }
            let Ok(reference) = self
                .host
                .import_file(&cand, ImportKind::Sample, self.author)
            else {
                continue;
            };
            let info = self.host.sample_info(&reference).ok()?;
            return Some(ph_formats::flp::ResolvedSample {
                reference,
                frames: info.frames,
                sample_rate: info.sample_rate,
                channels: info.channels,
            });
        }
        None
    }
}

/// Session-handled: builds a new project from an FL Studio file in a new folder beside it,
/// saves it there with its plugin states and `import-report.md`, and replaces the session's
/// project (like `file.open`).
pub(crate) fn import_flp_project(
    s: &mut Session,
    host: &mut dyn Host,
    author: &Author,
    params: Value,
) -> CmdResult<SessionOut> {
    let p: ImportFlp = parse("file.import_flp", params)?;
    let src = read_path(host, author, &p.path)?;
    let bytes =
        std::fs::read(&src).map_err(|e| CmdError::Host(format!("{}: {e}", src.display())))?;
    // Refuse a file that is not a project before anything is created.
    ph_formats::flp::parse(&bytes).map_err(|e| CmdError::Host(format!("{}: {e}", p.path)))?;
    let (dir, note) = choose_import_dir(host, author, &src, p.placement)?;
    let title = src
        .file_stem()
        .map_or_else(|| "FL project".into(), |s| s.to_string_lossy().into_owned());
    let previous = host.resolve_path(".", FileAccess::Read).ok();
    // The samples are copied into the new folder, so the host moves there first.
    host.set_project_dir(&dir);
    let built = {
        let mut samples = HostSamples {
            host: &mut *host,
            author,
            flp_dir: src.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        ph_formats::flp::import_flp(&bytes, &title, &mut samples)
    };
    let written = built
        .map_err(|e| CmdError::Host(format!("{}: {e}", p.path)))
        .and_then(|imp| write_import(&dir, &imp, note.as_deref()).map(|()| imp));
    let imp = match written {
        Ok(imp) => imp,
        Err(e) => {
            if let Some(prev) = previous {
                host.set_project_dir(&prev);
            }
            let _ = std::fs::remove_dir(&dir); // only removed when still empty
            return Err(e);
        }
    };
    let summary = format!(
        "imported {} from FL Studio: {}",
        imp.project.settings.title,
        imp.report.summary_line()
    );
    let result = json!({
        "path": dir,
        "report_file": "import-report.md",
        "channels": imp.project.channels.len(),
        "patterns": imp.project.patterns.len(),
        "report": imp.report.to_json(),
        "placement_note": note,
    });
    s.reset(imp.project);
    Ok(SessionOut {
        result,
        summary: Some(summary),
        ..Default::default()
    })
}

fn write_import(
    dir: &Path,
    imp: &ph_formats::flp::FlpImport,
    note: Option<&str>,
) -> CmdResult<()> {
    let host_err = |p: &Path, e: std::io::Error| CmdError::Host(format!("{}: {e}", p.display()));
    ph_formats::save_project(&imp.project, dir).map_err(|e| CmdError::Host(e.to_string()))?;
    for f in &imp.files {
        let target = dir.join(&f.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| host_err(parent, e))?;
        }
        std::fs::write(&target, &f.bytes).map_err(|e| host_err(&target, e))?;
    }
    let report = dir.join("import-report.md");
    let mut text = imp.report.to_markdown(&imp.project.settings.title);
    if let Some(n) = note {
        text.push_str(&format!("
## Placement

{n}
"));
    }
    std::fs::write(&report, text).map_err(|e| host_err(&report, e))
}
