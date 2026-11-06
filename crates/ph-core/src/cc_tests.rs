//! Controller lanes in the model (M3/T10).

use crate::*;

fn synth() -> InstrumentSpec {
    InstrumentSpec::Synth {
        preset: None,
        params: Default::default(),
    }
}

fn spec(c: Controller, tick: Tick, value: f64) -> CcPointSpec {
    CcPointSpec {
        controller: c,
        tick,
        value,
        curve: CurveKind::Linear,
        tension: 0.0,
    }
}

/// A project with one pattern, one channel and a two-point CC1 lane plus a bend point.
fn demo() -> (Project, PatternId, ChannelId, Vec<PointId>) {
    let base = Project::empty("t");
    let mut ed = Edit::new(&base);
    let pat = ed.add_pattern("Swell").unwrap();
    let ch = ed.add_channel("Strings", synth()).unwrap();
    ed.add_note(pat, ch, NoteSpec::new(60, 0, PPQ * 4)).unwrap();
    let a = ed
        .add_cc_point(pat, ch, spec(Controller::Cc(1), 0, 0.2))
        .unwrap();
    let b = ed
        .add_cc_point(pat, ch, spec(Controller::Cc(1), 1920, 0.9))
        .unwrap();
    let c = ed
        .add_cc_point(pat, ch, spec(Controller::PitchBend, 0, -0.5))
        .unwrap();
    let (p, _) = ed.finish();
    (p, pat, ch, vec![a, b, c])
}

#[test]
fn lanes_round_trip_through_json() {
    let (p, pat, ch, _) = demo();
    let text = serde_json::to_string_pretty(&p).unwrap();
    let back: Project = serde_json::from_str(&text).unwrap();
    assert_eq!(back, p);
    let lanes = back.pattern(pat).unwrap().lanes(ch);
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].controller, Controller::Cc(1));
    assert_eq!(lanes[0].points.len(), 2);
    assert_eq!(lanes[1].controller, Controller::PitchBend);
    assert_eq!(back.pattern(pat).unwrap().cc_count(), 3);
}

#[test]
fn a_project_without_lanes_loads_and_saves_unchanged() {
    // a pattern as saved before lanes existed: no `cc` key
    let old = r#"{"id":1,"name":"P","color":255,"length":null,"notes":{}}"#;
    let pat: Pattern = serde_json::from_str(old).unwrap();
    assert!(pat.cc.is_empty());
    let text = serde_json::to_string(&pat).unwrap();
    assert!(!text.contains("\"cc\""), "{text}");
    // the same for a whole project made through Edit
    let base = Project::empty("t");
    let mut ed = Edit::new(&base);
    let p = ed.add_pattern("A").unwrap();
    let c = ed.add_channel("B", synth()).unwrap();
    ed.add_note(p, c, NoteSpec::new(60, 0, STEP)).unwrap();
    let (proj, _) = ed.finish();
    assert!(!serde_json::to_string(&proj).unwrap().contains("\"cc\""));
}

#[test]
fn points_are_patches_and_undo_exactly() {
    let (p, pat, ch, ids) = demo();
    let mut ed = Edit::new(&p);
    let n = ed
        .add_cc_point(pat, ch, spec(Controller::Cc(11), 480, 0.5))
        .unwrap();
    ed.update_cc_point(pat, ch, ids[0], |x| x.value = 0.4)
        .unwrap();
    ed.remove_cc_point(pat, ch, ids[1]).unwrap();
    let (q, patches) = ed.finish();
    assert_eq!(patches.len(), 3);
    assert!(patches.paths().contains(&ObjectPath::CcPoint(pat, ch, n)));
    assert_eq!(
        q.pattern(pat)
            .unwrap()
            .lane(ch, Controller::Cc(1))
            .points
            .len(),
        1
    );
    assert_eq!(apply(&p, &patches).unwrap(), q);
    let undone = apply(&q, &patches.inverse()).unwrap();
    let mut want = p.clone();
    want.next_ids = q.next_ids.clone();
    assert_eq!(undone, want);
}

#[test]
fn object_path_string_form() {
    let path = ObjectPath::CcPoint(PatternId(1), ChannelId(2), PointId(33));
    assert_eq!(path.to_string(), "cc/1/2/33");
    assert_eq!("cc/1/2/33".parse::<ObjectPath>().unwrap(), path);
    assert!("cc/1/2".parse::<ObjectPath>().is_err());
    let (p, pat, ch, ids) = demo();
    let v = get(&p, ObjectPath::CcPoint(pat, ch, ids[0])).unwrap();
    assert_eq!(v["controller"], 1);
    assert_eq!(v["value"], 0.2);
    let v = get(&p, ObjectPath::CcPoint(pat, ch, ids[2])).unwrap();
    assert_eq!(v["controller"], "pitch_bend");
}

#[test]
fn values_and_controllers_are_validated() {
    let (p, pat, ch, ids) = demo();
    let mut ed = Edit::new(&p);
    assert!(
        ed.add_cc_point(pat, ch, spec(Controller::Cc(1), 0, 1.5))
            .is_err()
    );
    assert!(
        ed.add_cc_point(pat, ch, spec(Controller::Cc(1), 0, -0.1))
            .is_err()
    );
    assert!(
        ed.add_cc_point(pat, ch, spec(Controller::Cc(120), 0, 0.5))
            .is_err()
    );
    assert!(
        ed.add_cc_point(pat, ch, spec(Controller::PitchBend, 0, -1.0))
            .is_ok()
    );
    assert!(
        ed.add_cc_point(pat, ch, spec(Controller::Cc(1), 0, f64::NAN))
            .is_err()
    );
    assert!(
        ed.update_cc_point(pat, ch, ids[0], |x| x.tension = 2.0)
            .is_err()
    );
    assert!(
        ed.add_cc_point(pat, ChannelId(99), spec(Controller::Cc(1), 0, 0.5))
            .is_err()
    );
    assert!(
        ed.add_cc_point(PatternId(99), ch, spec(Controller::Cc(1), 0, 0.5))
            .is_err()
    );
}

#[test]
fn controller_names_numbers_and_wire_rounding() {
    assert_eq!(Controller::parse("1").unwrap(), Controller::Cc(1));
    assert_eq!(Controller::parse("CC1").unwrap(), Controller::Cc(1));
    assert_eq!(Controller::parse("Mod Wheel").unwrap(), Controller::Cc(1));
    assert_eq!(Controller::parse("mod_wheel").unwrap(), Controller::Cc(1));
    assert_eq!(
        Controller::parse("CC1 Mod Wheel").unwrap(),
        Controller::Cc(1)
    );
    assert_eq!(Controller::parse("expression").unwrap(), Controller::Cc(11));
    assert_eq!(Controller::parse("sustain").unwrap(), Controller::Cc(64));
    assert_eq!(Controller::parse("volume").unwrap(), Controller::Cc(7));
    assert_eq!(
        Controller::parse("pitch bend").unwrap(),
        Controller::PitchBend
    );
    assert_eq!(
        Controller::parse("pitch_bend").unwrap(),
        Controller::PitchBend
    );
    assert!(Controller::parse("120").is_err());
    assert!(Controller::parse("nonsense").is_err());
    assert_eq!(Controller::Cc(1).name(), "Mod Wheel");
    assert_eq!(Controller::Cc(11).label(), "CC11 Expression");
    assert_eq!(Controller::Cc(3).name(), "CC3");
    assert_eq!(Controller::PitchBend.name(), "Pitch Bend");
    // serde: numbers and names in, number or "pitch_bend" out
    let c: Controller = serde_json::from_str("\"expression\"").unwrap();
    assert_eq!(c, Controller::Cc(11));
    let c: Controller = serde_json::from_str("74").unwrap();
    assert_eq!(c, Controller::Cc(74));
    assert!(serde_json::from_str::<Controller>("200").is_err());
    assert_eq!(serde_json::to_string(&Controller::Cc(7)).unwrap(), "7");
    assert_eq!(
        serde_json::to_string(&Controller::PitchBend).unwrap(),
        "\"pitch_bend\""
    );
    // the 7-bit and 14-bit rounding
    assert_eq!(Controller::Cc(1).quantize(0.5), 64);
    assert_eq!(Controller::Cc(1).wire(1.0), 1.0);
    assert_eq!(Controller::Cc(1).wire(0.0), 0.0);
    assert_eq!(Controller::PitchBend.wire(0.0), 0.0);
    assert_eq!(Controller::PitchBend.wire(-1.0), -1.0);
    assert_eq!(Controller::PitchBend.quantize(1.0), 8191);
    for q in 0..=127 {
        assert_eq!(
            Controller::Cc(1).quantize(Controller::Cc(1).dequantize(q) as f64),
            q
        );
    }
}

#[test]
fn lane_values_follow_automation_semantics() {
    let (p, pat, ch, _) = demo();
    let lane = p.pattern(pat).unwrap().lane(ch, Controller::Cc(1));
    assert_eq!(lane.value_at(0.0), Some(0.2));
    assert!((lane.value_at(960.0).unwrap() - 0.55).abs() < 1e-9);
    assert_eq!(lane.value_at(5000.0), Some(0.9));
    assert_eq!(
        p.pattern(pat)
            .unwrap()
            .lane(ch, Controller::Cc(7))
            .value_at(0.0),
        None
    );
}

#[test]
fn deleting_a_channel_or_pattern_takes_its_lanes_along() {
    let (p, pat, ch, _) = demo();
    let mut ed = Edit::new(&p);
    ed.remove_channel(ch).unwrap();
    let (q, patches) = ed.finish();
    assert_eq!(q.pattern(pat).unwrap().cc_count(), 0);
    assert_eq!(q.pattern(pat).unwrap().note_count(), 0);
    // undo brings the lanes back
    let back = apply(&q, &patches.inverse()).unwrap();
    assert_eq!(back.pattern(pat).unwrap().cc, p.pattern(pat).unwrap().cc);
    // a lane-only pattern deletes cleanly too
    let mut ed = Edit::new(&p);
    ed.remove_pattern(pat).unwrap();
    let (q, _) = ed.finish();
    assert!(q.patterns.is_empty());
    // but a bare patch cannot orphan a lane
    let mut ed = Edit::new(&p);
    assert!(ed.put(ObjectPath::Channel(ch), None).is_err());
}
