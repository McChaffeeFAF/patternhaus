//! Signal tests for the mixer graph: sends, `route_to_master`, sidechain ducking, mute,
//! pan, and feedback-loop rejection.

use super::*;
use crate::{EngineError, PlayMode, RenderSettings, render_offline};

/// One DC sampler channel (0.5 × velocity 1 × volume 1) into insert 1, held for a beat.
fn dc_project(f: impl FnOnce(&mut Edit, ChannelId)) -> Project {
    let mut e = Edit::new(&Project::empty("r"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let c = e
        .add_channel(
            "DC",
            sampler(
                "gen:dc",
                &[("attack", 0.0), ("release", 0.0), ("sustain", 1.0)],
            ),
        )
        .unwrap();
    e.update_channel(c, |ch| {
        ch.volume = 1.0;
        ch.insert = InsertId(1);
    })
    .unwrap();
    e.update_insert(InsertId(1), |i| i.volume = 1.0).unwrap();
    let p = e.add_pattern("P").unwrap();
    let mut n = NoteSpec::new(60, 0, PPQ);
    n.velocity = 1.0;
    e.add_note(p, c, n).unwrap();
    e.add_clip(0, p, 0).unwrap();
    f(&mut e, c);
    e.finish().0
}

fn render(p: &Project) -> [Vec<f32>; 2] {
    let rs = RenderSettings {
        mode: PlayMode::Song,
        range: Some((0, PPQ)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    let b = render_offline(p, &rs, &GenAssets).unwrap();
    [b.channels[0].clone(), b.channels[1].clone()]
}

/// Level in the middle of the held note.
fn level(x: &[Vec<f32>; 2]) -> (f32, f32) {
    (x[0][12_000], x[1][12_000])
}

#[test]
fn direct_path_has_unity_gain() {
    let (l, r) = level(&render(&dc_project(|_, _| {})));
    assert!((l - 0.5).abs() < 1e-3 && (r - 0.5).abs() < 1e-3, "{l} {r}");
}

#[test]
fn sends_and_route_to_master() {
    // insert 1 does not route to master; it sends at 0.5 to insert 2, which does
    let p = dc_project(|e, _| {
        e.update_insert(InsertId(1), |i| {
            i.route_to_master = false;
            i.sends.push(ph_core::Send {
                to: InsertId(2),
                level: 0.5,
            });
        })
        .unwrap();
        e.update_insert(InsertId(2), |i| i.volume = 1.0).unwrap();
    });
    let (l, r) = level(&render(&p));
    assert!(
        (l - 0.25).abs() < 1e-3 && (r - 0.25).abs() < 1e-3,
        "{l} {r}"
    );
    // both paths: direct (1.0) + send (0.5)
    let p = dc_project(|e, _| {
        e.update_insert(InsertId(1), |i| {
            i.sends.push(ph_core::Send {
                to: InsertId(2),
                level: 0.5,
            })
        })
        .unwrap();
        e.update_insert(InsertId(2), |i| i.volume = 1.0).unwrap();
    });
    let (l, _) = level(&render(&p));
    assert!((l - 0.75).abs() < 1e-3, "{l}");
    // routed nowhere: silence
    let p = dc_project(|e, _| {
        e.update_insert(InsertId(1), |i| i.route_to_master = false)
            .unwrap()
    });
    assert_eq!(peak(&render(&p)[0]), 0.0);
}

#[test]
fn mute_pan_and_volume() {
    let p = dc_project(|e, c| e.update_channel(c, |ch| ch.muted = true).unwrap());
    assert_eq!(peak(&render(&p)[0]), 0.0);
    let p = dc_project(|e, _| e.update_insert(InsertId(1), |i| i.muted = true).unwrap());
    assert_eq!(peak(&render(&p)[1]), 0.0);
    let p = dc_project(|e, c| e.update_channel(c, |ch| ch.pan = 1.0).unwrap());
    let (l, r) = level(&render(&p));
    assert!(l.abs() < 1e-6 && (r - 0.5).abs() < 1e-3, "{l} {r}");
    let p = dc_project(|e, _| e.update_insert(InsertId(0), |i| i.volume = 0.5).unwrap());
    let (l, _) = level(&render(&p));
    assert!((l - 0.25).abs() < 1e-3, "{l}");
}

#[test]
fn sidechain_ducks_the_compressed_insert() {
    let make = |sidechain: bool| {
        dc_project(|e, _| {
            let kick = e.add_channel("Kick", sampler("gen:kick", &[])).unwrap();
            e.update_channel(kick, |ch| ch.insert = InsertId(2))
                .unwrap();
            // the kick insert feeds only the sidechain
            e.update_insert(InsertId(2), |i| i.route_to_master = false)
                .unwrap();
            let p = e.project().pattern_order[0];
            for k in 0..4 {
                let mut n = NoteSpec::new(60, k * PPQ / 4, STEP);
                n.velocity = 1.0;
                e.add_note(p, kick, n).unwrap();
            }
            e.update_insert(InsertId(1), |i| {
                i.effects.push(fx(
                    "compressor",
                    &[
                        ("threshold", -30.0),
                        ("ratio", 20.0),
                        ("attack", 0.5),
                        ("sidechain", 1.0),
                    ],
                ));
                if sidechain {
                    i.sidechain_from = Some(InsertId(2));
                }
            })
            .unwrap();
        })
    };
    let ducked = render(&make(true));
    let plain = render(&make(false));
    // without a sidechain source the external detector hears silence: no gain reduction
    let a = rms(&plain[0][2000..20_000]);
    let b = rms(&ducked[0][2000..20_000]);
    assert!((a - 0.5).abs() < 0.01, "undocked level {a}");
    assert!(b < 0.6 * a, "sidechain did not duck: {b} vs {a}");
}

#[test]
fn feedback_loops_are_rejected() {
    let p = dc_project(|e, _| {
        e.update_insert(InsertId(1), |i| {
            i.sends.push(ph_core::Send {
                to: InsertId(2),
                level: 1.0,
            })
        })
        .unwrap();
        e.update_insert(InsertId(2), |i| {
            i.sends.push(ph_core::Send {
                to: InsertId(1),
                level: 1.0,
            })
        })
        .unwrap();
    });
    let rs = RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    match render_offline(&p, &rs, &GenAssets) {
        Err(EngineError::Plan(m)) => assert!(m.contains("feedback"), "{m}"),
        other => panic!("expected a plan error, got {:?}", other.map(|b| b.frames())),
    }
    // the running engine keeps its old plan
    let (engine, _dev) = sim(256, 1);
    assert!(engine.set_project(Arc::new(p)).is_err());
}

#[test]
fn choke_group_cuts_other_channels() {
    let make = |group: f64| {
        let mut e = Edit::new(&Project::empty("c"));
        e.update_settings(|s| s.bpm = 120.0).unwrap();
        let open = e
            .add_channel(
                "Open",
                sampler(
                    "gen:dc",
                    &[("choke_group", group), ("release", 0.01), ("attack", 0.0)],
                ),
            )
            .unwrap();
        let closed = e
            .add_channel("Closed", sampler("gen:silence", &[("choke_group", group)]))
            .unwrap();
        let p = e.add_pattern("P").unwrap();
        e.add_note(p, open, NoteSpec::new(60, 0, 2 * PPQ)).unwrap();
        e.add_note(p, closed, NoteSpec::new(60, PPQ / 2, STEP))
            .unwrap();
        (e.finish().0, p)
    };
    let rs = |p| RenderSettings {
        mode: PlayMode::Pattern(p),
        range: Some((0, PPQ)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 100,
    };
    let (p, pat) = make(0.0);
    let free = render_offline(&p, &rs(pat), &GenAssets).unwrap().channels[0].clone();
    let (p, pat) = make(3.0);
    let choked = render_offline(&p, &rs(pat), &GenAssets).unwrap().channels[0].clone();
    // the closed hat starts at 12000; 10 ms release later the open one is gone
    assert!(free[20_000].abs() > 0.1);
    assert!(choked[11_000].abs() > 0.1, "sounds before the choke");
    assert!(
        choked[20_000].abs() < 1e-4,
        "still sounding after the choke"
    );
}

/// FL's documented channel anchors (docs/formats/fl-behaviour-notes.md §1, §2): 100% = 0 dB,
/// the 78% default = -5.2 dB, Circular pan law = -3 dB per side at centre, 0 dB on the
/// panned side at the extremes.
#[test]
fn channel_strip_follows_fl_volume_and_circular_pan() {
    use crate::plan::channel_gain;
    let db = |g: f32| 20.0 * (g as f64).log10();
    let [l, r] = channel_gain(1.0, 1.0, false);
    assert!(
        l.abs() < 1e-7 && (r - 1.0).abs() < 1e-6,
        "hard right at 100%: {l} {r}"
    );
    let [l, r] = channel_gain(1.0, -1.0, false);
    assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-7);
    let [l, r] = channel_gain(1.0, 0.0, false);
    assert!((db(l) + 3.01).abs() < 0.01 && (db(r) + 3.01).abs() < 0.01);
    let [_, r] = channel_gain(0.78, 1.0, false);
    assert!((db(r) + 5.2).abs() < 0.01, "78% = {} dB", db(r));
    // the FL default sampler state: 78% at centre = -8.2 dB (manual: [LVL])
    let [l, _] = channel_gain(0.78, 0.0, false);
    assert!((db(l) + 8.2).abs() < 0.02, "{}", db(l));
    assert_eq!(channel_gain(0.78, 0.0, true), [0.0, 0.0]);
    assert_eq!(channel_gain(0.0, 0.0, false), [0.0, 0.0]);
}
