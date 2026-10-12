//! M2 model: clip kinds, the sample registry, automation points and their paths.

use crate::*;
use serde_json::json;

fn entry(path: &str) -> SampleEntry {
    SampleEntry {
        id: SampleId(0),
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        frames: 96_000,
        sample_rate: 48_000,
        channels: 2,
        bpm: Some(SampleBpm {
            value: 120.0,
            source: BpmSource::Detected,
        }),
        root_key: None,
        analysis: SampleAnalysis::default(),
        provenance: Provenance {
            origin: Origin::Imported {
                from: format!("C:/loops/{path}"),
            },
            at: Some(1_760_000_000),
        },
    }
}

/// A project with one pattern clip, one sample, one audio clip and one automation clip with
/// three points.
fn song() -> (
    Project,
    Project,
    PatchSet,
    SampleId,
    ClipId,
    ClipId,
    Vec<PointId>,
) {
    let base = Project::empty("m2");
    let mut ed = Edit::new(&base);
    let pat = ed.add_pattern("Beat").unwrap();
    ed.add_clip(0, pat, 0).unwrap();
    let s = ed.add_sample(entry("samples/loop.wav")).unwrap();
    let mut a = AudioClip::new(s);
    a.fade_in = Fade {
        length: STEP,
        curve: FadeCurve::SCurve,
    };
    let audio = ed.add_audio_clip(1, 0, PPQ * 8, a).unwrap();
    let auto = ed
        .add_automation_clip(2, 0, PPQ * 16, ParamPath::InsertVolume(InsertId(1)))
        .unwrap();
    let pts = [(0, 0.0), (PPQ * 8, 1.0), (PPQ * 16, 0.5)]
        .into_iter()
        .map(|(t, v)| ed.add_point(auto, t, v, CurveKind::Linear, 0.0).unwrap())
        .collect();
    let (after, patches) = ed.finish();
    (base, after, patches, s, audio, auto, pts)
}

#[test]
fn format_one_clips_load_as_pattern_clips_and_save_unchanged() {
    let v1 = r#"{"id":3,"track":1,"pattern":2,"start":960,"length":3840,"offset":0}"#;
    let c: Clip = serde_json::from_str(v1).unwrap();
    assert_eq!(c.pattern(), Some(PatternId(2)));
    assert_eq!(serde_json::to_string(&c).unwrap(), v1);
    // an explicit `kind: pattern` is accepted too
    let tagged =
        r#"{"id":3,"track":1,"kind":"pattern","pattern":2,"start":960,"length":3840,"offset":0}"#;
    assert_eq!(serde_json::from_str::<Clip>(tagged).unwrap(), c);
    // unknown kinds and stray fields are errors, not silent pattern clips
    assert!(
        serde_json::from_str::<Clip>(&v1.replace("\"pattern\":2", "\"kind\":\"video\"")).is_err()
    );
    assert!(
        serde_json::from_str::<Clip>(&v1.replace("\"offset\":0", "\"offset\":0,\"x\":1")).is_err()
    );
}

#[test]
fn every_clip_kind_round_trips_through_json() {
    let (_, p, _, _, audio, auto, _) = song();
    for id in [audio, auto] {
        let c = p.clip(id).unwrap();
        let v = serde_json::to_value(c).unwrap();
        assert_eq!(v["kind"], c.kind_name());
        assert_eq!(&serde_json::from_value::<Clip>(v).unwrap(), c);
    }
    let text = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&text).unwrap();
    assert_eq!(back, p);
    let a = serde_json::to_value(p.clip(audio).unwrap()).unwrap();
    assert_eq!(a["fade_in"], json!({ "length": 240, "curve": "s_curve" }));
    assert_eq!(a["stretch"], "resample");
}

#[test]
fn m2_edits_replay_and_invert() {
    let (base, after, patches, ..) = song();
    assert_eq!(apply(&base, &patches).unwrap(), after);
    let mut undone = apply(&after, &patches.inverse()).unwrap();
    undone.next_ids = base.next_ids.clone();
    assert_eq!(undone, base);
    assert_eq!(after.next_ids.sample, 1);
    assert_eq!(after.next_ids.point, 3);
}

#[test]
fn points_are_their_own_small_objects() {
    let (_, p, patches, _, _, auto, pts) = song();
    // the clip's patch value never carries points
    let v = get(&p, ObjectPath::Clip(auto)).unwrap();
    assert!(v.get("points").is_none());
    assert!(patches.paths().contains(&ObjectPath::Point(auto, pts[1])));
    // moving one point is one patch on that point only
    let mut ed = Edit::new(&p);
    ed.move_point(auto, pts[1], PPQ * 4, Some(0.25)).unwrap();
    let (q, set) = ed.finish();
    assert_eq!(set.paths(), vec![ObjectPath::Point(auto, pts[1])]);
    assert_eq!(q.point(auto, pts[1]).unwrap().tick, PPQ * 4);
    // updating the clip keeps its points
    let mut ed = Edit::new(&q);
    ed.update_clip(auto, |c| c.start = PPQ).unwrap();
    let (r, _) = ed.finish();
    assert_eq!(r.clip(auto).unwrap().automation().unwrap().points.len(), 3);
    // a clip patch that carries points is refused
    let mut with = serde_json::to_value(r.clip(auto).unwrap()).unwrap();
    assert!(with.get("points").is_some());
    with["start"] = json!(0);
    assert!(set_value(&r, ObjectPath::Clip(auto), Some(with)).is_err());
}

fn set_value(p: &Project, path: ObjectPath, v: Option<serde_json::Value>) -> ModelResult<()> {
    let mut w = p.clone();
    set(&mut w, path, v.as_ref())
}

#[test]
fn removing_an_automation_clip_removes_its_points_first() {
    let (_, p, _, _, _, auto, pts) = song();
    assert!(set_value(&p, ObjectPath::Clip(auto), None).is_err());
    let mut ed = Edit::new(&p);
    ed.remove_clip(auto).unwrap();
    let (q, set) = ed.finish();
    assert!(q.clips.get(&auto).is_none());
    assert_eq!(set.len(), 4);
    assert_eq!(set.0.last().unwrap().path, ObjectPath::Clip(auto));
    assert_eq!(apply(&q, &set.inverse()).unwrap(), p);
    // a point needs an automation clip
    let mut ed = Edit::new(&p);
    assert!(
        ed.add_point(ClipId(0), 0, 1.0, CurveKind::Hold, 0.0)
            .is_err()
    );
    assert!(
        ed.add_point(auto, 0, f64::NAN, CurveKind::Hold, 0.0)
            .is_err()
    );
    assert!(
        ed.set_point_curve(auto, pts[0], CurveKind::SCurve, 2.0)
            .is_err()
    );
}

#[test]
fn sample_registry_invariants() {
    let (_, p, _, s, audio, ..) = song();
    let mut ed = Edit::new(&p);
    // one entry per path
    assert!(ed.add_sample(entry("samples/loop.wav")).is_err());
    // in use: cannot be removed until the clip is gone
    assert!(ed.remove_sample(s).is_err());
    ed.remove_clip(audio).unwrap();
    ed.remove_sample(s).unwrap();
    // a clip needs a registered sample
    assert!(ed.add_audio_clip(0, 0, PPQ, AudioClip::new(s)).is_err());
    assert_eq!(p.sample_by_path("samples/loop.wav").unwrap().id, s);
    assert_eq!(p.sample(s).unwrap().seconds(), 2.0);
    let mut bad = entry("x.wav");
    bad.sample_rate = 0;
    assert!(Edit::new(&p).add_sample(bad).is_err());
    let mut ed = Edit::new(&p);
    ed.set_sample_bpm(
        s,
        Some(SampleBpm {
            value: 87.5,
            source: BpmSource::User,
        }),
    )
    .unwrap();
    assert!(
        ed.set_sample_bpm(
            s,
            Some(SampleBpm {
                value: 0.0,
                source: BpmSource::User
            })
        )
        .is_err()
    );
}

#[test]
fn audio_clip_validation() {
    let (_, p, _, _, audio, ..) = song();
    let mut ed = Edit::new(&p);
    assert!(ed.update_clip(audio, |c| c.offset = 10).is_err());
    assert!(
        ed.update_clip(audio, |c| if let ClipKind::Audio(a) = &mut c.kind {
            a.fade_out.length = PPQ * 8;
        })
        .is_err()
    );
    for f in [
        |a: &mut AudioClip| a.gain_db = 30.0,
        |a: &mut AudioClip| a.pitch_semitones = 49,
        |a: &mut AudioClip| a.fine_cents = -101.0,
    ] {
        assert!(
            ed.update_clip(audio, |c| if let ClipKind::Audio(a) = &mut c.kind {
                f(a)
            })
            .is_err()
        );
    }
    ed.update_clip(audio, |c| {
        if let ClipKind::Audio(a) = &mut c.kind {
            a.source_offset = 4800;
            a.reverse = true;
            a.stretch = StretchMode::Stretch;
        }
    })
    .unwrap();
}

#[test]
fn time_ratio_follows_tempo_sync_and_stretch_mode() {
    let e = entry("samples/a.wav");
    let mut a = AudioClip::new(SampleId(0));
    assert_eq!(a.time_ratio(&e, 140.0), 1.0);
    a.tempo_sync = true;
    assert_eq!(a.time_ratio(&e, 150.0), 1.25);
    a.pitch_semitones = 12;
    assert_eq!(a.time_ratio(&e, 150.0), 2.5); // resample: pitch is speed
    a.stretch = StretchMode::Stretch;
    assert_eq!(a.time_ratio(&e, 150.0), 1.25);
    let mut no_bpm = e.clone();
    no_bpm.bpm = None;
    assert_eq!(a.time_ratio(&no_bpm, 150.0), 1.0);
}

#[test]
fn object_paths_for_new_objects_round_trip() {
    for s in ["sample/4", "point/7/12"] {
        let p: ObjectPath = s.parse().unwrap();
        assert_eq!(p.to_string(), s);
        assert!(!p.is_fixed());
    }
    assert!("point/7".parse::<ObjectPath>().is_err());
}

#[test]
fn param_paths_round_trip_and_reject_bad_forms() {
    let all = [
        ParamPath::Tempo,
        ParamPath::ChannelVolume(ChannelId(3)),
        ParamPath::ChannelPan(ChannelId(3)),
        ParamPath::ChannelParam(ChannelId(3), "cutoff".into()),
        ParamPath::InsertVolume(InsertId(2)),
        ParamPath::InsertPan(InsertId(2)),
        ParamPath::InsertMute(InsertId(2)),
        ParamPath::EffectParam {
            insert: InsertId(2),
            slot: 1,
            param: "mix".into(),
        },
    ];
    let strings = [
        "tempo",
        "channel/3/volume",
        "channel/3/pan",
        "channel/3/param/cutoff",
        "insert/2/volume",
        "insert/2/pan",
        "insert/2/mute",
        "insert/2/slot/1/param/mix",
    ];
    for (p, s) in all.iter().zip(strings) {
        assert_eq!(p.to_string(), s);
        assert_eq!(&s.parse::<ParamPath>().unwrap(), p);
        assert_eq!(serde_json::to_value(p).unwrap(), json!(s));
    }
    for bad in [
        "",
        "tempo/1",
        "channel/x/volume",
        "channel/3/param/",
        "channel/3/param/a/b",
        "insert/2/slot/1/mix",
        "mixer/2/volume",
    ] {
        assert!(bad.parse::<ParamPath>().is_err(), "{bad} parsed");
    }
    assert_eq!(all[1].object(), ObjectPath::Channel(ChannelId(3)));
    assert_eq!(all[0].fixed_range(), Some((10.0, 999.0)));
    let p = Project::empty("x");
    assert_eq!(ParamPath::Tempo.model_value(&p), Some(130.0));
    assert_eq!(
        ParamPath::InsertVolume(InsertId(0)).model_value(&p),
        Some(1.0)
    );
}

fn pt(id: u32, tick: Tick, value: f64, curve: CurveKind, tension: f32) -> AutomationPoint {
    AutomationPoint {
        id: PointId(id),
        tick,
        value,
        curve,
        tension,
    }
}

#[test]
fn curve_shapes_are_exact_at_the_ends_and_monotonic() {
    for k in [CurveKind::Linear, CurveKind::Hold, CurveKind::SCurve] {
        for tension in [-1.0, -0.3, 0.0, 0.5, 1.0] {
            assert_eq!(k.shape(tension, 0.0), 0.0);
            assert_eq!(k.shape(tension, 1.0), 1.0);
            let mut last = 0.0;
            for i in 0..=100 {
                let s = k.shape(tension, i as f64 / 100.0);
                assert!(s >= last - 1e-12 && (0.0..=1.0).contains(&s));
                last = s;
            }
        }
    }
    assert_eq!(CurveKind::Linear.shape(0.0, 0.25), 0.25);
    assert_eq!(CurveKind::SCurve.shape(0.0, 0.5), 0.5);
    for f in [
        FadeCurve::Linear,
        FadeCurve::Exp,
        FadeCurve::Log,
        FadeCurve::SCurve,
    ] {
        assert_eq!(f.gain(0.0), 0.0);
        assert_eq!(f.gain(1.0), 1.0);
    }
}

#[test]
fn value_at_holds_outside_jumps_on_shared_ticks_and_never_overshoots() {
    let a = pt(0, 100, 0.0, CurveKind::Linear, 0.0);
    let b = pt(1, 200, 1.0, CurveKind::Hold, 0.0);
    let c = pt(2, 200, 0.2, CurveKind::Smooth, 0.0);
    let d = pt(3, 300, 0.9, CurveKind::Smooth, 0.0);
    let e = pt(4, 400, 0.1, CurveKind::Linear, 0.0);
    let pts = [&a, &b, &c, &d, &e];
    assert_eq!(value_at::<AutomationPoint>(&[], 5.0), None);
    assert_eq!(value_at(&pts, 0.0), Some(0.0));
    assert_eq!(value_at(&pts, 150.0), Some(0.5));
    // two points on tick 200: the later id is the value leaving
    assert_eq!(
        value_at(&pts, 199.999).map(|v| (v * 1000.0).round()),
        Some(1000.0)
    );
    assert_eq!(value_at(&pts, 200.0), Some(0.2));
    for i in 200..=400 {
        let v = value_at(&pts, i as f64).unwrap();
        assert!((0.1..=0.9).contains(&v), "{i}: {v}");
    }
    assert_eq!(value_at(&pts, 300.0), Some(0.9));
    assert_eq!(value_at(&pts, 1e9), Some(0.1));
    // evaluation order is (tick, id), whatever the ids say
    let mut clip = AutomationClip::new(ParamPath::Tempo);
    for p in [
        pt(9, 0, 120.0, CurveKind::Linear, 0.0),
        pt(1, 960, 140.0, CurveKind::Linear, 0.0),
    ] {
        clip.points.insert(p.id, p);
    }
    assert_eq!(clip.value_at(480.0), Some(130.0));
}
