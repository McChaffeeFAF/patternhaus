//! M3/T10: controller lanes. Point events land on exact frames, ramps are sampled at the
//! control rate and deduped, play/seek/loop/clip start chase the value in force, trimmed
//! clips and tempo changes behave like notes, nothing allocates, and live output is
//! bit-identical to the offline render on a synth, a plugin and a Haus Sampler channel.

use super::*;
use crate::PlayMode;
use crate::cc::CC_RATE;
use ph_node::EventKind;

type Tap = Vec<(i64, EventKind)>;

fn cc(controller: u8, value: f32) -> EventKind {
    EventKind::Cc { controller, value }
}

fn w(controller: u8, v: f64) -> f32 {
    Controller::Cc(controller).wire(v)
}

/// 120 BPM at 48 kHz: 25 samples per tick. One synth channel, one pattern with a long note.
fn base() -> (Edit, PatternId, ChannelId) {
    let mut e = Edit::new(&Project::empty("cc"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let ch = e.add_channel("Strings", synth(&[])).unwrap();
    let pat = e.add_pattern("P").unwrap();
    e.add_note(pat, ch, NoteSpec::new(60, 0, 4 * PPQ)).unwrap();
    (e, pat, ch)
}

fn lane(
    e: &mut Edit,
    pat: PatternId,
    ch: ChannelId,
    c: Controller,
    pts: &[(Tick, f64, CurveKind)],
) {
    for (t, v, k) in pts {
        e.add_cc_point(
            pat,
            ch,
            CcPointSpec {
                controller: c,
                tick: *t,
                value: *v,
                curve: *k,
                tension: 0.0,
            },
        )
        .unwrap();
    }
}

/// Controller and bend events of channel 0 with their timeline samples, for a scripted run.
fn tapped(initial: &Project, script: &[(usize, Act)], frames: usize, block: usize) -> Tap {
    let a = GenAssets;
    let mut off = Offline::new(initial, settings(block), &a, 0).unwrap();
    off.rt.tap_on = true;
    let (mut at, mut k) = (0, 0);
    while at < frames {
        while k < script.len() && script[k].0 <= at {
            match &script[k].1 {
                Act::T(t) => off.transport(t.clone()),
                Act::P(p) => off.set_project(p, &a).unwrap(),
                Act::Preview(..) => {}
            }
            k += 1;
        }
        let next = script.get(k).map_or(frames, |s| s.0.min(frames));
        off.render(next - at, block);
        at = next;
    }
    off.rt
        .tap
        .iter()
        .filter(|(i, ..)| *i == 0)
        .map(|(_, s, k)| (*s, *k))
        .collect()
}

fn pattern_play(pat: PatternId) -> Vec<(usize, Act)> {
    vec![
        (0, Act::T(TransportCommand::SetMode(PlayMode::Pattern(pat)))),
        (0, Act::T(TransportCommand::Play)),
    ]
}

fn song_play() -> Vec<(usize, Act)> {
    vec![(0, Act::T(TransportCommand::Play))]
}

#[test]
fn point_events_land_on_exact_frames_whatever_the_block_size() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(1),
        &[
            (0, 0.0, CurveKind::Hold),
            (960, 1.0, CurveKind::Hold),
            (1920, 0.5, CurveKind::Hold),
        ],
    );
    let p = e.finish().0;
    let want: Tap = vec![
        (0, cc(1, 0.0)),
        (960 * 25, cc(1, 1.0)),
        (1920 * 25, cc(1, w(1, 0.5))),
    ];
    for block in [64, 100, 1000, 4096] {
        let got = tapped(&p, &pattern_play(pat), 90_000, block);
        assert_eq!(got, want, "block {block}");
    }
}

#[test]
fn ramps_are_sampled_at_the_control_rate_and_deduped() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(11),
        &[(0, 0.0, CurveKind::Linear), (960, 1.0, CurveKind::Hold)],
    );
    let p = e.finish().0;
    let got = tapped(&p, &pattern_play(pat), 60_000, 256);
    assert_eq!(CC_RATE, 64);
    // every event is either the start, the end point or on the 64-frame grid
    for (s, _) in &got {
        assert!(*s == 0 || *s == 24_000 || s % CC_RATE == 0, "off grid: {s}");
    }
    // 127 steps up, 127 distinct values after the first, never repeated back to back
    let vals: Vec<f32> = got
        .iter()
        .map(|(_, k)| match k {
            EventKind::Cc {
                controller: 11,
                value,
            } => *value,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(vals.len(), 128, "start + 127 changes");
    assert!(vals.windows(2).all(|p| p[1] > p[0]), "strictly rising");
    assert_eq!(vals[0], 0.0);
    assert_eq!(*vals.last().unwrap(), 1.0);
    // the ramp is spread over the segment (about 23 frames per step on a 24000-frame ramp)
    assert!(got[1].0 > 0 && got.last().unwrap().0 <= 24_000);
    // a flat lane sends only its start
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(11),
        &[(0, 0.5, CurveKind::Linear), (960, 0.5, CurveKind::Linear)],
    );
    let flat = tapped(&e.finish().0, &pattern_play(pat), 60_000, 256);
    assert_eq!(flat.len(), 1, "{flat:?}");
}

#[test]
fn pitch_bend_is_sent_as_14_bit_values() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::PitchBend,
        &[
            (0, -1.0, CurveKind::Hold),
            (480, 0.0, CurveKind::Hold),
            (960, 1.0, CurveKind::Hold),
        ],
    );
    let got = tapped(&e.finish().0, &pattern_play(pat), 40_000, 128);
    assert_eq!(
        got,
        vec![
            (0, EventKind::PitchBend { value: -1.0 }),
            (12_000, EventKind::PitchBend { value: 0.0 }),
            (
                24_000,
                EventKind::PitchBend {
                    value: 8191.0 / 8192.0
                }
            ),
        ]
    );
}

#[test]
fn play_seek_and_loop_wrap_chase_the_value_in_force() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(1),
        &[(0, 0.0, CurveKind::Linear), (3840, 1.0, CurveKind::Hold)],
    );
    e.add_clip(0, pat, 0).unwrap();
    let p = e.finish().0;
    // a bar is 3840 ticks = 96000 samples here; tick 1930 is sample 48250, off the 64 grid
    let whole = tapped(&p, &song_play(), 96_000, 128);
    let before = whole.iter().rev().find(|(s, _)| *s < 48_250).unwrap().1;
    // seek while playing: the first event at the new position is the chase of that value
    let mut sc = song_play();
    sc.push((6_400, Act::T(TransportCommand::Seek(1930))));
    let seeked = tapped(&p, &sc, 8_000, 128);
    let at: Vec<_> = seeked.iter().filter(|(s, _)| *s == 48_250).collect();
    assert_eq!(at.len(), 1, "{seeked:?}");
    assert_eq!(at[0].1, before);
    // play from the middle: seek while stopped, then play
    let sc = vec![
        (0, Act::T(TransportCommand::Seek(1930))),
        (0, Act::T(TransportCommand::Play)),
    ];
    let from_mid = tapped(&p, &sc, 4_000, 128);
    assert_eq!(from_mid[0], (48_250, before));
    // a loop wrap inside the clip: loop 1930..2890 ticks
    let sc = vec![
        (0, Act::T(TransportCommand::SetLoop(Some((1930, 2890))))),
        (0, Act::T(TransportCommand::Seek(1930))),
        (0, Act::T(TransportCommand::Play)),
    ];
    let looped = tapped(&p, &sc, 60_000, 128);
    // the loop is 960 ticks = 24000 samples long; at the wrap the chase sends the same value
    // again at the loop start
    let at_start: Vec<_> = looped.iter().filter(|(s, _)| *s == 48_250).collect();
    assert_eq!(at_start.len(), 3, "start + two wraps: {looped:?}");
    assert!(at_start.iter().all(|e| e.1 == before));
}

#[test]
fn pattern_mode_loop_restarts_with_the_first_value() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(7),
        &[(0, 0.5, CurveKind::Hold)],
    );
    let p = e.finish().0;
    // one bar = 96000 samples; the constant lane is re-sent at each wrap (the timeline
    // position wraps with the pattern)
    let got = tapped(&p, &pattern_play(pat), 2 * 96_000 + 500, 1000);
    let marks: Vec<i64> = got.iter().map(|(s, _)| *s).collect();
    assert_eq!(marks, vec![0, 0, 0]);
}

#[test]
fn every_clip_start_sends_its_value_even_when_unchanged() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(11),
        &[(0, 0.5, CurveKind::Hold)],
    );
    let bar = 4 * PPQ;
    let c1 = e.add_clip(0, pat, 0).unwrap();
    e.update_clip(c1, |c| c.length = bar / 2).unwrap();
    let c2 = e.add_clip(1, pat, 2 * bar).unwrap();
    e.update_clip(c2, |c| c.length = bar / 2).unwrap();
    let p = e.finish().0;
    let got = tapped(&p, &song_play(), 230_000, 512);
    let v = cc(11, w(11, 0.5));
    // nothing is sent at the clips' ends: the value stays
    assert_eq!(got, vec![(0, v), (2 * bar as i64 * 25, v)]);
}

#[test]
fn trimmed_clips_start_with_the_value_at_their_offset() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(1),
        &[(0, 0.0, CurveKind::Linear), (3840, 1.0, CurveKind::Hold)],
    );
    // the second half of the bar, starting at song tick 960
    let c = e.add_clip(0, pat, 960).unwrap();
    e.update_clip(c, |c| {
        c.offset = 1920;
        c.length = 1920;
    })
    .unwrap();
    let p = e.finish().0;
    let got = tapped(&p, &song_play(), 71_990, 128);
    assert_eq!(got[0], (960 * 25, cc(1, w(1, 0.5))));
    // the last point (tick 3840) is the clip's end: the ramp reaches 1.0 within a step
    let (last_s, last) = *got.last().unwrap();
    assert!(
        last_s < (960 + 1920) * 25 && last_s > (960 + 1900) * 25,
        "{last_s}"
    );
    assert_eq!(last, cc(1, 1.0));
    assert!(got.iter().all(|(s, _)| *s >= 960 * 25 && *s < 2880 * 25));
}

#[test]
fn points_follow_tempo_changes_like_notes() {
    let (mut e, pat, ch) = base();
    lane(
        &mut e,
        pat,
        ch,
        Controller::Cc(1),
        &[
            (0, 0.1, CurveKind::Hold),
            (1920, 0.9, CurveKind::Hold),
            (2880, 0.3, CurveKind::Hold),
        ],
    );
    e.add_clip(0, pat, 0).unwrap();
    let t = e
        .add_automation_clip(14, 0, 4 * PPQ, ParamPath::Tempo)
        .unwrap();
    e.add_point(t, 0, 120.0, CurveKind::Hold, 0.0).unwrap();
    e.add_point(t, 960, 240.0, CurveKind::Hold, 0.0).unwrap();
    let p = e.finish().0;
    let a = GenAssets;
    let off = Offline::new(&p, settings(256), &a, 0).unwrap();
    let want: Vec<i64> = [0u64, 1920, 2880]
        .iter()
        .map(|t| off.rt.plan.tempo.sample_at(*t))
        .collect();
    assert_ne!(want[1], 1920 * 25, "tempo really changes");
    let got = tapped(&p, &song_play(), 59_000, 300);
    let at: Vec<i64> = got.iter().map(|(s, _)| *s).collect();
    assert_eq!(at, want);
}

// ------------------------------------------------------------------ oracle and allocation

fn cc_beat() -> Project {
    let mut e = Edit::new(&beat());
    let music = e.project().pattern_order[1];
    let drums = e.project().pattern_order[0];
    let bass = e.project().channel_order[3];
    let chords = e.project().channel_order[4];
    let kick = e.project().channel_order[0];
    lane(
        &mut e,
        music,
        chords,
        Controller::Cc(1),
        &[
            (0, 0.2, CurveKind::Smooth),
            (1920, 0.9, CurveKind::Smooth),
            (3840, 0.3, CurveKind::SCurve),
            (7000, 1.0, CurveKind::Linear),
        ],
    );
    lane(
        &mut e,
        music,
        bass,
        Controller::PitchBend,
        &[
            (0, 0.0, CurveKind::Linear),
            (960, 0.4, CurveKind::Linear),
            (1920, -0.4, CurveKind::Hold),
        ],
    );
    lane(
        &mut e,
        music,
        bass,
        Controller::Cc(1),
        &[(480, 0.8, CurveKind::Hold), (2400, 0.1, CurveKind::Hold)],
    );
    // a sampler channel too: lanes reach processors that ignore them
    lane(
        &mut e,
        drums,
        kick,
        Controller::Cc(11),
        &[(0, 0.0, CurveKind::Linear), (3840, 1.0, CurveKind::Linear)],
    );
    e.finish().0
}

fn cc_script(base: &Project) -> Vec<(usize, Act)> {
    let music = base.pattern_order[1];
    let chords = base.channel_order[4];
    let mut e = Edit::new(base);
    e.add_cc_point(
        music,
        chords,
        CcPointSpec {
            controller: Controller::Cc(1),
            tick: 5000,
            value: 0.0,
            curve: CurveKind::Linear,
            tension: 0.0,
        },
    )
    .unwrap();
    let edited = Arc::new(e.finish().0);
    vec![
        (0, Act::T(TransportCommand::Play)),
        (12_288, Act::P(edited)),
        (24_576, Act::T(TransportCommand::Seek(3 * 4 * PPQ + 777))),
        (
            40_960,
            Act::T(TransportCommand::SetLoop(Some((4 * PPQ, 8 * PPQ)))),
        ),
        (
            73_728,
            Act::T(TransportCommand::SetMode(PlayMode::Pattern(music))),
        ),
        (77_824, Act::T(TransportCommand::Play)),
        (98_304, Act::T(TransportCommand::Stop)),
        (102_400, Act::T(TransportCommand::Play)),
    ]
}

#[test]
fn lanes_change_the_sound_and_live_matches_offline_bit_for_bit() {
    let p = cc_beat();
    let plain = beat();
    let sc = cc_script(&p);
    let frames = 122_880;
    let reference = run_offline(&p, &sc, frames, 1000, 0);
    assert!(peak(&reference[0]) > 0.05);
    let with_lanes = run_offline(&p, &song_play(), frames, 1000, 0);
    let without = run_offline(&plain, &song_play(), frames, 1000, 0);
    assert!(
        with_lanes[0] != without[0],
        "the lanes are audible on the synths"
    );
    for (block, workers) in [(64, 0), (37, 3), (4096, 1)] {
        let o = run_offline(&p, &sc, frames, block, workers);
        assert_bits_eq(&reference, &o, &format!("offline block {block}"));
    }
    // the simulated device runs the callback under `assert_no_alloc`
    for (buffer, pull, threads) in [(256, 256, 1), (128, 512, 2), (512, 128, 4)] {
        let s = run_sim(&p, &sc, frames, buffer, pull, threads);
        assert_bits_eq(&reference, &s, &format!("sim {buffer}/{pull}/{threads}"));
    }
}

#[test]
fn lane_events_are_identical_across_block_sizes_with_seeks_and_swaps() {
    let p = cc_beat();
    let sc = cc_script(&p);
    let a = tapped(&p, &sc, 122_880, 64);
    let b = tapped(&p, &sc, 122_880, 1000);
    assert!(!a.is_empty());
    // block size changes where chunks split, not what is sent
    assert_eq!(a, b);
}

#[test]
fn no_lane_data_means_no_events_and_no_cost() {
    let tap = tapped(&beat(), &song_play(), 50_000, 256);
    assert!(tap.is_empty());
}

// ------------------------------------------------------------------ plugin and Haus Sampler

#[test]
fn plugin_channel_follows_lanes_and_live_matches_offline() {
    use crate::{RenderSettings, render_offline_with};
    use ph_plugin_host::testing::SYNTH_ID;
    let dir = tempfile::tempdir().unwrap();
    let env = super::plugins::env(dir.path());
    let mut e = Edit::new(&Project::empty("pl"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let ps = e
        .add_channel(
            "Plugin synth",
            InstrumentSpec::Plugin {
                format: PluginFormat::Clap,
                plugin_id: SYNTH_ID.into(),
                state: None,
                params: Default::default(),
            },
        )
        .unwrap();
    let pat = e.add_pattern("P").unwrap();
    e.add_note(pat, ps, NoteSpec::new(60, 0, 4 * PPQ)).unwrap();
    e.add_clip(0, pat, 0).unwrap();
    let render = |p: &Project| -> [Vec<f32>; 2] {
        let a = render_offline_with(
            p,
            &RenderSettings {
                mode: PlayMode::Song,
                range: None,
                sample_rate: SR,
                quality: Quality::Live,
                include_tail: false,
                block_frames: 512,
            },
            &GenAssets,
            Some(&env),
        )
        .unwrap();
        let [l, r]: [Vec<f32>; 2] = a.channels.try_into().unwrap();
        [l, r]
    };
    let plain = e.project().clone();
    lane(
        &mut e,
        pat,
        ps,
        Controller::Cc(1),
        &[
            (0, 1.0, CurveKind::Hold),
            (1920, 0.0, CurveKind::Linear),
            (3840, 0.5, CurveKind::Hold),
        ],
    );
    lane(
        &mut e,
        pat,
        ps,
        Controller::PitchBend,
        &[(0, 0.0, CurveKind::Linear), (3840, 1.0, CurveKind::Hold)],
    );
    let with = e.finish().0;
    let a = render(&plain);
    let b = render(&with);
    assert!(peak(&a[0]) > 0.01);
    assert!(a[0] != b[0], "the plugin hears the lanes");
    let frames = b[0].len();
    let (engine, mut dev) = sim(256, 2);
    engine.set_plugins(Some(env.clone())).unwrap();
    engine.set_project(Arc::new(with.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let mut live = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut at = 0;
    while at < frames {
        let n = 256.min(frames - at);
        let [l, r] = &mut live;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    assert_bits_eq(&live, &b, "plugin lanes live vs offline");
}

/// A Haus Sampler instrument playing a looped tone, its zone 24 dB down and CC1 routed to
/// the volume at +24 dB: CC1 = 0 is quiet, CC1 = 1 is full level.
pub(super) fn haus_swell_project(
    lane_points: &[(Tick, f64, CurveKind)],
) -> (Project, PatternId, ChannelId) {
    let mut e = Edit::new(&Project::empty("swell"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let mut inst = HausInstrument::new("Swell");
    inst.modulation.push(ModRoute {
        source: ModSource::Cc { controller: 1 },
        target: ModTarget::Volume,
        depth: 24.0,
        curve: ModCurve::Linear,
        group: None,
    });
    let i = e.add_instrument(inst).unwrap();
    let g = e.add_group(i, Group::new("strings")).unwrap();
    let mut z = Zone::new(g, SampleRef::File("gen:tone".into()), KeyRange::ALL, 69);
    z.volume_db = -24.0;
    e.add_zone(i, z).unwrap();
    let ch = e
        .add_channel(
            "Strings",
            InstrumentSpec::MultiSampler {
                library: None,
                instrument: Some(i),
                params: Default::default(),
            },
        )
        .unwrap();
    let pat = e.add_pattern("Swell").unwrap();
    // 2.4 s at 120 BPM = 4608 ticks; the tone sample lasts 3 s
    e.add_note(pat, ch, NoteSpec::new(69, 0, 4608)).unwrap();
    lane(&mut e, pat, ch, Controller::Cc(1), lane_points);
    e.add_clip(0, pat, 0).unwrap();
    (e.finish().0, pat, ch)
}

#[test]
fn a_cc1_swell_on_a_haus_sampler_channel_rises_and_falls_in_the_render() {
    let (p, _, _) = haus_swell_project(&[
        (0, 0.0, CurveKind::Linear),
        (2304, 1.0, CurveKind::Linear),
        (4608, 0.0, CurveKind::Hold),
    ]);
    assert!(crate::project_warnings(&p, &GenAssets).is_empty());
    let sc = vec![(0, Act::T(TransportCommand::Play))];
    let frames = 2 * SR as usize + 20_000;
    let off = run_offline(&p, &sc, frames, 512, 0);
    let win = 4_800; // 100 ms windows
    let level = |t: f64| rms(&off[0][(t * SR as f64) as usize..][..win]);
    let (start, mid, end) = (level(0.05), level(1.15), level(2.3));
    assert!(mid > 8.0 * start, "swell rises: {start} -> {mid}");
    assert!(mid > 8.0 * end, "and falls: {mid} -> {end}");
    // the peak sits near the middle of the note
    let loud = (0..20)
        .map(|k| (k, level(k as f64 * 0.12)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!((8..=11).contains(&loud.0), "peak window {}", loud.0);
    // live is the same render
    for (buffer, pull, threads) in [(256, 256, 1), (128, 512, 2)] {
        let live = run_sim(&p, &sc, frames, buffer, pull, threads);
        assert_bits_eq(&off, &live, &format!("sampler lanes {buffer}/{pull}"));
    }
    // without the lane the note sits at CC1 = 0 for the whole note: quiet
    let (flat, ..) = haus_swell_project(&[]);
    let q = run_offline(&flat, &sc, frames, 512, 0);
    assert!(rms(&q[0][(1.15 * SR as f64) as usize..][..win]) < mid / 8.0);
}
