//! M3/T0 behaviour of the minimal path: zone selection, round robin, articulations, release
//! triggers, voice limits, multi-out fallback and block-size independence.

use super::*;
use ph_core::*;

const SR: f64 = 48_000.0;

/// A mono sample whose every frame is `level` (so the output shows which zone played), or a
/// ramp `0, 1/n, 2/n, ...` when `level` is negative.
fn data(frames: usize, level: f32) -> Arc<SampleData> {
    let ch = (0..frames)
        .map(|i| {
            if level < 0.0 {
                i as f32 / frames as f32
            } else {
                level
            }
        })
        .collect();
    Arc::new(SampleData {
        sample_rate: SR,
        channels: vec![ch],
        loop_range: None,
        root_key: None,
    })
}

/// An instrument from `(group, zone)` pairs whose samples are the given data, in order.
fn build(
    groups: Vec<Group>,
    zones: Vec<(Zone, Arc<SampleData>)>,
    edit: impl FnOnce(&mut HausInstrument),
) -> HausSampler {
    let mut inst = HausInstrument::new("test");
    for (i, mut g) in groups.into_iter().enumerate() {
        g.id = GroupId(i as u32);
        inst.groups.insert(g.id, g);
    }
    let mut set = SampleSet::default();
    for (i, (mut z, d)) in zones.into_iter().enumerate() {
        z.id = ZoneId(i as u32);
        set.samples.push(ZoneSample::whole(&format!("s{i}"), d));
        set.index.insert(z.id, i);
        inst.zones.insert(z.id, z);
    }
    edit(&mut inst);
    let mut s = HausSampler::new(&inst, set);
    s.activate(&ActivateInfo {
        sample_rate: SR,
        max_frames: 4096,
        quality: Quality::Live,
        seed: 42,
    });
    s
}

fn zone(group: u32, keys: (u8, u8), root: u8) -> Zone {
    let mut z = Zone::new(
        GroupId(group),
        SampleRef::File("x.wav".into()),
        KeyRange {
            lo: keys.0,
            hi: keys.1,
        },
        root,
    );
    z.one_shot = true;
    z
}

fn on(frame: u32, id: u32, key: u8, vel: f32) -> Event {
    Event {
        frame,
        kind: EventKind::NoteOn {
            note_id: id,
            key,
            velocity: vel,
            pan: 0.0,
            fine_pitch: 0.0,
        },
    }
}

fn off(frame: u32, id: u32, key: u8) -> Event {
    Event {
        frame,
        kind: EventKind::NoteOff {
            note_id: id,
            key,
            velocity: 0.0,
        },
    }
}

/// Renders `frames` frames in blocks of `block`, events at absolute frames; returns the
/// left channel of each of `ports` ports.
fn render(
    s: &mut HausSampler,
    events: &[Event],
    frames: usize,
    block: usize,
    ports: usize,
) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; frames]; ports * 2];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        let ev: Vec<Event> = events
            .iter()
            .filter(|e| (e.frame as usize) >= pos && (e.frame as usize) < pos + n)
            .map(|e| Event {
                frame: e.frame - pos as u32,
                kind: e.kind,
            })
            .collect();
        let mut bufs: Vec<Vec<f32>> = vec![vec![0.0; n]; ports * 2];
        {
            let mut chans: Vec<&mut [f32]> = bufs.iter_mut().map(|b| &mut b[..]).collect();
            let mut port_slices: Vec<&mut [&mut [f32]]> = chans.chunks_mut(2).collect();
            let ctx = ProcessContext {
                sample_rate: SR,
                frames: n,
                song_pos_samples: pos as i64,
                song_pos_ticks: 0.0,
                bpm: 120.0,
                playing: true,
                live: true,
            };
            let mut io = ProcessIo {
                inputs: &[],
                outputs: &mut port_slices,
                events: &ev,
            };
            s.process(&ctx, &mut io);
        }
        for (c, b) in bufs.iter().enumerate() {
            out[c][pos..pos + n].copy_from_slice(b);
        }
        pos += n;
    }
    out.into_iter().step_by(2).collect()
}

#[test]
fn one_zone_one_shot_plays_the_sample_exactly() {
    let mut s = build(
        vec![Group::new("g")],
        vec![(zone(0, (0, 127), 60), data(100, -1.0))],
        |_| {},
    );
    let out = render(&mut s, &[on(10, 1, 60, 1.0), off(20, 1, 60)], 200, 64, 1);
    let ramp = data(100, -1.0);
    assert!(out[0][..10].iter().all(|x| *x == 0.0));
    assert_eq!(
        &out[0][10..110],
        &ramp.channels[0][..],
        "one-shot ignores note-off"
    );
    assert!(out[0][110..].iter().all(|x| *x == 0.0));
    assert_eq!(s.active_voices(), 0);
    // an octave up reads every other frame
    let mut s = build(
        vec![Group::new("g")],
        vec![(zone(0, (0, 127), 60), data(100, -1.0))],
        |_| {},
    );
    let out = render(&mut s, &[on(0, 1, 72, 1.0)], 60, 64, 1);
    assert_eq!(out[0][3], ramp.channels[0][6]);
    assert_eq!(out[0][50], 0.0, "ends after half the frames");
}

#[test]
fn velocity_layers_and_keys_select_zones() {
    let mut soft = zone(0, (60, 72), 60);
    soft.velocity = VelRange { lo: 0, hi: 63 };
    let mut loud = zone(0, (60, 72), 60);
    loud.velocity = VelRange { lo: 64, hi: 127 };
    let mut s = build(
        vec![Group::new("g")],
        vec![(soft, data(50, 0.25)), (loud, data(50, 0.5))],
        |_| {},
    );
    let out = render(
        &mut s,
        &[on(0, 1, 60, 0.3), on(100, 2, 60, 0.9), on(200, 3, 50, 1.0)],
        300,
        64,
        1,
    );
    assert_eq!(out[0][10], 0.25 * 0.3);
    assert_eq!(out[0][110], 0.5 * 0.9);
    assert_eq!(out[0][210], 0.0, "key 50 maps no zone");
}

#[test]
fn round_robin_cycles_and_random_is_seeded() {
    let rr = |p: u32| {
        let mut z = zone(0, (0, 127), 60);
        z.round_robin = Some(p);
        z
    };
    let notes: Vec<Event> = (0..6).map(|i| on(i * 100, i, 60, 1.0)).collect();
    let mut s = build(
        vec![Group::new("g")],
        (0..3)
            .map(|p| (rr(p), data(50, 0.1 * (p + 1) as f32)))
            .collect(),
        |_| {},
    );
    let out = render(&mut s, &notes, 600, 64, 1);
    let played: Vec<f32> = (0..6).map(|i| out[0][i * 100 + 5]).collect();
    assert_eq!(
        played,
        [0.1, 0.2, 0.3, 0.1, 0.2, 0.3].map(|x: f64| x as f32)
    );

    let random = |block: usize| {
        let mut g = Group::new("g");
        g.round_robin = RoundRobinMode::Random;
        let mut s = build(
            vec![g],
            (0..3)
                .map(|p| (rr(p), data(50, 0.1 * (p + 1) as f32)))
                .collect(),
            |_| {},
        );
        let out = render(&mut s, &notes, 600, block, 1);
        (0..6).map(|i| out[0][i * 100 + 5]).collect::<Vec<f32>>()
    };
    let a = random(64);
    assert_eq!(a, random(1000), "the same at any block size");
    assert!(
        a.windows(2).all(|w| w[0] != w[1]),
        "never twice in a row: {a:?}"
    );
    // AllNotesOff (transport stop) returns the RNG and the cycles to the seed
    let mut g = Group::new("g");
    g.round_robin = RoundRobinMode::Random;
    let mut s = build(
        vec![g],
        (0..3)
            .map(|p| (rr(p), data(50, 0.1 * (p + 1) as f32)))
            .collect(),
        |_| {},
    );
    let first = render(&mut s, &notes, 600, 64, 1);
    let stop = [Event {
        frame: 0,
        kind: EventKind::AllNotesOff,
    }];
    render(&mut s, &stop, 1, 1, 1);
    let second = render(&mut s, &notes, 600, 64, 1);
    assert_eq!(first, second);
}

#[test]
fn keyswitches_and_cc_switches_select_articulations() {
    let mut sus = Group::new("sus");
    sus.articulations = vec![ArticulationId(0)];
    let mut stac = Group::new("stac");
    stac.articulations = vec![ArticulationId(1)];
    let mut s = build(
        vec![sus, stac],
        vec![
            (zone(0, (48, 96), 60), data(50, 0.25)),
            (zone(1, (48, 96), 60), data(50, 0.5)),
        ],
        |i| {
            i.articulations = vec![
                Articulation {
                    id: ArticulationId(0),
                    name: "sus".into(),
                    trigger: ArticulationTrigger::Key { key: 24 },
                    color: None,
                },
                Articulation {
                    id: ArticulationId(1),
                    name: "stac".into(),
                    trigger: ArticulationTrigger::Cc {
                        controller: 32,
                        lo: 0.5,
                        hi: 1.0,
                    },
                    color: None,
                },
            ];
        },
    );
    let events = [
        on(0, 1, 60, 1.0),
        Event {
            frame: 100,
            kind: EventKind::Cc {
                controller: 32,
                value: 0.8,
            },
        },
        on(110, 2, 60, 1.0),
        on(200, 3, 24, 1.0), // keyswitch: no sound
        on(210, 4, 60, 1.0),
    ];
    let out = render(&mut s, &events, 300, 64, 1);
    assert_eq!(out[0][5], 0.25, "default articulation: the first");
    assert_eq!(out[0][115], 0.5);
    assert_eq!(out[0][205], 0.0, "the keyswitch itself is silent");
    assert_eq!(out[0][215], 0.25);
    assert_eq!(s.articulation(), 0);
}

#[test]
fn release_triggers_fire_on_note_off_and_decay_with_hold_time() {
    let mut rel = zone(0, (0, 127), 60);
    rel.trigger = ZoneTrigger::Release;
    rel.release_decay_db = 20.0; // -20 dB per second held
    let mut sus = zone(0, (0, 127), 60);
    sus.one_shot = false;
    let mut s = build(
        vec![Group::new("g")],
        vec![(sus, data(48_000, 0.5)), (rel, data(100, 1.0))],
        |_| {},
    );
    let out = render(
        &mut s,
        &[on(0, 1, 60, 1.0), off(48_000, 1, 60)],
        48_200,
        512,
        1,
    );
    assert_eq!(out[0][100], 0.5, "the sustain zone plays while held");
    let r = out[0][48_050];
    assert!((r - 0.1).abs() < 1e-5, "one second held: -20 dB, got {r}");
    assert_eq!(out[0][48_150], 0.0);
}

#[test]
fn voice_limits_steal_and_mono_groups_cut() {
    let mut g = Group::new("g");
    g.voice_limit = Some(2);
    g.stealing = StealMode::Oldest;
    let mut s = build(
        vec![g],
        vec![(zone(0, (0, 127), 60), data(1000, 0.1))],
        |_| {},
    );
    render(
        &mut s,
        &[on(0, 1, 60, 1.0), on(1, 2, 61, 1.0), on(2, 3, 62, 1.0)],
        10,
        64,
        1,
    );
    assert_eq!(s.active_voices(), 3, "the stolen voice fades out");
    render(&mut s, &[], 300, 64, 1);
    assert_eq!(s.active_voices(), 2);
    let mut keys: Vec<u8> = s
        .pool
        .voices
        .iter()
        .filter(|v| v.active)
        .map(|v| v.key)
        .collect();
    keys.sort();
    assert_eq!(keys, [61, 62], "the oldest gave way");
    // `none` refuses the new note
    let mut g = Group::new("g");
    g.voice_limit = Some(1);
    g.stealing = StealMode::None;
    let mut s = build(
        vec![g],
        vec![(zone(0, (0, 127), 60), data(1000, 0.1))],
        |_| {},
    );
    render(&mut s, &[on(0, 1, 60, 1.0), on(1, 2, 61, 1.0)], 10, 64, 1);
    assert_eq!(s.pool.voices.iter().find(|v| v.active).unwrap().key, 60);
    // mono: a new note cuts the old one
    let mut g = Group::new("g");
    g.polyphony = Polyphony::Mono;
    let mut s = build(
        vec![g],
        vec![(zone(0, (0, 127), 60), data(1000, 0.1))],
        |_| {},
    );
    render(&mut s, &[on(0, 1, 60, 1.0), on(1, 2, 64, 1.0)], 300, 64, 1);
    assert_eq!(s.active_voices(), 1);
}

#[test]
fn extra_outputs_use_their_port_or_fold_into_main() {
    let mut g1 = Group::new("drums");
    g1.output = 0;
    let mut g2 = Group::new("snare");
    g2.output = 1;
    let mk = || {
        build(
            vec![g1.clone(), g2.clone()],
            vec![
                (zone(0, (36, 36), 36), data(50, 0.25)),
                (zone(1, (38, 38), 38), data(50, 0.5)),
            ],
            |_| {},
        )
    };
    let s = mk();
    let d = s.descriptor();
    assert_eq!(d.id, "ph.haus_sampler");
    assert_eq!(
        d.audio_outputs
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["main", "out 2"]
    );
    let ev = [on(0, 1, 36, 1.0), on(0, 2, 38, 1.0)];
    let two = render(&mut mk(), &ev, 10, 64, 2);
    assert_eq!((two[0][5], two[1][5]), (0.25, 0.5));
    let one = render(&mut mk(), &ev, 10, 64, 1);
    assert_eq!(one[0][5], 0.75, "no port 1 from the host: folded into main");
}

#[test]
fn output_is_identical_at_any_block_size() {
    let mut g = Group::new("g");
    g.round_robin = RoundRobinMode::Random;
    let zs = (0..4)
        .map(|p| {
            let mut z = zone(0, (40, 80), 60);
            z.round_robin = Some(p);
            z.reverse = p == 3;
            (z, data(3000, -1.0))
        })
        .collect::<Vec<_>>();
    let events: Vec<Event> = (0..40)
        .map(|i| {
            on(
                i * 997 % 20_000,
                i,
                40 + (i * 7 % 40) as u8,
                0.2 + (i % 5) as f32 * 0.2,
            )
        })
        .collect::<Vec<_>>();
    let mut sorted = events.clone();
    sorted.sort_by_key(|e| e.frame);
    let mut a = build(vec![g.clone()], zs.clone(), |_| {});
    let mut b = build(vec![g], zs, |_| {});
    let x = render(&mut a, &sorted, 24_000, 1, 1);
    let y = render(&mut b, &sorted, 24_000, 777, 1);
    assert!(
        x[0].iter()
            .zip(&y[0])
            .all(|(p, q)| p.to_bits() == q.to_bits())
    );
    assert!(x[0].iter().any(|v| *v != 0.0));
}
