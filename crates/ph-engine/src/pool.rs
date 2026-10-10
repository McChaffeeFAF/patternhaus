//! The real-time worker pool for the multicore graph.
//!
//! - Threads are spawned once, up front. A block's work is published as a pointer to a
//!   [`Job`] plus a generation bump; the audio thread works on the job too.
//! - Scheduling is by atomic dependency counters and a fixed ready queue (`sched.rs`), so
//!   work is shared without locks or allocation.
//! - The job hand-off ([`Handoff`]) keeps a published job alive until every worker that
//!   picked it up has let go (`busy`). Both protocols are model-checked by loom
//!   (`loom_models.rs`) and run under Miri (`tests::rt_miri`).
//! - Idle workers spin briefly, then park. The audio thread unparks only workers that
//!   announced they are parked.
//! - Determinism does not depend on the schedule: a node sums its inputs itself in a fixed
//!   order (see `plan.rs`).

use crate::plan::Plan;
use crate::runtime::{ChunkCtx, process_node};
use crate::sync::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, spin};
use std::sync::Arc;
use std::sync::atomic::Ordering::*;
use std::thread::{JoinHandle, Thread};

/// Spins before a worker parks (about tens of microseconds).
const SPIN_BEFORE_PARK: u32 = 20_000;

/// One block of graph work.
pub(crate) struct Job {
    pub plan: *const Plan,
    pub ctx: ChunkCtx,
}

// SAFETY: `Job` holds a raw plan pointer; plans are `Sync`, and the hand-off keeps the job
// (and the plan it points to) alive while workers use it.
unsafe impl Sync for Job {}

/// Publishes one job at a time from the audio thread to the workers.
///
/// - [`Handoff::publish`] stores the job pointer, then bumps the generation.
/// - A worker that sees a new generation calls [`Handoff::with_job`]: it announces itself in
///   `busy` *before* loading the pointer, so
/// - [`Handoff::retract`] (null the pointer, then wait for `busy == 0`) guarantees that no
///   worker still uses the job when it returns. A worker that loads after the retract sees
///   null, or the next block's job (which is valid), and never the old one.
pub(crate) struct Handoff<J> {
    job: AtomicPtr<J>,
    generation: AtomicU64,
    busy: AtomicUsize,
    shutdown: AtomicBool,
}

// SAFETY: the hand-off only shares `&J` between threads.
unsafe impl<J: Sync> Send for Handoff<J> {}
unsafe impl<J: Sync> Sync for Handoff<J> {}

impl<J> Handoff<J> {
    pub fn new() -> Self {
        Self {
            job: AtomicPtr::new(std::ptr::null_mut()),
            generation: AtomicU64::new(0),
            busy: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Audio thread: makes `job` visible to workers. It must stay alive until `retract`.
    pub fn publish(&self, job: &J) {
        self.job.store(job as *const J as *mut J, SeqCst);
        self.generation.fetch_add(1, SeqCst);
    }

    /// Audio thread: withdraws the job and waits until no worker holds it.
    pub fn retract(&self) {
        self.job.store(std::ptr::null_mut(), SeqCst);
        // Read `busy` with a read-modify-write, not a load. A plain load lets the null store
        // and this read reorder against a worker's `busy` increment and job load (the
        // store-buffer interleaving loom found, `loom_models::handoff_*`), so both could miss
        // each other. With an RMW, either it is ordered before the worker's increment (which
        // then synchronizes with it and sees the null), or it sees the worker as busy.
        while self.busy.fetch_add(0, AcqRel) != 0 {
            spin();
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(SeqCst)
    }

    /// The generation read with a read-modify-write: a worker that announced it is going to
    /// park uses this, so either it sees the new generation or the publisher (whose bump is
    /// an RMW too) sees its `sleeping` flag and unparks it.
    pub fn generation_rmw(&self) -> u64 {
        self.generation.fetch_add(0, AcqRel)
    }

    pub fn shut_down(&self) {
        self.shutdown.store(true, SeqCst);
        self.generation.fetch_add(1, SeqCst);
    }

    pub fn is_shut_down(&self) -> bool {
        self.shutdown.load(SeqCst)
    }

    /// Worker: runs `f` on the current job, if one is published.
    pub fn with_job(&self, f: impl FnOnce(&J)) {
        self.busy.fetch_add(1, SeqCst);
        let job = self.job.load(SeqCst);
        if !job.is_null() {
            // SAFETY: the publisher keeps the job alive until `busy` is back to 0.
            f(unsafe { &*job });
        }
        self.busy.fetch_sub(1, SeqCst);
    }
}

struct PoolShared {
    handoff: Handoff<Job>,
    sleeping: Vec<std::sync::atomic::AtomicBool>,
}

pub(crate) struct Pool {
    shared: Arc<PoolShared>,
    threads: Vec<Thread>,
    handles: Vec<JoinHandle<()>>,
}

impl Pool {
    /// Spawns `workers` threads (the audio thread is the extra participant).
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(PoolShared {
            handoff: Handoff::new(),
            sleeping: (0..workers)
                .map(|_| std::sync::atomic::AtomicBool::new(false))
                .collect(),
        });
        let mut handles = Vec::with_capacity(workers);
        for w in 0..workers {
            let s = shared.clone();
            let h = std::thread::Builder::new()
                .name(format!("ph-engine-worker-{w}"))
                .spawn(move || worker(s, w))
                .expect("spawn engine worker");
            handles.push(h);
        }
        let threads = handles.iter().map(|h| h.thread().clone()).collect();
        Self {
            shared,
            threads,
            handles,
        }
    }

    #[cfg(test)]
    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// Runs one block of the graph across the pool and the calling (audio) thread.
    /// Returns when every node is done and no worker still holds the job. The plan's
    /// scheduler must have been reset ([`reset_sched`]).
    pub fn run(&self, job: &Job) {
        let s = &*self.shared;
        s.handoff.publish(job);
        for (i, sleeping) in s.sleeping.iter().enumerate() {
            if sleeping.load(SeqCst) {
                self.threads[i].unpark();
            }
        }
        // SAFETY: the plan outlives this call (owned by the runtime).
        let plan = unsafe { &*job.plan };
        work(plan, &job.ctx);
        plan.sched.wait_done();
        s.handoff.retract();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.handoff.shut_down();
        for t in &self.threads {
            t.unpark();
        }
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn worker(s: Arc<PoolShared>, index: usize) {
    crate::rt::enable_ftz_daz();
    let h = &s.handoff;
    let mut last = h.generation();
    loop {
        // wait for a new generation: spin, then park
        let mut spins = 0u32;
        loop {
            if h.is_shut_down() {
                return;
            }
            let g = h.generation();
            if g != last {
                last = g;
                break;
            }
            spins += 1;
            if spins < SPIN_BEFORE_PARK {
                std::hint::spin_loop();
            } else {
                s.sleeping[index].store(true, SeqCst);
                if h.generation_rmw() == last && !h.is_shut_down() {
                    std::thread::park();
                }
                s.sleeping[index].store(false, SeqCst);
                spins = 0;
            }
        }
        h.with_job(|job| {
            // SAFETY: the job's plan lives as long as the job (see `Pool::run`).
            let plan = unsafe { &*job.plan };
            crate::rt::no_alloc(|| work(plan, &job.ctx));
        });
    }
}

/// Prepares the plan's counters for a block and queues the root nodes. Audio thread only,
/// while no worker holds a job.
pub(crate) fn reset_sched(plan: &Plan) {
    plan.sched.reset(|i| plan.nodes[i].indegree, &plan.roots);
}

/// Processes ready nodes until every node of the block has been claimed.
fn work(plan: &Plan, ctx: &ChunkCtx) {
    plan.sched.work(
        |i| &plan.nodes[i].dependents,
        // SAFETY: the ready queue hands node `i` to this thread only, after its inputs.
        |i| unsafe { process_node(plan, i, ctx) },
    );
}
