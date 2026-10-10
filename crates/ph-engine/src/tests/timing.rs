//! Sample-exact timing: note starts, loop wraps, swing, an hour of song time.

use super::*;
use crate::tempo::TempoMap;
use crate::{PlayMode, RenderSettings, render_offline};

/// One impulse sampler on the master, unity gains, no envelope ramp: the output's first
/// non-zero sample is the note start.
fn click_project(bpm: f64, swing: f32, notes: &[Tick]) -> (Project, PatternId) {
    let mut e = Edit::new(&Project::empty("t"));
    e.update_settings(|s| {
        s.bpm = bpm;
        s.swing = swing;
    })
    .unwrap();
    let c = e
        .add_channel("Click", sampler("gen:impulse", &[("attack", 0.0)]))
        .unwrap();
    e.update_channel(c, |ch| ch.volume = 1.0).unwrap();
    let p = e.add_pattern("P").unwrap();
    for t in notes {
        let mut n = NoteSpec::new(60, *t, STEP);
        n.velocity = 1.0;
        e.add_note(p, c, n).unwrap();
    }
    (e.finish().0, p)
}

fn onsets(x: &[f32]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut last: Option<usize> = None;
    for (i, s) in x.iter().enumerate() {
        if s.abs() > 1e-4 && last.is_none_or(|l| i > l + 32) {
            v.push(i);
            last = Some(i);
        } else if s.abs() > 1e-4 {
            last = Some(i);
        }
    }
    v
}

fn pattern_render(p: &Project, pat: PatternId, range: Option<(Tick, Tick)>) -> Vec<f32> {
    let rs = RenderSettings {
        mode: PlayMode::Pattern(pat),
        range,
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    render_offline(p, &rs, &GenAssets).unwrap().channels[0].clone()
}

#[test]
fn notes_start_exactly_at_rounded_tick_positions() {
    let ticks = [0, 1, 7, 240, 333, 960, 1919, 2881];
    let (p, pat) = click_project(131.7, 0.0, &ticks);
    let out = pattern_render(&p, pat, None);
    let map = TempoMap::constant(131.7, SR);
    let expect: Vec<usize> = ticks.iter().map(|t| map.sample_at(*t) as usize).collect();
    // 0 and 1 tick are 22 samples apart: check exact onset of each separately
    for (t, s) in ticks.iter().zip(&expect) {
        assert!(out[*s].abs() > 0.5, "tick {t}: no onset at sample {s}");
        if *s > 0 {
            assert_eq!(out[*s - 1], 0.0, "tick {t}: early output before sample {s}");
        }
    }
}

#[test]
fn pattern_loop_wraps_sample_exactly() {
    let (p, pat) = click_project(140.0, 0.0, &[0, 3 * PPQ + 480]);
    let sc = vec![
        (0, Act::T(TransportCommand::SetMode(PlayMode::Pattern(pat)))),
        (0, Act::T(TransportCommand::Play)),
    ];
    let map = TempoMap::constant(140.0, SR);
    let bar = map.sample_at(4 * PPQ) as usize;
    let out = run_sim(&p, &sc, bar * 3 - 100, 256, 256, 1);
    let got = onsets(&out[0]);
    let second = map.sample_at(3 * PPQ + 480) as usize;
    assert_eq!(
        got,
        vec![0, second, bar, bar + second, 2 * bar, 2 * bar + second],
        "loop of {bar} samples"
    );
}

#[test]
fn song_loop_region_wraps_sample_exactly() {
    let (mut p, pat) = click_project(120.0, 0.0, &[0, PPQ]);
    let mut e = Edit::new(&p);
    for b in 0..4 {
        e.add_clip(0, pat, b * 4 * PPQ).unwrap();
    }
    p = e.finish().0;
    let sc = vec![
        (
            0,
            Act::T(TransportCommand::SetLoop(Some((
                4 * PPQ + PPQ / 2,
                6 * PPQ,
            )))),
        ),
        (0, Act::T(TransportCommand::Seek(4 * PPQ + PPQ / 2))),
        (0, Act::T(TransportCommand::Play)),
    ];
    // loop: 1.5 beats = 36000 samples at 120 BPM, a note at 5 beats (12000 in) each pass
    let out = run_sim(&p, &sc, 36_000 * 3, 256, 256, 2);
    assert_eq!(onsets(&out[0]), vec![12_000, 48_000, 84_000]);
}

#[test]
fn swing_delays_off_beat_sixteenths_as_specified() {
    // swing s delays notes on odd 16ths by s × half a step (0.5 → 60 ticks)
    let (p, pat) = click_project(120.0, 0.5, &[0, 240, 480, 720]);
    let out = pattern_render(&p, pat, None);
    let map = TempoMap::constant(120.0, SR);
    let expect = vec![
        0,
        map.sample_at(300) as usize,
        map.sample_at(480) as usize,
        map.sample_at(780) as usize,
    ];
    assert_eq!(onsets(&out), expect);
    assert_eq!(
        expect[1], 7500,
        "tick 240 + 60 at 120 BPM (25 samples per tick)"
    );
    // full swing is a dotted-16th feel: 120 ticks
    let (p, pat) = click_project(120.0, 1.0, &[240]);
    assert_eq!(
        onsets(&pattern_render(&p, pat, None)),
        vec![map.sample_at(360) as usize]
    );
}

#[test]
fn no_drift_after_an_hour_of_song_time() {
    let bpm = 133.0;
    let hour = (bpm * 60.0) as Tick * PPQ; // 7980 beats
    let target = hour - 7; // an odd tick, well into the hour
    let mut e = Edit::new(&Project::empty("long"));
    e.update_settings(|s| s.bpm = bpm).unwrap();
    let c = e
        .add_channel("Click", sampler("gen:impulse", &[("attack", 0.0)]))
        .unwrap();
    e.update_channel(c, |ch| ch.volume = 1.0).unwrap();
    let p = e.add_pattern("P").unwrap();
    e.update_pattern(p, |h| h.length = Some(PPQ)).unwrap();
    e.add_note(p, c, NoteSpec::new(60, 0, 10)).unwrap();
    let clip = e.add_clip(0, p, target).unwrap();
    e.update_clip(clip, |c| c.length = PPQ).unwrap();
    let proj = e.finish().0;
    let from = target - 2 * PPQ;
    let rs = RenderSettings {
        mode: PlayMode::Song,
        range: Some((from, target + PPQ)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 512,
    };
    let out = render_offline(&proj, &rs, &GenAssets).unwrap();
    // exact rational reference, computed independently
    let exact = |t: Tick| {
        let n = t as u128 * SR as u128 * 60_000;
        let d = 133_000u128 * 960;
        ((2 * n + d) / (2 * d)) as usize
    };
    assert_eq!(exact(hour), SR as usize * 3600);
    assert_eq!(onsets(&out.channels[0]), vec![exact(target) - exact(from)]);
}
