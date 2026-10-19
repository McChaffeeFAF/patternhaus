//! The project folder: `project.json` plus `samples/`, `renders/` and `history/`.

use crate::{FormatError, FormatResult};
use ph_core::{ClipKind, FORMAT_VERSION, MIXER_INSERTS, PLAYLIST_TRACKS, Project};
use serde_json::Value;
use std::fs;
use std::path::Path;

pub const PROJECT_FILE: &str = "project.json";
pub const SAMPLES_DIR: &str = "samples";
pub const RENDERS_DIR: &str = "renders";
pub const HISTORY_DIR: &str = "history";
pub const HISTORY_LOG: &str = "history/log.jsonl";

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> FormatError + '_ {
    move |source| FormatError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The exact text `project.json` holds for `project`: pretty-printed, trailing newline.
/// Key order follows the model's struct field order and its sorted maps, so the same project
/// always produces the same bytes.
pub fn project_to_json(project: &Project) -> String {
    let mut s = serde_json::to_string_pretty(project).expect("project always serializes");
    s.push('\n');
    s
}

/// Saves `project` into the folder `dir` (created if missing), along with empty `samples/`,
/// `renders/` and `history/` folders. `project.json` is written to a temporary file and
/// renamed, so a crash never leaves half a project. Saving twice is byte-identical.
pub fn save_project(project: &Project, dir: &Path) -> FormatResult<()> {
    for sub in [SAMPLES_DIR, RENDERS_DIR, HISTORY_DIR] {
        let p = dir.join(sub);
        fs::create_dir_all(&p).map_err(io(&p))?;
    }
    let target = dir.join(PROJECT_FILE);
    let tmp = dir.join(format!("{PROJECT_FILE}.tmp"));
    fs::write(&tmp, project_to_json(project)).map_err(io(&tmp))?;
    fs::rename(&tmp, &target).map_err(io(&target))
}

/// Loads a project folder, migrating older versions to the current one and checking the
/// result is consistent.
pub fn load_project(dir: &Path) -> FormatResult<Project> {
    let file = dir.join(PROJECT_FILE);
    let text = fs::read_to_string(&file).map_err(io(&file))?;
    project_from_json(&text)
}

/// Parses the text of a `project.json`, migrating and validating it.
pub fn project_from_json(text: &str) -> FormatResult<Project> {
    let mut value: Value =
        serde_json::from_str(text).map_err(|e| FormatError::Json(e.to_string()))?;
    migrate(&mut value)?;
    let project: Project =
        serde_json::from_value(value).map_err(|e| FormatError::Json(e.to_string()))?;
    validate(&project)?;
    Ok(project)
}

/// Writes the optional persisted patch log (`history/log.jsonl`, one JSON value per line).
/// `ph-formats` does not know the history entry type, so callers pass serialized entries.
pub fn write_history_log(dir: &Path, entries: &[Value]) -> FormatResult<()> {
    let p = dir.join(HISTORY_DIR);
    fs::create_dir_all(&p).map_err(io(&p))?;
    let mut text = String::new();
    for e in entries {
        text.push_str(&serde_json::to_string(e).expect("value serializes"));
        text.push('\n');
    }
    let file = dir.join(HISTORY_LOG);
    fs::write(&file, text).map_err(io(&file))
}

/// Reads the persisted patch log, if there is one.
pub fn read_history_log(dir: &Path) -> FormatResult<Vec<Value>> {
    let file = dir.join(HISTORY_LOG);
    if !file.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&file).map_err(io(&file))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| FormatError::Json(e.to_string())))
        .collect()
}

/// One migration step: upgrades the JSON of version `n` to version `n + 1` in place.
type Migration = fn(&mut Value) -> FormatResult<()>;

/// `MIGRATIONS[n - 1]` upgrades version `n` to `n + 1`. Add a step here and bump
/// `FORMAT_VERSION` together, and add a fixture of the old version to the tests
/// (`tests/fixtures/`).
const MIGRATIONS: &[Migration] = &[v1_to_v2];

/// 1 → 2 (M2): audio and automation clips, the sample registry (`samples`), automation
/// points and the `sample`/`point` id counters were added, all with serde defaults, and a
/// clip without `kind` is a pattern clip. So a version 1 file is already a valid version 2
/// file: nothing to rewrite. Checked, so a damaged file fails here with a clear message.
fn v1_to_v2(value: &mut Value) -> FormatResult<()> {
    let clips = value.get("clips").and_then(Value::as_object);
    if let Some(c) = clips
        .into_iter()
        .flat_map(|m| m.values())
        .find(|c| c.get("kind").is_some())
    {
        return Err(FormatError::Invalid(format!(
            "a version 1 project cannot hold {} clips",
            c["kind"]
        )));
    }
    if value.get("samples").is_some() {
        return Err(FormatError::Invalid(
            "a version 1 project cannot have a sample registry".into(),
        ));
    }
    Ok(())
}

/// Upgrades `value` (a parsed `project.json`) to [`FORMAT_VERSION`].
pub fn migrate(value: &mut Value) -> FormatResult<()> {
    let found = value
        .get("format_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| FormatError::Json("missing format_version".into()))?;
    if found == 0 {
        return Err(FormatError::BadVersion(found));
    }
    if found > FORMAT_VERSION as u64 {
        return Err(FormatError::TooNew {
            found,
            supported: FORMAT_VERSION,
        });
    }
    for v in found..FORMAT_VERSION as u64 {
        let step = MIGRATIONS
            .get(v as usize - 1)
            .ok_or(FormatError::BadVersion(v))?;
        step(value)?;
        value["format_version"] = Value::from(v + 1);
    }
    Ok(())
}

/// Structural checks a hand-edited or damaged file could break.
pub fn validate(p: &Project) -> FormatResult<()> {
    let bad = |m: String| Err(FormatError::Invalid(m));
    if p.mixer.len() != MIXER_INSERTS + 1 {
        return bad(format!("mixer has {} inserts", p.mixer.len()));
    }
    if p.tracks.len() != PLAYLIST_TRACKS {
        return bad(format!("playlist has {} tracks", p.tracks.len()));
    }
    for id in p.channel_order.iter() {
        if !p.channels.contains_key(id) {
            return bad(format!("channel_order lists missing channel {id}"));
        }
    }
    for id in p.pattern_order.iter() {
        if !p.patterns.contains_key(id) {
            return bad(format!("pattern_order lists missing pattern {id}"));
        }
    }
    for (id, ch) in p.channels.iter() {
        if ch.id != *id || ch.insert.0 as usize > MIXER_INSERTS {
            return bad(format!("channel {id} is malformed"));
        }
    }
    for pat in p.patterns.values() {
        for (cid, notes) in &pat.notes {
            if !p.channels.contains_key(cid) {
                return bad(format!(
                    "pattern {} has notes for missing channel {cid}",
                    pat.id
                ));
            }
            for n in notes.values() {
                ph_core::validate_note(n)?;
            }
        }
    }
    let mut paths = std::collections::BTreeSet::new();
    for (id, e) in p.samples.iter() {
        if e.id != *id || !paths.insert(e.path.as_str()) {
            return bad(format!("sample {id} is malformed or registered twice"));
        }
    }
    for (id, c) in p.clips.iter() {
        let source_ok = match &c.kind {
            ClipKind::Pattern { pattern } => p.patterns.contains_key(pattern),
            ClipKind::Audio(a) => {
                a.validate()?;
                p.samples.contains_key(&a.sample) && c.offset == 0
            }
            ClipKind::Automation(a) => a.points.iter().all(|(k, pt)| pt.id == *k),
        };
        if c.id != *id || !source_ok || c.track as usize >= p.tracks.len() || c.length == 0 {
            return bad(format!("clip {} points outside the project", c.id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::*;

    fn sample() -> Project {
        let mut ed = Edit::new(&Project::empty("Folder test"));
        let pat = ed.add_pattern("Beat").unwrap();
        let kick = ed
            .add_channel(
                "Kick",
                InstrumentSpec::Sampler {
                    slices: Vec::new(),
                    sample: Some("factory:kick".into()),
                    root_key: 60,
                    params: Default::default(),
                },
            )
            .unwrap();
        let synth = ed
            .add_channel(
                "Bass",
                InstrumentSpec::Synth {
                    preset: None,
                    params: [("cutoff".to_string(), 0.4)].into(),
                },
            )
            .unwrap();
        for i in 0..4 {
            ed.add_note(pat, kick, NoteSpec::new(36, i * PPQ, STEP))
                .unwrap();
        }
        ed.add_note(pat, synth, NoteSpec::new(33, 0, PPQ)).unwrap();
        ed.add_clip(0, pat, 0).unwrap();
        ed.update_insert(InsertId(1), |i| {
            i.effects.push(EffectSlot {
                effect: EffectSpec::Builtin {
                    name: "reverb".into(),
                    params: Default::default(),
                },
                bypass: false,
                mix: 0.3,
            })
        })
        .unwrap();
        ed.finish().0
    }

    #[test]
    fn round_trips_and_is_byte_stable() {
        let dir = tempfile::tempdir().unwrap();
        let p = sample();
        save_project(&p, dir.path()).unwrap();
        let first = fs::read(dir.path().join(PROJECT_FILE)).unwrap();
        for sub in [SAMPLES_DIR, RENDERS_DIR, HISTORY_DIR] {
            assert!(dir.path().join(sub).is_dir());
        }
        let loaded = load_project(dir.path()).unwrap();
        assert_eq!(loaded, p);
        save_project(&loaded, dir.path()).unwrap();
        assert_eq!(fs::read(dir.path().join(PROJECT_FILE)).unwrap(), first);
        assert!(!dir.path().join("project.json.tmp").exists());
    }

    #[test]
    fn json_is_readable() {
        let text = project_to_json(&sample());
        assert!(text.contains("\n  \"settings\""));
        assert!(text.contains("\"volume\": 0.78"), "f32 values print short");
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn newer_version_is_refused() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["format_version"] = Value::from(FORMAT_VERSION + 1);
        let err = project_from_json(&v.to_string()).unwrap_err();
        assert!(matches!(err, FormatError::TooNew { .. }), "{err}");
    }

    #[test]
    fn missing_or_zero_version_is_refused() {
        assert!(project_from_json("{}").is_err());
        let mut v = serde_json::to_value(sample()).unwrap();
        v["format_version"] = Value::from(0);
        assert!(matches!(
            project_from_json(&v.to_string()),
            Err(FormatError::BadVersion(0))
        ));
    }

    #[test]
    fn inconsistent_projects_are_refused() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["channel_order"] = serde_json::json!([0, 99]);
        assert!(matches!(
            project_from_json(&v.to_string()),
            Err(FormatError::Invalid(_))
        ));
    }

    #[test]
    fn history_log_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![serde_json::json!({"txn": 1}), serde_json::json!({"txn": 2})];
        write_history_log(dir.path(), &entries).unwrap();
        assert_eq!(read_history_log(dir.path()).unwrap(), entries);
        let empty = tempfile::tempdir().unwrap();
        assert!(read_history_log(empty.path()).unwrap().is_empty());
    }

    #[test]
    fn missing_folder_reports_the_path() {
        let err = load_project(Path::new("definitely/not/here")).unwrap_err();
        assert!(err.to_string().contains("project.json"), "{err}");
    }
}
