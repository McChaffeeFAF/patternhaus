//! Small tests of the lock-free engine pieces, sized for Miri (DESIGN §12, T3 follow-up):
//! `cargo +nightly miri test -p ph-engine --lib rt_miri`. They also run in the normal test
//! suite. Miri checks the `unsafe` in them for undefined behaviour (use after free, double
//! free, data races, aliasing) with real threads.

use super::*;
use crate::rt::Mailbox;
use crate::sched::Sched;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[test]
fn mailbox_moves_boxes_between_threads() {
    let mb = Arc::new(Mailbox::<Vec<u32>>::new());
    let m2 = mb.clone();
    let taker = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..200 {
            if let Some(b) = m2.take() {
                seen.push(b[0]);
            }
            std::thread::yield_now();
        }
        seen
    });
    let mut returned = Vec::new();
    for v in 0..20u32 {
        if let Some(old) = mb.put(Box::new(vec![v; 8])) {
            returned.push(old[0]);
        }
    }
    let mut all = taker.join().unwrap();
    if let Some(b) = mb.take() {
        all.push(b[0]);
    }
    all.extend(returned);
    all.sort();
    assert_eq!(all, (0..20).collect::<Vec<_>>());
    // try_put never overwrites; a box left in the slot is freed by Drop
    assert!(mb.try_put(Box::new(vec![1])).is_ok());
    assert!(mb.try_put(Box::new(vec![2])).is_err());
}

#[test]
fn sched_runs_a_graph_on_three_threads() {
    // 0 → {1, 2, 3} → 4 → 5
    let deps: Vec<Vec<u32>> = vec![vec![1, 2, 3], vec![4], vec![4], vec![4], vec![5], vec![]];
    let indegree = [0u32, 1, 1, 1, 3, 1];
    let sched = Sched::new(6);
    let ran: Vec<AtomicUsize> = (0..6).map(|_| AtomicUsize::new(0)).collect();
    for _block in 0..3 {
        sched.reset(|i| indegree[i], &[0]);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    sched.work(
                        |i| deps[i].as_slice(),
                        |i| {
                            ran[i].fetch_add(1, Relaxed);
                        },
                    )
                });
            }
            sched.work(
                |i| deps[i].as_slice(),
                |i| {
                    ran[i].fetch_add(1, Relaxed);
                },
            );
            sched.wait_done();
        });
    }
    assert!(ran.iter().all(|r| r.load(Relaxed) == 3));
}

/// The real engine on a simulated device with one graph worker: plan swaps go through the
/// mailbox, retired plans through the collector ring, blocks through the pool hand-off and
/// the scheduler, auditions through their two mailboxes.
#[test]
fn engine_swaps_plans_and_runs_the_pool() {
    let mut e = Edit::new(&Project::empty("miri"));
    let pat = e.add_pattern("p").unwrap();
    let a = e.add_channel("a", sampler("gen:impulse", &[])).unwrap();
    let b = e.add_channel("b", sampler("gen:impulse", &[])).unwrap();
    e.update_channel(b, |c| c.insert = InsertId(1)).unwrap();
    e.add_note(pat, a, NoteSpec::new(60, 0, STEP)).unwrap();
    e.add_note(pat, b, NoteSpec::new(60, STEP, STEP)).unwrap();
    let p1 = e.finish().0;
    let mut e = Edit::new(&p1);
    e.update_channel(a, |c| c.volume = 0.5).unwrap();
    let p2 = e.finish().0;

    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: 32,
            threads: 2,
            ..EngineConfig::default()
        },
        assets(),
    )
    .unwrap();
    assert_eq!(dev.workers(), 1);
    engine.set_project(Arc::new(p1.clone())).unwrap();
    engine.transport(TransportCommand::SetMode(crate::PlayMode::Pattern(pat)));
    engine.transport(TransportCommand::Play);
    let (mut l, mut r) = (vec![0.0f32; 32], vec![0.0f32; 32]);
    for k in 0..6 {
        match k {
            1 => engine.set_project(Arc::new(p2.clone())).unwrap(),
            2 => engine.audition("gen:impulse", 1.0).unwrap(),
            3 => engine.set_project(Arc::new(p1.clone())).unwrap(),
            4 => engine.stop_audition(),
            _ => {}
        }
        dev.pull_into(&mut l, &mut r);
    }
    let _ = engine.stats();
    drop(dev);
    drop(engine);
}

/// Input monitoring (M2/T8) on the simulated device: the rig's message ring, the source moving
/// into the audio thread and back out to the writer thread's trash ring, the monitor feed's
/// raw pointers into the insert node, and a controller message. (No take is recorded: Miri
/// has no file system by default.)
#[test]
fn input_monitoring_moves_sources_between_threads() {
    use crate::record::{Arm, RecordTarget, SimulatedInputs};
    let mut e = Edit::new(&Project::empty("miri"));
    let c = e.add_channel("a", synth(&[])).unwrap();
    e.update_channel(c, |ch| ch.insert = InsertId(1)).unwrap();
    let p = e.finish().0;
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: 32,
            threads: 2,
            ..EngineConfig::default()
        },
        assets(),
    )
    .unwrap();
    engine.set_project(Arc::new(p)).unwrap();
    let dc = vec![vec![0.25f32; 4096]; 2];
    engine.set_input_provider(Some(Box::new(SimulatedInputs::new().with("DC", SR, dc, 4))));
    let id = engine.inputs()[0].id;
    let (mut l, mut r) = (vec![0.0f32; 32], vec![0.0f32; 32]);
    for k in 0..8 {
        match k {
            0 => engine
                .arm_with(Arm {
                    input: id,
                    target: RecordTarget::Channel(c),
                    monitor: true,
                })
                .unwrap(),
            2 => engine.preview_midi(c, crate::midi::MidiMod::Bend { value: 0.5 }),
            4 => engine.set_monitor(id, false).unwrap(),
            6 => engine.disarm(id),
            _ => {}
        }
        dev.pull_into(&mut l, &mut r);
        if k == 1 {
            assert!(peak(&l) > 0.01, "the monitored input is heard");
        }
    }
    drop(dev);
    drop(engine);
}
