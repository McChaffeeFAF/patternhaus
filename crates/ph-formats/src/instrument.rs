//! The native Haus Sampler instrument format (M3): a folder with `instrument.toml`.
//!
//! ```text
//! <Name>/
//!   instrument.toml   the zone map and settings (this module)
//!   samples/          audio files the zones play (paths in instrument.toml are relative
//!                     to the folder: `samples/C4.wav`)
//!   ui/               reserved for M3b (skins); never written or read in M3
//!   scripts/          reserved for M3b (Luau, KSP); never written or read in M3
//! ```
//!
//! Every key, type, unit, default and version is documented in
//! `docs/formats/haus-instrument.md`. The file is versioned on its own
//! ([`INSTRUMENT_FILE_VERSION`]); older versions are migrated on load, newer ones refused.
//!
//! The file holds the [`HausInstrument`] model without project-specific data: no
//! instrument id and no `folder` (the folder is wherever the file is). Group ids are the
//! file's own (zones and modulation routes refer to them), and so are zone ids.
//! Importing into a project (`Edit::add_instrument`) gives everything fresh project ids.

use crate::{FormatError, FormatResult};
use ph_core::{
    Articulation, Group, HausInstrument, ImportNote, InstrumentId, InstrumentSettings,
    InstrumentSource, ModRoute, SampleRef, Zone,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const INSTRUMENT_FILE: &str = "instrument.toml";
pub const INSTRUMENT_SAMPLES_DIR: &str = "samples";
/// Reserved for M3b skins.
pub const INSTRUMENT_UI_DIR: &str = "ui";
/// Reserved for M3b scripts.
pub const INSTRUMENT_SCRIPTS_DIR: &str = "scripts";
/// The value of the file's `format` key.
pub const INSTRUMENT_FORMAT_NAME: &str = "patternhaus-instrument";
/// Current version of `instrument.toml`.
pub const INSTRUMENT_FILE_VERSION: u32 = 1;

/// The saved form. Field order is the file's key order.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentFile {
    format: String,
    version: u32,
    name: String,
    #[serde(default)]
    settings: InstrumentSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<InstrumentSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    extra: BTreeMap<String, String>,
    #[serde(default, rename = "import_note", skip_serializing_if = "Vec::is_empty")]
    import_notes: Vec<ImportNote>,
    #[serde(
        default,
        rename = "articulation",
        skip_serializing_if = "Vec::is_empty"
    )]
    articulations: Vec<Articulation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modulation: Vec<ModRoute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    group: Vec<Group>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    zone: Vec<Zone>,
}

fn invalid(m: impl Into<String>) -> FormatError {
    FormatError::Invalid(m.into())
}

/// One migration step of `instrument.toml`: upgrades version `n` to `n + 1` in place.
type Migration = fn(&mut toml::Table) -> FormatResult<()>;

/// `MIGRATIONS[n - 1]` upgrades version `n` to `n + 1` (none yet: version 1 is the first).
const MIGRATIONS: &[Migration] = &[];

fn migrate(table: &mut toml::Table) -> FormatResult<()> {
    match table.get("format").and_then(|v| v.as_str()) {
        Some(INSTRUMENT_FORMAT_NAME) => {}
        Some(other) => {
            return Err(invalid(format!(
                "not a Patternhaus instrument (format = {other:?})"
            )));
        }
        None => return Err(invalid("instrument.toml has no `format` key")),
    }
    let found = table
        .get("version")
        .and_then(|v| v.as_integer())
        .ok_or_else(|| invalid("instrument.toml has no `version`"))?;
    if found < 1 {
        return Err(FormatError::BadVersion(found.max(0) as u64));
    }
    if found > INSTRUMENT_FILE_VERSION as i64 {
        return Err(FormatError::TooNew {
            found: found as u64,
            supported: INSTRUMENT_FILE_VERSION,
        });
    }
    for v in found..INSTRUMENT_FILE_VERSION as i64 {
        let step = MIGRATIONS
            .get(v as usize - 1)
            .ok_or(FormatError::BadVersion(v as u64))?;
        step(table)?;
        table.insert("version".into(), toml::Value::Integer(v + 1));
    }
    Ok(())
}

/// Parses `instrument.toml` text into an instrument (id 0, no folder), migrating and checking
/// it. Never panics on any input.
pub fn instrument_from_toml(text: &str) -> FormatResult<HausInstrument> {
    let mut table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| invalid(format!("instrument.toml: {}", e.message())))?;
    migrate(&mut table)?;
    let file: InstrumentFile = toml::Value::Table(table)
        .try_into()
        .map_err(|e: toml::de::Error| invalid(format!("instrument.toml: {}", e.message())))?;
    let mut inst = HausInstrument::new(&file.name);
    inst.settings = file.settings;
    inst.source = file.source;
    inst.extra = file.extra;
    inst.import_notes = file.import_notes;
    inst.articulations = file.articulations;
    inst.modulation = file.modulation;
    for g in file.group {
        if inst.groups.insert(g.id, g.clone()).is_some() {
            return Err(invalid(format!("group {} appears twice", g.id)));
        }
    }
    for z in file.zone {
        let id = z.id;
        if inst.zones.insert(id, z).is_some() {
            return Err(invalid(format!("zone {id} appears twice")));
        }
    }
    check(&inst)?;
    Ok(inst)
}

/// The exact text `instrument.toml` holds for `inst`. Fails if a zone plays a project
/// registry sample (only files inside the instrument folder can be saved) or the
/// instrument breaks a model invariant. Saving the same instrument twice gives the same
/// bytes.
pub fn instrument_to_toml(inst: &HausInstrument) -> FormatResult<String> {
    check(inst)?;
    let file = InstrumentFile {
        format: INSTRUMENT_FORMAT_NAME.into(),
        version: INSTRUMENT_FILE_VERSION,
        name: inst.name.clone(),
        settings: inst.settings.clone(),
        source: inst.source.clone(),
        extra: inst.extra.clone(),
        import_notes: inst.import_notes.clone(),
        articulations: inst.articulations.clone(),
        modulation: inst.modulation.clone(),
        group: inst.groups.values().cloned().collect(),
        zone: inst.zones.values().cloned().collect(),
    };
    toml::to_string(&file).map_err(|e| invalid(format!("instrument.toml: {e}")))
}

/// The model's invariants for a standalone instrument file, plus the file's own rules:
/// zone samples are relative paths inside the folder.
fn check(inst: &HausInstrument) -> FormatResult<()> {
    ph_core::validate_instrument_header(inst)?;
    for g in inst.groups.values() {
        ph_core::validate_group(g)?;
        if let Some(a) = g
            .articulations
            .iter()
            .find(|a| inst.articulation(**a).is_none())
        {
            return Err(invalid(format!(
                "group {} plays missing articulation {a}",
                g.id
            )));
        }
    }
    for z in inst.zones.values() {
        ph_core::validate_zone(z)?;
        if !inst.groups.contains_key(&z.group) {
            return Err(invalid(format!(
                "zone {} is in missing group {}",
                z.id, z.group
            )));
        }
        match &z.sample {
            SampleRef::Registry(s) => {
                return Err(invalid(format!(
                    "zone {} plays project sample {s}; copy it into the instrument's samples/ first",
                    z.id
                )));
            }
            SampleRef::File(p) => check_relative(p)?,
        }
    }
    if let Some(r) = inst
        .modulation
        .iter()
        .find(|r| r.group.is_some_and(|g| !inst.groups.contains_key(&g)))
    {
        return Err(invalid(format!(
            "a modulation route names missing group {}",
            r.group.map_or(0, |g| g.0)
        )));
    }
    Ok(())
}

/// A sample path must stay inside the instrument folder: relative, forward slashes, no
/// `..`, no drive or root.
pub fn check_relative(p: &str) -> FormatResult<()> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.contains(':')
        || p.split('/').any(|c| c == ".." || c.is_empty());
    if bad {
        Err(invalid(format!(
            "sample path {p:?} must be relative to the instrument folder (forward slashes, no ..)"
        )))
    } else {
        Ok(())
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> FormatError + '_ {
    move |source| FormatError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Writes `instrument.toml` into the folder `dir` (created with `samples/` if missing),
/// through a temporary file, so a crash never leaves half a file. Copying the sample files
/// into `samples/` is the caller's job (the command layer, which knows the project).
pub fn save_instrument(inst: &HausInstrument, dir: &Path) -> FormatResult<PathBuf> {
    let text = instrument_to_toml(inst)?;
    let samples = dir.join(INSTRUMENT_SAMPLES_DIR);
    fs::create_dir_all(&samples).map_err(io(&samples))?;
    let target = dir.join(INSTRUMENT_FILE);
    let tmp = dir.join(format!("{INSTRUMENT_FILE}.tmp"));
    fs::write(&tmp, text).map_err(io(&tmp))?;
    fs::rename(&tmp, &target).map_err(io(&target))?;
    Ok(target)
}

/// Loads an instrument from its folder, or from the `instrument.toml` itself. The result
/// has id 0 and no `folder`: the caller sets the folder reference it will resolve samples
/// against (see `docs/formats/haus-instrument.md`).
pub fn load_instrument(path: &Path) -> FormatResult<HausInstrument> {
    let file = if path.is_dir() {
        path.join(INSTRUMENT_FILE)
    } else {
        path.to_path_buf()
    };
    let text = fs::read_to_string(&file).map_err(io(&file))?;
    let mut inst = instrument_from_toml(&text)?;
    inst.id = InstrumentId(0);
    Ok(inst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::*;

    /// An instrument using every part of the model.
    pub(crate) fn full() -> HausInstrument {
        let mut inst = HausInstrument::new("Strings \"Ensemble\"");
        inst.settings.volume_db = -3.0;
        inst.settings.preload_frames = Some(16_384);
        inst.settings.default_articulation = Some(ArticulationId(1));
        inst.source = Some(InstrumentSource {
            format: "sfz".into(),
            file: "strings.sfz".into(),
            importer_version: 1,
        });
        inst.import_notes.push(ImportNote {
            kind: ImportNoteKind::Approximated,
            what: "opcode:fil_type=lpf_6p".into(),
            detail: "6-pole low-pass played as 4-pole".into(),
            count: 3,
        });
        inst.extra
            .insert("ksp.script".into(), "on note\nend on".into());
        for (i, (name, key)) in [("legato", 24u8), ("staccato", 25)].iter().enumerate() {
            inst.articulations.push(Articulation {
                id: ArticulationId(i as u32),
                name: name.to_string(),
                trigger: ArticulationTrigger::Key { key: *key },
                color: None,
            });
        }
        inst.articulations.push(Articulation {
            id: ArticulationId(2),
            name: "tremolo".into(),
            trigger: ArticulationTrigger::Cc {
                controller: 32,
                lo: 0.5,
                hi: 1.0,
            },
            color: Some(0x3366ff),
        });
        let mut g = Group::new("Legato");
        g.id = GroupId(3);
        g.articulations = vec![ArticulationId(0)];
        g.polyphony = Polyphony::Legato;
        g.portamento = 0.08;
        g.filter = Some(Filter {
            kind: FilterKind::LowPass24,
            cutoff: 8000.0,
            resonance: 3.0,
            key_track: 50.0,
            env_depth: 1200.0,
        });
        g.filter_env = Some(Envelope {
            attack: 0.2,
            ..Default::default()
        });
        g.lfos.push(Lfo {
            shape: LfoShape::Triangle,
            rate: 5.5,
            delay: 0.3,
            fade: 0.5,
        });
        g.effects.push(GroupEffect {
            name: "eq".into(),
            params: [("low_gain".to_string(), -2.0)].into(),
            bypass: false,
            mix: 1.0,
        });
        g.output = 2;
        g.voice_limit = Some(16);
        g.stealing = StealMode::Quietest;
        g.extra.insert("dspreset.ui".into(), "<ui/>".into());
        inst.groups.insert(g.id, g);
        let mut s = Group::new("Staccato");
        s.id = GroupId(4);
        s.articulations = vec![ArticulationId(1)];
        s.round_robin = RoundRobinMode::Random;
        inst.groups.insert(s.id, s);
        let mut z = Zone::new(
            GroupId(3),
            SampleRef::File("samples/leg_C4.wav".into()),
            KeyRange { lo: 55, hi: 66 },
            60,
        );
        z.id = ZoneId(10);
        z.velocity = VelRange { lo: 1, hi: 90 };
        z.tune = -1;
        z.fine = 12.5;
        z.looping = Some(ZoneLoop {
            mode: LoopMode::Forward,
            start: 4000,
            end: 90_000,
            crossfade: 1024,
            on_release: LoopRelease::Exit,
        });
        z.cc_conditions.push(CcCondition {
            controller: 64,
            lo: 0.0,
            hi: 0.5,
        });
        z.crossfades.push(Crossfade {
            source: CrossfadeSource::Cc { controller: 1 },
            in_lo: 0.0,
            in_hi: 0.2,
            out_lo: 0.6,
            out_hi: 0.9,
            curve: CrossfadeCurve::Power,
        });
        inst.zones.insert(z.id, z);
        for rr in 0..3u32 {
            let mut z = Zone::new(
                GroupId(4),
                SampleRef::File(format!("samples/stac_C4_rr{rr}.wav")),
                KeyRange { lo: 55, hi: 66 },
                60,
            );
            z.id = ZoneId(20 + rr);
            z.round_robin = Some(rr);
            z.one_shot = true;
            z.reverse = rr == 2;
            inst.zones.insert(z.id, z);
        }
        let mut rel = Zone::new(
            GroupId(4),
            SampleRef::File("samples/rel.wav".into()),
            KeyRange::ALL,
            60,
        );
        rel.id = ZoneId(30);
        rel.trigger = ZoneTrigger::Release;
        rel.release_decay_db = 6.0;
        inst.zones.insert(rel.id, rel);
        inst.modulation.push(ModRoute {
            source: ModSource::Cc { controller: 1 },
            target: ModTarget::Volume,
            depth: 12.0,
            curve: ModCurve::Convex,
            group: Some(GroupId(3)),
        });
        inst.modulation.push(ModRoute {
            source: ModSource::Lfo { index: 0 },
            target: ModTarget::Pitch,
            depth: 15.0,
            curve: ModCurve::Linear,
            group: None,
        });
        inst
    }

    #[test]
    fn round_trips_every_field_and_is_byte_stable() {
        let inst = full();
        let text = instrument_to_toml(&inst).unwrap();
        if std::env::var("PH_PRINT_TOML").is_ok() {
            println!("{text}");
        }
        assert!(text.starts_with("format = \"patternhaus-instrument\"\nversion = 1\n"));
        let back = instrument_from_toml(&text).unwrap();
        assert_eq!(back, inst);
        assert_eq!(instrument_to_toml(&back).unwrap(), text);
    }

    #[test]
    fn folder_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_instrument(&full(), dir.path()).unwrap();
        assert!(path.ends_with(INSTRUMENT_FILE));
        assert!(dir.path().join(INSTRUMENT_SAMPLES_DIR).is_dir());
        assert!(
            !dir.path().join(INSTRUMENT_UI_DIR).exists(),
            "reserved, not made"
        );
        assert!(!dir.path().join("instrument.toml.tmp").exists());
        assert_eq!(load_instrument(dir.path()).unwrap(), full());
        assert_eq!(load_instrument(&path).unwrap(), full());
    }

    #[test]
    fn a_minimal_hand_written_file_loads_with_defaults() {
        let text = r#"
format = "patternhaus-instrument"
version = 1
name = "Hit"

[[group]]
id = 0
name = "Main"

[[zone]]
id = 0
group = 0
sample = "samples/hit.wav"
root_key = 60
"#;
        let inst = instrument_from_toml(text).unwrap();
        let z = inst.zone(ZoneId(0)).unwrap();
        assert_eq!(z.keys, KeyRange::ALL);
        assert_eq!(z.velocity, VelRange::ALL);
        assert_eq!(inst.settings, InstrumentSettings::default());
        assert_eq!(inst.group(GroupId(0)).unwrap().amp_env, Envelope::default());
    }

    #[test]
    fn bad_files_are_refused_with_reasons() {
        let ok = instrument_to_toml(&full()).unwrap();
        let cases = [
            (ok.replace("version = 1", "version = 2"), "newer"),
            (ok.replace("version = 1", "version = 0"), "not valid"),
            (
                ok.replace("patternhaus-instrument", "other"),
                "not a Patternhaus",
            ),
            (ok.replace("samples/rel.wav", "../rel.wav"), "relative"),
            (ok.replace("samples/rel.wav", "C:/x.wav"), "relative"),
            (
                ok.replacen("\n\n", "\nsurprise = 1\n\n", 1),
                "unknown field",
            ),
            ("not toml [".to_string(), "instrument.toml"),
            (String::new(), "format"),
        ];
        for (text, why) in cases {
            let err = instrument_from_toml(&text).unwrap_err().to_string();
            assert!(err.contains(why), "{why}: {err}");
        }
        // a registry sample cannot be saved
        let mut inst = full();
        inst.zones.get_mut(&ZoneId(10)).unwrap().sample = SampleRef::Registry(SampleId(1));
        assert!(instrument_to_toml(&inst).is_err());
    }

    /// The example in `docs/formats/haus-instrument.md` loads.
    #[test]
    fn the_documented_example_loads() {
        let doc = include_str!("../../../docs/formats/haus-instrument.md");
        let start = doc.find("```toml\n").expect("example") + 8;
        let end = start + doc[start..].find("```").expect("end of example");
        let inst = instrument_from_toml(&doc[start..end]).unwrap();
        assert_eq!(inst.name, "Felt Piano");
        assert_eq!(inst.zones.len(), 2);
        assert_eq!(inst.group(GroupId(0)).unwrap().amp_env.release, 0.6);
        assert_eq!(
            inst.zone(ZoneId(1))
                .unwrap()
                .looping
                .as_ref()
                .unwrap()
                .crossfade,
            2048
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(300))]

        /// Arbitrary bytes and damaged files never panic the loader.
        #[test]
        fn loader_never_panics(
            bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..400),
            cut in 0usize..2000,
        ) {
            let _ = instrument_from_toml(&String::from_utf8_lossy(&bytes));
            let ok = instrument_to_toml(&full()).unwrap();
            let _ = instrument_from_toml(&ok[..cut.min(ok.len())]);
        }
    }
}
