//! Behaviour tests for the M3/T10 `cc.*` commands (controller lanes in patterns).

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::sync::Arc;

struct Rig {
    s: Session,
    h: NullHost,
    who: Caller,
    pat: u32,
    ch: u32,
}

impl Rig {
    fn new() -> Self {
        let mut r = Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("cc")),
            h: NullHost::default(),
            who: Caller::human(),
            pat: 0,
            ch: 0,
        };
        r.pat = r.run("pattern.add", json!({ "name": "Strings" }))["pattern"]
            .as_u64()
            .unwrap() as u32;
        r.ch = r.run(
            "channel.add",
            json!({ "name": "Strings", "instrument": { "kind": "synth" } }),
        )["channel"]
            .as_u64()
            .unwrap() as u32;
        r.run(
            "note.add",
            json!({ "pattern": r.pat, "channel": r.ch, "notes": [{ "key": 60, "start": 0, "length": 3840 }] }),
        );
        r
    }

    fn p(&self) -> &Project {
        self.s.project()
    }

    fn try_run(&mut self, cmd: &str, params: Value) -> CmdResult<Executed> {
        self.s
            .execute(&self.who, Request::new(cmd, params), &mut self.h)
    }

    fn run(&mut self, cmd: &str, params: Value) -> Value {
        match self.try_run(cmd, params) {
            Ok(o) => o.result,
            Err(e) => panic!("{cmd}: {e}"),
        }
    }

    fn summary(&mut self, cmd: &str, params: Value) -> String {
        self.try_run(cmd, params)
            .unwrap_or_else(|e| panic!("{cmd}: {e}"))
            .summary
            .unwrap_or_default()
    }

    fn err(&mut self, cmd: &str, params: Value) -> CmdError {
        match self.try_run(cmd, params) {
            Ok(_) => panic!("{cmd} should have failed"),
            Err(e) => e,
        }
    }

    fn lane(&self, c: Controller) -> CcLane {
        self.p()
            .pattern(PatternId(self.pat))
            .unwrap()
            .lane(ChannelId(self.ch), c)
    }

    fn base(&self, extra: Value) -> Value {
        let mut v = json!({ "pattern": self.pat, "channel": self.ch });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }
}

fn values(l: &CcLane) -> Vec<(Tick, f64)> {
    l.points.iter().map(|p| (p.tick, p.value)).collect()
}

#[test]
fn cc_add_points_writes_points_and_accepts_names_and_numbers() {
    let mut r = Rig::new();
    let a = r.base(json!({
        "controller": 1,
        "points": [{ "tick": 0, "value": 0.2 }, { "tick": 960, "value": 0.8, "curve": "hold" }],
    }));
    let out = r.run("cc.add_points", a);
    assert_eq!(out["points"].as_array().unwrap().len(), 2);
    assert_eq!(out["name"], "Mod Wheel");
    // the same lane by name, and by the label form
    let b =
        r.base(json!({ "controller": "Mod Wheel", "points": [{ "tick": 1920, "value": 0.4 }] }));
    r.run("cc.add_points", b);
    let c = r
        .base(json!({ "controller": "cc1 mod wheel", "points": [{ "tick": 2400, "value": 0.1 }] }));
    r.run("cc.add_points", c);
    let l = r.lane(Controller::Cc(1));
    assert_eq!(
        values(&l),
        vec![(0, 0.2), (960, 0.8), (1920, 0.4), (2400, 0.1)]
    );
    assert_eq!(l.points[1].curve, CurveKind::Hold);
    // pitch bend
    let d = r.base(json!({ "controller": "pitch bend", "points": [{ "tick": 0, "value": -0.5 }] }));
    r.run("cc.add_points", d);
    assert_eq!(r.lane(Controller::PitchBend).points.len(), 1);
    // out-of-range values and unknown controllers are refused, nothing is left half done
    let before = r.p().clone();
    let bad = r.base(json!({ "controller": 1, "points": [{ "tick": 0, "value": 0.5 }, { "tick": 5, "value": 1.5 }] }));
    assert!(matches!(r.err("cc.add_points", bad), CmdError::Model(_)));
    let bad = r.base(json!({ "controller": 120, "points": [{ "tick": 0, "value": 0.5 }] }));
    r.err("cc.add_points", bad);
    let bad = r.base(json!({ "controller": "nonsense", "points": [] }));
    r.err("cc.add_points", bad);
    assert_eq!(*r.p(), before);
    // pitch bend takes negative values, a CC does not
    let neg = r.base(json!({ "controller": 7, "points": [{ "tick": 0, "value": -0.5 }] }));
    r.err("cc.add_points", neg);
    // unknown fields are refused
    let typo = r.base(json!({ "controller": 1, "points": [{ "tick": 0, "vlaue": 0.5 }] }));
    r.err("cc.add_points", typo);
}

#[test]
fn cc_set_replaces_the_whole_lane_or_a_tick_range() {
    let mut r = Rig::new();
    let a = r.base(json!({
        "controller": 11,
        "points": [{ "tick": 0, "value": 0.1 }, { "tick": 480, "value": 0.2 }, { "tick": 960, "value": 0.3 }, { "tick": 1440, "value": 0.4 }],
    }));
    r.run("cc.add_points", a);
    // a range: only the points in [480, 1440) go
    let s = r.base(json!({ "controller": 11, "start": 480, "end": 1440, "points": [{ "tick": 700, "value": 0.9 }] }));
    let out = r.run("cc.set", s);
    assert_eq!(out["removed"], 2);
    assert_eq!(
        values(&r.lane(Controller::Cc(11))),
        vec![(0, 0.1), (700, 0.9), (1440, 0.4)]
    );
    // the whole lane
    let w = r.base(json!({ "controller": 11, "points": [{ "tick": 0, "value": 0.5 }] }));
    let sum = r.summary("cc.set", w);
    assert_eq!(values(&r.lane(Controller::Cc(11))), vec![(0, 0.5)]);
    assert!(
        sum.contains("CC11 Expression") && sum.contains("hold 0.5"),
        "{sum}"
    );
    // other lanes are untouched
    let o = r.base(json!({ "controller": 1, "points": [{ "tick": 0, "value": 0.3 }] }));
    r.run("cc.add_points", o);
    let w = r.base(json!({ "controller": 11, "points": [] }));
    r.run("cc.set", w);
    assert!(r.lane(Controller::Cc(11)).points.is_empty());
    assert_eq!(r.lane(Controller::Cc(1)).points.len(), 1);
    // a backwards range is refused
    let bad = r.base(json!({ "controller": 11, "start": 100, "end": 100, "points": [] }));
    r.err("cc.set", bad);
    // points past the pattern end are accepted but flagged
    let late = r.base(json!({ "controller": 11, "points": [{ "tick": 9999, "value": 0.5 }] }));
    let out = r.run("cc.set", late);
    assert!(out["warning"].as_str().unwrap().contains("will not play"));
}

#[test]
fn cc_remove_points_by_id_or_by_range() {
    let mut r = Rig::new();
    let a = r.base(json!({
        "controller": 1,
        "points": [{ "tick": 0, "value": 0.1 }, { "tick": 480, "value": 0.2 }, { "tick": 960, "value": 0.3 }],
    }));
    let ids = r.run("cc.add_points", a)["points"].clone();
    let rm = r.base(json!({ "ids": [ids[1]] }));
    r.run("cc.remove_points", rm);
    assert_eq!(
        values(&r.lane(Controller::Cc(1))),
        vec![(0, 0.1), (960, 0.3)]
    );
    let rm = r.base(json!({ "controller": 1, "start": 500 }));
    let out = r.run("cc.remove_points", rm);
    assert_eq!(out["removed"].as_array().unwrap().len(), 1);
    assert_eq!(values(&r.lane(Controller::Cc(1))), vec![(0, 0.1)]);
    // an unknown id is an error and changes nothing; so is giving neither selector
    let before = r.p().clone();
    let rm = r.base(json!({ "ids": [999] }));
    r.err("cc.remove_points", rm);
    let rm = r.base(json!({}));
    r.err("cc.remove_points", rm);
    let rm = r.base(json!({ "ids": [1], "controller": 1 }));
    r.err("cc.remove_points", rm);
    assert_eq!(*r.p(), before);
}

#[test]
fn cc_clear_one_lane_or_all_of_them() {
    let mut r = Rig::new();
    for c in [1, 11] {
        let a = r.base(json!({ "controller": c, "points": [{ "tick": 0, "value": 0.1 }, { "tick": 480, "value": 0.5 }] }));
        r.run("cc.add_points", a);
    }
    let a = r.base(json!({ "controller": "pitch bend", "points": [{ "tick": 0, "value": 0.1 }] }));
    r.run("cc.add_points", a);
    let c = r.base(json!({ "controller": 1 }));
    assert_eq!(r.run("cc.clear", c)["removed"], 2);
    assert!(r.lane(Controller::Cc(1)).points.is_empty());
    assert_eq!(r.lane(Controller::Cc(11)).points.len(), 2);
    let c = r.base(json!({}));
    let sum = r.summary("cc.clear", c);
    assert!(sum.contains("all controller lanes"), "{sum}");
    assert_eq!(r.p().pattern(PatternId(r.pat)).unwrap().cc_count(), 0);
}

#[test]
fn cc_list_reports_lanes_with_shape_and_range() {
    let mut r = Rig::new();
    let a = r.base(
        json!({ "controller": 1, "start": 0, "end": 3840, "from": 0.3, "to": 0.4, "peak": 0.9 }),
    );
    r.run("cc.shape", a);
    let a = r.base(json!({ "controller": 64, "points": [{ "tick": 0, "value": 1.0, "curve": "hold" }, { "tick": 960, "value": 0.0 }] }));
    r.run("cc.add_points", a);
    let l = r.run("cc.list", json!({ "pattern": r.pat }));
    assert_eq!(l["count"], 2);
    let lanes = l["lanes"].as_array().unwrap();
    assert_eq!(lanes[0]["label"], "CC1 Mod Wheel");
    assert_eq!(lanes[0]["shape"], "swell 0.3→0.9→0.4");
    assert_eq!(lanes[0]["channel_name"], "Strings");
    assert_eq!(lanes[0]["min"], 0.3);
    assert_eq!(lanes[0]["max"], 0.9);
    assert_eq!(lanes[0]["points"].as_array().unwrap().len(), 3);
    assert_eq!(lanes[1]["name"], "Sustain");
    // filters, and the compact form without points
    let l = r.run(
        "cc.list",
        r.base(json!({ "controller": "sustain", "points": false })),
    );
    assert_eq!(l["count"], 1);
    assert!(l["lanes"][0].get("points").is_none());
    r.err("cc.list", json!({ "pattern": r.pat, "channel": 99 }));
}

#[test]
fn cc_shape_writes_a_ramp_a_swell_and_a_hold_and_returns_the_points() {
    let mut r = Rig::new();
    // a ramp, smooth by default
    let a = r.base(json!({ "controller": 1, "start": 0, "end": 1920, "from": 0.2, "to": 0.9 }));
    let out = r.run("cc.shape", a);
    assert_eq!(out["shape"], "ramp 0.2→0.9");
    let pts = out["points"].as_array().unwrap();
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0]["curve"], "smooth");
    assert_eq!(pts[1]["tick"], 1920);
    assert_eq!(pts[1]["curve"], "hold");
    assert_eq!(
        values(&r.lane(Controller::Cc(1))),
        vec![(0, 0.2), (1920, 0.9)]
    );
    // a swell 0.3 -> 0.9 -> 0.4 replaces what was in range (inclusive of both ends)
    let a = r.base(
        json!({ "controller": 1, "start": 0, "end": 1920, "from": 0.3, "peak": 0.9, "to": 0.4 }),
    );
    let out = r.run("cc.shape", a);
    assert_eq!(out["replaced"], 2);
    assert_eq!(out["shape"], "swell 0.3→0.9→0.4");
    assert_eq!(
        values(&r.lane(Controller::Cc(1))),
        vec![(0, 0.3), (960, 0.9), (1920, 0.4)]
    );
    // the peak can move
    let a = r.base(json!({ "controller": 1, "start": 0, "end": 1920, "from": 0.3, "peak": 0.9, "to": 0.4, "peak_at": 0.25 }));
    r.run("cc.shape", a);
    assert_eq!(r.lane(Controller::Cc(1)).points[1].tick, 480);
    // a hold, and `from` defaulting to the value in force
    let a = r.base(json!({ "controller": 11, "start": 960, "end": 1920, "from": 0.7 }));
    let out = r.run("cc.shape", a);
    assert_eq!(out["shape"], "hold 0.7");
    let a = r.base(
        json!({ "controller": 11, "start": 2000, "end": 2400, "to": 0.2, "curve": "linear" }),
    );
    r.run("cc.shape", a);
    assert_eq!(values(&r.lane(Controller::Cc(11)))[2], (2000, 0.7));
    // replace: false keeps the points in range
    let a = r.base(
        json!({ "controller": 11, "start": 960, "end": 1500, "from": 0.1, "replace": false }),
    );
    r.run("cc.shape", a);
    assert_eq!(r.lane(Controller::Cc(11)).points.len(), 6);
    // pitch bend swells use signed values
    let a = r.base(json!({ "controller": "pitch bend", "start": 0, "end": 960, "from": 0.0, "peak": -0.5, "to": 0.0 }));
    let out = r.run("cc.shape", a);
    assert_eq!(out["shape"], "dip 0→-0.5→0");
    // errors: backwards range, bad peak position, out-of-range value
    let bad = r.base(json!({ "controller": 1, "start": 100, "end": 50, "from": 0.1 }));
    r.err("cc.shape", bad);
    let bad = r.base(json!({ "controller": 1, "start": 0, "end": 50, "from": 0.1, "peak_at": 2 }));
    r.err("cc.shape", bad);
    let bad = r.base(json!({ "controller": 1, "start": 0, "end": 50, "from": 1.1 }));
    r.err("cc.shape", bad);
}

#[test]
fn cc_edits_are_one_undo_step_and_undo_restores_the_lane() {
    let mut r = Rig::new();
    let before = r.p().clone();
    let a = r.base(
        json!({ "controller": 1, "start": 0, "end": 1920, "from": 0.3, "peak": 0.9, "to": 0.4 }),
    );
    r.run("cc.shape", a);
    assert_eq!(r.lane(Controller::Cc(1)).points.len(), 3);
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert_eq!(r.p().patterns, before.patterns);
    r.s.redo(&Author::Human).unwrap().unwrap();
    assert_eq!(r.lane(Controller::Cc(1)).points.len(), 3);
}

#[test]
fn lanes_follow_pattern_clone_channel_clone_and_removal() {
    let mut r = Rig::new();
    let a = r.base(
        json!({ "controller": 1, "start": 0, "end": 1920, "from": 0.3, "peak": 0.9, "to": 0.4 }),
    );
    r.run("cc.shape", a);
    // pattern.clone copies lanes
    let out = r.run("pattern.clone", json!({ "pattern": r.pat }));
    assert_eq!(out["cc_points"], 3);
    let copy = PatternId(out["pattern"].as_u64().unwrap() as u32);
    let l = r
        .p()
        .pattern(copy)
        .unwrap()
        .lane(ChannelId(r.ch), Controller::Cc(1));
    assert_eq!(values(&l), vec![(0, 0.3), (960, 0.9), (1920, 0.4)]);
    // channel.clone copies them into every pattern
    let out = r.run("channel.clone", json!({ "channel": r.ch }));
    assert_eq!(out["cc_points"], 6);
    let c2 = ChannelId(out["channel"].as_u64().unwrap() as u32);
    assert_eq!(
        r.p()
            .pattern(PatternId(r.pat))
            .unwrap()
            .lane(c2, Controller::Cc(1))
            .points
            .len(),
        3
    );
    // clear_channel leaves lanes alone
    r.run(
        "pattern.clear_channel",
        json!({ "pattern": r.pat, "channel": r.ch }),
    );
    assert_eq!(r.lane(Controller::Cc(1)).points.len(), 3);
    // removing the channel takes its lanes with it, and undo brings them back
    r.run("channel.remove", json!({ "channel": r.ch }));
    assert!(
        !r.p()
            .pattern(PatternId(r.pat))
            .unwrap()
            .cc
            .contains_key(&ChannelId(r.ch))
    );
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert_eq!(r.lane(Controller::Cc(1)).points.len(), 3);
    // removing a pattern removes its lanes
    r.run("pattern.remove", json!({ "pattern": copy }));
    assert!(r.p().pattern(copy).is_err());
}

#[test]
fn query_pattern_notes_summarises_lanes() {
    let mut r = Rig::new();
    let a = r.base(json!({ "controller": 11, "start": 0, "end": 960, "from": 0.2, "to": 0.8 }));
    r.run("cc.shape", a);
    let q = r.run("query.pattern_notes", json!({ "pattern": r.pat }));
    let lanes = &q["controller_lanes"][r.ch.to_string()];
    assert_eq!(lanes[0]["name"], "Expression");
    assert_eq!(lanes[0]["shape"], "ramp 0.2→0.8");
    let obj = r.run("query.object", json!({ "path": format!("cc/{}/{}/{}", r.pat, r.ch, r.lane(Controller::Cc(11)).points[0].id) }));
    assert_eq!(obj["value"]["controller"], 11);
}

#[test]
fn two_authors_edit_one_lane_and_undo_only_takes_back_their_own_points() {
    let mut r = Rig::new();
    let a = r.base(json!({
        "controller": 1,
        "points": [{ "tick": 0, "value": 0.1 }, { "tick": 960, "value": 0.2 }],
    }));
    r.run("cc.add_points", a);
    // an agent rewrites the lane; the human adds a point to it meanwhile
    let bot = Caller::agent("bot", Grant::full());
    let rewrite = r.base(json!({ "controller": 1, "points": [{ "tick": 0, "value": 0.7 }, { "tick": 1920, "value": 0.9 }] }));
    r.s.execute(&bot, Request::new("cc.set", rewrite), &mut r.h)
        .unwrap();
    let mine = r.base(json!({ "controller": 1, "points": [{ "tick": 480, "value": 0.5 }] }));
    r.run("cc.add_points", mine);
    assert_eq!(
        values(&r.lane(Controller::Cc(1))),
        vec![(0, 0.7), (480, 0.5), (1920, 0.9)]
    );
    // undoing the agent restores the original points and keeps the human's new one
    r.s.undo(&Author::Agent("bot".into())).unwrap().unwrap();
    assert_eq!(
        values(&r.lane(Controller::Cc(1))),
        vec![(0, 0.1), (480, 0.5), (960, 0.2)]
    );
}
