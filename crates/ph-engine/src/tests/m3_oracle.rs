//! M3/T0: the Haus Sampler (`ph.haus_sampler`) wired into the engine. A project instrument
//! with velocity layers, random round robins, a keyswitch and a release trigger plays through
//! the plan; the simulated device is bit-identical to the offline render across buffer and
//! block sizes, thread counts, a stop/start and a zone-map edit mid-song (which rebuilds the
//! node); nothing allocates on the audio thread (the test allocator aborts).

use super::*;

/// One bar per pattern clip, 4 bars: the instrument on channel 0.
fn sampler_song() -> Project {
    let mut e = Edit::new(&Project::empty("haus"));
    let mut inst = HausInstrument::new("Kit");
    inst.articulations = vec![
        Articulation {
            id: ArticulationId(0),
            name: "drums".into(),
            trigger: ArticulationTrigger::Key { key: 24 },
            color: None,
        },
        Articulation {
            id: ArticulationId(1),
            name: "tone".into(),
            trigger: ArticulationTrigger::Key { key: 25 },
            color: None,
        },
    ];
    let i = e.add_instrument(inst).unwrap();
    let mut drums = Group::new("drums");
    drums.round_robin = RoundRobinMode::Random;
    drums.articulations = vec![ArticulationId(0)];
    let drums = e.add_group(i, drums).unwrap();
    let mut tone = Group::new("tone");
    tone.articulations = vec![ArticulationId(1)];
    let tone = e.add_group(i, tone).unwrap();
    for (p, s) in ["gen:kick", "gen:snare", "gen:hat"].iter().enumerate() {
        let mut z = Zone::new(
            drums,
            SampleRef::File(s.to_string()),
            KeyRange { lo: 36, hi: 60 },
            48,
        );
        z.round_robin = Some(p as u32);
        z.one_shot = true;
        z.velocity = VelRange { lo: 0, hi: 100 };
        e.add_zone(i, z).unwrap();
    }
    let mut loud = Zone::new(
        drums,
        SampleRef::File("gen:impulse".into()),
        KeyRange { lo: 36, hi: 60 },
        48,
    );
    loud.velocity = VelRange { lo: 101, hi: 127 };
    loud.one_shot = true;
    e.add_zone(i, loud).unwrap();
    let t = e
        .add_zone(
            i,
            Zone::new(tone, SampleRef::File("gen:tone".into()), KeyRange::ALL, 69),
        )
        .unwrap();
    let mut rel = Zone::new(tone, SampleRef::File("gen:hat".into()), KeyRange::ALL, 60);
    rel.trigger = ZoneTrigger::Release;
    rel.release_decay_db = 6.0;
    e.add_zone(i, rel).unwrap();
    let _ = t;
    let ch = e
        .add_channel(
            "Kit",
            InstrumentSpec::MultiSampler {
                library: None,
                instrument: Some(i),
                params: [("volume".to_string(), -3.0)].into(),
            },
        )
        .unwrap();
    let pat = e.add_pattern("Beat").unwrap();
    for s in 0..16u64 {
        let mut n = NoteSpec::new(36 + (s % 5) as u8, s * STEP, STEP);
        n.velocity = if s % 4 == 0 { 0.95 } else { 0.6 };
        e.add_note(pat, ch, n).unwrap();
    }
    // switch to the tone articulation for the last beat, play a held note, switch back
    e.add_note(pat, ch, NoteSpec::new(25, 12 * STEP, STEP))
        .unwrap();
    e.add_note(pat, ch, NoteSpec::new(69, 12 * STEP + 10, 2 * STEP))
        .unwrap();
    e.add_note(pat, ch, NoteSpec::new(24, 15 * STEP, 10))
        .unwrap();
    for bar in 0..4 {
        e.add_clip(0, pat, bar * 4 * PPQ).unwrap();
    }
    e.finish().0
}

/// The same song after a zone-map edit: the loud layer moves up an octave.
fn edited(p: &Project) -> Project {
    let mut e = Edit::new(p);
    e.update_zone(InstrumentId(0), ZoneId(3), |z| {
        z.keys = KeyRange { lo: 48, hi: 72 };
        z.root_key = 60;
    })
    .unwrap();
    e.finish().0
}

#[test]
fn haus_sampler_plays_and_live_matches_offline() {
    let p = sampler_song();
    assert!(
        crate::project_warnings(&p, &GenAssets).is_empty(),
        "{:?}",
        crate::project_warnings(&p, &GenAssets)
    );
    let frames = 4 * 2 * SR as usize; // 4 bars at 130 BPM is ~7.4 s; 8 s covers it
    let script = vec![
        (0, Act::T(TransportCommand::Play)),
        (2 * 48_128, Act::P(Arc::new(edited(&p)))),
        (5 * 48_128, Act::T(TransportCommand::Stop)),
        (5 * 48_128 + 2048, Act::T(TransportCommand::Play)),
    ];
    let reference = run_offline(&p, &script, frames, 256, 1);
    assert!(peak(&reference[0]) > 0.05, "the sampler sounds");
    for (block, workers) in [(64, 1), (1000, 2)] {
        let off = run_offline(&p, &script, frames, block, workers);
        assert_bits_eq(&reference, &off, &format!("offline block {block}"));
    }
    for (buffer, pull, threads) in [(256, 256, 1), (128, 512, 2)] {
        let live = run_sim(&p, &script, frames, buffer, pull, threads);
        assert_bits_eq(
            &reference,
            &live,
            &format!("live buffer {buffer} pull {pull}"),
        );
    }
}

#[test]
fn a_channel_without_an_instrument_warns_and_plays_silence() {
    let mut e = Edit::new(&Project::empty("x"));
    e.add_channel(
        "Empty",
        InstrumentSpec::MultiSampler {
            library: Some("C:/Libraries/Old".into()),
            instrument: None,
            params: Default::default(),
        },
    )
    .unwrap();
    let w = crate::project_warnings(&e.finish().0, &GenAssets);
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("no instrument"), "{w:?}");
    // a zone on a sample that does not load: a warning, the rest plays
    let mut p = sampler_song();
    let mut e = Edit::new(&p);
    e.update_zone(InstrumentId(0), ZoneId(0), |z| {
        z.sample = SampleRef::File("gen:nothing".into())
    })
    .unwrap();
    p = e.finish().0;
    let w = crate::project_warnings(&p, &GenAssets);
    assert!(w.iter().any(|w| w.contains("gen:nothing")), "{w:?}");
}

// ------------------------------------------------------------------------------ M3/T2

/// An instrument that uses every M3/T2 feature, its long sample streamed (`gen:long`, with a
/// 4096-frame preload): envelopes and a filter with its envelope, LFOs, the modulation matrix
/// (LFO → pitch and pan, random → cutoff, velocity → volume and sample start), loops with a
/// crossfade and ping-pong, velocity crossfades, random round robins, a voice limit that
/// steals, a legato group with portamento, release triggers, and a group insert chain.
fn full_song() -> Project {
    let mut e = Edit::new(&Project::empty("haus-t2"));
    let mut inst = HausInstrument::new("Everything");
    inst.settings.voice_limit = 10;
    inst.settings.preload_frames = Some(4096);
    let i = e.add_instrument(inst).unwrap();
    let mut pad = Group::new("pad");
    pad.amp_env = Envelope {
        delay: 0.0,
        attack: 0.01,
        hold: 0.02,
        decay: 0.2,
        sustain: 0.5,
        release: 0.3,
    };
    pad.filter_env = Some(Envelope {
        attack: 0.05,
        decay: 0.3,
        sustain: 0.3,
        release: 0.2,
        ..Envelope::default()
    });
    pad.filter = Some(Filter {
        kind: FilterKind::LowPass24,
        cutoff: 1200.0,
        resonance: 6.0,
        key_track: 50.0,
        env_depth: 2400.0,
    });
    pad.lfos = vec![
        Lfo {
            shape: LfoShape::Sine,
            rate: 4.0,
            delay: 0.05,
            fade: 0.1,
        },
        Lfo {
            shape: LfoShape::Random,
            rate: 6.0,
            delay: 0.0,
            fade: 0.0,
        },
    ];
    pad.voice_limit = Some(3);
    pad.round_robin = RoundRobinMode::Random;
    pad.effects = vec![GroupEffect {
        name: "delay".into(),
        params: Default::default(),
        bypass: false,
        mix: 0.4,
    }];
    let pad = e.add_group(i, pad).unwrap();
    let mut lead = Group::new("lead");
    lead.polyphony = Polyphony::Legato;
    lead.portamento = 0.05;
    let lead = e.add_group(i, lead).unwrap();
    let rel = e.add_group(i, Group::new("release")).unwrap();
    // pad: two streamed round robins (one from a later start) and a crossfaded loop
    for (p, start) in [(0u32, 0u64), (1, 24_000)] {
        let mut z = Zone::new(
            pad,
            SampleRef::File("gen:long".into()),
            KeyRange { lo: 36, hi: 71 },
            57,
        );
        z.round_robin = Some(p);
        z.start = start;
        e.add_zone(i, z).unwrap();
    }
    let mut looped = Zone::new(
        pad,
        SampleRef::File("gen:tone".into()),
        KeyRange { lo: 36, hi: 71 },
        69,
    );
    looped.round_robin = Some(2);
    looped.looping = Some(ZoneLoop {
        mode: LoopMode::Forward,
        start: 12_000,
        end: 36_000,
        crossfade: 4000,
        on_release: LoopRelease::Continue,
    });
    e.add_zone(i, looped).unwrap();
    // lead: two velocity layers crossfaded with equal power, a ping-pong sustain loop
    let layer = |sample: &str, xf: (f32, f32, f32, f32)| {
        let mut z = Zone::new(
            lead,
            SampleRef::File(sample.into()),
            KeyRange { lo: 72, hi: 96 },
            57,
        );
        z.crossfades = vec![Crossfade {
            source: CrossfadeSource::Velocity,
            in_lo: xf.0,
            in_hi: xf.1,
            out_lo: xf.2,
            out_hi: xf.3,
            curve: CrossfadeCurve::Power,
        }];
        z.looping = Some(ZoneLoop {
            mode: LoopMode::PingPong,
            start: 20_000,
            end: 40_000,
            crossfade: 0,
            on_release: LoopRelease::Exit,
        });
        z
    };
    e.add_zone(i, layer("gen:harm", (0.0, 0.0, 0.5, 0.8)))
        .unwrap();
    e.add_zone(i, layer("gen:tone", (0.5, 0.8, 1.0, 1.0)))
        .unwrap();
    let mut r = Zone::new(rel, SampleRef::File("gen:hat".into()), KeyRange::ALL, 60);
    r.trigger = ZoneTrigger::Release;
    r.release_decay_db = 6.0;
    r.volume_db = -6.0;
    e.add_zone(i, r).unwrap();
    let route = |source, target, depth, curve, group| ModRoute {
        source,
        target,
        depth,
        curve,
        group,
    };
    e.update_instrument(i, |inst| {
        inst.modulation = vec![
            route(
                ModSource::Lfo { index: 0 },
                ModTarget::Pitch,
                20.0,
                ModCurve::Linear,
                Some(pad),
            ),
            route(
                ModSource::Lfo { index: 1 },
                ModTarget::Pan,
                0.3,
                ModCurve::Linear,
                Some(pad),
            ),
            route(
                ModSource::Random,
                ModTarget::Cutoff,
                600.0,
                ModCurve::Concave,
                None,
            ),
            route(
                ModSource::Velocity,
                ModTarget::Volume,
                -6.0,
                ModCurve::Convex,
                None,
            ),
            route(
                ModSource::Velocity,
                ModTarget::SampleStart,
                2000.0,
                ModCurve::Linear,
                Some(pad),
            ),
        ];
    })
    .unwrap();
    let ch = e
        .add_channel(
            "Everything",
            InstrumentSpec::MultiSampler {
                library: None,
                instrument: Some(i),
                params: [("volume".to_string(), -6.0)].into(),
            },
        )
        .unwrap();
    let pat = e.add_pattern("Song").unwrap();
    // pad chords of four (the limit of three steals), lead lines that overlap (legato)
    for bar in 0..2u64 {
        let t = bar * 4 * PPQ;
        for (k, key) in [48u8, 52, 55, 59].iter().enumerate() {
            let mut n = NoteSpec::new(*key + bar as u8 * 2, t + k as u64 * 20, 3 * PPQ);
            n.velocity = 0.5 + 0.1 * k as f32;
            e.add_note(pat, ch, n).unwrap();
        }
        for s in 0..6u64 {
            let mut n = NoteSpec::new(76 + ((s * 5 + bar) % 9) as u8, t + s * 2 * STEP, 3 * STEP);
            n.velocity = 0.4 + 0.1 * s as f32;
            e.add_note(pat, ch, n).unwrap();
        }
    }
    for bar in 0..2 {
        e.add_clip(0, pat, bar * 8 * PPQ).unwrap();
    }
    e.finish().0
}

/// M3 criterion 5 for every feature, streamed: the simulated device (buffers of 256, 128 and
/// 512 frames, one to three threads) is bit-identical to the offline render through a seek, a
/// stop and a zone edit mid-song; the audio thread never allocates (the test allocator aborts),
/// stealing included.
#[test]
fn haus_sampler_every_feature_streamed_live_matches_offline() {
    let p = full_song();
    let w = crate::project_warnings(&p, &GenAssets);
    assert!(w.is_empty(), "{w:?}");
    let edited = {
        let mut e = Edit::new(&p);
        let z = *p.instruments[&InstrumentId(0)].zones.keys().nth(2).unwrap();
        e.update_zone(InstrumentId(0), z, |z| {
            z.looping.as_mut().unwrap().crossfade = 1000;
            z.tune = 2;
        })
        .unwrap();
        e.finish().0
    };
    let frames = 8 * 48_128;
    let script = vec![
        (0, Act::T(TransportCommand::Play)),
        (2 * 48_128, Act::T(TransportCommand::Seek(6 * PPQ))),
        (3 * 48_128, Act::P(Arc::new(edited))),
        (5 * 48_128, Act::T(TransportCommand::Stop)),
        (5 * 48_128 + 4096, Act::T(TransportCommand::Play)),
    ];
    let reference = run_offline(&p, &script, frames, 256, 1);
    assert!(peak(&reference[0]) > 0.02, "it sounds");
    let off = run_offline(&p, &script, frames, 1000, 3);
    assert_bits_eq(&reference, &off, "offline block 1000, 3 workers");
    for (buffer, pull, threads) in [(256, 256, 1), (128, 512, 2), (512, 512, 3)] {
        let live = run_sim(&p, &script, frames, buffer, pull, threads);
        assert_bits_eq(
            &reference,
            &live,
            &format!("live buffer {buffer} pull {pull} threads {threads}"),
        );
    }
}

/// `gen:long` read slowly (every block read takes 40 ms): a `Live` stream never catches up.
struct SlowAssets;

struct SlowStream;

impl SampleStream for SlowStream {
    fn frames(&self) -> u64 {
        GenStream.frames()
    }
    fn sample_rate(&self) -> f64 {
        SR as f64
    }
    fn channels(&self) -> usize {
        2
    }
    fn read(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, StreamError> {
        // the head is read when the instrument loads: only later reads are slow
        if start > 0 {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        GenStream.read(start, out)
    }
}

impl AssetResolver for SlowAssets {
    fn stream(&self, reference: &str) -> Option<Result<Box<dyn SampleStream>, String>> {
        (reference == "gen:long").then(|| Ok(Box::new(SlowStream) as Box<dyn SampleStream>))
    }
    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        GenAssets.sample(reference)
    }
}

/// A starved stream under the real device's policy: past the preload the voice is silent,
/// the underruns are counted for `EngineStats`, the reader never waits and nothing
/// allocates.
#[test]
fn a_starved_sampler_stream_is_silence_and_never_allocates() {
    let mut e = Edit::new(&Project::empty("starve"));
    let mut inst = HausInstrument::new("Long");
    inst.settings.preload_frames = Some(2048);
    let i = e.add_instrument(inst).unwrap();
    let g = e.add_group(i, Group::new("g")).unwrap();
    e.add_zone(
        i,
        Zone::new(g, SampleRef::File("gen:long".into()), KeyRange::ALL, 60),
    )
    .unwrap();
    let spec = InstrumentSpec::MultiSampler {
        library: None,
        instrument: Some(i),
        params: Default::default(),
    };
    let p = e.finish().0;
    let mut audio = crate::player::AudioCtx::new(crate::stream::StreamPolicy::Live);
    let (mut node, w) =
        crate::plan::create_instrument(&p, &spec, &SlowAssets, Some(&mut audio)).unwrap();
    assert!(w.is_empty(), "{w:?}");
    node.activate(&ph_node::ActivateInfo {
        sample_rate: SR as f64,
        max_frames: 256,
        quality: Quality::Live,
        seed: 1,
    });
    let mut l = vec![0.0f32; 256];
    let mut r = vec![0.0f32; 256];
    let mut out = Vec::new();
    let t0 = std::time::Instant::now();
    for b in 0..40u32 {
        let events = if b == 0 {
            vec![ph_node::Event {
                frame: 0,
                kind: ph_node::EventKind::NoteOn {
                    note_id: 1,
                    key: 60,
                    velocity: 1.0,
                    pan: 0.0,
                    fine_pitch: 0.0,
                },
            }]
        } else {
            Vec::new()
        };
        let ctx = ph_node::ProcessContext {
            sample_rate: SR as f64,
            frames: 256,
            song_pos_samples: b as i64 * 256,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        assert_no_alloc::assert_no_alloc(|| {
            let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
            let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
            let mut io = ph_node::ProcessIo {
                inputs: &[],
                outputs: &mut ports,
                events: &events,
            };
            node.process(&ctx, &mut io);
        });
        out.extend_from_slice(&l);
    }
    assert!(
        t0.elapsed() < std::time::Duration::from_millis(400),
        "the audio thread waited for the disk"
    );
    assert!(peak(&out[..2000]) > 0.05, "the preload plays");
    assert!(
        out[2100..3900].iter().all(|x| *x == 0.0),
        "past the preload, nothing has arrived: silence"
    );
    let underruns = audio.underruns.load(std::sync::atomic::Ordering::Relaxed);
    assert!(underruns > 0, "underruns are counted");
}
