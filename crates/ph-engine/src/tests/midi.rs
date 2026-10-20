//! MIDI input (M2/T8) through the fake MIDI source. No test opens a real MIDI device.
//!
//! The router thread is asynchronous, so tests wait for its effects (events, sound) with a
//! timeout instead of sleeping for a fixed time.

use super::*;
use crate::midi::*;
use std::time::{Duration, Instant};

fn project() -> (Arc<Project>, ChannelId) {
    let mut e = Edit::new(&Project::empty("midi"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let c = e
        .add_channel("Keys", synth(&[("osc1_wave", 0.0), ("amp_release", 0.05)]))
        .unwrap();
    (Arc::new(e.finish().0), c)
}

fn setup() -> (
    Arc<Engine>,
    SimulatedDevice,
    ChannelId,
    FakeMidi,
    MidiService,
) {
    let (engine, dev) = sim(256, 1);
    let (p, c) = project();
    engine.set_project(p).unwrap();
    let engine = Arc::new(engine);
    let fake = FakeMidi::new();
    fake.add_port("Test Keys");
    let svc = MidiService::start(
        engine.clone(),
        Box::new(fake.clone()),
        MappingStore::memory(),
    );
    wait(|| fake.connected() == ["Test Keys"], "the port connects");
    svc.set_target_channel(Some(c));
    (engine, dev, c, fake, svc)
}

fn wait(mut f: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_event(svc: &MidiService, mut want: impl FnMut(&MidiEvent) -> bool) -> Vec<MidiEvent> {
    let mut seen = Vec::new();
    let t = Instant::now();
    loop {
        seen.extend(svc.drain_events());
        if seen.iter().any(&mut want) {
            return seen;
        }
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "timed out; saw {seen:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn pull(dev: &mut SimulatedDevice, frames: usize) -> Vec<f32> {
    let mut l = vec![0.0f32; frames];
    let mut r = vec![0.0f32; frames];
    let mut at = 0;
    while at < frames {
        let n = 256.min(frames - at);
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    l
}

/// Zero crossings per second of a mono block (a pitch estimate).
fn crossings(x: &[f32]) -> f32 {
    let n = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    n as f32 * SR as f32 / x.len() as f32
}

#[test]
fn a_note_from_the_keyboard_sounds_on_the_selected_channel_and_stops_on_release() {
    let (_engine, mut dev, _c, fake, svc) = setup();
    assert!(peak(&pull(&mut dev, 2048)) < 1e-6);
    assert!(fake.send("Test Keys", &[0x90, 69, 100]));
    wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, key: 69 }));
    let sound = pull(&mut dev, 8192);
    assert!(peak(&sound) > 0.05, "A4 sounds: {}", peak(&sound));
    let hz = crossings(&sound[2048..]);
    assert!((hz - 440.0).abs() < 12.0, "pitch {hz}");
    fake.send("Test Keys", &[0x80, 69, 0]);
    wait_event(&svc, |e| {
        matches!(e, MidiEvent::Note { on: false, key: 69 })
    });
    let _ = pull(&mut dev, 8192);
    assert!(peak(&pull(&mut dev, 2048)) < 1e-4, "released and silent");
    // no channel selected: nothing sounds
    svc.set_target_channel(None);
    fake.send("Test Keys", &[0x90, 60, 100]);
    wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, key: 60 }));
    assert!(peak(&pull(&mut dev, 4096)) < 1e-6);
}

#[test]
fn pitch_bend_from_the_keyboard_moves_the_note() {
    let (_engine, mut dev, _c, fake, svc) = setup();
    fake.send("Test Keys", &[0x90, 69, 100]);
    wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, .. }));
    let _ = pull(&mut dev, 4096);
    let flat = crossings(&pull(&mut dev, 8192));
    // full bend up: the default range is 2 semitones (about 494 Hz)
    fake.send("Test Keys", &[0xe0, 0x7f, 0x7f]);
    // the router applies it asynchronously: wait for the pitch to move
    let t = Instant::now();
    let mut bent = flat;
    while t.elapsed() < Duration::from_secs(5) {
        bent = crossings(&pull(&mut dev, 8192));
        if bent > flat * 1.08 {
            break;
        }
    }
    assert!(bent > flat * 1.08, "bent {bent} vs {flat}");
}

#[test]
fn controllers_reach_the_instrument_as_events() {
    // the deterministic half: the engine path itself, without the router thread
    let (engine, mut dev) = sim(256, 1);
    let (p, c) = project();
    engine.set_project(p).unwrap();
    engine.preview_note(c, 69, 1.0, true);
    let _ = pull(&mut dev, 4096);
    let flat = crossings(&pull(&mut dev, 8192));
    engine.preview_midi(c, MidiMod::Bend { value: -1.0 });
    let down = crossings(&pull(&mut dev, 8192));
    assert!(down < flat * 0.94, "bend down {down} vs {flat}");
    engine.preview_midi(c, MidiMod::Bend { value: 0.0 });
    engine.preview_midi(c, MidiMod::Pressure { value: 1.0 });
    engine.preview_midi(
        c,
        MidiMod::Cc {
            controller: 1,
            value: 1.0,
        },
    );
    // these must not disturb anything (and are allocation-free: pull_into checks)
    let back = crossings(&pull(&mut dev, 8192));
    assert!((back - flat).abs() < flat * 0.03, "back {back} vs {flat}");
}

#[test]
fn hot_plugged_devices_connect_and_vanished_ones_drop() {
    let (_engine, _dev, _c, fake, svc) = setup();
    assert_eq!(svc.inputs().len(), 1);
    svc.drain_events();
    fake.add_port("Pads");
    svc.refresh();
    wait(|| fake.connected().len() == 2, "the new port connects");
    wait_event(&svc, |e| matches!(e, MidiEvent::Devices));
    assert_eq!(svc.inputs().len(), 2);
    fake.remove_port("Test Keys");
    svc.refresh();
    wait(|| fake.connected() == ["Pads"], "the vanished port drops");
    wait(|| svc.inputs().len() == 1, "the listing follows");
    // plugging it back connects again
    fake.add_port("Test Keys");
    svc.refresh();
    wait(|| fake.connected().len() == 2, "it reconnects");
    assert!(fake.connect_count() >= 3);
}

#[test]
fn midi_learn_maps_the_next_controller_and_mapped_ones_become_control_events() {
    let (_engine, _dev, c, fake, svc) = setup();
    let target = ParamPath::ChannelVolume(c);
    // not mapped: a controller is the instrument's business, no event
    fake.send("Test Keys", &[0xb0, 21, 64]);
    // learning: the next CC is caught and mapped, to the device that sent it
    svc.learn(Some(target.clone()));
    assert_eq!(svc.learning(), Some(target.clone()));
    fake.send("Test Keys", &[0xb0, 21, 10]);
    let seen = wait_event(&svc, |e| matches!(e, MidiEvent::Learned { .. }));
    assert!(seen.contains(&MidiEvent::Learned {
        controller: 21,
        device: "Test Keys".into(),
        target: target.clone(),
    }));
    assert_eq!(svc.learning(), None, "learning ends after one");
    assert_eq!(
        svc.mappings(),
        [Mapping {
            controller: 21,
            device: Some("Test Keys".into()),
            target: target.clone()
        }]
    );
    // mapped: a Control event with the 0..1 value
    fake.send("Test Keys", &[0xb0, 21, 127]);
    let seen = wait_event(&svc, |e| matches!(e, MidiEvent::Control { .. }));
    assert!(seen.contains(&MidiEvent::Control {
        target: target.clone(),
        value: 1.0,
        controller: 21,
        device: "Test Keys".into()
    }));
    // learning can be cancelled
    svc.learn(Some(ParamPath::Tempo));
    svc.learn(None);
    fake.send("Test Keys", &[0xb0, 22, 5]);
    fake.send("Test Keys", &[0xb0, 21, 0]);
    let seen = wait_event(
        &svc,
        |e| matches!(e, MidiEvent::Control { value, .. } if *value == 0.0),
    );
    assert!(!seen.iter().any(|e| matches!(e, MidiEvent::Learned { .. })));
    // unmapping stops the events
    assert!(svc.unmap(21, Some("Test Keys")).unwrap().is_some());
    fake.send("Test Keys", &[0xb0, 21, 99]);
    fake.send("Test Keys", &[0x90, 60, 100]);
    let seen = wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, .. }));
    assert!(!seen.iter().any(|e| matches!(e, MidiEvent::Control { .. })));
}

#[test]
fn mappings_are_per_user_files_not_part_of_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _dev, c, fake, _svc) = setup();
    let svc = MidiService::start(
        engine,
        Box::new(fake.clone()),
        MappingStore::open(dir.path()),
    );
    svc.map(Mapping {
        controller: 74,
        device: None,
        target: ParamPath::ChannelParam(c, "f1_cutoff".into()),
    })
    .unwrap();
    let text = std::fs::read_to_string(dir.path().join("midi-mappings.json")).unwrap();
    assert!(
        text.contains(&format!("channel/{}/param/f1_cutoff", c.0)),
        "{text}"
    );
    assert_eq!(MappingStore::open(dir.path()).list().len(), 1);
}

#[test]
fn played_notes_are_collected_with_their_positions_while_recording() {
    let (engine, mut dev, _c, fake, svc) = setup();
    engine.transport(TransportCommand::Play);
    // two beats in (120 BPM: 1 s = 2 beats = 1920 ticks)
    let _ = pull(&mut dev, 48_000);
    let at_on = engine.song_position();
    svc.start_note_recording();
    fake.send("Test Keys", &[0x90, 64, 80]);
    wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, key: 64 }));
    let _ = pull(&mut dev, 24_000); // a beat
    fake.send("Test Keys", &[0x80, 64, 0]);
    wait_event(&svc, |e| {
        matches!(e, MidiEvent::Note { on: false, key: 64 })
    });
    // a note still held when recording stops ends at the stop position
    fake.send("Test Keys", &[0x90, 67, 127]);
    wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, key: 67 }));
    let _ = pull(&mut dev, 12_000);
    let end = engine.song_position();
    let notes = svc.finish_note_recording(end);
    assert!(!svc.is_note_recording());
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!((notes[0].key, notes[0].start), (64, at_on));
    assert_eq!(notes[0].length, 960, "one beat");
    assert!((notes[0].velocity - 80.0 / 127.0).abs() < 1e-6);
    assert_eq!(notes[1].key, 67);
    assert_eq!(notes[1].start + notes[1].length, end);
    // after stopping, played notes are not collected
    fake.send("Test Keys", &[0x90, 70, 80]);
    wait_event(&svc, |e| matches!(e, MidiEvent::Note { on: true, key: 70 }));
    assert!(svc.finish_note_recording(end).is_empty());
}

#[test]
fn keyboard_events_under_plan_swaps_allocate_nothing() {
    // notes, controllers, bends and pressure arrive from one thread while another swaps plans
    // and the audio thread runs under `assert_no_alloc` (`pull_into`)
    let (engine, mut dev) = sim(128, 1);
    let engine = Arc::new(engine);
    let (p, c) = project();
    engine.set_project(p.clone()).unwrap();
    // one held note, so something sounds however the threads are scheduled
    engine.preview_note(c, 57, 1.0, true);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let player = {
        let (e, stop, sent) = (engine.clone(), stop.clone(), sent.clone());
        std::thread::spawn(move || {
            let mut r = Rng::new(7);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let key = 40 + r.below(40) as u8;
                e.preview_note(c, key, 0.2 + r.below(8) as f32 / 10.0, true);
                e.preview_midi(
                    c,
                    MidiMod::Bend {
                        value: r.below(200) as f32 / 100.0 - 1.0,
                    },
                );
                e.preview_midi(
                    c,
                    MidiMod::Cc {
                        controller: 1,
                        value: r.below(100) as f32 / 100.0,
                    },
                );
                e.preview_midi(
                    c,
                    MidiMod::Pressure {
                        value: r.below(100) as f32 / 100.0,
                    },
                );
                if r.below(3) == 0 {
                    e.preview_note(c, key, 0.0, false);
                }
                sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                std::thread::sleep(Duration::from_micros(150));
            }
        })
    };
    let swapper = {
        let (e, stop, p) = (engine.clone(), stop.clone(), p.clone());
        std::thread::spawn(move || {
            let mut k = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let mut ed = Edit::new(&p);
                ed.update_channel(c, |ch| ch.volume = 0.4 + (k % 5) as f32 * 0.1)
                    .unwrap();
                e.set_project(Arc::new(ed.finish().0)).unwrap();
                k += 1;
                std::thread::sleep(Duration::from_micros(400));
            }
        })
    };
    // pull until the player has sent a few hundred events (and at least 1500 blocks)
    let mut out = Vec::new();
    let t0 = Instant::now();
    while out.len() < 128 * 1500
        || (sent.load(std::sync::atomic::Ordering::Relaxed) < 300
            && t0.elapsed() < Duration::from_secs(20))
    {
        out.extend(pull(&mut dev, 128));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    player.join().unwrap();
    swapper.join().unwrap();
    assert!(out.iter().all(|v| v.is_finite()));
    assert!(peak(&out) > 0.01, "something sounded");
}
