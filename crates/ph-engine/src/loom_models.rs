//! loom models of the engine's lock-free protocols (DESIGN §12, T3 follow-up).
//!
//! Run with `RUSTFLAGS="--cfg loom" cargo test -p ph-engine --lib --release loom_models`.
//! The models drive the real `Mailbox`, `Sched` and `Handoff` code: under `--cfg loom` their
//! atomics are loom's (`sync.rs`), so loom explores every interleaving (and weak-memory
//! reordering it models) of the operations below.

use crate::pool::Handoff;
use crate::rt::Mailbox;
use crate::sched::Sched;
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicUsize};
use loom::thread;
use std::sync::atomic::Ordering::*;

/// A payload whose contents must be visible to whoever takes it.
struct Payload(u64);

/// Mailbox: the control thread posts two plans back to back while the audio thread takes;
/// every box is received or returned exactly once, with its contents intact.
#[test]
fn mailbox_put_take_race() {
    loom::model(|| {
        let mb = Arc::new(Mailbox::<Payload>::new());
        let m2 = mb.clone();
        let audio = thread::spawn(move || {
            let mut got = Vec::new();
            for _ in 0..2 {
                if let Some(p) = m2.take() {
                    got.push(p.0);
                }
            }
            got
        });
        let mut returned = Vec::new();
        for v in [1u64, 2] {
            if let Some(old) = mb.put(Box::new(Payload(v))) {
                returned.push(old.0);
            }
        }
        let mut seen = audio.join().unwrap();
        if let Some(p) = mb.take() {
            seen.push(p.0);
        }
        seen.extend(returned);
        seen.sort();
        assert_eq!(seen, vec![1, 2], "every plan is delivered or returned once");
    });
}

/// Mailbox `try_put` (the audition return slot): the audio thread hands back a voice while
/// the control thread collects; nothing is lost or freed twice.
#[test]
fn mailbox_try_put_vs_take() {
    loom::model(|| {
        let mb = Arc::new(Mailbox::<Payload>::new());
        let m2 = mb.clone();
        let audio = thread::spawn(move || {
            let mut kept = Vec::new();
            for v in [10u64, 11] {
                if let Err(b) = m2.try_put(Box::new(Payload(v))) {
                    kept.push(b.0); // slot full: the audio thread keeps it
                }
            }
            kept
        });
        let mut freed = Vec::new();
        if let Some(p) = mb.take() {
            freed.push(p.0);
        }
        let kept = audio.join().unwrap();
        if let Some(p) = mb.take() {
            freed.push(p.0);
        }
        freed.extend(kept);
        freed.sort();
        assert_eq!(freed, vec![10, 11]);
    });
}

/// Ready queue and dependency counters: a diamond graph (0 → 1, 2 → 3) on two threads.
/// Each node runs exactly once, only after all of its inputs finished, and the block ends
/// with every node done.
#[test]
fn sched_diamond_two_threads() {
    loom::model(|| {
        let deps: Arc<Vec<Vec<u32>>> = Arc::new(vec![vec![1, 2], vec![3], vec![3], vec![]]);
        let indegree = [0u32, 1, 1, 2];
        let inputs: [&[usize]; 4] = [&[], &[0], &[0], &[1, 2]];
        let sched = Arc::new(Sched::new(4));
        let ran: Arc<Vec<AtomicUsize>> = Arc::new((0..4).map(|_| AtomicUsize::new(0)).collect());
        let finished: Arc<Vec<AtomicBool>> =
            Arc::new((0..4).map(|_| AtomicBool::new(false)).collect());
        sched.reset(|i| indegree[i], &[0]);

        let body = {
            let (sched, deps, ran, finished) =
                (sched.clone(), deps.clone(), ran.clone(), finished.clone());
            move || {
                sched.work(
                    |i| deps[i].as_slice(),
                    |i| {
                        for &x in inputs[i] {
                            assert!(finished[x].load(Relaxed), "node {i} ran before input {x}");
                        }
                        ran[i].fetch_add(1, Relaxed);
                        finished[i].store(true, Relaxed);
                    },
                )
            }
        };
        let worker = thread::spawn(body.clone());
        body();
        sched.wait_done();
        worker.join().unwrap();
        for (i, r) in ran.iter().enumerate() {
            assert_eq!(r.load(Relaxed), 1, "node {i} ran once");
        }
    });
}

/// Two independent roots feeding one sink: the two finishing nodes race on the sink's
/// counter, and exactly one of them queues it.
#[test]
fn sched_join_race() {
    loom::model(|| {
        let deps: Arc<Vec<Vec<u32>>> = Arc::new(vec![vec![2], vec![2], vec![]]);
        let sched = Arc::new(Sched::new(3));
        let ran: Arc<Vec<AtomicUsize>> = Arc::new((0..3).map(|_| AtomicUsize::new(0)).collect());
        let finished: Arc<Vec<AtomicBool>> =
            Arc::new((0..3).map(|_| AtomicBool::new(false)).collect());
        sched.reset(|i| [0u32, 0, 2][i], &[0, 1]);
        let body = {
            let (sched, deps, ran, finished) =
                (sched.clone(), deps.clone(), ran.clone(), finished.clone());
            move || {
                sched.work(
                    |i| deps[i].as_slice(),
                    |i| {
                        if i == 2 {
                            assert!(finished[0].load(Relaxed) && finished[1].load(Relaxed));
                        }
                        ran[i].fetch_add(1, Relaxed);
                        finished[i].store(true, Relaxed);
                    },
                )
            }
        };
        let worker = thread::spawn(body.clone());
        body();
        sched.wait_done();
        worker.join().unwrap();
        assert!(ran.iter().all(|r| r.load(Relaxed) == 1));
    });
}

/// A job for the hand-off model: `alive` is cleared when the audio thread is done with it
/// (standing in for the stack frame going away).
struct FakeJob {
    alive: AtomicBool,
    block: u64,
}

/// Pool hand-off across two blocks: a worker that saw a generation may pick up the job
/// late, but it never touches a job after `retract` returned for it.
#[test]
fn handoff_never_uses_a_retracted_job() {
    loom::model(|| {
        let h = Arc::new(Handoff::<FakeJob>::new());
        let jobs = Arc::new([
            FakeJob {
                alive: AtomicBool::new(false),
                block: 0,
            },
            FakeJob {
                alive: AtomicBool::new(false),
                block: 1,
            },
        ]);
        let worker = {
            let h = h.clone();
            thread::spawn(move || {
                let mut last = 0;
                let mut used = 0;
                loop {
                    if h.is_shut_down() {
                        return used;
                    }
                    let g = h.generation();
                    if g == last {
                        thread::yield_now();
                        continue;
                    }
                    last = g;
                    h.with_job(|j| {
                        assert!(
                            j.alive.load(Relaxed),
                            "worker used block {} after retract",
                            j.block
                        );
                        used += 1;
                    });
                }
            })
        };
        for j in jobs.iter() {
            j.alive.store(true, Relaxed);
            h.publish(j);
            h.retract();
            j.alive.store(false, Relaxed);
        }
        h.shut_down();
        let used = worker.join().unwrap();
        assert!(used <= 2);
    });
}
