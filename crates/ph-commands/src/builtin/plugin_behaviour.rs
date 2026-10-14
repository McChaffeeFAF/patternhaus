//! Behaviour tests for `plugin.*` (M2/T3) against a host that pretends to host three
//! plugins. The real hosting (CLAP, the engine, state files) is tested in `ph-plugin-host`,
//! `ph-engine` and `ph-control`.

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn entry(id: &str, name: &str, kind: &str, status: &str) -> PluginEntry {
    PluginEntry {
        format: PluginFormat::Clap,
        id: id.into(),
        name: name.into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        kind: kind.into(),
        has_editor: true,
        status: status.into(),
        reason: if status == "ok" {
            String::new()
        } else {
            "crashed".into()
        },
        path: "/plugins/test.clap".into(),
    }
}

/// Pretends to host `t.synth` (instrument), `t.gain` (effect) and `t.bad` (isolated).
#[derive(Default)]
struct PluginHostFake {
    inner: NullHost,
    scanned: Vec<Vec<PathBuf>>,
    editors: Vec<(PluginTarget, bool)>,
    saved: Vec<PathBuf>,
}

impl Host for PluginHostFake {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        self.inner.transport(cmd)
    }
    fn transport_state(&self) -> TransportState {
        self.inner.transport_state()
    }
    fn render(&mut self, p: &Project, r: &RenderRequest) -> CmdResult<RenderReport> {
        self.inner.render(p, r)
    }
    fn resolve_path(&self, p: &str, _: FileAccess) -> CmdResult<PathBuf> {
        Ok(PathBuf::from("/projects").join(p))
    }
    fn ui(&mut self, r: UiRequest) -> CmdResult<()> {
        self.inner.ui(r)
    }
    fn plugin_scan(&mut self, paths: &[PathBuf]) -> CmdResult<PluginScanResult> {
        self.scanned.push(paths.to_vec());
        Ok(PluginScanResult {
            plugins: self.plugin_list()?,
            failed: vec![PluginScanFailure {
                path: "/plugins/broken.clap".into(),
                error: "the plugin crashed the scan helper".into(),
            }],
        })
    }
    fn plugin_list(&self) -> CmdResult<Vec<PluginEntry>> {
        Ok(vec![
            entry("t.synth", "Test Synth", "instrument", "ok"),
            entry("t.gain", "Test Gain", "effect", "ok"),
            entry("t.bad", "Bad", "effect", "isolate"),
        ])
    }
    fn plugin_params(&self, project: &Project, target: PluginTarget) -> CmdResult<PluginParams> {
        let (id, values) = match target {
            PluginTarget::Channel(c) => match &project.channel(c)?.instrument {
                InstrumentSpec::Plugin {
                    plugin_id, params, ..
                } => (plugin_id.clone(), params.clone()),
                _ => unreachable!(),
            },
            PluginTarget::Effect { insert, slot } => {
                match &project.mixer[insert as usize].effects[slot as usize].effect {
                    EffectSpec::Plugin {
                        plugin_id, params, ..
                    } => (plugin_id.clone(), params.clone()),
                    _ => unreachable!(),
                }
            }
        };
        let param = |pid: &str, name: &str, max: f64, stepped: bool| PluginParam {
            id: pid.into(),
            name: name.into(),
            min: 0.0,
            max,
            default: 0.5,
            value: values.get(pid).copied().unwrap_or(0.5),
            text: None,
            stepped,
            choices: Vec::new(),
            automatable: true,
        };
        Ok(PluginParams {
            target: Some(target),
            format: PluginFormat::Clap,
            plugin_id: id,
            name: "Test".into(),
            live: false,
            latency: 0,
            has_editor: true,
            params: vec![
                param("0", "Gain", 1.0, false),
                param("1", "Wave", 3.0, true),
            ],
        })
    }
    fn save_plugin_states(&mut self, project: &Project, dir: &Path) -> CmdResult<Vec<String>> {
        self.saved.push(dir.to_path_buf());
        let _ = project;
        Ok(vec!["plugins/test-synth.state".into()])
    }
    fn plugin_editor(&mut self, target: PluginTarget, open: bool) -> CmdResult<bool> {
        self.editors.push((target, open));
        Ok(false)
    }
}

struct Rig {
    s: Session,
    h: PluginHostFake,
    who: Caller,
}

impl Rig {
    fn new() -> Self {
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("P")),
            h: PluginHostFake::default(),
            who: Caller::human(),
        }
    }
    fn p(&self) -> &Project {
        self.s.project()
    }
    fn try_run(&mut self, cmd: &str, params: Value) -> CmdResult<Executed> {
        self.s
            .execute(&self.who, Request::new(cmd, params), &mut self.h)
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
}

#[test]
fn plugin_scan_and_list_report_what_the_host_found() {
    let mut r = Rig::new();
    let out = r
        .try_run("plugin.scan", json!({ "paths": ["/my/plugins"] }))
        .unwrap();
    assert_eq!(
        out.summary.as_deref(),
        Some("found 3 plugins (1 file could not be read)")
    );
    assert_eq!(out.result["plugins"].as_array().unwrap().len(), 3);
    assert_eq!(out.result["failed"][0]["path"], "/plugins/broken.clap");
    assert_eq!(r.h.scanned, vec![vec![PathBuf::from("/my/plugins")]]);
    r.run("plugin.scan", json!({}));
    assert_eq!(
        r.h.scanned[1],
        Vec::<PathBuf>::new(),
        "empty = standard folders"
    );
    let list = r.run("plugin.list", json!({}));
    assert_eq!(list["plugins"][0]["id"], "t.synth");
    assert_eq!(list["plugins"][0]["kind"], "instrument");
    assert_eq!(list["plugins"][2]["status"], "isolate");
    // a host without plugin hosting refuses
    let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("x"));
    let e = s
        .execute(
            &Caller::human(),
            Request::new("plugin.list", json!({})),
            &mut NullHost::default(),
        )
        .unwrap_err();
    assert!(e.to_string().contains("cannot load plugins"), "{e}");
}

#[test]
fn plugin_add_instrument_and_effect_name_unique_state_files() {
    let mut r = Rig::new();
    let a = r.run(
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": "t.synth" }),
    );
    assert_eq!(a["state"], "plugins/test-synth.state");
    let ch = ChannelId(a["channel"].as_u64().unwrap() as u32);
    assert_eq!(r.p().channel(ch).unwrap().name, "Test Synth");
    assert_eq!(
        r.p().channel(ch).unwrap().instrument,
        InstrumentSpec::Plugin {
            format: PluginFormat::Clap,
            plugin_id: "t.synth".into(),
            state: Some("plugins/test-synth.state".into()),
            params: Default::default(),
        }
    );
    // a second one gets its own file; a custom name is kept
    let b = r.run(
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": "t.synth", "name": "Lead" }),
    );
    assert_eq!(b["state"], "plugins/test-synth-2.state");
    let lead = ChannelId(b["channel"].as_u64().unwrap() as u32);
    assert_eq!(r.p().channel(lead).unwrap().name, "Lead");
    // effects go to the end of an insert's chain
    r.run(
        "mixer.add_effect",
        json!({ "insert": 2, "effect": { "kind": "builtin", "name": "eq" } }),
    );
    let fx = r.run(
        "plugin.add_effect",
        json!({ "insert": 2, "format": "clap", "plugin": "t.gain" }),
    );
    assert_eq!(
        fx,
        json!({ "insert": 2, "slot": 1, "state": "plugins/test-gain.state" })
    );
    assert!(matches!(
        &r.p().mixer[2].effects[1].effect,
        EffectSpec::Plugin { plugin_id, .. } if plugin_id == "t.gain"
    ));
    // refusals: unknown, wrong kind, isolated, no such insert
    assert!(
        r.err(
            "plugin.add_instrument",
            json!({ "format": "clap", "plugin": "nope" })
        )
        .contains("plugin.scan")
    );
    assert!(
        r.err(
            "plugin.add_instrument",
            json!({ "format": "clap", "plugin": "t.gain" })
        )
        .contains("plugin.add_effect")
    );
    assert!(
        r.err(
            "plugin.add_effect",
            json!({ "insert": 1, "format": "clap", "plugin": "t.synth" })
        )
        .contains("plugin.add_instrument")
    );
    assert!(
        r.err(
            "plugin.add_effect",
            json!({ "insert": 1, "format": "clap", "plugin": "t.bad" })
        )
        .contains("isolate")
    );
    r.err(
        "plugin.add_effect",
        json!({ "insert": 999, "format": "clap", "plugin": "t.gain" }),
    );
    // undo removes the channel again
    let before = r.p().channels.len();
    r.s.undo(&Author::Human).unwrap();
    r.s.undo(&Author::Human).unwrap();
    assert!(r.p().channels.len() < before + 1);
}

#[test]
fn plugin_set_param_and_get_params_by_the_plugins_own_ids() {
    let mut r = Rig::new();
    let a = r.run(
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": "t.synth" }),
    );
    let ch = a["channel"].as_u64().unwrap();
    let out = r
        .try_run(
            "plugin.set_param",
            json!({ "channel": ch, "param": "0", "value": 0.25 }),
        )
        .unwrap();
    assert_eq!(out.result["value"], 0.25);
    assert_eq!(
        out.summary.as_deref(),
        Some("set Gain of Test (Test Synth) to 0.25")
    );
    // clamped, and stepped parameters round
    assert_eq!(
        r.run(
            "plugin.set_param",
            json!({ "channel": ch, "param": "0", "value": 9 })
        )["value"],
        1.0
    );
    assert_eq!(
        r.run(
            "plugin.set_param",
            json!({ "channel": ch, "param": "1", "value": 1.6 })
        )["value"],
        2.0
    );
    let InstrumentSpec::Plugin { params, .. } =
        &r.p().channel(ChannelId(ch as u32)).unwrap().instrument
    else {
        panic!()
    };
    assert_eq!(params.get("0"), Some(&1.0));
    assert_eq!(params.get("1"), Some(&2.0));
    let got = r.run("plugin.get_params", json!({ "channel": ch }));
    assert_eq!(got["plugin_id"], "t.synth");
    assert_eq!(got["params"][0]["value"], 1.0);
    assert_eq!(got["params"][1]["stepped"], true);
    // effects by insert and slot
    r.run(
        "plugin.add_effect",
        json!({ "insert": 0, "format": "clap", "plugin": "t.gain" }),
    );
    r.run(
        "plugin.set_param",
        json!({ "insert": 0, "slot": 0, "param": "0", "value": 0.75 }),
    );
    let got = r.run("plugin.get_params", json!({ "insert": 0, "slot": 0 }));
    assert_eq!(
        got["target"],
        json!({ "effect": { "insert": 0, "slot": 0 } })
    );
    assert_eq!(got["params"][0]["value"], 0.75);
    // refusals
    assert!(
        r.err(
            "plugin.set_param",
            json!({ "channel": ch, "param": "9", "value": 0 })
        )
        .contains("no parameter")
    );
    assert!(
        r.err("plugin.get_params", json!({}))
            .contains("give `channel`")
    );
    assert!(
        r.err("plugin.get_params", json!({ "insert": 0, "slot": 4 }))
            .contains("no effect slot")
    );
    let plain = r.run(
        "channel.add",
        json!({ "name": "Haus", "instrument": { "kind": "synth", "preset": null } }),
    );
    assert!(
        r.err("plugin.get_params", json!({ "channel": plain["channel"] }))
            .contains("does not play a plugin")
    );
    // channel.set_param points at plugin.set_param for plugin channels
    assert!(
        r.err(
            "channel.set_param",
            json!({ "channel": ch, "param": "0", "value": 1 })
        )
        .contains("plugin.set_param")
    );
}

#[test]
fn plugin_editors_are_asked_of_the_host_and_save_writes_states() {
    let mut r = Rig::new();
    let a = r.run(
        "plugin.add_instrument",
        json!({ "format": "clap", "plugin": "t.synth" }),
    );
    let ch = ChannelId(a["channel"].as_u64().unwrap() as u32);
    let open = r.run("plugin.open_editor", json!({ "channel": ch }));
    assert_eq!(open["open"], true);
    assert_eq!(open["ui"], false);
    r.run(
        "plugin.add_effect",
        json!({ "insert": 1, "format": "clap", "plugin": "t.gain" }),
    );
    r.run("plugin.close_editor", json!({ "insert": 1, "slot": 0 }));
    assert_eq!(
        r.h.editors,
        vec![
            (PluginTarget::Channel(ch), true),
            (PluginTarget::Effect { insert: 1, slot: 0 }, false)
        ]
    );
    // file.save asks the host to write the plugin states next to project.json
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new(Arc::new(Registry::with_builtins()), r.p().clone());
    let mut h = PluginHostFake::default();
    struct DirHost<'a> {
        h: &'a mut PluginHostFake,
        dir: PathBuf,
    }
    impl Host for DirHost<'_> {
        fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
            self.h.transport(cmd)
        }
        fn transport_state(&self) -> TransportState {
            self.h.transport_state()
        }
        fn render(&mut self, p: &Project, r: &RenderRequest) -> CmdResult<RenderReport> {
            self.h.render(p, r)
        }
        fn resolve_path(&self, _: &str, _: FileAccess) -> CmdResult<PathBuf> {
            Ok(self.dir.clone())
        }
        fn ui(&mut self, r: UiRequest) -> CmdResult<()> {
            self.h.ui(r)
        }
        fn save_plugin_states(&mut self, p: &Project, d: &Path) -> CmdResult<Vec<String>> {
            self.h.save_plugin_states(p, d)
        }
    }
    let mut dh = DirHost {
        h: &mut h,
        dir: dir.path().to_path_buf(),
    };
    let out = s
        .execute(
            &Caller::human(),
            Request::new("file.save", json!({})),
            &mut dh,
        )
        .unwrap();
    assert_eq!(
        out.result["plugin_states"],
        json!(["plugins/test-synth.state"])
    );
    assert_eq!(h.saved, vec![dir.path().to_path_buf()]);
    assert!(dir.path().join("project.json").is_file());
}
