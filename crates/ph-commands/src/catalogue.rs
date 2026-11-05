//! The plugin instrument catalogue (M3/T9): saved plugin instruments with playing guides, in a
//! per-user folder outside any project (`docs/formats/catalogue.md`).
//!
//! ```text
//! <catalogue>/
//!   <entry id>/
//!     entry.toml     name, description, tags, the plugin, the playing guide (hand-editable)
//!     plugin.state   the plugin's state as a Patternhaus plugin-state file (opaque)
//! ```
//!
//! The entry id is the folder name, chosen from the name when the entry is saved and never
//! changed (renaming an entry changes its `name` only). `plugin.state` holds exactly the
//! bytes of a project's `plugins/<file>.state`: a magic line, one JSON line naming the plugin,
//! then the plugin's own state bytes, which are never parsed. A project using an entry gets
//! its own copy of the state, and a copy of the guide beside it
//! (`plugins/<file>.catalogue.toml`, [`ProjectGuide`]).

use crate::{CmdError, CmdResult};
use ph_core::PluginFormat;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ENTRY_FILE: &str = "entry.toml";
pub const STATE_FILE: &str = "plugin.state";
/// Extension of the guide copy beside a project's state file.
pub const PROJECT_GUIDE_EXT: &str = "catalogue.toml";

/// How to play an instrument: what an agent reads before writing a part.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayingGuide {
    /// Lowest and highest playable keys, inclusive (MIDI, 60 = C4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[u8; 2]>,
    /// How velocity behaves (e.g. "layers switch at 64 and 100; soft notes are breathy").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub velocity: String,
    /// One note at a time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mono: bool,
    /// Overlapping notes play legato transitions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legato: bool,
    /// Anything else a player should know.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Keyswitches: a key that selects an articulation and makes no sound itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyswitches: Vec<Keyswitch>,
    /// Controllers the instrument responds to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ccs: Vec<CcUse>,
}

/// A keyswitch of a [`PlayingGuide`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Keyswitch {
    /// The key (MIDI, 60 = C4).
    pub key: u8,
    /// The articulation it selects (e.g. "staccato").
    pub articulation: String,
}

/// A controller of a [`PlayingGuide`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CcUse {
    /// MIDI controller number (1 = mod wheel, 11 = expression, 64 = sustain).
    pub cc: u8,
    /// What it does (e.g. "dynamics (crossfades layers)").
    pub meaning: String,
}

/// The plugin an entry restores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryPlugin {
    /// `clap` or `vst3`.
    pub format: PluginFormat,
    /// CLAP id or VST3 class id.
    pub id: String,
    /// The plugin's name when the entry was saved.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    /// The plugin's version when the entry was saved.
    #[serde(default)]
    pub version: String,
}

/// `entry.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogueEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Free tags: instrument family, library, vendor, character.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub plugin: EntryPlugin,
    #[serde(default)]
    pub guide: PlayingGuide,
}

impl CatalogueEntry {
    /// True when every word of `text` is found (case-insensitively) in the name, description,
    /// tags, plugin name or vendor, or the guide's notes and articulations.
    pub fn matches_text(&self, text: &str) -> bool {
        let mut hay = vec![
            self.name.as_str(),
            &self.description,
            &self.plugin.name,
            &self.plugin.vendor,
            &self.guide.notes,
        ];
        hay.extend(self.tags.iter().map(String::as_str));
        hay.extend(
            self.guide
                .keyswitches
                .iter()
                .map(|k| k.articulation.as_str()),
        );
        let hay = hay.join("\n").to_lowercase();
        text.split_whitespace()
            .all(|w| hay.contains(&w.to_lowercase()))
    }

    /// True when the entry has every tag in `tags` (case-insensitively).
    pub fn has_tags(&self, tags: &[String]) -> bool {
        tags.iter()
            .all(|t| self.tags.iter().any(|x| x.eq_ignore_ascii_case(t)))
    }
}

/// The guide copy a project keeps beside a state file restored from the catalogue
/// (`plugins/<file>.catalogue.toml`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectGuide {
    /// The catalogue entry id it came from.
    pub entry: String,
    pub name: String,
    pub plugin: EntryPlugin,
    #[serde(default)]
    pub guide: PlayingGuide,
}

impl ProjectGuide {
    /// Reads a guide copy (`plugins/<file>.catalogue.toml`).
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

/// `C4` for 60.
pub fn note_name(key: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[key as usize % 12], key as i32 / 12 - 1)
}

/// The guide in a sentence or two, for agents.
pub fn guide_text(g: &PlayingGuide) -> String {
    let mut parts = Vec::new();
    match g.range {
        Some([lo, hi]) => parts.push(format!(
            "plays {}-{} ({lo}-{hi})",
            note_name(lo),
            note_name(hi)
        )),
        None => parts.push("range not set".into()),
    }
    if g.mono {
        parts.push("mono".into());
    }
    if g.legato {
        parts.push("legato (overlap notes)".into());
    }
    if !g.keyswitches.is_empty() {
        parts.push(format!(
            "keyswitches {}",
            g.keyswitches
                .iter()
                .map(|k| format!("{} ({}) {}", note_name(k.key), k.key, k.articulation))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !g.ccs.is_empty() {
        parts.push(format!(
            "CCs {}",
            g.ccs
                .iter()
                .map(|c| format!("CC{} {}", c.cc, c.meaning))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !g.velocity.is_empty() {
        parts.push(format!("velocity: {}", g.velocity));
    }
    if !g.notes.is_empty() {
        parts.push(g.notes.clone());
    }
    parts.join("; ")
}

/// Where a project's guide copy lives for a state reference (`plugins/x.state` →
/// `plugins/x.catalogue.toml`).
pub fn project_guide_ref(state_ref: &str) -> String {
    let stem = state_ref.strip_suffix(".state").unwrap_or(state_ref);
    format!("{stem}.{PROJECT_GUIDE_EXT}")
}

fn err(path: &Path, e: impl std::fmt::Display) -> CmdError {
    CmdError::Host(format!("{}: {e}", path.display()))
}

/// Writes through a temporary file, so a crash never leaves half an entry.
fn write_atomic(path: &Path, bytes: &[u8]) -> CmdResult<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| err(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| err(path, e))
}

/// An entry that could not be read, as `catalogue.list` reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BrokenEntry {
    pub id: String,
    pub error: String,
}

/// A catalogue folder.
#[derive(Clone, Debug)]
pub struct Catalogue {
    dir: PathBuf,
}

impl Catalogue {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_dir(&self, id: &str) -> CmdResult<PathBuf> {
        let ok = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err(CmdError::Model(ph_core::ModelError::Invalid(format!(
                "{id:?} is not a catalogue entry id (see catalogue.list)"
            ))));
        }
        Ok(self.dir.join(id))
    }

    /// Every entry, sorted by id, and the folders that hold no readable entry.
    pub fn list(&self) -> (Vec<(String, CatalogueEntry)>, Vec<BrokenEntry>) {
        let (mut ok, mut bad) = (Vec::new(), Vec::new());
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return (ok, bad);
        };
        let mut ids: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();
        ids.sort();
        for id in ids {
            match self.get(&id) {
                Ok(e) => ok.push((id, e)),
                Err(e) => bad.push(BrokenEntry {
                    id,
                    error: e.to_string(),
                }),
            }
        }
        (ok, bad)
    }

    pub fn get(&self, id: &str) -> CmdResult<CatalogueEntry> {
        let path = self.entry_dir(id)?.join(ENTRY_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CmdError::Model(ph_core::ModelError::Invalid(format!(
                    "no catalogue entry {id:?} (see catalogue.list)"
                )))
            } else {
                err(&path, e)
            }
        })?;
        toml::from_str(&text).map_err(|e| err(&path, e))
    }

    /// The entry's state file (opaque bytes).
    pub fn state(&self, id: &str) -> CmdResult<Vec<u8>> {
        let path = self.entry_dir(id)?.join(STATE_FILE);
        std::fs::read(&path).map_err(|e| err(&path, e))
    }

    /// Rewrites `entry.toml` of an existing entry.
    pub fn put(&self, id: &str, entry: &CatalogueEntry) -> CmdResult<()> {
        let dir = self.entry_dir(id)?;
        if !dir.join(ENTRY_FILE).is_file() {
            self.get(id)?;
        }
        let text = toml::to_string_pretty(entry).map_err(|e| err(&dir, e))?;
        write_atomic(&dir.join(ENTRY_FILE), text.as_bytes())
    }

    /// Saves a new entry and returns its id: the name made into a folder name, numbered when
    /// taken (`-2`, `-3`, ...).
    pub fn create(&self, entry: &CatalogueEntry, state: &[u8]) -> CmdResult<String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| err(&self.dir, e))?;
        let slug = slug(&entry.name);
        let text = toml::to_string_pretty(entry).map_err(|e| err(&self.dir, e))?;
        for n in 1.. {
            let id = match n {
                1 => slug.clone(),
                n => format!("{slug}-{n}"),
            };
            let dir = self.dir.join(&id);
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    write_atomic(&dir.join(STATE_FILE), state)?;
                    write_atomic(&dir.join(ENTRY_FILE), text.as_bytes())?;
                    return Ok(id);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(err(&dir, e)),
            }
        }
        unreachable!("an unused id")
    }

    /// Deletes an entry's folder (a broken entry too).
    pub fn remove(&self, id: &str) -> CmdResult<()> {
        let dir = self.entry_dir(id)?;
        if !dir.join(ENTRY_FILE).is_file() && !dir.join(STATE_FILE).is_file() {
            self.get(id)?;
        }
        std::fs::remove_dir_all(&dir).map_err(|e| err(&dir, e))
    }
}

/// A folder name from an entry name: lowercase ASCII letters, digits and `-`.
pub fn slug(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    if s.is_empty() {
        "entry".into()
    } else {
        s.chars()
            .take(60)
            .collect::<String>()
            .trim_end_matches('-')
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> CatalogueEntry {
        CatalogueEntry {
            name: "Warm Strings (legato)".into(),
            description: "Section strings".into(),
            tags: vec!["strings".into(), "Orchestral".into()],
            plugin: EntryPlugin {
                format: PluginFormat::Vst3,
                id: "ABCDEF".into(),
                name: "Some Sampler".into(),
                vendor: "Vendor".into(),
                version: "7.1".into(),
            },
            guide: PlayingGuide {
                range: Some([28, 96]),
                velocity: "soft below 50".into(),
                mono: false,
                legato: true,
                notes: String::new(),
                keyswitches: vec![Keyswitch {
                    key: 24,
                    articulation: "sustain".into(),
                }],
                ccs: vec![CcUse {
                    cc: 1,
                    meaning: "dynamics".into(),
                }],
            },
        }
    }

    #[test]
    fn entries_round_trip_through_plain_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Catalogue::open(tmp.path().join("catalogue"));
        let e = entry();
        let id = c.create(&e, b"opaque\0bytes").unwrap();
        assert_eq!(id, "warm-strings-legato");
        assert_eq!(c.create(&e, b"x").unwrap(), "warm-strings-legato-2");
        assert_eq!(c.get(&id).unwrap(), e);
        assert_eq!(c.state(&id).unwrap(), b"opaque\0bytes");
        let text = std::fs::read_to_string(tmp.path().join("catalogue").join(&id).join(ENTRY_FILE))
            .unwrap();
        assert!(text.contains("name = \"Warm Strings (legato)\""), "{text}");
        assert!(text.contains("[[guide.keyswitches]]"), "{text}");
        // hand edits are read back
        std::fs::write(
            tmp.path().join("catalogue").join(&id).join(ENTRY_FILE),
            text.replace("range = [", "range = [\n")
                .replace("soft below 50", "soft below 40"),
        )
        .unwrap();
        assert_eq!(c.get(&id).unwrap().guide.velocity, "soft below 40");
        // a broken entry is listed as broken, not dropped silently
        std::fs::write(
            tmp.path()
                .join("catalogue/warm-strings-legato-2/entry.toml"),
            "nope",
        )
        .unwrap();
        let (ok, bad) = c.list();
        assert_eq!(ok.len(), 1);
        assert_eq!(bad[0].id, "warm-strings-legato-2");
        c.remove("warm-strings-legato-2").unwrap();
        c.remove(&id).unwrap();
        assert!(c.get(&id).is_err());
        assert!(c.get("../x").is_err());
    }

    #[test]
    fn search_reads_text_and_tags() {
        let e = entry();
        assert!(e.matches_text("warm STRINGS"));
        assert!(e.matches_text("sustain"));
        assert!(e.matches_text("vendor"));
        assert!(!e.matches_text("brass"));
        assert!(e.has_tags(&["orchestral".into()]));
        assert!(!e.has_tags(&["orchestral".into(), "brass".into()]));
        assert_eq!(
            project_guide_ref("plugins/a.state"),
            "plugins/a.catalogue.toml"
        );
        assert_eq!(slug("  !! "), "entry");
    }
}
