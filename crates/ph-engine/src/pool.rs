//! The real-time worker pool for the multicore graph.
//!
//! - Threads are spawned once, up front. A block's work is published as a pointer to a
//!   [`Job`] plus a generation bump; the audio thread works on the job too.
//! - Scheduling is by atomic dependency counters (`Sched::pending`). When a node finishes,
//!   it decrements its dependents; whoever brings a counter to zero pushes that node onto
//!   the plan's ready queue (a fixed array: each node is pushed exactly once per block).
//!   Any thread pops from it, so work is shared without locks or allocation.
//! - Idle workers spin briefly, then park. The audio thread unparks only workers that
//!   announced they are parked.
//! - Determinism does not depend on the schedule: a node sums its inputs itself in a fixed
//!   order (see `plan.rs`).

use crate::plan::Plan;
use crate::runtime::{ChunkCtx, process_node};
use std::hint::spin_loop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering::*};
use std::thread::{JoinHandle, Thread};

/// Spins before a worker parks (≈ tens of microseconds).
const SPIN_BEFORE_PARK: u32 = 20_000;

/// One block of graph work.
pub(crate) struct Job {
    pub plan: *const Plan,
    pub ctx: ChunkCtx,
}

struct PoolShared {
    job: AtomicPtr<Job>,
    generation: AtomicU64,
    busy: AtomicUsize,
    sleeping: Vec<AtomicBool>,
    shutdown: AtomicBool,
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
            job: AtomicPtr::new(std::ptr::null_mut()),
            generation: AtomicU64::new(0),
            busy: AtomicUsize::new(0),
            sleeping: (0..workers).map(|_| AtomicBool::new(false)).collect(),
            shutdown: AtomicBool::new(false),
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
        s.job.store(job as *const Job as *mut Job, SeqCst);
        s.generation.fetch_add(1, SeqCst);
        for (i, sleeping) in s.sleeping.iter().enumerate() {
            if sleeping.load(SeqCst) {
                self.threads[i].unpark();
            }
        }
        // SAFETY: the plan outlives this call (owned by the runtime).
        let plan = unsafe { &*job.plan };
        work(plan, &job.ctx);
        let n = plan.nodes.len();
        while plan.sched.done.load(Acquire) < n {
            spin_loop();
        }
        s.job.store(std::ptr::null_mut(), SeqCst);
        while s.busy.load(SeqCst) != 0 {
            spin_loop();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, SeqCst);
        self.shared.generation.fetch_add(1, SeqCst);
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
    let mut last = s.generation.load(SeqCst);
    loop {
        // wait for a new generation: spin, then park
        let mut spins = 0u32;
        loop {
            if s.shutdown.load(Acquire) {
                return;
            }
            let g = s.generation.load(SeqCst);
            if g != last {
                last = g;
                break;
            }
            spins += 1;
            if spins < SPIN_BEFORE_PARK {
                spin_loop();
            } else {
                s.sleeping[index].store(true, SeqCst);
                if s.generation.load(SeqCst) == last && !s.shutdown.load(SeqCst) {
                    std::thread::park();
                }
                s.sleeping[index].store(false, SeqCst);
                spins = 0;
            }
        }
        s.busy.fetch_add(1, SeqCst);
        let job = s.job.load(SeqCst);
        if !job.is_null() {
            // SAFETY: the audio thread keeps the job and its plan alive until `busy` is 0.
            let job = unsafe { &*job };
            let plan = unsafe { &*job.plan };
            crate::rt::no_alloc(|| work(plan, &job.ctx));
        }
        s.busy.fetch_sub(1, SeqCst);
    }
}

/// Prepares the plan's counters for a block and queues the root nodes. Audio thread only,
/// while no worker holds a job.
pub(crate) fn reset_sched(plan: &Plan) {
    let sc = &plan.sched;
    sc.head.store(0, Relaxed);
    sc.tail.store(0, Relaxed);
    sc.done.store(0, Relaxed);
    for (i, node) in plan.nodes.iter().enumerate() {
        sc.pending[i].store(node.indegree, Relaxed);
        sc.slots[i].store(0, Relaxed);
    }
    for r in &plan.roots {
        push(plan, *r);
    }
}

fn push(plan: &Plan, node: u32) {
    let i = plan.sched.tail.fetch_add(1, AcqRel);
    plan.sched.slots[i].store(node + 1, Release);
}

enum Pop {
    Node(usize),
    Empty,
    Finished,
}

fn pop(plan: &Plan) -> Pop {
    let sc = &plan.sched;
    let n = plan.nodes.len();
    loop {
        let h = sc.head.load(Acquire);
        if h >= n {
            return Pop::Finished;
        }
        if h >= sc.tail.load(Acquire) {
            return Pop::Empty;
        }
        if sc
            .head
            .compare_exchange_weak(h, h + 1, AcqRel, Acquire)
            .is_ok()
        {
            loop {
                let v = sc.slots[h].load(Acquire);
                if v != 0 {
                    return Pop::Node(v as usize - 1);
                }
                spin_loop();
            }
        }
    }
}

/// Processes ready nodes until every node of the block has been claimed.
fn work(plan: &Plan, ctx: &ChunkCtx) {
    loop {
        match pop(plan) {
            Pop::Node(i) => {
                // SAFETY: the ready queue hands node `i` to this thread only.
                unsafe { process_node(plan, i, ctx) };
                for d in &plan.nodes[i].dependents {
                    if plan.sched.pending[*d as usize].fetch_sub(1, AcqRel) == 1 {
                        push(plan, *d);
                    }
                }
                plan.sched.done.fetch_add(1, AcqRel);
            }
            Pop::Empty => spin_loop(),
            Pop::Finished => return,
        }
    }
}
