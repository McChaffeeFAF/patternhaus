//! Real-time contract: `process` (note-ons with round robin and stealing, note-offs with
//! release triggers, keyswitches, controllers, AllNotesOff) never allocates.

use ph_core::*;
use ph_node::*;
use ph_sampler::{HausSampler, SampleSet, ZoneSample};
use std::sync::Arc;

#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn sampler() -> HausSampler {
    let mut inst = HausInstrument::new("rt");
    inst.settings.voice_limit = 4;
    inst.articulations.push(Articulation {
        id: ArticulationId(0),
        name: "a".into(),
        trigger: ArticulationTrigger::Key { key: 24 },
        color: None,
    });
    let mut g = Group::new("g");
    g.id = GroupId(0);
    g.round_robin = RoundRobinMode::Random;
    g.voice_limit = Some(3);
    g.stealing = StealMode::Quietest;
    inst.groups.insert(g.id, g);
    let mut set = SampleSet::default();
    let data = Arc::new(SampleData {
        sample_rate: 44_100.0,
        channels: vec![vec![0.5; 20_000], vec![-0.5; 20_000]],
        loop_range: None,
        root_key: None,
    });
    for i in 0..4u32 {
        let mut z = Zone::new(
            GroupId(0),
            SampleRef::File(format!("{i}.wav")),
            KeyRange::ALL,
            60,
        );
        z.id = ZoneId(i);
        z.round_robin = (i < 3).then_some(i);
        z.trigger = if i == 3 {
            ZoneTrigger::Release
        } else {
            ZoneTrigger::Attack
        };
        z.release_decay_db = 3.0;
        set.samples
            .push(ZoneSample::whole(&format!("{i}.wav"), data.clone()));
        set.index.insert(z.id, i as usize);
        inst.zones.insert(z.id, z);
    }
    HausSampler::new(&inst, set)
}

#[test]
fn process_never_allocates() {
    let mut s = sampler();
    s.activate(&ActivateInfo {
        sample_rate: 48_000.0,
        max_frames: 256,
        quality: Quality::Live,
        seed: 9,
    });
    let mut events = Vec::with_capacity(64);
    let mut l = vec![0.0f32; 256];
    let mut r = vec![0.0f32; 256];
    for block in 0..200u32 {
        events.clear();
        let k = 40 + (block % 40) as u8;
        events.push(Event {
            frame: 0,
            kind: EventKind::NoteOn {
                note_id: block,
                key: if block % 17 == 0 { 24 } else { k },
                velocity: 0.7,
                pan: 0.1,
                fine_pitch: 3.0,
            },
        });
        events.push(Event {
            frame: 10,
            kind: EventKind::Cc {
                controller: 1,
                value: 0.5,
            },
        });
        if block > 0 {
            events.push(Event {
                frame: 100,
                kind: EventKind::NoteOff {
                    note_id: block - 1,
                    key: 40 + ((block - 1) % 40) as u8,
                    velocity: 0.0,
                },
            });
        }
        if block % 50 == 49 {
            events.push(Event {
                frame: 200,
                kind: EventKind::AllNotesOff,
            });
        }
        let ctx = ProcessContext {
            sample_rate: 48_000.0,
            frames: 256,
            song_pos_samples: block as i64 * 256,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        assert_no_alloc::assert_no_alloc(|| {
            let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
            let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
            let mut io = ProcessIo {
                inputs: &[],
                outputs: &mut ports,
                events: &events,
            };
            s.process(&ctx, &mut io);
        });
    }
    assert!(s.active_voices() <= 4);
}
