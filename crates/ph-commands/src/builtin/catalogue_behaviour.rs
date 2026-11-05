//! Behaviour tests for `catalogue.*` (M3/T9) against a host that pretends to host one plugin
//! synth and keeps project files in memory. The real thing (test CLAP/VST3 plugins, the
//! engine, offline probe renders) is tested in `ph-control` (`catalogue_tests.rs`).

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

struct CatHost {
    inner: NullHost,
    dir: Option<PathBuf>,
    /// version the "installed" synth reports
    version: String,
    files: BTreeMap<String, Vec<u8>>,
    probes: Vec<ProbeRequest>,
}

impl Host for CatHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        self.inner.transport(cmd)
    }
    fn transport_state(&self) -> TransportState {
        self.inner.transport_state()
    }
    fn render(&mut self, p: &Project, r: &RenderRequest) -> CmdResult<RenderReport> {
        self.inner.render(p, r)
    }
    fn resolve_path(&self, p: &str, a: FileAccess) -> CmdResult<PathBuf> {
        self.inner.resolve_path(p, a)
    }
    fn ui(&mut self, r: UiRequest) -> CmdResult<()> {
        self.inner.ui(r)
    }
    fn catalogue_dir(&self) -> Option<PathBuf> {
        self.dir.clone()
    }
    fn plugin_list(&self) -> CmdResult<Vec<PluginEntry>> {
        Ok(vec![PluginEntry {
            format: PluginFormat::Clap,
            id: "t.synth".into(),
            name: "Test Synth".into(),
            vendor: "Test".into(),
            version: self.version.clone(),
            kind: "instrument".into(),
            has_editor: true,
            status: "ok".into(),
            reason: String::new(),
            path: "/plugins/test.clap".into(),
        }])
    }
    fn plugin_state(&self, project: &Project, target: PluginTarget) -> CmdResult<CapturedState> {
        let PluginTarget::Channel(c) = target else {
            unreachable!()
        };
        let InstrumentSpec::Plugin { plugin_id, .. } = &project.channel(c)?.instrument else {
            unreachable!()
        };
        Ok(CapturedState {
            format: PluginFormat::Clap,
            plugin_id: plugin_id.clone(),
            name: "Test Synth".into(),
            vendor: "Test".into(),
            version: "1.0".into(),
            file: format!("STATE of channel {}", c.0).into_bytes(),
        })
    }
    fn write_project_file(&mut self, reference: &str, bytes: &[u8]) -> CmdResult<()> {
        self.files.insert(reference.into(), bytes.to_vec());
        Ok(())
    }
    /// Keys 48..=72 sound; 36 is a silent keyswitch that changes the test note, 37 a silent
    /// key that does not, 40 sounds itself.
    fn plugin_probe(&mut self, r: &ProbeRequest) -> CmdResult<ProbeReport> {
        self.probes.push(r.clone());
        let level = |key: u8, velocity: f32, sounds: bool, centroid: f64| ProbeLevel {
            key,
            velocity,
            rms_db: sounds.then_some(-20.0),
            peak_db: sounds.then_some(-12.0),
            centroid_hz: sounds.then_some(centroid),
            sounds,
        };
        let sweep: Vec<ProbeLevel> = r
            .velocities
            .iter()
            .flat_map(|v| r.keys.iter().map(move |k| (*k, *v)))
            .map(|(k, v)| level(k, v, (48..=72).contains(&k), 440.0))
            .collect();
        let sounding: Vec<u8> = r
            .keys
            .iter()
            .copied()
            .filter(|k| (48..=72).contains(k))
            .collect();
        let range = sounding.first().zip(sounding.last()).map(|(a, b)| [*a, *b]);
        Ok(ProbeReport {
            sweep,
            range,
            sounding,
            baseline: Some(level(r.test_key, 0.8, true, 440.0)),
            keyswitches: r
                .keyswitches
                .iter()
                .map(|k| KeyswitchProbe {
                    key: *k,
                    sounds: *k == 40,
                    after: level(r.test_key, 0.8, true, if *k == 36 { 900.0 } else { 440.0 }),
                    changes_sound: *k == 36,
                })
                .collect(),
            seconds: 1.0,
            warnings: vec![],
        })
    }
}

struct Rig {
    s: Session,
    h: CatHost,
    _tmp: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("P")),
            h: CatHost {
                inner: NullHost::default(),
                dir: Some(tmp.path().join("catalogue")),
                version: "1.0".into(),
                files: BTreeMap::new(),
                probes: Vec::new(),
            },
            _tmp: tmp,
        }
    }
    fn try_run(&mut self, cmd: &str, params: Value) -> CmdResult<Executed> {
        let req = Request {
            confirm: true,
            ..Request::new(cmd, params)
        };
        self.s.execute(&Caller::human(), req, &mut self.h)
    }
    fn run(&mut self, cmd: &str, params: Value) -> Value {
        self.try_run(cmd, params)
            .unwrap_or_else(|e| panic!("{cmd}: {e}"))
            .result
    }
    fn err(&mut self, cmd: &str, params: Value) -> String {
        match self.try_run(cmd, params) {
            Ok(_) => panic!("{cmd} should fail"),
            Err(e) => e.to_string(),
        }
    }
    /// A channel playing the fake synth.
    fn synth_channel(&mut self) -> u64 {
        let mut ed = Edit::new(self.s.project());
        let id = ed
            .add_channel(
                "Strings",
                InstrumentSpec::Plugin {
                    format: PluginFormat::Clap,
                    plugin_id: "t.synth".into(),
                    state: Some("plugins/test-synth.state".into()),
                    params: Default::default(),
                },
            )
            .unwrap();
        let (p, _) = ed.finish();
        self.s = Session::new(Arc::new(Registry::with_builtins()), p);
        id.0 as u64
    }
}

#[test]
fn catalogue_save_list_get_search_rename_remove() {
    let mut r = Rig::new();
    let ch = r.synth_channel();
    let saved = r
        .try_run(
            "catalogue.save",
            json!({ "channel": ch, "tags": ["strings", "warm"], "description": "soft section",
                    "guide": { "range": [36, 84], "legato": true,
                               "keyswitches": [{ "key": 24, "articulation": "sustain" }],
                               "ccs": [{ "cc": 1, "meaning": "dynamics" }] } }),
        )
        .unwrap();
    assert_eq!(saved.result["entry"], "strings");
    assert_eq!(
        saved.summary.as_deref(),
        Some("saved Strings to the catalogue as strings")
    );
    assert!(
        saved.changed.is_empty() && saved.txn.is_none(),
        "the project does not change"
    );
    let dir = r.h.dir.clone().unwrap();
    assert_eq!(
        std::fs::read(dir.join("strings/plugin.state")).unwrap(),
        format!("STATE of channel {ch}").into_bytes(),
        "the state is stored as the host gave it"
    );
    // a second save of the same name gets its own id
    let second = r.run(
        "catalogue.save",
        json!({ "channel": ch, "name": "Brass Stabs", "tags": ["brass"] }),
    );
    assert_eq!(second["entry"], "brass-stabs");

    let list = r.run("catalogue.list", json!({}));
    let rows = list["entries"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["entry"], "strings");
    assert_eq!(rows[1]["status"], "ok");
    assert_eq!(rows[1]["range_text"], "C2-C6");
    assert_eq!(rows[1]["articulations"], json!(["sustain"]));

    let got = r.run("catalogue.get", json!({ "entry": "strings" }));
    assert_eq!(got["guide"]["range"], json!([36, 84]));
    assert_eq!(got["plugin"]["version"], "1.0");
    assert_eq!(
        got["guide_text"],
        "plays C2-C6 (36-84); legato (overlap notes); keyswitches C1 (24) sustain; CCs CC1 dynamics"
    );

    let found = r.run("catalogue.search", json!({ "text": "SOFT section" }));
    assert_eq!(found["entries"].as_array().unwrap().len(), 1);
    let found = r.run("catalogue.search", json!({ "tags": ["Brass"] }));
    assert_eq!(found["entries"][0]["entry"], "brass-stabs");
    let found = r.run(
        "catalogue.search",
        json!({ "text": "sustain", "tags": ["brass"] }),
    );
    assert!(found["entries"].as_array().unwrap().is_empty());

    let renamed = r.run(
        "catalogue.rename",
        json!({ "entry": "strings", "name": "Warm Strings" }),
    );
    assert_eq!(renamed["was"], "Strings");
    assert_eq!(
        r.run("catalogue.get", json!({ "entry": "strings" }))["name"],
        "Warm Strings",
        "the id stays"
    );
    assert!(
        r.err(
            "catalogue.rename",
            json!({ "entry": "strings", "name": " " })
        )
        .contains("empty")
    );

    // remove is destructive: it needs confirm
    let e =
        r.s.execute(
            &Caller::human(),
            Request::new("catalogue.remove", json!({ "entry": "brass-stabs" })),
            &mut r.h,
        )
        .unwrap_err();
    assert!(e.to_string().contains("confirm"), "{e}");
    r.run("catalogue.remove", json!({ "entry": "brass-stabs" }));
    assert!(!dir.join("brass-stabs").exists());
    assert!(
        r.err("catalogue.get", json!({ "entry": "brass-stabs" }))
            .contains("no catalogue entry")
    );
    assert!(
        r.err("catalogue.get", json!({ "entry": "../x" }))
            .contains("not a catalogue entry id")
    );
}

#[test]
fn catalogue_add_to_channel_copies_state_and_guide_into_the_project() {
    let mut r = Rig::new();
    let ch = r.synth_channel();
    r.run(
        "catalogue.save",
        json!({ "channel": ch, "guide": { "range": [48, 72] } }),
    );
    let out = r
        .try_run(
            "catalogue.add_to_channel",
            json!({ "entry": "strings", "name": "Pad" }),
        )
        .unwrap();
    let new = ChannelId(out.result["channel"].as_u64().unwrap() as u32);
    assert_eq!(out.result["state"], "plugins/strings.state");
    assert_eq!(out.result["guide_text"], "plays C3-C5 (48-72)");
    assert!(out.result.get("warnings").is_none());
    assert_eq!(
        out.summary.as_deref(),
        Some("added channel Pad playing Strings from the catalogue")
    );
    let c = r.s.project().channel(new).unwrap();
    assert_eq!(c.name, "Pad");
    assert_eq!(
        c.instrument,
        InstrumentSpec::Plugin {
            format: PluginFormat::Clap,
            plugin_id: "t.synth".into(),
            state: Some("plugins/strings.state".into()),
            params: Default::default(),
        }
    );
    assert_eq!(
        r.h.files["plugins/strings.state"],
        format!("STATE of channel {ch}").into_bytes()
    );
    let copy: crate::catalogue::ProjectGuide =
        toml::from_str(std::str::from_utf8(&r.h.files["plugins/strings.catalogue.toml"]).unwrap())
            .unwrap();
    assert_eq!(copy.entry, "strings");
    assert_eq!(copy.guide.range, Some([48, 72]));
    // a second copy gets its own state file
    let again = r.run("catalogue.add_to_channel", json!({ "entry": "strings" }));
    assert_eq!(again["state"], "plugins/strings-2.state");

    // a new plugin version: added anyway, with a plain warning
    r.h.version = "2.0".into();
    let out = r.run("catalogue.add_to_channel", json!({ "entry": "strings" }));
    assert!(
        out["warnings"][0]
            .as_str()
            .unwrap()
            .contains("saved with Test Synth 1.0, installed is 2.0"),
        "{out}"
    );
    assert_eq!(
        r.run("catalogue.list", json!({}))["entries"][0]["installed_version"],
        "2.0"
    );
    // a missing plugin: refused, plainly
    let dir = r.h.dir.clone().unwrap();
    let toml_path = dir.join("strings/entry.toml");
    let text = std::fs::read_to_string(&toml_path).unwrap();
    std::fs::write(&toml_path, text.replace("t.synth", "t.gone")).unwrap();
    assert_eq!(
        r.run("catalogue.list", json!({}))["entries"][0]["status"],
        "missing"
    );
    let e = r.err("catalogue.add_to_channel", json!({ "entry": "strings" }));
    assert!(e.contains("not installed, or not scanned yet"), "{e}");
}

#[test]
fn catalogue_update_guide_validates_and_replaces() {
    let mut r = Rig::new();
    let ch = r.synth_channel();
    r.run("catalogue.save", json!({ "channel": ch, "tags": ["a"] }));
    let out = r.run(
        "catalogue.update_guide",
        json!({ "entry": "strings", "tags": ["pad"], "guide": { "range": [40, 90], "mono": true,
                "velocity": "two layers", "ccs": [{ "cc": 11, "meaning": "expression" }] } }),
    );
    assert_eq!(out["guide"]["mono"], true);
    assert_eq!(out["tags"], json!(["pad"]));
    let got = r.run("catalogue.get", json!({ "entry": "strings" }));
    assert_eq!(got["guide"]["ccs"][0]["cc"], 11);
    assert!(
        r.err(
            "catalogue.update_guide",
            json!({ "entry": "strings", "guide": { "range": [90, 40] } })
        )
        .contains("low <= high")
    );
    assert!(
        r.err(
            "catalogue.update_guide",
            json!({ "entry": "strings", "guide": { "rnage": [1, 2] } })
        )
        .contains("rnage")
    );
    // a channel without a plugin cannot be saved; a host without a catalogue refuses
    let mut ed = Edit::new(r.s.project());
    let synth = ed
        .add_channel(
            "Haus",
            InstrumentSpec::Synth {
                preset: None,
                params: Default::default(),
            },
        )
        .unwrap();
    r.s = Session::new(Arc::new(Registry::with_builtins()), ed.finish().0);
    assert!(
        r.err("catalogue.save", json!({ "channel": synth.0 }))
            .contains("does not play a plugin")
    );
    r.h.dir = None;
    assert!(
        r.err("catalogue.list", json!({}))
            .contains("no plugin catalogue")
    );
}

#[test]
fn catalogue_probe_proposes_range_silent_keys_and_keyswitches() {
    let mut r = Rig::new();
    let ch = r.synth_channel();
    r.run(
        "catalogue.save",
        json!({ "channel": ch, "guide": { "range": [50, 70] } }),
    );
    let out = r
        .try_run(
            "catalogue.probe",
            json!({ "entry": "strings", "low": 30, "high": 80, "velocities": [64],
                    "keyswitches": [36, 37, 40] }),
        )
        .unwrap();
    assert_eq!(
        out.summary.as_deref(),
        Some("probed Strings: sounds on C3-C5 (48-72)")
    );
    let v = &out.result;
    assert_eq!(v["proposal"]["range"], json!([48, 72]));
    assert_eq!(v["proposal"]["silent_keys"], json!([[30, 47], [73, 80]]));
    assert_eq!(
        v["proposal"]["keyswitches"],
        json!([
            { "key": 36, "articulation": "articulation 1 (name it)", "changes_sound": true },
            { "key": 37, "articulation": "articulation 2 (name it)", "changes_sound": false },
        ])
    );
    assert_eq!(v["test_key"], 60, "the middle of the guide's range");
    assert!(
        v["report"]["sweep"].as_array().unwrap().is_empty(),
        "detail off"
    );
    let req = &r.h.probes[0];
    assert_eq!(req.keys.len(), 51);
    assert_eq!(req.velocities, vec![64.0 / 127.0]);
    assert_eq!(req.state, format!("STATE of channel {ch}").into_bytes());
    assert_eq!((req.note_seconds, req.gap_seconds), (0.3, 0.3));
    // by channel, with every note's measurement
    let v = r.run(
        "catalogue.probe",
        json!({ "channel": ch, "low": 47, "high": 49, "detail": true }),
    );
    assert_eq!(v["report"]["sweep"].as_array().unwrap().len(), 6);
    assert_eq!(v["proposal"]["range"], json!([48, 49]));
    assert!(
        r.err(
            "catalogue.probe",
            json!({ "entry": "strings", "velocities": [0] })
        )
        .contains("velocities")
    );
    assert!(
        r.err("catalogue.probe", json!({}))
            .contains("give `entry` or `channel`")
    );
    assert!(
        r.err(
            "catalogue.probe",
            json!({ "channel": ch, "gap_ms": 10000, "note_ms": 5000 })
        )
        .contains("narrow the keys")
    );
}
