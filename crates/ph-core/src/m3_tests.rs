//! M3/T0: the Haus Sampler instrument model, its object paths, patches and invariants.

use crate::*;
use serde_json::json;

fn kit() -> HausInstrument {
    let mut inst = HausInstrument::new("Kit");
    inst.articulations.push(Articulation {
        id: ArticulationId(0),
        name: "sustain".into(),
        trigger: ArticulationTrigger::Key { key: 24 },
        color: None,
    });
    inst.articulations.push(Articulation {
        id: ArticulationId(1),
        name: "staccato".into(),
        trigger: ArticulationTrigger::Key { key: 25 },
        color: Some(0xff0000),
    });
    inst.settings.default_articulation = Some(ArticulationId(0));
    let mut g = Group::new("Sustain");
    g.id = GroupId(7); // importer-local ids are remapped by add_instrument
    g.articulations = vec![ArticulationId(0)];
    g.round_robin = RoundRobinMode::Random;
    inst.groups.insert(g.id, g);
    for (i, key) in [48u8, 60].iter().enumerate() {
        let mut z = Zone::new(
            GroupId(7),
            SampleRef::File(format!("samples/{key}.wav")),
            KeyRange {
                lo: key - 5,
                hi: key + 6,
            },
            *key,
        );
        z.id = ZoneId(100 + i as u32);
        z.round_robin = Some(i as u32);
        inst.zones.insert(z.id, z);
    }
    inst.modulation.push(ModRoute {
        source: ModSource::Velocity,
        target: ModTarget::Volume,
        depth: -12.0,
        curve: ModCurve::Concave,
        group: Some(GroupId(7)),
    });
    inst
}

fn with_kit() -> (Project, PatchSet, InstrumentId) {
    let base = Project::empty("m3");
    let mut ed = Edit::new(&base);
    let id = ed.add_instrument(kit()).unwrap();
    ed.add_channel(
        "Kit",
        InstrumentSpec::MultiSampler {
            library: Some("C:/Libraries/Kit".into()),
            instrument: Some(id),
            params: Default::default(),
        },
    )
    .unwrap();
    let (p, set) = ed.finish();
    (p, set, id)
}

#[test]
fn add_instrument_assigns_fresh_ids_and_remaps_references() {
    let (p, _, id) = with_kit();
    let inst = p.instrument(id).unwrap();
    assert_eq!(id, InstrumentId(0));
    assert_eq!(
        inst.groups.keys().copied().collect::<Vec<_>>(),
        [GroupId(0)]
    );
    assert_eq!(
        inst.zones.keys().copied().collect::<Vec<_>>(),
        [ZoneId(0), ZoneId(1)]
    );
    assert!(inst.zones.values().all(|z| z.group == GroupId(0)));
    assert_eq!(inst.modulation[0].group, Some(GroupId(0)));
    assert_eq!(
        (p.next_ids.instrument, p.next_ids.group, p.next_ids.zone),
        (1, 1, 2)
    );
    assert_eq!(p.channels_playing(id).count(), 1);
}

#[test]
fn instrument_edits_replay_invert_and_never_free_ids() {
    let base = Project::empty("m3");
    let (after, set, _) = with_kit();
    assert_eq!(apply(&base, &set).unwrap(), after);
    let undone = apply(&after, &set.inverse()).unwrap();
    assert!(undone.instruments.is_empty() && undone.channels.is_empty());
    assert_eq!(undone.next_ids, after.next_ids);
    // the patch log never carries a group or zone inside an instrument value
    for patch in set.iter() {
        if let ObjectPath::Instrument(_) = patch.path
            && let Some(v) = &patch.after
        {
            assert!(v.get("groups").is_none() && v.get("zones").is_none(), "{v}");
        }
    }
}

#[test]
fn object_paths_round_trip() {
    for s in [
        "instrument/3",
        "instrument/3/group/4",
        "instrument/3/zone/12",
    ] {
        let p: ObjectPath = s.parse().unwrap();
        assert_eq!(p.to_string(), s);
        assert!(!p.is_fixed() && !p.is_order_list());
    }
    for s in [
        "instrument",
        "instrument/x",
        "instrument/3/voice/1",
        "instrument/3/zone",
    ] {
        assert!(s.parse::<ObjectPath>().is_err(), "{s}");
    }
}

#[test]
fn groups_and_zones_are_their_own_small_objects() {
    let (p, _, id) = with_kit();
    let header = get(&p, ObjectPath::Instrument(id)).unwrap();
    assert!(header.get("zones").is_none() && header.get("groups").is_none());
    let z = get(&p, ObjectPath::Zone(id, ZoneId(1))).unwrap();
    assert_eq!(z["root_key"], 60);
    assert_eq!(z["sample"], json!("samples/60.wav"));
    let r: SampleRef = serde_json::from_value(json!(4)).unwrap();
    assert_eq!(r, SampleRef::Registry(SampleId(4)));
    // an instrument value carrying zones is refused
    let mut v = serde_json::to_value(p.instrument(id).unwrap()).unwrap();
    assert!(v.get("zones").is_some());
    let mut w = p.clone();
    assert!(set(&mut w, ObjectPath::Instrument(id), Some(&v)).is_err());
    // the header keeps the existing groups and zones when changed
    v.as_object_mut().unwrap().remove("zones");
    v.as_object_mut().unwrap().remove("groups");
    v["name"] = json!("Kit 2");
    set(&mut w, ObjectPath::Instrument(id), Some(&v)).unwrap();
    assert_eq!(w.instrument(id).unwrap().zones.len(), 2);
    assert_eq!(w.instrument(id).unwrap().name, "Kit 2");
}

#[test]
fn model_invariants_hold() {
    let (p, _, id) = with_kit();
    let try_set = |path, v: Option<serde_json::Value>| {
        let mut w = p.clone();
        set(&mut w, path, v.as_ref())
    };
    // a zone in a missing group
    let mut z = p.instrument(id).unwrap().zone(ZoneId(0)).unwrap().clone();
    z.group = GroupId(9);
    assert!(
        try_set(
            ObjectPath::Zone(id, ZoneId(0)),
            Some(serde_json::to_value(&z).unwrap())
        )
        .is_err()
    );
    // a zone in a missing instrument
    z.group = GroupId(0);
    assert!(
        try_set(
            ObjectPath::Zone(InstrumentId(5), ZoneId(0)),
            Some(serde_json::to_value(&z).unwrap())
        )
        .is_err()
    );
    // a zone on a missing registry sample
    z.sample = SampleRef::Registry(SampleId(3));
    assert!(
        try_set(
            ObjectPath::Zone(id, ZoneId(0)),
            Some(serde_json::to_value(&z).unwrap())
        )
        .is_err()
    );
    // reversed ranges, loops outside the zone
    let mut z = p.instrument(id).unwrap().zone(ZoneId(0)).unwrap().clone();
    z.keys = KeyRange { lo: 70, hi: 60 };
    assert!(validate_zone(&z).is_err());
    z.keys = KeyRange::ALL;
    z.end = Some(1000);
    z.looping = Some(ZoneLoop {
        mode: LoopMode::Forward,
        start: 500,
        end: 2000,
        crossfade: 0,
        on_release: LoopRelease::Exit,
    });
    assert!(validate_zone(&z).is_err());
    // a group still holding zones, or scoped by a route, cannot be deleted
    assert!(try_set(ObjectPath::Group(id, GroupId(0)), None).is_err());
    // a group joining a missing articulation
    let mut g = p.instrument(id).unwrap().group(GroupId(0)).unwrap().clone();
    g.articulations = vec![ArticulationId(9)];
    assert!(
        try_set(
            ObjectPath::Group(id, GroupId(0)),
            Some(serde_json::to_value(&g).unwrap())
        )
        .is_err()
    );
    // an instrument with zones, or played by a channel, cannot be deleted
    assert!(try_set(ObjectPath::Instrument(id), None).is_err());
    let mut ed = Edit::new(&p);
    assert!(ed.remove_instrument(id).is_err(), "a channel plays it");
    // a channel cannot point at a missing instrument
    let mut ch = p.channels_in_order().next().unwrap().clone();
    ch.instrument = InstrumentSpec::MultiSampler {
        library: None,
        instrument: Some(InstrumentId(42)),
        params: Default::default(),
    };
    assert_eq!(
        try_set(
            ObjectPath::Channel(ch.id),
            Some(serde_json::to_value(&ch).unwrap())
        ),
        Err(ModelError::NoInstrument(InstrumentId(42)))
    );
}

#[test]
fn removing_a_registry_sample_played_by_a_zone_is_refused() {
    let mut ed = Edit::new(&Project::empty("m3"));
    let s = ed
        .add_sample(SampleEntry {
            id: SampleId(0),
            path: "samples/a.wav".into(),
            name: "a.wav".into(),
            frames: 10,
            sample_rate: 48_000,
            channels: 1,
            bpm: None,
            root_key: None,
            analysis: Default::default(),
            provenance: Default::default(),
        })
        .unwrap();
    let i = ed.add_instrument(HausInstrument::new("one")).unwrap();
    let g = ed.add_group(i, Group::new("g")).unwrap();
    let z = ed
        .add_zone(i, Zone::new(g, SampleRef::Registry(s), KeyRange::ALL, 60))
        .unwrap();
    assert!(ed.remove_sample(s).is_err());
    ed.remove_zone(i, z).unwrap();
    ed.remove_sample(s).unwrap();
    // remove_group / remove_instrument take their zones and groups with them
    ed.add_zone(
        i,
        Zone::new(g, SampleRef::File("x.wav".into()), KeyRange::ALL, 60),
    )
    .unwrap();
    ed.remove_instrument(i).unwrap();
    assert!(ed.project().instruments.is_empty());
}

#[test]
fn move_zone_shifts_the_root_when_asked() {
    let (p, _, id) = with_kit();
    let mut ed = Edit::new(&p);
    let to = ZonePlace {
        keys: Some(KeyRange { lo: 57, hi: 68 }),
        velocity: Some(VelRange { lo: 64, hi: 127 }),
        group: None,
    };
    ed.move_zone(id, ZoneId(1), to.clone(), true).unwrap();
    let z = ed
        .project()
        .instrument(id)
        .unwrap()
        .zone(ZoneId(1))
        .unwrap()
        .clone();
    assert_eq!((z.keys.lo, z.keys.hi, z.root_key), (57, 68, 62));
    assert_eq!(z.velocity, VelRange { lo: 64, hi: 127 });
    ed.move_zone(id, ZoneId(0), to, false).unwrap();
    assert_eq!(
        ed.project()
            .instrument(id)
            .unwrap()
            .zone(ZoneId(0))
            .unwrap()
            .root_key,
        48
    );
    let (_, set) = ed.finish();
    assert_eq!(set.len(), 2, "one patch per zone moved");
}

#[test]
fn projects_with_instruments_round_trip_through_json() {
    let (p, _, id) = with_kit();
    let text = serde_json::to_string_pretty(&p).unwrap();
    let back: Project = serde_json::from_str(&text).unwrap();
    assert_eq!(back, p);
    assert_eq!(back.instrument(id).unwrap().zones.len(), 2);
    // a project without instruments has no new keys at all
    let plain = serde_json::to_value(Project::empty("x")).unwrap();
    assert!(plain.get("instruments").is_none());
    assert!(plain["next_ids"].get("zone").is_none());
}

#[test]
fn sample_references_resolve_against_the_folder() {
    let (p, _, id) = with_kit();
    let mut inst = p.instrument(id).unwrap().clone();
    let f = SampleRef::File("samples/60.wav".into());
    assert_eq!(inst.sample_reference(&p, &f).unwrap(), "samples/60.wav");
    inst.folder = Some("instruments/Kit/".into());
    assert_eq!(
        inst.sample_reference(&p, &f).unwrap(),
        "instruments/Kit/samples/60.wav"
    );
    assert!(
        inst.sample_reference(&p, &SampleRef::Registry(SampleId(0)))
            .is_none()
    );
    assert_eq!(midi_velocity(0.78), 99);
    assert_eq!(midi_velocity(1.0), 127);
}
