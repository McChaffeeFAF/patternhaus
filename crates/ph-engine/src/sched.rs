//! The per-block dependency scheduler shared by the audio thread and the worker pool.
//!
//! - `pending[i]` counts node `i`'s unfinished inputs. When a node finishes, it decrements
//!   its dependents; whoever brings a counter to zero pushes that node onto the ready queue.
//! - The ready queue is a fixed array: each node is pushed exactly once per block, so `tail`
//!   only grows and no slot is reused within a block. A slot holds `node + 1` (0 = claimed
//!   by a pusher but not yet written).
//! - Any thread pops by advancing `head` with a compare-exchange; the winner owns the node.
//!
//! The atomics come from [`crate::sync`], so the loom models (`loom_models.rs`) check this
//! exact code. Miri runs it with real threads (`tests::rt_miri`).

use crate::sync::{AtomicU32, AtomicUsize, spin};
use std::sync::atomic::Ordering::*;

/// Per-block scheduling counters. Reset by the audio thread before each block.
pub(crate) struct Sched {
    pending: Box<[AtomicU32]>,
    /// Ready queue: node + 1, 0 = not yet written.
    slots: Box<[AtomicU32]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    done: AtomicUsize,
}

pub(crate) enum Pop {
    Node(usize),
    Empty,
    Finished,
}

impl Sched {
    pub fn new(nodes: usize) -> Self {
        Self {
            pending: (0..nodes).map(|_| AtomicU32::new(0)).collect(),
            slots: (0..nodes).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Prepares the counters for a block and queues the roots. Only while no other thread
    /// works on this scheduler; the pool's hand-off publishes these stores.
    pub fn reset(&self, indegree: impl Fn(usize) -> u32, roots: &[u32]) {
        self.head.store(0, Relaxed);
        self.tail.store(0, Relaxed);
        self.done.store(0, Relaxed);
        for i in 0..self.len() {
            self.pending[i].store(indegree(i), Relaxed);
            self.slots[i].store(0, Relaxed);
        }
        for r in roots {
            self.push(*r);
        }
    }

    fn push(&self, node: u32) {
        let i = self.tail.fetch_add(1, AcqRel);
        self.slots[i].store(node + 1, Release);
    }

    pub fn pop(&self) -> Pop {
        let n = self.len();
        loop {
            let h = self.head.load(Acquire);
            if h >= n {
                return Pop::Finished;
            }
            if h >= self.tail.load(Acquire) {
                return Pop::Empty;
            }
            if self
                .head
                .compare_exchange_weak(h, h + 1, AcqRel, Acquire)
                .is_ok()
            {
                loop {
                    let v = self.slots[h].load(Acquire);
                    if v != 0 {
                        return Pop::Node(v as usize - 1);
                    }
                    spin();
                }
            }
        }
    }

    /// Marks node `i` done: releases its dependents (queueing those now ready).
    pub fn complete(&self, dependents: &[u32]) {
        for d in dependents {
            if self.pending[*d as usize].fetch_sub(1, AcqRel) == 1 {
                self.push(*d);
            }
        }
        self.done.fetch_add(1, AcqRel);
    }

    /// Nodes completed this block.
    pub fn done(&self) -> usize {
        self.done.load(Acquire)
    }

    /// Processes ready nodes until every node of the block has been claimed.
    pub fn work<'a>(
        &self,
        dependents: impl Fn(usize) -> &'a [u32],
        mut process: impl FnMut(usize),
    ) {
        loop {
            match self.pop() {
                Pop::Node(i) => {
                    process(i);
                    self.complete(dependents(i));
                }
                Pop::Empty => spin(),
                Pop::Finished => return,
            }
        }
    }

    /// Waits until every node of the block is done (the audio thread, after its own `work`).
    pub fn wait_done(&self) {
        while self.done() < self.len() {
            spin();
        }
    }
}
