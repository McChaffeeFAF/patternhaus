//! One behaviour test per area, covering every built-in command, plus the guards that keep
//! the command set honest (every command is exercised here, scopes match flags).

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// A host with a real folder to read and write, a recording UI and a fake renderer.
struct FakeHost {
    root: PathBuf,
    inner: NullHost,
    ui: Vec<UiRequest>,
    renders: Vec<RenderRequest>,
}

impl FakeHost {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            inner: NullHost::default(),
            ui: Vec::new(),
            renders: Vec::new(),
        }
    }
}

impl Host for FakeHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        self.inner.transport(cmd)
    }
    fn transport_state(&self) -> TransportState {
        self.inner.transport_state()
    }
    fn render(&mut self, _: &Project, request: &RenderRequest) -> CmdResult<RenderReport> {
        self.renders.push(request.clone());
        Ok(RenderReport {
            path: request.path.clone().into(),
            seconds: 1.5,
            peak_dbfs: -6.0,
        })
    }
    fn resolve_path(&self, path: &str, _: FileAccess) -> CmdResult<PathBuf> {
        let p = Path::new(path);
        if p.is_absolute() || p.components().any(|c| c == Component::ParentDir) {
            return Err(CmdError::Host(format!("{path} is outside the project")));
        }
        Ok(self.root.join(p))
    }
    fn ui(&mut self, request: UiRequest) -> CmdResult<()> {
        self.ui.push(request);
        Ok(())
    }
}

struct Rig {
    s: Session,
    h: FakeHost,
    who: Caller,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("Test")),
            h: FakeHost::new(dir.path()),
            who: Caller::human(),
            _dir: dir,
        }
    }

    fn dir(&self) -> &Path {
        &self.h.root
    }

    fn p(&self) -> &Project {
        self.s.project()
    }

    fn run(&mut self, cmd: &str, params: Value) -> Executed {
        self.s
            .execute(&self.who, Request::new(cmd, params), &mut self.h)
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }

    fn confirmed(&mut self, cmd: &str, params: Value) -> Executed {
        let mut req = Request::new(cmd, params);
        req.confirm = true;
        self.s
            .execute(&self.who, req, &mut self.h)
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }

    fn err_confirmed(&mut self, cmd: &str, params: Value) -> CmdError {
        let mut req = Request::new(cmd, params);
        req.confirm = true;
        match self.s.execute(&self.who, req, &mut self.h) {
            Ok(_) => panic!("{cmd} should have failed"),
            Err(e) => e,
        }
    }

    fn err(&mut self, cmd: &str, params: Value) -> CmdError {
        match self
            .s
            .execute(&self.who, Request::new(cmd, params), &mut self.h)
        {
            Ok(_) => panic!("{cmd} should have failed"),
            Err(e) => e,
        }
    }

    fn id(&mut self, cmd: &str, params: Value, key: &str) -> u32 {
        self.run(cmd, params).result[key].as_u64().unwrap() as u32
    }

    fn sampler(&mut self, name: &str) -> u32 {
        self.id(
            "channel.add",
            json!({ "name": name, "instrument": { "kind": "sampler", "sample": "factory:kick", "root_key": 36 } }),
            "channel",
        )
    }

    fn synth(&mut self, name: &str) -> u32 {
        self.id(
            "channel.add",
            json!({ "name": name, "instrument": { "kind": "synth", "preset": null } }),
            "channel",
        )
    }

    fn pattern(&mut self) -> u32 {
        self.id("pattern.add", json!({}), "pattern")
    }

    fn notes(&mut self, pat: u32, ch: u32, specs: &[(u8, u64, u64)]) -> Vec<u32> {
        let notes: Vec<_> = specs
            .iter()
            .map(|(k, s, l)| json!({ "key": k, "start": s, "length": l }))
            .collect();
        let out = self.run(
            "note.add",
            json!({ "pattern": pat, "channel": ch, "notes": notes }),
        );
        out.result["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect()
    }

    fn note(&self, pat: u32, ch: u32, id: u32) -> Note {
        self.p()
            .pattern(PatternId(pat))
            .unwrap()
            .notes
            .get(&ChannelId(ch))
            .and_then(|m| m.get(&NoteId(id)))
            .unwrap_or_else(|| panic!("no note {id}"))
            .clone()
    }

    fn note_count(&self, pat: u32) -> usize {
        self.p().pattern(PatternId(pat)).unwrap().note_count()
    }
}

#[test]
fn project_commands() {
    let mut r = Rig::new();
    let pat = r.pattern();
    let ch = r.sampler("Kick");
    r.notes(pat, ch, &[(36, 0, 240)]);

    assert_eq!(
        r.run("project.get_settings", json!({})).result["bpm"],
        130.0
    );
    r.run("project.set_tempo", json!({ "bpm": 90 }));
    r.run(
        "project.set_time_signature",
        json!({ "numerator": 3, "denominator": 4 }),
    );
    r.run("project.set_swing", json!({ "swing": 0.4 }));
    r.run("project.rename", json!({ "title": "Demo" }));
    let s = &r.p().settings;
    assert_eq!(
        (s.bpm, s.time_signature.numerator, s.title.as_str()),
        (90.0, 3, "Demo")
    );
    assert!((s.swing - 0.4).abs() < 1e-6);
    assert!(matches!(
        r.err("project.set_swing", json!({ "swing": 3 })),
        CmdError::Model(_)
    ));
    assert!(matches!(
        r.err(
            "project.set_time_signature",
            json!({ "numerator": 4, "denominator": 5 })
        ),
        CmdError::Model(_)
    ));

    // New needs confirm, replaces everything, never reuses ids, and is undoable.
    let before = r.p().clone();
    assert!(matches!(
        r.err("project.new", json!({})),
        CmdError::NeedsConfirm(_)
    ));
    let out = r.confirmed("project.new", json!({ "title": "Fresh" }));
    let p = r.p();
    assert!(p.channels.is_empty() && p.clips.is_empty());
    assert_eq!(
        (p.settings.title.as_str(), p.settings.bpm),
        ("Fresh", 130.0)
    );
    assert_eq!(p.patterns.len(), 1);
    let new_pat = out.result["pattern"].as_u64().unwrap() as u32;
    assert!(new_pat > pat, "ids continue, never reused");
    assert_eq!(r.p().pattern(PatternId(new_pat)).unwrap().name, "Pattern 1");
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert_eq!(r.p().channels, before.channels);
    assert_eq!(r.p().patterns, before.patterns);
    assert_eq!(r.p().settings, before.settings);
}

#[test]
fn channel_commands() {
    let mut r = Rig::new();
    let pat = r.pattern();
    let kick = r.sampler("Kick");
    let lead = r.id(
        "channel.add",
        json!({ "name": "Lead", "instrument": { "kind": "synth", "preset": "pluck" }, "insert": 3 }),
        "channel",
    );
    assert_eq!(r.p().channel(ChannelId(lead)).unwrap().insert, InsertId(3));
    r.notes(pat, kick, &[(36, 0, 240), (36, 960, 240)]);

    r.run("channel.rename", json!({ "channel": kick, "name": "Boom" }));
    r.run(
        "channel.set_color",
        json!({ "channel": kick, "color": 0xff0000 }),
    );
    r.run(
        "channel.set_volume",
        json!({ "channel": kick, "volume": 0.5 }),
    );
    r.run("channel.set_pan", json!({ "channel": kick, "pan": -0.25 }));
    r.run(
        "channel.set_mute",
        json!({ "channel": kick, "muted": true }),
    );
    r.run(
        "channel.set_insert",
        json!({ "channel": kick, "insert": 2 }),
    );
    let c = r.p().channel(ChannelId(kick)).unwrap().clone();
    assert_eq!(
        (c.name.as_str(), c.color, c.volume, c.pan, c.muted, c.insert),
        ("Boom", 0xff0000, 0.5, -0.25, true, InsertId(2))
    );
    for bad in [
        (
            "channel.set_volume",
            json!({ "channel": kick, "volume": 1.5 }),
        ),
        ("channel.set_pan", json!({ "channel": kick, "pan": 2 })),
        (
            "channel.set_insert",
            json!({ "channel": kick, "insert": 99 }),
        ),
        (
            "channel.set_color",
            json!({ "channel": kick, "color": 0x1000000 }),
        ),
    ] {
        assert!(
            matches!(r.err(bad.0, bad.1), CmdError::Model(_)),
            "{}",
            bad.0
        );
    }

    // Solo is session state: no history entry, no model change.
    let history = r.s.history().len();
    let project = r.p().clone();
    let out = r.run("channel.solo", json!({ "channel": kick }));
    assert_eq!(out.result["solo"], true);
    assert!(r.s.solo().channels.contains(&ChannelId(kick)));
    r.run("channel.solo", json!({ "channel": lead, "solo": true }));
    assert_eq!(r.s.solo().channels.len(), 2);
    r.run("channel.solo", json!({ "channel": kick }));
    assert!(!r.s.solo().channels.contains(&ChannelId(kick)));
    assert_eq!((r.s.history().len(), r.p()), (history, &project));

    r.run(
        "channel.set_param",
        json!({ "channel": lead, "param": "cutoff", "value": 0.3 }),
    );
    match &r.p().channel(ChannelId(lead)).unwrap().instrument {
        InstrumentSpec::Synth { params, .. } => assert_eq!(params["cutoff"], 0.3),
        other => panic!("{other:?}"),
    }
    r.run(
        "channel.set_instrument",
        json!({ "channel": lead, "instrument": { "kind": "sampler", "sample": null, "root_key": 48 } }),
    );
    assert!(matches!(
        r.p().channel(ChannelId(lead)).unwrap().instrument,
        InstrumentSpec::Sampler { root_key: 48, .. }
    ));
    r.run(
        "channel.set_instrument",
        json!({ "channel": lead, "instrument": { "kind": "plugin", "format": "clap", "plugin_id": "x.y", "state": null } }),
    );
    assert!(matches!(
        r.err(
            "channel.set_param",
            json!({ "channel": lead, "param": "a", "value": 1 })
        ),
        CmdError::Model(_)
    ));

    // Clone lands next to the original and copies its notes.
    let copy = r.id("channel.clone", json!({ "channel": kick }), "channel");
    assert_eq!(r.p().channel_order[1], ChannelId(copy));
    let cc = r.p().channel(ChannelId(copy)).unwrap();
    assert_eq!(
        (cc.name.as_str(), cc.volume, cc.insert),
        ("Boom copy", 0.5, InsertId(2))
    );
    assert_eq!(r.note_count(pat), 4);

    r.run("channel.move", json!({ "channel": copy, "index": 0 }));
    assert_eq!(r.p().channel_order[0], ChannelId(copy));
    r.run("channel.move", json!({ "channel": copy, "index": 99 }));
    assert_eq!(*r.p().channel_order.last().unwrap(), ChannelId(copy));

    r.run("channel.remove", json!({ "channel": copy }));
    assert_eq!(r.note_count(pat), 2);
    assert!(r.p().channel(ChannelId(copy)).is_err());
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert_eq!(
        r.note_count(pat),
        4,
        "undo brings the channel and notes back"
    );
    assert!(matches!(
        r.err("channel.remove", json!({ "channel": 99 })),
        CmdError::Model(ModelError::NoChannel(_))
    ));
}

#[test]
fn pattern_commands() {
    let mut r = Rig::new();
    let a = r.pattern();
    let ch = r.sampler("Kick");
    r.notes(a, ch, &[(36, 0, 240), (36, 960, 240)]);
    r.run("pattern.rename", json!({ "pattern": a, "name": "Verse" }));
    r.run(
        "pattern.set_color",
        json!({ "pattern": a, "color": 0x00ff00 }),
    );
    let h = r.p().pattern(PatternId(a)).unwrap().header();
    assert_eq!((h.name.as_str(), h.color), ("Verse", 0x00ff00));

    let out = r.run("pattern.set_length", json!({ "pattern": a, "steps": 32 }));
    assert_eq!(out.result["length"], 32 * 240);
    let out = r.run(
        "pattern.set_length",
        json!({ "pattern": a, "length": 1920 }),
    );
    assert_eq!(out.result["steps"], 8);
    let out = r.run("pattern.set_length", json!({ "pattern": a }));
    assert_eq!(out.result["length"], 3840, "auto length is a whole bar");
    assert!(matches!(
        r.err(
            "pattern.set_length",
            json!({ "pattern": a, "length": 5, "steps": 1 })
        ),
        CmdError::Model(_)
    ));
    assert!(matches!(
        r.err("pattern.set_length", json!({ "pattern": a, "length": 0 })),
        CmdError::Model(_)
    ));

    let b = r.id("pattern.clone", json!({ "pattern": a }), "pattern");
    assert_eq!(
        r.p().pattern_order,
        Arc::new(vec![PatternId(a), PatternId(b)])
    );
    assert_eq!(r.p().pattern(PatternId(b)).unwrap().name, "Verse copy");
    assert_eq!(r.note_count(b), 2);

    r.run("pattern.move", json!({ "pattern": b, "index": 0 }));
    assert_eq!(r.p().pattern_order[0], PatternId(b));

    r.run(
        "pattern.clear_channel",
        json!({ "pattern": b, "channel": ch }),
    );
    assert_eq!(r.note_count(b), 0);
    assert_eq!(r.note_count(a), 2, "other patterns untouched");

    r.run(
        "playlist.add_clip",
        json!({ "track": 0, "pattern": a, "start": 0 }),
    );
    let out = r.run("pattern.remove", json!({ "pattern": a }));
    assert_eq!(
        (out.result["notes"].as_u64(), out.result["clips"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(r.p().clips.is_empty() && r.p().pattern(PatternId(a)).is_err());
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert_eq!((r.p().clips.len(), r.note_count(a)), (1, 2));
}

#[test]
fn muting_a_note_is_an_ordinary_undoable_update_and_survives_copies() {
    let mut r = Rig::new();
    let pat = r.pattern();
    let other = r.pattern();
    let ch = r.synth("Lead");
    let ids = r.notes(pat, ch, &[(60, 0, 240), (64, 480, 240)]);
    assert!(!r.note(pat, ch, ids[0]).muted);
    r.run(
        "note.update",
        json!({ "pattern": pat, "channel": ch, "updates": [{ "id": ids[0], "muted": true }] }),
    );
    assert!(r.note(pat, ch, ids[0]).muted);
    assert!(!r.note(pat, ch, ids[1]).muted, "only the named note");
    // other edits leave the flag alone
    r.run(
        "note.move",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[0]], "delta_ticks": 240 }),
    );
    assert!(r.note(pat, ch, ids[0]).muted);
    // copies and duplicated patterns keep it
    let out = r.run(
        "note.copy",
        json!({ "pattern": pat, "channel": ch, "to_pattern": other, "to_channel": ch, "ids": [ids[0]] }),
    );
    let copy = out.result["notes"][0].as_u64().unwrap() as u32;
    assert!(r.note(other, ch, copy).muted);
    r.run(
        "note.update",
        json!({ "pattern": pat, "channel": ch, "updates": [{ "id": ids[0], "muted": false }] }),
    );
    assert!(!r.note(pat, ch, ids[0]).muted);
}

#[test]
fn note_commands() {
    let mut r = Rig::new();
    let pat = r.pattern();
    let other = r.pattern();
    let ch = r.synth("Lead");
    let ch2 = r.synth("Pad");
    let ids = r.notes(pat, ch, &[(60, 0, 240), (64, 250, 240), (67, 960, 480)]);

    r.run(
        "note.update",
        json!({ "pattern": pat, "channel": ch, "updates": [
            { "id": ids[0], "key": 61, "velocity": 0.5 },
            { "id": ids[1], "length": 100, "pan": -0.5, "fine_pitch": 20.0 } ] }),
    );
    let (n0, n1) = (r.note(pat, ch, ids[0]), r.note(pat, ch, ids[1]));
    assert_eq!((n0.key, n0.velocity, n0.start), (61, 0.5, 0));
    assert_eq!(
        (n1.length, n1.pan, n1.fine_pitch, n1.key),
        (100, -0.5, 20.0, 64)
    );
    assert!(matches!(
        r.err(
            "note.update",
            json!({ "pattern": pat, "channel": ch, "updates": [{ "id": ids[0], "velocity": 2 }] })
        ),
        CmdError::Model(_)
    ));

    r.run(
        "note.move",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[2]], "delta_ticks": -480, "delta_keys": 5 }),
    );
    let n = r.note(pat, ch, ids[2]);
    assert_eq!((n.start, n.key), (480, 72));
    assert!(matches!(
        r.err(
            "note.move",
            json!({ "pattern": pat, "channel": ch, "ids": [ids[0]], "delta_ticks": -1 })
        ),
        CmdError::Model(_)
    ));
    assert_eq!(
        r.note(pat, ch, ids[0]).start,
        0,
        "failed move changes nothing"
    );

    r.run(
        "note.resize",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[0]], "length": 600 }),
    );
    r.run(
        "note.resize",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[1]], "delta": -1000 }),
    );
    assert_eq!(
        (
            r.note(pat, ch, ids[0]).length,
            r.note(pat, ch, ids[1]).length
        ),
        (600, 1)
    );
    assert!(matches!(
        r.err(
            "note.resize",
            json!({ "pattern": pat, "channel": ch, "ids": [ids[0]] })
        ),
        CmdError::Model(_)
    ));

    r.run(
        "note.set_velocity",
        json!({ "pattern": pat, "channel": ch, "ids": ids, "velocity": 0.9 }),
    );
    assert!(ids.iter().all(|i| r.note(pat, ch, *i).velocity == 0.9));

    r.run(
        "note.transpose",
        json!({ "pattern": pat, "channel": ch, "semitones": -12 }),
    );
    assert_eq!(r.note(pat, ch, ids[0]).key, 49);
    assert!(matches!(
        r.err(
            "note.transpose",
            json!({ "pattern": pat, "channel": ch, "ids": [ids[0]], "semitones": -60 })
        ),
        CmdError::Model(_)
    ));

    // Starts are 0, 250 and 480: quantize half-way to the step grid, then fully.
    r.run(
        "note.quantize",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[1]], "strength": 0.5 }),
    );
    assert_eq!(r.note(pat, ch, ids[1]).start, 245);
    r.run("note.quantize", json!({ "pattern": pat, "channel": ch }));
    let starts: Vec<_> = ids.iter().map(|i| r.note(pat, ch, *i).start).collect();
    assert_eq!(starts, vec![0, 240, 480]);
    r.run(
        "note.quantize",
        json!({ "pattern": pat, "channel": ch, "grid": 960 }),
    );
    let starts: Vec<_> = ids.iter().map(|i| r.note(pat, ch, *i).start).collect();
    assert_eq!(starts, vec![0, 0, 960], "halfway rounds up");

    // Copy: later in the same pattern, then into another pattern and channel.
    let out = r.run(
        "note.copy",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[0]], "offset": 1920 }),
    );
    let copy = out.result["notes"][0].as_u64().unwrap() as u32;
    assert_eq!(r.note(pat, ch, copy).start, 1920);
    let out = r.run(
        "note.copy",
        json!({ "pattern": pat, "channel": ch, "to_pattern": other, "to_channel": ch2 }),
    );
    assert_eq!(out.result["notes"].as_array().unwrap().len(), 4);
    assert_eq!(r.note_count(other), 4);
    assert!(matches!(
        r.err(
            "note.copy",
            json!({ "pattern": pat, "channel": ch, "offset": -5000 })
        ),
        CmdError::Model(_)
    ));

    let out = r.run(
        "note.select_range",
        json!({ "pattern": pat, "start": 1900, "end": 2000 }),
    );
    assert_eq!(out.result["count"], 1);
    let out = r.run("note.select_range", json!({ "pattern": pat, "channel": ch, "start": 0, "end": 4000, "low_key": 61, "high_key": 127 }));
    assert_eq!(out.result["count"], 0, "only key 60 and below remain");
    let out = r.run(
        "note.select_range",
        json!({ "pattern": pat, "start": 0, "end": 4000, "high_key": 59 }),
    );
    assert_eq!(out.result["count"], 3);

    r.run(
        "note.remove",
        json!({ "pattern": pat, "channel": ch, "ids": [ids[0], copy] }),
    );
    assert_eq!(r.note_count(pat), 2);
    assert!(matches!(
        r.err(
            "note.remove",
            json!({ "pattern": pat, "channel": ch, "ids": [9999] })
        ),
        CmdError::Model(ModelError::NoNote(_))
    ));
}

#[test]
fn step_commands() {
    let mut r = Rig::new();
    let pat = r.pattern();
    let kick = r.sampler("Kick");
    // A piano-roll note that must not be mistaken for a step.
    r.notes(pat, kick, &[(40, 0, 960)]);

    let out = r.run(
        "step.toggle",
        json!({ "pattern": pat, "channel": kick, "step": 4 }),
    );
    assert_eq!(out.result["on"], true);
    let id = r.p().pattern(PatternId(pat)).unwrap().notes[&ChannelId(kick)]
        .values()
        .find(|n| n.start == 960 && n.length == STEP)
        .unwrap()
        .id;
    assert_eq!(
        r.note(pat, kick, id.0).key,
        36,
        "defaults to the sampler's root key"
    );
    let out = r.run(
        "step.toggle",
        json!({ "pattern": pat, "channel": kick, "step": 4 }),
    );
    assert_eq!(out.result["on"], false);
    assert_eq!(r.note_count(pat), 1);

    let row = [true, false, false, true, false, false, true, false];
    r.run(
        "step.set",
        json!({ "pattern": pat, "channel": kick, "steps": row, "velocity": 0.6 }),
    );
    let out = r.run(
        "step.get_row",
        json!({ "pattern": pat, "channel": kick, "steps": 8 }),
    );
    assert_eq!(out.result["on"], json!(row));
    assert_eq!(out.result["velocities"][3], 0.6f32 as f64);
    assert_eq!(out.result["keys"][0], 36);
    assert_eq!(out.result["keys"][1], Value::Null);

    r.run(
        "step.set_velocity",
        json!({ "pattern": pat, "channel": kick, "step": 3, "velocity": 1.0 }),
    );
    r.run(
        "step.set_pitch",
        json!({ "pattern": pat, "channel": kick, "step": 3, "key": 41 }),
    );
    let out = r.run(
        "step.get_row",
        json!({ "pattern": pat, "channel": kick, "steps": 4 }),
    );
    assert_eq!(
        (
            out.result["velocities"][3].as_f64(),
            out.result["keys"][3].as_u64()
        ),
        (Some(1.0), Some(41))
    );
    assert!(matches!(
        r.err(
            "step.set_velocity",
            json!({ "pattern": pat, "channel": kick, "step": 1, "velocity": 1.0 })
        ),
        CmdError::Model(_)
    ));

    let off = vec![false; 8];
    // Turning steps off through `set` leaves the long piano-roll note alone.
    r.run(
        "step.set",
        json!({ "pattern": pat, "channel": kick, "steps": off }),
    );
    assert_eq!(r.note_count(pat), 1);
    let out = r.run("step.get_row", json!({ "pattern": pat, "channel": kick }));
    assert_eq!(
        out.result["steps"], 16,
        "defaults to the pattern length in steps"
    );
}

#[test]
fn playlist_commands() {
    let mut r = Rig::new();
    let pat = r.pattern();
    let ch = r.sampler("Kick");
    r.notes(pat, ch, &[(36, 0, 240)]);
    let clip = r.id(
        "playlist.add_clip",
        json!({ "track": 1, "pattern": pat, "start": 0 }),
        "clip",
    );
    assert_eq!(
        r.p().clip(ClipId(clip)).unwrap().length,
        3840,
        "defaults to the pattern length"
    );
    let long = r.id(
        "playlist.add_clip",
        json!({ "track": 2, "pattern": pat, "start": 100, "length": 960 }),
        "clip",
    );
    assert_eq!(r.p().clip(ClipId(long)).unwrap().length, 960);

    r.run(
        "playlist.move_clip",
        json!({ "clip": clip, "start": 7680, "track": 3 }),
    );
    let c = r.p().clip(ClipId(clip)).unwrap();
    assert_eq!((c.start, c.track), (7680, 3));
    r.run(
        "playlist.resize_clip",
        json!({ "clip": clip, "length": 1920, "offset": 480 }),
    );
    let c = r.p().clip(ClipId(clip)).unwrap();
    assert_eq!((c.length, c.offset), (1920, 480));
    assert!(matches!(
        r.err("playlist.resize_clip", json!({ "clip": clip, "length": 0 })),
        CmdError::Model(_)
    ));
    assert!(matches!(
        r.err("playlist.move_clip", json!({ "clip": clip, "track": 64 })),
        CmdError::Model(_)
    ));

    r.run(
        "playlist.set_track_mute",
        json!({ "track": 3, "muted": true }),
    );
    r.run(
        "playlist.rename_track",
        json!({ "track": 3, "name": "Drums" }),
    );
    let t = &r.p().tracks[3];
    assert_eq!((t.muted, t.name.as_str()), (true, "Drums"));

    let out = r.run(
        "playlist.fill",
        json!({ "track": 5, "pattern": pat, "start": 3840, "count": 4 }),
    );
    assert_eq!(out.result["clips"].as_array().unwrap().len(), 4);
    assert_eq!(out.result["end"], 3840 * 5);
    let on_track: Vec<_> = r
        .p()
        .clips
        .values()
        .filter(|c| c.track == 5)
        .map(|c| c.start)
        .collect();
    assert_eq!(on_track, vec![3840, 7680, 11520, 15360]);
    assert!(matches!(
        r.err(
            "playlist.fill",
            json!({ "track": 5, "pattern": pat, "start": 0, "count": 0 })
        ),
        CmdError::Model(_)
    ));

    r.run("playlist.remove_clip", json!({ "clip": long }));
    assert!(r.p().clip(ClipId(long)).is_err());
}

#[test]
fn mixer_commands() {
    let mut r = Rig::new();
    r.run("mixer.set_volume", json!({ "insert": 1, "volume": 0.5 }));
    r.run("mixer.set_pan", json!({ "insert": 1, "pan": 0.3 }));
    r.run("mixer.set_mute", json!({ "insert": 1, "muted": true }));
    r.run("mixer.rename", json!({ "insert": 1, "name": "Drums" }));
    let i = r.p().insert(InsertId(1)).unwrap();
    assert_eq!(
        (i.volume, i.pan, i.muted, i.name.as_str()),
        (0.5, 0.3, true, "Drums")
    );
    assert!(matches!(
        r.err("mixer.set_volume", json!({ "insert": 99, "volume": 0.5 })),
        CmdError::Model(ModelError::NoInsert(_))
    ));
    assert!(matches!(
        r.err("mixer.set_volume", json!({ "insert": 1, "volume": 5 })),
        CmdError::Model(_)
    ));

    let history = r.s.history().len();
    r.run("mixer.solo", json!({ "insert": 1 }));
    assert!(r.s.solo().inserts.contains(&1));
    r.run("mixer.solo", json!({ "insert": 1, "solo": false }));
    assert!(r.s.solo().inserts.is_empty());
    assert_eq!(r.s.history().len(), history, "solo is not history");

    // Effect chain.
    for name in ["eq", "compressor", "reverb"] {
        r.run(
            "mixer.add_effect",
            json!({ "insert": 1, "effect": { "kind": "builtin", "name": name } }),
        );
    }
    r.run("mixer.add_effect", json!({ "insert": 1, "effect": { "kind": "builtin", "name": "limiter" }, "index": 0, "mix": 0.5 }));
    let names = |r: &Rig| -> Vec<String> {
        r.p()
            .insert(InsertId(1))
            .unwrap()
            .effects
            .iter()
            .map(|e| match &e.effect {
                EffectSpec::Builtin { name, .. } => name.clone(),
                _ => "plugin".into(),
            })
            .collect()
    };
    assert_eq!(names(&r), ["limiter", "eq", "compressor", "reverb"]);
    r.run(
        "mixer.move_effect",
        json!({ "insert": 1, "from": 0, "to": 99 }),
    );
    assert_eq!(names(&r), ["eq", "compressor", "reverb", "limiter"]);
    r.run(
        "mixer.set_effect_param",
        json!({ "insert": 1, "index": 1, "param": "threshold", "value": -18.0 }),
    );
    r.run(
        "mixer.set_effect_bypass",
        json!({ "insert": 1, "index": 1, "bypass": true }),
    );
    r.run(
        "mixer.set_effect_mix",
        json!({ "insert": 1, "index": 1, "mix": 0.25 }),
    );
    let slot = &r.p().insert(InsertId(1)).unwrap().effects[1];
    assert!(slot.bypass && slot.mix == 0.25);
    match &slot.effect {
        EffectSpec::Builtin { params, .. } => assert_eq!(params["threshold"], -18.0),
        _ => panic!(),
    }
    assert!(matches!(
        r.err(
            "mixer.set_effect_mix",
            json!({ "insert": 1, "index": 9, "mix": 0.5 })
        ),
        CmdError::Model(_)
    ));
    r.run("mixer.remove_effect", json!({ "insert": 1, "index": 0 }));
    assert_eq!(names(&r), ["compressor", "reverb", "limiter"]);
    for _ in 0..7 {
        r.run(
            "mixer.add_effect",
            json!({ "insert": 2, "effect": { "kind": "builtin", "name": "delay" } }),
        );
    }
    for _ in 0..3 {
        r.run(
            "mixer.add_effect",
            json!({ "insert": 2, "effect": { "kind": "builtin", "name": "delay" } }),
        );
    }
    assert!(
        matches!(
            r.err(
                "mixer.add_effect",
                json!({ "insert": 2, "effect": { "kind": "builtin", "name": "delay" } })
            ),
            CmdError::Model(_)
        ),
        "ten slots per insert"
    );

    // Sends, sidechain, routing.
    r.run(
        "mixer.add_send",
        json!({ "from": 1, "to": 5, "level": 0.4 }),
    );
    r.run(
        "mixer.set_send_level",
        json!({ "from": 1, "to": 5, "level": 0.7 }),
    );
    assert_eq!(r.p().insert(InsertId(1)).unwrap().sends[0].level, 0.7);
    assert!(matches!(
        r.err(
            "mixer.add_send",
            json!({ "from": 1, "to": 5, "level": 0.4 })
        ),
        CmdError::Model(_)
    ));
    assert!(matches!(
        r.err(
            "mixer.add_send",
            json!({ "from": 1, "to": 1, "level": 0.4 })
        ),
        CmdError::Model(_)
    ));
    r.run("mixer.remove_send", json!({ "from": 1, "to": 5 }));
    assert!(r.p().insert(InsertId(1)).unwrap().sends.is_empty());
    assert!(matches!(
        r.err("mixer.remove_send", json!({ "from": 1, "to": 5 })),
        CmdError::Model(_)
    ));

    r.run("mixer.set_sidechain", json!({ "insert": 2, "source": 1 }));
    assert_eq!(
        r.p().insert(InsertId(2)).unwrap().sidechain_from,
        Some(InsertId(1))
    );
    r.run("mixer.set_sidechain", json!({ "insert": 2 }));
    assert_eq!(r.p().insert(InsertId(2)).unwrap().sidechain_from, None);
    // The target compressor follows the route: external when fed, internal when cleared.
    r.run(
        "mixer.add_effect",
        json!({ "insert": 4, "effect": { "kind": "builtin", "name": "compressor" } }),
    );
    let sc = |r: &Rig| -> Option<f64> {
        r.p()
            .insert(InsertId(4))
            .unwrap()
            .effects
            .iter()
            .find_map(|e| match &e.effect {
                EffectSpec::Builtin { name, params } if name == "compressor" => {
                    params.get("sidechain").copied()
                }
                _ => None,
            })
    };
    r.run("mixer.set_sidechain", json!({ "insert": 4, "source": 1 }));
    assert_eq!(sc(&r), Some(1.0));
    r.run("mixer.set_sidechain", json!({ "insert": 4 }));
    assert_eq!(sc(&r), Some(0.0));
    assert!(matches!(
        r.err("mixer.set_sidechain", json!({ "insert": 2, "source": 2 })),
        CmdError::Model(_)
    ));

    r.run(
        "mixer.set_route_to_master",
        json!({ "insert": 3, "route": false }),
    );
    assert!(!r.p().insert(InsertId(3)).unwrap().route_to_master);
    assert!(matches!(
        r.err(
            "mixer.set_route_to_master",
            json!({ "insert": 0, "route": false })
        ),
        CmdError::Model(_)
    ));
}

#[test]
fn transport_commands() {
    let mut r = Rig::new();
    assert!(
        r.run("transport.play", json!({})).result["playing"]
            .as_bool()
            .unwrap()
    );
    r.run("transport.stop", json!({}));
    assert!(!r.h.transport_state().playing);
    assert!(
        r.run("transport.toggle", json!({})).result["playing"]
            .as_bool()
            .unwrap()
    );
    r.run("transport.toggle", json!({}));
    r.run("transport.seek", json!({ "position": 1920 }));
    r.run("transport.set_mode", json!({ "mode": "song" }));
    r.run("transport.set_loop", json!({ "range": [0, 3840] }));
    r.run("transport.set_metronome", json!({ "on": true }));
    assert!(
        r.run("transport.record", json!({})).result["recording"]
            .as_bool()
            .unwrap()
    );
    let st = r.run("transport.get_state", json!({})).result;
    assert_eq!(st["position"], 1920);
    assert_eq!(st["mode"], "song");
    assert_eq!(st["loop_range"], json!([0, 3840]));
    assert_eq!(st["metronome"], true);
    r.run("transport.set_loop", json!({ "range": null }));
    assert_eq!(r.h.transport_state().loop_range, None);
    assert!(matches!(
        r.err("transport.set_loop", json!({ "range": [10, 10] })),
        CmdError::Model(_)
    ));
    assert!(
        r.s.history().is_empty(),
        "transport changes are not undoable edits"
    );
}

fn small_song(r: &mut Rig) -> (u32, u32) {
    let pat = r.pattern();
    let ch = r.synth("Lead");
    r.notes(pat, ch, &[(60, 0, 240), (64, 480, 480), (67, 960, 240)]);
    r.run(
        "playlist.add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    );
    (pat, ch)
}

#[test]
fn file_commands_save_open() {
    let mut r = Rig::new();
    small_song(&mut r);
    let out = r.run("file.save", json!({}));
    let saved = r.dir().join("project.json");
    assert!(saved.exists(), "{out:?}");
    let first = std::fs::read(&saved).unwrap();
    r.run("file.save", json!({}));
    assert_eq!(std::fs::read(&saved).unwrap(), first, "byte-stable");

    // save_as to a new folder needs no confirm; replacing a saved project needs overwrite.
    r.run("file.save_as", json!({ "path": "copy" }));
    assert!(r.dir().join("copy/project.json").exists());
    assert!(matches!(
        r.err("file.save_as", json!({ "path": "copy" })),
        CmdError::Other(_)
    ));
    r.run("file.save_as", json!({ "path": "copy", "overwrite": true }));
    assert!(matches!(
        r.err("file.save", json!({ "path": "../x" })),
        CmdError::Host(_)
    ));

    // Open replaces the project and the history; needs confirm.
    let saved_project = r.p().clone();
    r.run("project.set_tempo", json!({ "bpm": 77 }));
    r.run("channel.solo", json!({ "channel": 0 }));
    assert!(matches!(
        r.err("file.open", json!({ "path": "copy" })),
        CmdError::NeedsConfirm(_)
    ));
    assert!(!r.s.history().is_empty());
    r.confirmed("file.open", json!({ "path": "copy" }));
    assert_eq!(r.p(), &saved_project);
    assert!(r.s.history().is_empty() && r.s.solo().channels.is_empty());
    assert_eq!(
        r.s.undo(&Author::Human).unwrap(),
        None,
        "nothing left to undo"
    );
    // Editing after open works and gets fresh transaction numbers.
    let out = r.run("project.set_tempo", json!({ "bpm": 100 }));
    assert!(out.txn.is_some());
    assert!(matches!(
        r.err_confirmed("file.open", json!({ "path": "nope" })),
        CmdError::Host(_)
    ));
}

#[test]
fn file_commands_export() {
    let mut r = Rig::new();
    let (pat, _) = small_song(&mut r);
    // Audio export goes through Host::render; a new file needs no confirm.
    let out = r.run(
        "file.export_audio",
        json!({ "path": "out/mix.wav", "range": [0, 3840], "sample_rate": 44100, "bit_depth": 16 }),
    );
    assert_eq!(out.result["peak_dbfs"], -6.0);
    let req = r.h.renders.last().unwrap().clone();
    assert!(req.path.ends_with("mix.wav"));
    assert_eq!(
        (req.range, req.sample_rate, req.bit_depth, req.high_quality),
        (Some((0, 3840)), 44100, 16, true)
    );
    assert!(matches!(
        r.err(
            "file.export_audio",
            json!({ "path": "x.wav", "range": [5, 1] })
        ),
        CmdError::Model(_)
    ));

    // MIDI: export the pattern, import it back as a second pattern.
    r.run("project.set_tempo", json!({ "bpm": 100 }));
    r.run(
        "file.export_midi",
        json!({ "path": "out/pat.mid", "pattern": pat }),
    );
    r.run("file.export_midi", json!({ "path": "out/song.mid" }));
    assert!(r.dir().join("out/song.mid").exists());
    // An existing file is never replaced silently.
    assert!(matches!(
        r.err("file.export_midi", json!({ "path": "out/song.mid" })),
        CmdError::Other(_)
    ));
    r.run(
        "file.export_midi",
        json!({ "path": "out/song.mid", "overwrite": true }),
    );
    r.run("project.set_tempo", json!({ "bpm": 120 }));
    let out = r.run(
        "file.import_midi",
        json!({ "path": "out/pat.mid", "apply_tempo": true }),
    );
    assert_eq!(out.result["notes"], 3);
    let new_pat = out.result["pattern"].as_u64().unwrap() as u32;
    assert_eq!(r.p().pattern(PatternId(new_pat)).unwrap().name, "pat");
    assert_eq!(r.note_count(new_pat), 3);
    assert_eq!(
        r.p().settings.bpm.round(),
        100.0,
        "apply_tempo took the file's tempo"
    );
    let out = r.run(
        "file.import_midi",
        json!({ "path": "out/song.mid", "pattern_name": "Again" }),
    );
    assert_eq!(
        r.p()
            .pattern(PatternId(out.result["pattern"].as_u64().unwrap() as u32))
            .unwrap()
            .name,
        "Again"
    );
    assert_eq!(
        r.p().settings.bpm.round(),
        100.0,
        "tempo untouched without apply_tempo"
    );
    std::fs::write(r.dir().join("bad.mid"), b"nope").unwrap();
    assert!(matches!(
        r.err("file.import_midi", json!({ "path": "bad.mid" })),
        CmdError::Host(_)
    ));
}

#[test]
fn file_import_flp_creates_a_project_beside_the_file() {
    let mut r = Rig::new();
    small_song(&mut r);
    let src = r.dir().join("song.flp");
    std::fs::write(&src, ph_formats::flp::build::sample_project()).unwrap();
    let params = json!({ "path": src.to_string_lossy() });
    // destructive like file.open: needs confirm
    assert!(matches!(
        r.err("file.import_flp", params.clone()),
        CmdError::NeedsConfirm(_)
    ));
    assert!(!r.s.history().is_empty(), "nothing happened yet");
    let out = r.confirmed("file.import_flp", params.clone());
    let dir = r.dir().join("song (Patternhaus)");
    assert_eq!(out.result["path"], json!(dir));
    assert_eq!(out.result["report"]["counts"]["notes"], 3);
    assert_eq!(out.result["report"]["counts"]["clips"], 4);
    assert!(out.summary.unwrap().contains("3 notes"));
    // the session now holds the imported project, with a fresh history
    assert_eq!(r.p().settings.title, "Synthetic song");
    assert_eq!(r.p().channels.len(), 3);
    assert!(r.s.history().is_empty());
    // and the folder holds the project, the plugin states and the report
    assert_eq!(ph_formats::load_project(&dir).unwrap(), *r.p());
    assert!(dir.join("plugins/flp-channel-1.flpstate").is_file());
    let report = std::fs::read_to_string(dir.join("import-report.md")).unwrap();
    assert!(report.contains("## Skipped"));
    // importing again never overwrites: a numbered folder
    let again = r.confirmed("file.import_flp", params);
    assert_eq!(
        again.result["path"],
        json!(r.dir().join("song (Patternhaus) (2)"))
    );
    // not an FL file, and a missing file, are errors that change nothing
    std::fs::write(r.dir().join("bad.flp"), b"not a project at all").unwrap();
    let before = r.p().clone();
    for name in ["bad.flp", "missing.flp"] {
        let path = r.dir().join(name);
        assert!(matches!(
            r.err_confirmed("file.import_flp", json!({ "path": path.to_string_lossy() })),
            CmdError::Host(_)
        ));
    }
    assert_eq!(r.p(), &before);
    assert!(!r.dir().join("bad (Patternhaus)").exists());
}

#[test]
fn ui_commands() {
    let mut r = Rig::new();
    let (pat, ch) = small_song(&mut r);
    r.run("ui.open_panel", json!({ "panel": "mixer" }));
    r.run("ui.close_panel", json!({ "panel": "mixer" }));
    r.run("ui.select", json!({ "paths": ["channel/0", "pattern/0"] }));
    r.run(
        "ui.show_piano_roll",
        json!({ "pattern": pat, "channel": ch }),
    );
    r.run("ui.notify", json!({ "message": "hello" }));
    assert_eq!(r.h.ui.len(), 5);
    assert_eq!(
        r.h.ui[0],
        UiRequest::Open {
            panel: "mixer".into()
        }
    );
    assert_eq!(
        r.h.ui[4],
        UiRequest::Notify {
            message: "hello".into()
        }
    );
    assert!(matches!(
        r.err("ui.open_panel", json!({ "panel": "nope" })),
        CmdError::BadParams { .. }
    ));
    assert!(matches!(
        r.err("ui.select", json!({ "paths": ["channel/77"] })),
        CmdError::Other(_)
    ));
    assert!(matches!(
        r.err(
            "ui.show_piano_roll",
            json!({ "pattern": 99, "channel": ch })
        ),
        CmdError::Model(_)
    ));
    assert_eq!(r.h.ui.len(), 5, "failed requests reach no UI");
}

#[test]
fn ui_panels_accept_instrument_and_effect_windows() {
    let mut r = Rig::new();
    let (_pat, ch) = small_song(&mut r);
    r.run("mixer.add_effect", json!({ "insert": 1, "effect": { "kind": "builtin", "name": "limiter" }, "index": 0, "mix": 0.5 }));
    r.run(
        "ui.open_panel",
        json!({ "panel": format!("instrument:{ch}") }),
    );
    r.run("ui.open_panel", json!({ "panel": "effect:1:0" }));
    r.run("ui.close_panel", json!({ "panel": "effect:1:0" }));
    r.run("ui.open_panel", json!({ "panel": "agents" }));
    assert_eq!(r.h.ui.len(), 4);
    for bad in [
        "instrument:999",
        "instrument:x",
        "effect:1:5",
        "effect:1",
        "effect:99:0",
        "nope",
    ] {
        assert!(
            matches!(
                r.err("ui.open_panel", json!({ "panel": bad })),
                CmdError::BadParams { .. }
            ),
            "{bad}"
        );
    }
    assert_eq!(r.h.ui.len(), 4);
}

#[test]
fn query_commands() {
    let mut r = Rig::new();
    let (pat, ch) = small_song(&mut r);
    let q = |r: &mut Rig, c: &str, p: Value| r.run(c, p).result;
    let all = q(&mut r, "query.project", json!({}));
    assert_eq!(all["settings"]["title"], "Test");
    assert_eq!(all["format_version"], FORMAT_VERSION);
    let chs = q(&mut r, "query.channels", json!({}));
    assert_eq!(chs["channels"][0]["name"], "Lead");
    let pats = q(&mut r, "query.patterns", json!({}));
    assert_eq!(pats["patterns"][0]["notes"], 3);
    assert_eq!(pats["patterns"][0]["effective_length"], 3840);
    let notes = q(&mut r, "query.pattern_notes", json!({ "pattern": pat }));
    assert_eq!(
        notes["channels"][ch.to_string()].as_array().unwrap().len(),
        3
    );
    let none = q(
        &mut r,
        "query.pattern_notes",
        json!({ "pattern": pat, "channel": 99 }),
    );
    assert!(none["channels"].as_object().unwrap().is_empty());
    assert_eq!(
        q(&mut r, "query.mixer", json!({}))["inserts"]
            .as_array()
            .unwrap()
            .len(),
        33
    );
    let pl = q(&mut r, "query.playlist", json!({}));
    assert_eq!(
        (
            pl["clips"].as_array().unwrap().len(),
            pl["song_length"].as_u64()
        ),
        (1, Some(3840))
    );
    let o = q(
        &mut r,
        "query.object",
        json!({ "path": format!("channel/{ch}") }),
    );
    assert_eq!(o["value"]["name"], "Lead");
    assert!(matches!(
        r.err("query.object", json!({ "path": "clip/55" })),
        CmdError::Other(_)
    ));
    assert!(matches!(
        r.err("query.object", json!({ "path": "bogus" })),
        CmdError::BadParams { .. }
    ));
    assert!(r.s.history().len() == 4, "queries record nothing");
}

#[test]
fn history_commands() {
    let mut r = Rig::new();
    let agent = Caller::agent("claude", Grant(vec![Scope::Compose]));
    let (pat, ch) = small_song(&mut r);
    let as_agent =
        |r: &mut Rig, c: &str, p: Value| r.s.execute(&agent, Request::new(c, p), &mut r.h).unwrap();
    as_agent(&mut r, "project.set_tempo", json!({ "bpm": 90 }));
    as_agent(
        &mut r,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": [{ "key": 50, "start": 0, "length": 240 }] }),
    );
    r.run("project.set_swing", json!({ "swing": 0.3 }));

    let list = r
        .run("history.list", json!({ "author": "agent:claude" }))
        .result;
    assert_eq!(list["entries"].as_array().unwrap().len(), 2);
    assert_eq!(list["entries"][1]["commands"], json!(["note.add"]));
    assert_eq!(list["entries"][1]["live"], true);
    let list = r.run("history.list", json!({ "limit": 2 })).result;
    assert_eq!(
        list["entries"].as_array().unwrap().len(),
        2,
        "limit keeps the most recent"
    );

    // The human's own undo takes the swing change, not the agent's work.
    let out = r.run("history.undo", json!({}));
    assert_eq!(out.result["reverted"], 1);
    assert_eq!(r.p().settings.swing, 0.0);
    assert_eq!(r.p().settings.bpm, 90.0);
    let out = r.run("history.redo", json!({}));
    assert_eq!(out.result["reverted"], 1);
    assert!((r.p().settings.swing - 0.3).abs() < 1e-6);

    // Agent scope is not enough to undo other people's work.
    let denied = r.s.execute(
        &agent,
        Request::new("history.undo_author", json!({ "author": "human" })),
        &mut r.h,
    );
    assert!(matches!(denied, Err(CmdError::Scope { .. })));

    // Undo all of the agent's work; the human's tempo-independent swing stays.
    let out = r.run("history.undo_author", json!({ "author": "agent:claude" }));
    assert_eq!(out.result["reverted"], 2);
    assert_eq!(r.p().settings.bpm, 130.0);
    assert_eq!(r.note_count(pat), 3);
    assert!((r.p().settings.swing - 0.3).abs() < 1e-6);
    assert!(out.txn.is_some() && !out.changed.is_empty());
    // The undo is the human's own and can be redone.
    r.run("history.redo", json!({}));
    assert_eq!(r.note_count(pat), 3);

    // Transaction by number. Undoing the channel's creation is refused by the model while
    // notes still use it, which is reported as a conflict on the channel.
    let txn = list_txn(&mut r, "channel.add");
    match r.err("history.undo_txn", json!({ "txn": txn })) {
        CmdError::UndoConflict(c) => assert!(
            c.iter()
                .any(|c| c.path == ObjectPath::Channel(ChannelId(ch)))
        ),
        other => panic!("{other:?}"),
    }
    assert!(r.p().channel(ChannelId(ch)).is_ok());
    assert!(matches!(
        r.err("history.undo_txn", json!({ "txn": 9999 })),
        CmdError::Other(_)
    ));
    assert!(matches!(
        r.err("history.undo", json!({ "mode": "sideways" })),
        CmdError::BadParams { .. }
    ));
    // Nothing of the human's own left to undo is not an error.
    let mut fresh = Rig::new();
    assert_eq!(fresh.run("history.undo", json!({})).result["reverted"], 0);
}

fn list_txn(r: &mut Rig, command: &str) -> u64 {
    r.run("history.list", json!({})).result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["commands"][0] == command)
        .unwrap()["txn"]
        .as_u64()
        .unwrap()
}

#[test]
fn undo_modes_through_commands() {
    let mut r = Rig::new();
    let agent = Caller::agent("claude", Grant(vec![Scope::Compose]));
    let (pat, ch) = small_song(&mut r);
    let n = r.p().pattern(PatternId(pat)).unwrap().notes[&ChannelId(ch)]
        .keys()
        .next()
        .unwrap()
        .0;
    r.s.execute(
        &agent,
        Request::new(
            "note.set_velocity",
            json!({ "pattern": pat, "channel": ch, "ids": [n], "velocity": 0.2 }),
        ),
        &mut r.h,
    )
    .unwrap();
    r.s.execute(
        &agent,
        Request::new(
            "note.transpose",
            json!({ "pattern": pat, "channel": ch, "ids": [n], "semitones": 2 }),
        ),
        &mut r.h,
    )
    .unwrap();
    let txn = list_txn(&mut r, "note.set_velocity");
    // The human changes the velocity afterwards: reverting the agent's velocity conflicts.
    r.run(
        "note.set_velocity",
        json!({ "pattern": pat, "channel": ch, "ids": [n], "velocity": 0.9 }),
    );

    match r.err("history.undo_txn", json!({ "txn": txn })) {
        CmdError::UndoConflict(c) => {
            assert_eq!(
                c[0].path,
                ObjectPath::Note(PatternId(pat), ChannelId(ch), NoteId(n))
            );
            assert_eq!(c[0].fields, vec!["/velocity"]);
            assert_eq!(c[0].changed_by, vec![Author::Human]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(r.note(pat, ch, n).velocity, 0.9, "stop changes nothing");

    let out = r.run("history.undo_txn", json!({ "txn": txn, "mode": "skip" }));
    assert_eq!(out.result["conflicts"][0]["fields"], json!(["/velocity"]));
    assert_eq!(
        r.note(pat, ch, n).velocity,
        0.9,
        "skip keeps the human's value"
    );

    let mut r2 = Rig::new();
    let (pat, ch) = small_song(&mut r2);
    r2.s.execute(
        &agent,
        Request::new(
            "note.set_velocity",
            json!({ "pattern": pat, "channel": ch, "ids": [0], "velocity": 0.2 }),
        ),
        &mut r2.h,
    )
    .unwrap();
    let txn = list_txn(&mut r2, "note.set_velocity");
    r2.run(
        "note.set_velocity",
        json!({ "pattern": pat, "channel": ch, "ids": [0], "velocity": 0.9 }),
    );
    r2.run("history.undo_txn", json!({ "txn": txn, "mode": "force" }));
    assert_eq!(
        r2.note(pat, ch, 0).velocity,
        0.78,
        "force restores the value from before"
    );
}

#[test]
fn every_command_has_a_behaviour_test() {
    let sources = [
        include_str!("behaviour.rs"),
        include_str!("tests.rs"),
        include_str!("m2_behaviour.rs"),
    ]
    .join("\n");
    let untested: Vec<_> = Registry::with_builtins()
        .list("")
        .iter()
        .filter(|s| !sources.contains(&format!("\"{}\"", s.id)))
        .map(|s| s.id)
        .collect();
    assert!(untested.is_empty(), "no test mentions {untested:?}");
}

#[test]
fn registry_is_consistent() {
    let reg = Registry::with_builtins();
    assert!(
        reg.len() >= 90,
        "the M1 set has about 90 commands, found {}",
        reg.len()
    );
    for s in reg.list("") {
        assert!(!s.summary.is_empty() && s.summary.len() < 200, "{}", s.id);
        assert!(
            s.summary.ends_with('.'),
            "{} summary should be a sentence",
            s.id
        );
        if s.scope == Scope::Read {
            assert!(!s.mutating, "{} is Read scope but mutating", s.id);
        }
        if s.mutating {
            assert_ne!(s.scope, Scope::Read, "{}", s.id);
        }
        // Every parameter has a description for agents to read.
        if let Some(props) = s.params["properties"].as_object() {
            for (name, p) in props {
                assert!(
                    p.get("description").is_some(),
                    "{}.{name} has no description",
                    s.id
                );
            }
        }
    }
    for id in crate::session_cmd::session_command_ids() {
        assert!(
            reg.get(id).is_ok(),
            "{id} is session-handled but not registered"
        );
    }
}

#[test]
fn schemas_name_every_required_field() {
    // Spot check that schemas carry what an agent needs to call a command correctly.
    let reg = Registry::with_builtins();
    let spec = reg.get("note.move").unwrap().spec().clone();
    let required: Vec<_> = spec.params["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let mut required = required;
    required.sort();
    assert_eq!(required, ["channel", "ids", "pattern"]);
    assert!(
        spec.params["properties"]["delta_ticks"]["description"]
            .as_str()
            .unwrap()
            .contains("ticks")
    );
}

#[test]
fn sample_import_needs_a_host_that_can_import() {
    let mut r = Rig::new();
    // the default Host::import_file refuses, so nothing is created
    let e = r.err("sample.import", json!({ "paths": ["a.wav"] }));
    assert!(matches!(e, CmdError::Host(m) if m.contains("cannot import")));
    assert!(r.p().channels.is_empty());
    assert!(matches!(
        r.err("sample.import", json!({ "paths": [] })),
        CmdError::Model(_)
    ));
}
