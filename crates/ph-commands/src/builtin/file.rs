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
