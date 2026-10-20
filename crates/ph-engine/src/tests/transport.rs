//! Voice preservation across swaps, previews, metronome, meters/stats, tails, stop.

use super::*;
use crate::{PlayMode, RenderSettings, render_offline};

fn pad_project(cutoff: f64, wave: f64) -> (Project, ChannelId) {
    let mut e = Edit::new(&Project::empty("pad"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let c = e
        .add_channel(
            "Pad",
            synth(&[
                ("osc1_wave", wave),
                ("f1_cutoff", cutoff),
                ("amp_attack", 0.0),
            ]),
        )
        .unwrap();
    let p = e.add_pattern("P").unwrap();
    e.add_note(p, c, NoteSpec::new(57, 0, 16 * PPQ)).unwrap();
    e.add_clip(0, p, 0).unwrap();
    (e.finish().0, c)
}

#[test]
fn param_only_change_keeps_voices_without_a_click() {
    let (a, ch) = pad_project(3000.0, 0.0);
    let (b, _) = pad_project(1200.0, 0.0);
    let mut c = b.clone();
    // also a strip change: volume ramps, no jump
    Arc::make_mut(&mut c.channels).get_mut(&ch).unwrap().volume = 0.5;
    let swap = 24_576;
    let sc = vec![
        (0, Act::T(TransportCommand::Play)),
        (swap, Act::P(Arc::new(c))),
    ];
    let out = run_sim(&a, &sc, 48_000, 256, 256, 2);
    let x = &out[0];
    let before = rms(&x[swap - 4800..swap]);
    let after = rms(&x[swap + 4800..swap + 9600]);
    assert!(before > 0.02, "pad silent before the swap");
    // the note started before the swap still sounds (a fresh instance would be silent)
    assert!(after > 0.2 * before, "voice lost: {after} vs {before}");
    // no click: the largest step around the swap is like the steps before it
    let step =
        |r: std::ops::Range<usize>| r.map(|i| (x[i] - x[i - 1]).abs()).fold(0.0f32, f32::max);
    let normal = step(swap - 4800..swap);
    let at = step(swap..swap + 256);
    assert!(at <= normal * 1.5 + 1e-4, "click at swap: {at} vs {normal}");
}

#[test]
fn changed_instrument_kind_gets_a_new_instance() {
    let (a, ch) = pad_project(3000.0, 0.0);
    let mut b = a.clone();
    Arc::make_mut(&mut b.channels)
        .get_mut(&ch)
        .unwrap()
        .instrument = sampler("gen:dc", &[]);
    let swap = 24_576;
    let sc = vec![
        (0, Act::T(TransportCommand::Play)),
        (swap, Act::P(Arc::new(b))),
    ];
    let out = run_sim(&a, &sc, 48_000, 256, 256, 1);
    // the new sampler has no sounding note (its note started before the swap)
    assert!(peak(&out[0][swap + 512..]) < 1e-6);
}

#[test]
fn preview_notes_sound_while_stopped() {
    let (p, ch) = pad_project(3000.0, 2.0);
    let sc = vec![
        (4096, Act::Preview(ch, 60, 1.0, true)),
        (24_576, Act::Preview(ch, 60, 0.0, false)),
    ];
    let out = run_sim(&p, &sc, 48_000, 256, 256, 1);
    assert_eq!(
        peak(&out[0][..4096]),
        0.0,
        "stopped and silent before the preview"
    );
    assert!(rms(&out[0][8000..24_000]) > 0.01);
    assert!(peak(&out[0][40_000..]) < 1e-4, "preview note released");
    let off = run_offline(&p, &sc, 48_000, 300, 0);
    assert_bits_eq(&off, &out, "preview oracle");
}

#[test]
fn metronome_clicks_on_beats() {
    let p = Project::empty("m");
    let sc = vec![
        (0, Act::T(TransportCommand::SetMetronome(true))),
        (0, Act::T(TransportCommand::Play)),
    ];
    // 130 BPM: a beat is 22153.8 samples
    let out = run_sim(&p, &sc, 70_000, 256, 256, 1);
    let map = crate::tempo::TempoMap::constant(130.0, SR);
    for k in 0..3 {
        let s = map.sample_at(k * PPQ) as usize;
        assert!(peak(&out[0][s..s + 200]) > 0.1, "no click at beat {k}");
        if s > 0 {
            assert!(peak(&out[0][s - 2000..s]) < 1e-3);
        }
    }
}

#[test]
fn stop_silences_and_rewinds() {
    let (p, _) = pad_project(3000.0, 0.0);
    let (engine, mut dev) = sim(256, 1);
    engine.set_project(Arc::new(p)).unwrap();
    engine.transport(TransportCommand::Play);
    for _ in 0..40 {
        dev.pull(256);
    }
    let st = engine.status();
    assert!(st.playing && st.position > 0);
    let st = engine.transport(TransportCommand::Stop);
    assert!(!st.playing && st.position == 0);
    dev.pull(256);
    let b = dev.pull(256);
    assert_eq!(peak(&b.channels[0]), 0.0, "AllNotesOff on stop");
    assert_eq!(engine.status().position, 0);
}

#[test]
fn meters_and_stats_are_published() {
    let (engine, mut dev) = sim(256, 2);
    engine.set_project(Arc::new(beat())).unwrap();
    engine.transport(TransportCommand::Play);
    for _ in 0..200 {
        dev.pull(256);
    }
    let m = engine.meters();
    assert_eq!(m.peak.len(), MIXER_INSERTS + 1);
    assert!(m.peak[0][0] > 0.01, "master meter");
    assert!(m.peak[1][0] > 0.01 && m.rms[1][0] > 0.0, "insert 1 meter");
    assert_eq!(m.peak[9], [0.0, 0.0], "unused insert");
    // peak resets on read
    assert!(engine.meters().peak[0][0] <= m.peak[0][0]);
    let s = engine.stats();
    assert!(s.dsp_load > 0.0 && s.dsp_load.is_finite());
    assert_eq!((s.sample_rate, s.buffer_frames), (SR, 256));
    let kick = s.nodes.iter().find(|n| n.label == "Kick").unwrap();
    assert_eq!(kick.owner, Some(ObjectPath::Channel(ChannelId(0))));
    assert!(kick.micros > 0.0);
    assert!(s.nodes.iter().any(|n| n.label.ends_with("/ reverb")));
    assert_eq!(s.shedding, 0);
}

#[test]
fn tail_renders_until_quiet_and_is_capped() {
    let base = beat();
    let rs = |tail| RenderSettings {
        mode: PlayMode::Song,
        range: Some((0, 4 * PPQ)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: tail,
        block_frames: 512,
    };
    let dry = render_offline(&base, &rs(false), &GenAssets).unwrap();
    let wet = render_offline(&base, &rs(true), &GenAssets).unwrap();
    assert!(
        wet.frames() > dry.frames() + SR as usize / 10,
        "reverb tail"
    );
    assert!(wet.frames() <= dry.frames() + 10 * SR as usize);
    assert_bits_eq(
        &[dry.channels[0].clone(), dry.channels[1].clone()],
        &[
            wet.channels[0][..dry.frames()].to_vec(),
            wet.channels[1][..dry.frames()].to_vec(),
        ],
        "tail does not change the range",
    );
    let end = peak(&wet.channels[0][wet.frames() - 100..]);
    assert!(end < 1e-3, "tail end level {end}");
}

#[test]
fn file_resolver_feeds_the_sampler() {
    use crate::assets::FileResolver;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("samples/drums/haus808/kick.wav");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let k = GenAssets.sample("gen:kick").unwrap();
    ph_formats::encode::write_wav(
        &path,
        &k.channels,
        SR,
        ph_formats::encode::WavFormat::Float32,
    )
    .unwrap();
    let mut e = Edit::new(&Project::empty("f"));
    let c = e
        .add_channel(
            "Kick",
            InstrumentSpec::Sampler {
                slices: Vec::new(),
                sample: Some("factory:drums/haus808/kick".into()),
                root_key: 60,
                params: Default::default(),
            },
        )
        .unwrap();
    let p = e.add_pattern("P").unwrap();
    e.add_note(p, c, NoteSpec::new(60, 0, PPQ)).unwrap();
    let proj = e.finish().0;
    let r = FileResolver::new().with_assets_dir(tmp.path());
    let rs = RenderSettings {
        mode: PlayMode::Pattern(p),
        range: None,
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    let out = render_offline(&proj, &rs, &r).unwrap();
    assert!(peak(&out.channels[0]) > 0.1);
    // a missing sample leaves a silent channel rather than failing the whole project
    let missing = FileResolver::new();
    let out = render_offline(&proj, &rs, &missing).unwrap();
    assert_eq!(peak(&out.channels[0]), 0.0);
}

#[test]
fn stats_warn_about_unplayable_instruments_and_effects() {
    use super::GenAssets;
    let mut p = Project::empty("w");
    let mut ch = p.channels.values().next().cloned();
    if ch.is_none() {
        let id = ChannelId(1);
        let c = Channel {
            id,
            name: "Ghost".into(),
            color: 0,
            instrument: InstrumentSpec::Sampler {
                slices: Vec::new(),
                sample: Some("gen:nothing".into()),
                root_key: 60,
                params: Default::default(),
            },
            volume: 0.8,
            pan: 0.0,
            muted: false,
            insert: InsertId(0),
        };
        p.channels = Arc::new([(id, c.clone())].into_iter().collect());
        p.channel_order = Arc::new(vec![id]);
        ch = Some(c);
    }
    let _ = ch;
    p.mixer = Arc::new({
        let mut m = (*p.mixer).clone();
        m[0].effects.push(EffectSlot {
            effect: EffectSpec::Builtin {
                name: "no_such_effect".into(),
                params: Default::default(),
            },
            bypass: false,
            mix: 1.0,
        });
        m
    });
    let w = crate::project_warnings(&p, &GenAssets);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].contains("Ghost") && w[0].contains("silence"), "{w:?}");
    assert!(w[1].contains("effect slot 0"), "{w:?}");
    let clean = Project::empty("c");
    assert!(crate::project_warnings(&clean, &GenAssets).is_empty());
}
