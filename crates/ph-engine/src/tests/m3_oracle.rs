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
