use super::build::*;
use super::*;
use crate::validate;
use ph_core::*;
use proptest::prelude::*;

fn import(bytes: &[u8]) -> FlpImport {
    import_flp(bytes, "fallback", &mut NoSamples).expect("imports")
}

#[test]
fn container_errors_are_errors_not_panics() {
    assert_eq!(parse(b"").unwrap_err(), FlpError::TooShort);
    assert!(matches!(
        parse(b"RIFF\x06\0\0\0\0\0\0\0\0\0\0\0").unwrap_err(),
        FlpError::BadMagic { .. }
    ));
    let good = sample_project();
    // every truncation of a good file is an error or (for a cut on an event boundary
    // that the length field no longer matches) an error: never a panic
    for n in 0..good.len() {
        assert!(parse(&good[..n]).is_err(), "cut at {n}");
    }
    // zero PPQ and a non-project format are refused
    let mut zero = good.clone();
    zero[12] = 0;
    zero[13] = 0;
    assert_eq!(parse(&zero).unwrap_err(), FlpError::BadPpq);
    let mut fmt = good.clone();
    fmt[8] = 16;
    assert_eq!(parse(&fmt).unwrap_err(), FlpError::UnsupportedFormat(16));
    // an event whose size runs past the data
    let mut b = FlpBuilder::new(96);
    b.version("21.0.0.1");
    let mut f = b.finish();
    f.extend([0xC4, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]); // var event, huge size
    let len = (f.len() - 22) as u32;
    f[18..22].copy_from_slice(&len.to_le_bytes());
    assert!(parse(&f).is_err());
}

#[test]
fn varint_sizes_round_trip() {
    for n in [0usize, 1, 0x26, 127, 128, 200, 16_383, 16_384, 70_000] {
        let mut b = FlpBuilder::new(96);
        let payload = vec![7u8; n];
        b.bytes(213, &payload);
        let f = b.finish();
        let raw = parse(&f).unwrap();
        assert_eq!(raw.events.len(), 1);
        assert_eq!(raw.events[0].bytes().unwrap().len(), n);
    }
}

#[test]
fn reads_the_synthetic_project() {
    let bytes = sample_project();
    let raw = parse(&bytes).unwrap();
    let f = read(&raw);
    assert_eq!(f.version.as_deref(), Some("21.2.2.3914"));
    assert_eq!(f.fl_major, 21);
    assert_eq!(f.tempo_milli, Some(123_456));
    assert_eq!(f.channels.len(), 5);
    assert_eq!(f.patterns[&1].notes.len(), 3);
    assert_eq!(f.patterns[&1].name.as_deref(), Some("Beat"));
    assert_eq!(f.inserts.len(), 3);
    assert_eq!(f.inserts[1].name.as_deref(), Some("Drum bus"));
    assert_eq!(f.inserts[1].slots.len(), 1);
    assert_eq!(
        f.inserts[1].slots[0].internal_name,
        "Fruity Parametric EQ 2"
    );
    assert_eq!(f.arrangements[0].items.len(), 4);
    assert_eq!(f.arrangements[0].tracks.len(), 3);
    assert_eq!(f.arrangements[0].tracks[0].name.as_deref(), Some("Drums"));
    assert_eq!(f.auto_targets[&3] & 0xff, 192);
    assert!(f.unread.is_empty() || f.unread.keys().all(|k| *k != 224));
}

#[test]
fn maps_every_feature_of_the_synthetic_project() {
    let out = import(&sample_project());
    let p = &out.project;
    validate(p).expect("a valid project");
    assert_eq!(p.settings.title, "Synthetic song");
    assert_eq!(p.settings.bpm, 123.456);
    // channels: sampler, VST3, native generator; audio and automation sources are not channels
    assert_eq!(p.channels.len(), 3);
    let kick = &p.channels[&ChannelId(0)];
    assert_eq!(kick.name, "Kick");
    assert_eq!(kick.color, 0x40_80_ff);
    assert_eq!(kick.insert, InsertId(1));
    assert!((kick.volume - 8000.0 / 12800.0).abs() < 1e-6);
    assert!((kick.pan - (3200.0 - 6400.0) / 6400.0).abs() < 1e-6);
    assert!(matches!(
        &kick.instrument,
        InstrumentSpec::Sampler { sample: Some(s), .. } if s.starts_with("%FLStudioFactoryData%")
    ));
    let vst = &p.channels[&ChannelId(1)];
    assert!(vst.muted);
    assert_eq!(vst.name, "Super Synth");
    match &vst.instrument {
        InstrumentSpec::Plugin {
            format,
            plugin_id,
            state,
            ..
        } => {
            assert_eq!(*format, PluginFormat::Vst3);
            assert_eq!(plugin_id, "0102030405060708090a0b0c0d0e0f10");
            assert_eq!(state.as_deref(), Some("plugins/flp-channel-1.flpstate"));
        }
        other => panic!("{other:?}"),
    }
    match &p.channels[&ChannelId(2)].instrument {
        InstrumentSpec::Plugin { plugin_id, .. } => assert_eq!(plugin_id, "fl-native:3x Osc"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        out.files.len(),
        3,
        "two generators and one effect keep state"
    );
    assert!(
        out.files
            .iter()
            .any(|f| f.path == "plugins/flp-channel-1.flpstate")
    );
    assert!(
        out.files
            .iter()
            .any(|f| f.path == "plugins/flp-insert1-slot0.flpstate")
    );

    // patterns and notes: PPQ 96 -> 960 exactly
    assert_eq!(p.patterns.len(), 2);
    let pat = &p.patterns[&PatternId(0)];
    assert_eq!(pat.name, "Beat");
    assert_eq!(pat.color, 0xff_00_00);
    let k = &pat.notes[&ChannelId(0)];
    let mut notes: Vec<_> = k.values().collect();
    notes.sort_by_key(|n| n.start);
    assert_eq!(
        (notes[0].start, notes[0].length, notes[0].key),
        (0, 960, 60)
    );
    assert_eq!(
        (notes[1].start, notes[1].length, notes[1].key),
        (960, 480, 64)
    );
    assert!((notes[0].velocity - 100.0 / 128.0).abs() < 1e-6);
    assert_eq!(notes[0].pan, 0.0);
    assert_eq!(notes[0].fine_pitch, 0.0);
    let v = pat.notes[&ChannelId(1)].values().next().unwrap();
    assert_eq!(v.length, STEP, "a zero-length note gets a step");
    assert_eq!(v.velocity, 1.0);

    // mixer
    let ins1 = &p.mixer[1];
    assert_eq!(ins1.name, "Drum bus");
    assert_eq!(ins1.color, Some(0xff_00_00));
    assert!((ins1.volume - 0.5).abs() < 1e-6);
    assert!((ins1.pan - (-0.5)).abs() < 1e-6);
    assert!(ins1.route_to_master);
    assert_eq!(
        ins1.sends,
        vec![Send {
            to: InsertId(2),
            level: 1.0
        }]
    );
    assert_eq!(ins1.effects.len(), 1);
    assert!(ins1.effects[0].bypass);
    assert!((ins1.effects[0].mix - 0.5).abs() < 1e-6);
    assert_eq!(p.mixer[0].volume, 1.0);
    assert!(!p.mixer[0].route_to_master);

    // playlist: tracks, pattern clips, audio clip, automation clip
    assert_eq!(p.tracks[0].name, "Drums");
    assert_eq!(p.clips.len(), 4);
    let pc: Vec<_> = p.clips.values().filter(|c| c.pattern().is_some()).collect();
    assert_eq!(pc.len(), 2);
    assert!(
        pc.iter()
            .any(|c| c.start == 3840 && c.track == 2 && c.length == 3840)
    );
    let audio = p.clips.values().find(|c| c.audio().is_some()).unwrap();
    assert_eq!((audio.start, audio.length, audio.track), (7680, 19_200, 1));
    assert_eq!(p.samples.len(), 1);
    assert_eq!(p.samples.values().next().unwrap().name, "loop.wav");
    let auto = p.clips.values().find(|c| c.automation().is_some()).unwrap();
    let a = auto.automation().unwrap();
    assert_eq!(a.target, ParamPath::InsertVolume(InsertId(2)));
    let pts = a.points_in_order();
    assert_eq!(pts.len(), 2);
    assert_eq!((pts[0].tick, pts[1].tick), (0, 3840), "4 quarter notes");
    assert!(
        (pts[0].value - 1.25).abs() < 1e-9,
        "1.0 of 16000 raw = 1.25"
    );
    assert!((pts[1].tension - 0.25).abs() < 1e-6);

    // report
    let r = &out.report;
    assert_eq!(r.count("notes_in_file"), 3);
    assert_eq!(r.count("notes"), 3);
    assert_eq!(r.count("clips_in_file"), 4);
    assert_eq!(r.count("clips"), 4);
    assert_eq!(r.count("channels_in_file"), 5);
    assert_eq!(r.count("channels"), 3);
    assert_eq!(
        r.count("routes"),
        3,
        "insert 1 to master and to 2, insert 2 to master"
    );
    assert!(r.approximated() > 0 && r.skipped() > 0);
    assert!(
        r.items()
            .iter()
            .any(|i| i.what.contains("sample file not found"))
    );
    let md = r.to_markdown("Synthetic song");
    assert!(md.contains("## Approximated") && md.contains("## Skipped"));
    assert!(md.contains("Test Author"));
}

#[test]
fn a_resolver_supplies_sample_references() {
    struct Fake;
    impl SampleResolver for Fake {
        fn resolve(&mut self, path: &str) -> Option<ResolvedSample> {
            path.ends_with("loop.wav").then(|| ResolvedSample {
                reference: "samples/loop.wav".into(),
                frames: 44_100,
                sample_rate: 44_100,
                channels: 1,
            })
        }
    }
    let out = import_flp(&sample_project(), "x", &mut Fake).unwrap();
    let e = out.project.samples.values().next().unwrap();
    assert_eq!(
        (e.path.as_str(), e.frames, e.channels),
        ("samples/loop.wav", 44_100, 1)
    );
    assert_eq!(out.report.count("samples_found"), 1);
    assert_eq!(
        out.report.count("samples_missing"),
        1,
        "the sampler's factory sample"
    );
    validate(&out.project).unwrap();
}

#[test]
fn other_ppq_scales_every_position() {
    let mut b = FlpBuilder::new(480);
    b.version("21.0.0.1").u8(17, 4).u8(18, 4);
    b.channel(0, 0).text(201, "");
    b.u16(65, 1)
        .bytes(224, &FlpBuilder::note_record(480, 0, 240, 60, 100));
    let out = import(&b.finish());
    let n = out.project.patterns[&PatternId(0)].notes[&ChannelId(0)]
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!((n.start, n.length), (960, 480));
}

#[test]
fn clips_beyond_the_playlist_and_cyclic_routes_are_reported() {
    let mut b = FlpBuilder::new(96);
    b.version("21.0.0.1");
    b.channel(0, 0).text(201, "");
    b.u16(65, 1)
        .bytes(224, &FlpBuilder::note_record(0, 0, 96, 60, 100));
    b.u16(99, 0);
    let mut items = FlpBuilder::item_record(0, 0x5001, 96, 70); // track 70 > 64
    items.extend(FlpBuilder::item_record(0, 0x5001, 96, 3));
    b.bytes(233, &items);
    // insert 1 -> 2 and insert 2 -> 1: the second would loop
    let mk = |to: usize| {
        let mut r = vec![0u8; 127];
        r[to] = 1;
        r
    };
    let flags = [0u8; 12];
    b.bytes(236, &flags).bytes(235, &[0u8; 127]);
    b.bytes(236, &flags).bytes(235, &mk(2));
    b.bytes(236, &flags).bytes(235, &mk(1));
    let out = import(&b.finish());
    assert_eq!(out.project.clips.len(), 1);
    assert_eq!(out.report.count("clips_in_file"), 2);
    let skipped: Vec<_> = out.report.items().into_iter().map(|i| i.what).collect();
    assert!(skipped.iter().any(|w| w.contains("track above 64")));
    assert!(skipped.iter().any(|w| w.contains("loop")));
    validate(&out.project).unwrap();
}

#[test]
fn text_is_utf16_and_old_files_are_8_bit() {
    assert_eq!(
        decode_text(&utf16("Caf\u{e9} \u{1f3b5}"), true),
        "Caf\u{e9} \u{1f3b5}"
    );
    assert_eq!(decode_text(b"Hello\0junk", false), "Hello");
    // an unpaired surrogate does not panic
    assert!(!decode_text(&[0x00, 0xd8, 0x41, 0x00, 0, 0], true).is_empty());
}

#[test]
fn an_item_record_size_is_chosen_by_the_pattern_base() {
    // 32-byte records in a file claiming FL 21: the 0x5000 base picks the right size
    let mut rec = FlpBuilder::item_record(96, 0x5001, 96, 0);
    rec.truncate(32);
    let mut two = rec.clone();
    two.extend(rec);
    let mut b = FlpBuilder::new(96);
    b.version("21.2.2.3914").channel(0, 0).u16(65, 1);
    b.u16(99, 0).bytes(233, &two);
    let bytes = b.finish();
    let raw = parse(&bytes).unwrap();
    let f = read(&raw);
    assert_eq!(f.arrangements[0].items.len(), 2);
}

fn mutate(mut v: Vec<u8>, edits: &[(usize, u8)], cut: usize) -> Vec<u8> {
    for (at, b) in edits {
        if !v.is_empty() {
            let i = at % v.len();
            v[i] = *b;
        }
    }
    v.truncate(v.len().saturating_sub(cut));
    v
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Fuzz 1: arbitrary bytes never panic.
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let _ = import_flp(&bytes, "fuzz", &mut NoSamples);
    }

    /// Fuzz 2: random bytes behind a valid header and data-chunk length reach the event
    /// reader and the mapping.
    #[test]
    fn random_events_never_panic(body in proptest::collection::vec(any::<u8>(), 0..1500)) {
        let mut f = b"FLhd".to_vec();
        f.extend(6u32.to_le_bytes());
        f.extend([0, 0, 2, 0, 96, 0]);
        f.extend(b"FLdt");
        f.extend((body.len() as u32).to_le_bytes());
        f.extend(&body);
        let _ = import_flp(&f, "fuzz", &mut NoSamples);
    }

    /// Fuzz 3: a valid project with bytes flipped and the tail cut. Every outcome is an
    /// error or a project that passes validation.
    #[test]
    fn mutated_projects_never_panic(
        edits in proptest::collection::vec((any::<usize>(), any::<u8>()), 0..12),
        cut in 0usize..40,
    ) {
        let f = mutate(sample_project(), &edits, cut);
        if let Ok(out) = import_flp(&f, "fuzz", &mut NoSamples) {
            prop_assert!(validate(&out.project).is_ok(), "{:?}", validate(&out.project));
        }
    }
}
