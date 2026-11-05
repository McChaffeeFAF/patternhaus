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
    activate(&mut s, Quality::Live);
    s
}

/// The instrument [`build`] makes, and its sample set, before activation.
fn instrument(
    groups: Vec<Group>,
    zones: Vec<(Zone, Arc<SampleData>)>,
    edit: impl FnOnce(&mut HausInstrument),
) -> (HausInstrument, SampleSet) {
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
    (inst, set)
}

fn activate(s: &mut HausSampler, quality: Quality) {
    s.activate(&ActivateInfo {
        sample_rate: SR,
        max_frames: 4096,
        quality,
        seed: 42,
    });
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

// ------------------------------------------------------------------------------ M3/T2

fn sine(freq: f64, frames: usize) -> Arc<SampleData> {
    let ch = (0..frames)
        .map(|i| (std::f64::consts::TAU * freq * i as f64 / SR).sin() as f32)
        .collect();
    Arc::new(SampleData {
        sample_rate: SR,
        channels: vec![ch],
        loop_range: None,
        root_key: None,
    })
}

fn cc(frame: u32, controller: u8, value: f32) -> Event {
    Event {
        frame,
        kind: EventKind::Cc { controller, value },
    }
}

/// Known answer: an A4 sample played a fifth up reads at 2^(7/12), in Live (Hermite) and High
/// (windowed sinc) quality, within each interpolator's error.
#[test]
fn pitch_known_answer_live_and_high_quality() {
    for (quality, tol) in [(Quality::Live, 2e-3), (Quality::High, 5e-4)] {
        let (inst, set) = instrument(
            vec![Group::new("g")],
            vec![(zone(0, (0, 127), 69), sine(440.0, 96_000))],
            |_| {},
        );
        let mut s = HausSampler::new(&inst, set);
        activate(&mut s, quality);
        let out = render(&mut s, &[on(0, 1, 76, 1.0)], 24_000, 256, 1);
        let ratio = (7.0f64 / 12.0).exp2();
        let mut err = 0.0f64;
        for (i, y) in out[0].iter().enumerate().skip(200) {
            let want = (std::f64::consts::TAU * 440.0 * ratio * i as f64 / SR).sin();
            err = err.max((*y as f64 - want).abs());
        }
        assert!(err < tol, "{quality:?}: error {err}");
    }
}

/// Known answer: a forward loop with a crossfade has no step at its seam (a ramp sample, so a
/// hard loop would jump by the loop's height); ping-pong turns; a sustain loop (`exit`) is
/// left at note-off and plays on to the end, a `continue` loop keeps going.
#[test]
fn loop_crossfade_is_continuous_and_loops_turn_and_exit() {
    let looped = |crossfade: u64, mode: LoopMode, on_release: LoopRelease| {
        let mut z = zone(0, (0, 127), 60);
        z.looping = Some(ZoneLoop {
            mode,
            start: 2000,
            end: 6000,
            crossfade,
            on_release,
        });
        build(vec![Group::new("g")], vec![(z, data(10_000, -1.0))], |_| {})
    };
    let max_step = |x: &[f32]| {
        x.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let hard = render(
        &mut looped(0, LoopMode::Forward, LoopRelease::Continue),
        &[on(0, 1, 60, 1.0)],
        20_000,
        256,
        1,
    );
    assert!(max_step(&hard[0]) > 0.3, "a hard loop jumps");
    let soft = render(
        &mut looped(1000, LoopMode::Forward, LoopRelease::Continue),
        &[on(0, 1, 60, 1.0)],
        20_000,
        256,
        1,
    );
    assert!(max_step(&soft[0]) < 5e-4, "{}", max_step(&soft[0]));
    // right after the seam the loop plays its start again
    assert_eq!(soft[0][6000], data(10_000, -1.0).channels[0][2000]);
    // ping-pong: up to the end, then back down
    let pp = render(
        &mut looped(0, LoopMode::PingPong, LoopRelease::Continue),
        &[on(0, 1, 60, 1.0)],
        12_000,
        256,
        1,
    );
    assert!(pp[0][5990] > pp[0][5980] && pp[0][6010] < pp[0][6000]);
    assert!(
        pp[0][10_010] > pp[0][10_000],
        "turns again at the loop start"
    );
    // sustain loop on a one-shot zone: loops while held, plays out after note-off
    let mut s = looped(0, LoopMode::Forward, LoopRelease::Exit);
    render(&mut s, &[on(0, 1, 60, 1.0)], 30_000, 256, 1);
    assert_eq!(s.active_voices(), 1, "still looping while held");
    render(&mut s, &[off(0, 1, 60)], 10_000, 256, 1);
    assert_eq!(s.active_voices(), 0, "left the loop and reached the end");
    let mut s = looped(0, LoopMode::Forward, LoopRelease::Continue);
    render(
        &mut s,
        &[on(0, 1, 60, 1.0), off(100, 1, 60)],
        30_000,
        256,
        1,
    );
    assert_eq!(s.active_voices(), 1, "a continue loop keeps going");
}

/// Known answer: the group's amp envelope shapes the output frame by frame (linear attack,
/// linear release from the held level).
#[test]
fn amp_envelope_stages_in_the_output() {
    let mut g = Group::new("g");
    g.amp_env = Envelope {
        delay: 0.0,
        attack: 0.001, // 48 frames
        hold: 0.0,
        decay: 0.0,
        sustain: 1.0,
        release: 0.002, // 96 frames
    };
    let mut z = zone(0, (0, 127), 60);
    z.one_shot = false;
    let mut s = build(vec![g], vec![(z, data(10_000, 0.5))], |_| {});
    let out = render(&mut s, &[on(0, 1, 60, 1.0), off(1000, 1, 60)], 1200, 64, 1);
    for k in [0usize, 12, 24, 47] {
        assert!(
            (out[0][k] - 0.5 * k as f32 / 48.0).abs() < 1e-6,
            "attack {k}"
        );
    }
    assert_eq!(out[0][500], 0.5);
    for k in [0usize, 24, 48, 95] {
        let want = 0.5 * (1.0 - k as f32 / 96.0);
        assert!((out[0][1000 + k] - want).abs() < 1e-6, "release {k}");
    }
    assert_eq!(out[0][1096], 0.0);
    assert_eq!(s.active_voices(), 0);
}

/// Known answer: a stolen voice fades out linearly over 3 ms (144 frames at 48 kHz) while the
/// new note starts at once.
#[test]
fn stealing_fades_known_answer() {
    let mut g = Group::new("g");
    g.voice_limit = Some(1);
    let a = zone(0, (60, 60), 60);
    let b = zone(0, (61, 61), 61);
    let mut s = build(
        vec![g],
        vec![(a, data(5000, 0.5)), (b, data(5000, 0.25))],
        |_| {},
    );
    let out = render(
        &mut s,
        &[on(0, 1, 60, 1.0), on(100, 2, 61, 1.0)],
        400,
        64,
        1,
    );
    assert_eq!(out[0][99], 0.5);
    for k in [0usize, 36, 72, 143] {
        let want = 0.5 * (144 - k) as f32 / 144.0 + 0.25;
        assert!((out[0][100 + k] - want).abs() < 1e-6, "fade frame {k}");
    }
    assert_eq!(out[0][244], 0.25);
    assert_eq!(s.active_voices(), 1);
}

/// Known answer: a legato group glides the held voice to the new key over the portamento
/// time (linear in semitones) without retriggering; a mono group retriggers and glides from
/// the previous pitch.
#[test]
fn legato_glide_time_known_answer() {
    for poly in [Polyphony::Legato, Polyphony::Mono] {
        let mut g = Group::new("g");
        g.polyphony = poly;
        g.portamento = 0.1; // 4800 frames
        let mut z = zone(0, (0, 127), 60);
        z.one_shot = false;
        let mut s = build(vec![g], vec![(z, data(96_000, 0.5))], |_| {});
        render(
            &mut s,
            &[on(0, 1, 60, 1.0), on(1000, 2, 72, 1.0)],
            1000 + 2400,
            64,
            1,
        );
        let v = s.voices().iter().find(|v| v.counts()).unwrap().clone();
        assert_eq!(v.key, 72);
        assert!(
            (v.step - 2.0).abs() < 1e-12,
            "{poly:?}: the new key's pitch"
        );
        // half way: -6 semitones to go (control rate: within one update of 32 frames)
        assert!((v.glide + 6.0).abs() < 0.1, "{poly:?}: {}", v.glide);
        if poly == Polyphony::Legato {
            assert_eq!(s.active_voices(), 1, "legato: one voice, not retriggered");
            assert!(v.pos > 3000.0, "legato keeps the sample position");
        }
        render(&mut s, &[], 2400 + 64, 64, 1);
        let v = s.voices().iter().find(|v| v.counts()).unwrap();
        assert_eq!(v.glide, 0.0);
        assert_eq!(v.rate, 2.0, "{poly:?}: arrived");
    }
}

/// The matrix: velocity → volume (dB), CC → pitch (cents), pitch bend over the bend range,
/// pressure → volume, a square LFO → pan, random per note → sample start (seeded).
#[test]
fn modulation_matrix_sources_and_targets() {
    let route = |source, target, depth| ModRoute {
        source,
        target,
        depth,
        curve: ModCurve::Linear,
        group: None,
    };
    let mut z = zone(0, (0, 127), 60);
    z.one_shot = false;
    let mk = |routes: Vec<ModRoute>, lfos: Vec<Lfo>| {
        let mut g = Group::new("g");
        g.lfos = lfos;
        build(vec![g], vec![(z.clone(), data(96_000, 0.5))], |i| {
            i.modulation = routes;
            i.settings.bend_up = 2.0;
        })
    };
    // velocity → -6 dB at full velocity
    let mut s = mk(
        vec![route(ModSource::Velocity, ModTarget::Volume, -6.0)],
        vec![],
    );
    let out = render(&mut s, &[on(0, 1, 60, 1.0)], 100, 64, 1);
    assert!((out[0][50] - 0.5 * 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
    // CC 1 → +1 octave; pitch bend up a whole range: +2 semitones
    let mut s = mk(
        vec![route(
            ModSource::Cc { controller: 1 },
            ModTarget::Pitch,
            1200.0,
        )],
        vec![],
    );
    render(&mut s, &[on(0, 1, 60, 1.0), cc(10, 1, 1.0)], 100, 64, 1);
    assert_eq!(s.voices().iter().find(|v| v.active).unwrap().rate, 2.0);
    let bend = Event {
        frame: 0,
        kind: EventKind::PitchBend { value: 1.0 },
    };
    render(&mut s, &[bend, cc(0, 1, 0.0)], 100, 64, 1);
    let r = s.voices().iter().find(|v| v.active).unwrap().rate;
    assert!((r - (2.0f64 / 12.0).exp2()).abs() < 1e-12, "{r}");
    // pressure → volume
    let mut s = mk(
        vec![route(ModSource::Aftertouch, ModTarget::Volume, -20.0)],
        vec![],
    );
    let press = Event {
        frame: 10,
        kind: EventKind::NoteExpression {
            note_id: 1,
            kind: NoteExpressionKind::Pressure,
            value: 1.0,
        },
    };
    let out = render(&mut s, &[on(0, 1, 60, 1.0), press], 200, 64, 1);
    assert!((out[0][150] - 0.05).abs() < 1e-6, "{}", out[0][150]);
    // a 1 Hz square LFO → pan: right while positive, left after half a cycle
    let lfo = Lfo {
        shape: LfoShape::Square,
        rate: 1.0,
        delay: 0.0,
        fade: 0.0,
    };
    let mut s = mk(
        vec![route(ModSource::Lfo { index: 0 }, ModTarget::Pan, 1.0)],
        vec![lfo],
    );
    let gain = |s: &HausSampler| s.voices().iter().find(|v| v.active).unwrap().gain;
    let out = render(&mut s, &[on(0, 1, 60, 1.0)], 1000, 512, 1);
    assert_eq!(out[0][900], 0.0, "panned right: no left");
    assert_eq!(gain(&s), [0.0, 1.0]);
    let out = render(&mut s, &[], 29_000, 512, 1);
    assert_eq!(out[0][28_000], 0.5, "second half: left");
    assert_eq!(gain(&s), [1.0, 0.0]);
    // random → sample start: seeded (the same twice), and not at the zone start
    let starts = || {
        let mut s = mk(
            vec![route(ModSource::Random, ModTarget::SampleStart, 40_000.0)],
            vec![],
        );
        render(&mut s, &[on(0, 1, 60, 1.0)], 1, 1, 1);
        s.voices().iter().find(|v| v.active).unwrap().pos
    };
    let (a, b) = (starts(), starts());
    assert_eq!(a, b);
    assert!(a > 0.0, "{a}");
}

/// A group low-pass (24 dB at 500 Hz) removes a 10 kHz tone; key tracking and the filter
/// envelope move the cutoff.
#[test]
fn group_filter_attenuates_and_tracks() {
    let mut g = Group::new("g");
    g.filter = Some(Filter {
        kind: FilterKind::LowPass24,
        cutoff: 500.0,
        resonance: 0.0,
        key_track: 0.0,
        env_depth: 0.0,
    });
    let mut s = build(
        vec![g],
        vec![(zone(0, (0, 127), 60), sine(10_000.0, 48_000))],
        |_| {},
    );
    let out = render(&mut s, &[on(0, 1, 60, 1.0)], 24_000, 256, 1);
    let rms = (out[0][4000..].iter().map(|x| x * x).sum::<f32>() / 20_000.0).sqrt();
    assert!(rms < 0.001, "{rms}");
    // a 200 Hz tone passes
    let mut g = Group::new("g");
    g.filter = Some(Filter {
        kind: FilterKind::LowPass24,
        cutoff: 500.0,
        resonance: 0.0,
        key_track: 0.0,
        env_depth: 0.0,
    });
    let mut s = build(
        vec![g],
        vec![(zone(0, (0, 127), 60), sine(200.0, 48_000))],
        |_| {},
    );
    let out = render(&mut s, &[on(0, 1, 60, 1.0)], 24_000, 256, 1);
    let rms = (out[0][4000..].iter().map(|x| x * x).sum::<f32>() / 20_000.0).sqrt();
    assert!(rms > 0.6, "{rms}");
}

/// Dynamics on the mod wheel: two layers crossfaded over CC 1 with equal power; moving the
/// wheel mid-note moves the layers' gains.
#[test]
fn cc_crossfades_follow_the_mod_wheel() {
    let xf = |i: (f32, f32), o: (f32, f32)| Crossfade {
        source: CrossfadeSource::Cc { controller: 1 },
        in_lo: i.0,
        in_hi: i.1,
        out_lo: o.0,
        out_hi: o.1,
        curve: CrossfadeCurve::Power,
    };
    let mut soft = zone(0, (0, 127), 60);
    soft.crossfades = vec![xf((0.0, 0.0), (0.0, 1.0))];
    let mut loud = zone(0, (0, 127), 60);
    loud.crossfades = vec![xf((0.0, 1.0), (1.0, 1.0))];
    let mut s = build(
        vec![Group::new("g")],
        vec![(soft, data(96_000, 0.5)), (loud, data(96_000, 0.25))],
        |_| {},
    );
    let out = render(
        &mut s,
        &[on(0, 1, 60, 1.0), cc(1000, 1, 0.5), cc(2000, 1, 1.0)],
        3000,
        64,
        1,
    );
    assert_eq!(out[0][500], 0.5, "wheel down: the soft layer only");
    let h = std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (out[0][1500] - (0.5 * h + 0.25 * h)).abs() < 1e-5,
        "{}",
        out[0][1500]
    );
    assert!(
        (out[0][2500] - 0.25).abs() < 1e-6,
        "wheel up: the loud layer only"
    );
}

/// A gain stage standing in for a built-in effect (the engine passes real ones).
struct Gain(NodeDescriptor, f32);

impl Processor for Gain {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.0
    }
    fn activate(&mut self, _: &ActivateInfo) {}
    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        for c in 0..2 {
            for i in 0..ctx.frames {
                io.outputs[0][c][i] = io.inputs[0][c][i] * self.1;
            }
        }
        ProcessStatus::Sleep
    }
    fn set_param(&mut self, _: ParamId, _: f64) {}
    fn param(&self, _: ParamId) -> f64 {
        0.0
    }
    fn reset(&mut self) {}
}

fn gain_fx(g: f32) -> Box<dyn Processor> {
    let mut d = descriptor(0);
    d.id = "test.gain".into();
    Box::new(Gain(d, g))
}

/// A group's insert chain runs in order with each slot's dry/wet mix, and only on that group.
#[test]
fn group_effects_run_in_order_with_mix() {
    let (inst, set) = instrument(
        vec![Group::new("fx"), Group::new("dry")],
        vec![
            (zone(0, (60, 60), 60), data(1000, 0.5)),
            (zone(1, (61, 61), 61), data(1000, 0.5)),
        ],
        |_| {},
    );
    let mut s = HausSampler::new(&inst, set);
    // ×3 at half mix is ×2, then ×0.25 fully wet
    s.set_group_effects(0, vec![(gain_fx(3.0), 0.5), (gain_fx(0.25), 1.0)]);
    activate(&mut s, Quality::Live);
    let out = render(&mut s, &[on(0, 1, 60, 1.0)], 1100, 64, 1);
    assert_eq!(out[0][10], 0.5 * 2.0 * 0.25);
    let out = render(&mut s, &[on(0, 2, 61, 1.0)], 100, 64, 1);
    assert_eq!(out[0][10], 0.5, "the other group plays dry");
}

/// A [`StreamSource`] over memory, for the sampler's side of streaming.
struct MemStream {
    data: Vec<Arc<SampleData>>,
    starve: bool,
    underruns: std::sync::atomic::AtomicU64,
    wants: std::sync::atomic::AtomicU64,
}

impl StreamSource for MemStream {
    fn want(&self, _: usize, _: usize, _: u64, _: bool) {
        self.wants
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn release(&self, _: usize) {}
    fn read(&self, source: usize, start: i64, out: [&mut [f32]; 2]) -> bool {
        let [l, r] = out;
        if self.starve {
            l.fill(0.0);
            r.fill(0.0);
            self.underruns
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        let d = &self.data[source].channels;
        for i in 0..l.len() {
            let f = start + i as i64;
            let at = |c: &Vec<f32>| {
                if f >= 0 {
                    c.get(f as usize).copied().unwrap_or(0.0)
                } else {
                    0.0
                }
            };
            l[i] = at(&d[0]);
            r[i] = at(d.get(1).unwrap_or(&d[0]));
        }
        true
    }
}

/// `zones` with their samples held as a head of `head` frames plus a stream.
fn build_streamed(
    groups: Vec<Group>,
    zones: Vec<(Zone, Arc<SampleData>)>,
    head: usize,
    starve: bool,
    quality: Quality,
) -> (HausSampler, Arc<MemStream>) {
    let (inst, whole) = instrument(groups, zones, |_| {});
    let mut set = SampleSet::default();
    let mut data = Vec::new();
    for (i, s) in whole.samples.iter().enumerate() {
        let h = Arc::new(SampleData {
            sample_rate: s.head.sample_rate,
            channels: s.head.channels.iter().map(|c| c[..head].to_vec()).collect(),
            loop_range: None,
            root_key: None,
        });
        set.samples.push(ZoneSample {
            reference: s.reference.clone(),
            head: h,
            frames: s.frames,
            complete: false,
            stream: Some(i),
        });
        data.push(s.head.clone());
    }
    set.index = whole.index.clone();
    let src = Arc::new(MemStream {
        data,
        starve,
        underruns: Default::default(),
        wants: Default::default(),
    });
    set.streams = Some(src.clone());
    let mut s = HausSampler::new(&inst, set);
    activate(&mut s, quality);
    (s, src)
}

/// Streamed playback (a head in memory, the rest read through the stream window) is
/// bit-identical to the whole sample in memory, forwards, pitched, in High quality; a
/// starved stream is silence past the head and counts underruns.
#[test]
fn streamed_playback_matches_whole_and_starves_to_silence() {
    for quality in [Quality::Live, Quality::High] {
        let zones = || vec![(zone(0, (0, 127), 60), sine(330.0, 30_000))];
        let events = [
            on(0, 1, 60, 1.0),
            on(3000, 2, 67, 0.8),
            on(5000, 3, 55, 0.6),
        ];
        let (inst, set) = instrument(vec![Group::new("g")], zones(), |_| {});
        let mut whole = HausSampler::new(&inst, set);
        activate(&mut whole, quality);
        let want = render(&mut whole, &events, 40_000, 256, 1);
        let (mut streamed, src) =
            build_streamed(vec![Group::new("g")], zones(), 1000, false, quality);
        let got = render(&mut streamed, &events, 40_000, 256, 1);
        assert!(
            want[0]
                .iter()
                .zip(&got[0])
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "{quality:?}"
        );
        assert!(src.wants.load(std::sync::atomic::Ordering::Relaxed) > 3);
    }
    let (mut s, src) = build_streamed(
        vec![Group::new("g")],
        vec![(zone(0, (0, 127), 60), data(30_000, 0.5))],
        1000,
        true,
        Quality::Live,
    );
    let out = render(&mut s, &[on(0, 1, 60, 1.0)], 3000, 256, 1);
    assert_eq!(out[0][500], 0.5, "the head plays");
    assert!(out[0][1100..].iter().all(|x| *x == 0.0), "starved: silence");
    assert!(src.underruns.load(std::sync::atomic::Ordering::Relaxed) > 0);
}

/// Every feature at once is identical at any block size (control updates count voice frames,
/// not blocks).
#[test]
fn all_features_identical_at_any_block_size() {
    let mk = || {
        let mut g = Group::new("g");
        g.amp_env.attack = 0.003;
        g.amp_env.decay = 0.05;
        g.amp_env.sustain = 0.6;
        g.amp_env.release = 0.02;
        g.filter_env = Some(Envelope {
            attack: 0.01,
            decay: 0.1,
            sustain: 0.2,
            ..Envelope::default()
        });
        g.filter = Some(Filter {
            kind: FilterKind::LowPass12,
            cutoff: 2000.0,
            resonance: 6.0,
            key_track: 50.0,
            env_depth: 2400.0,
        });
        g.lfos = vec![
            Lfo {
                shape: LfoShape::Sine,
                rate: 5.0,
                delay: 0.01,
                fade: 0.05,
            },
            Lfo {
                shape: LfoShape::Random,
                rate: 9.0,
                delay: 0.0,
                fade: 0.0,
            },
        ];
        g.voice_limit = Some(3);
        g.round_robin = RoundRobinMode::Random;
        let mut legato = Group::new("lead");
        legato.polyphony = Polyphony::Legato;
        legato.portamento = 0.05;
        let mut zs = Vec::new();
        for p in 0..3 {
            let mut z = zone(0, (40, 80), 60);
            z.one_shot = false;
            z.round_robin = Some(p);
            z.looping = Some(ZoneLoop {
                mode: if p == 1 {
                    LoopMode::PingPong
                } else {
                    LoopMode::Forward
                },
                start: 2000,
                end: 7000,
                crossfade: 500,
                on_release: LoopRelease::Exit,
            });
            zs.push((z, sine(220.0 + 50.0 * p as f64, 9000)));
        }
        let mut lead = zone(1, (81, 100), 90);
        lead.one_shot = false;
        lead.crossfades = vec![Crossfade {
            source: CrossfadeSource::Cc { controller: 1 },
            in_lo: 0.0,
            in_hi: 0.5,
            out_lo: 1.0,
            out_hi: 1.0,
            curve: CrossfadeCurve::Power,
        }];
        zs.push((lead, sine(440.0, 48_000)));
        let (inst, set) = instrument(vec![g, legato], zs, |i| {
            i.modulation = vec![
                ModRoute {
                    source: ModSource::Lfo { index: 0 },
                    target: ModTarget::Pitch,
                    depth: 30.0,
                    curve: ModCurve::Linear,
                    group: None,
                },
                ModRoute {
                    source: ModSource::Lfo { index: 1 },
                    target: ModTarget::Pan,
                    depth: 0.5,
                    curve: ModCurve::Linear,
                    group: None,
                },
                ModRoute {
                    source: ModSource::Random,
                    target: ModTarget::Cutoff,
                    depth: 600.0,
                    curve: ModCurve::Concave,
                    group: None,
                },
                ModRoute {
                    source: ModSource::Velocity,
                    target: ModTarget::SampleStart,
                    depth: 300.0,
                    curve: ModCurve::Convex,
                    group: None,
                },
                ModRoute {
                    source: ModSource::Envelope { index: 1 },
                    target: ModTarget::Resonance,
                    depth: 6.0,
                    curve: ModCurve::Linear,
                    group: None,
                },
            ];
        });
        let mut s = HausSampler::new(&inst, set);
        s.set_group_effects(1, vec![(gain_fx(0.5), 0.7)]);
        activate(&mut s, Quality::Live);
        s
    };
    let mut events = Vec::new();
    for i in 0..60u32 {
        let f = i * 811 % 40_000;
        let key = if i.is_multiple_of(4) {
            81 + (i % 12) as u8
        } else {
            40 + (i * 7 % 40) as u8
        };
        events.push(on(f, i, key, 0.3 + (i % 7) as f32 * 0.1));
        events.push(off(f + 3000 + i * 13 % 900, i, key));
        if i.is_multiple_of(9) {
            events.push(cc(f + 7, 1, (i % 5) as f32 / 4.0));
            events.push(Event {
                frame: f + 11,
                kind: EventKind::PitchBend {
                    value: (i % 3) as f32 - 1.0,
                },
            });
        }
    }
    events.sort_by_key(|e| e.frame);
    let a = render(&mut mk(), &events, 48_000, 1, 1);
    let b = render(&mut mk(), &events, 48_000, 777, 1);
    let c = render(&mut mk(), &events, 48_000, 64, 1);
    assert!(a[0].iter().any(|v| *v != 0.0));
    for (x, name) in [(&b, "777"), (&c, "64")] {
        let same = a[0]
            .iter()
            .zip(&x[0])
            .all(|(p, q)| p.to_bits() == q.to_bits());
        assert!(same, "block {name}");
    }
}
