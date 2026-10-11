//! Atomics used by the lock-free engine pieces ([`crate::rt::Mailbox`], `sched`, the pool
//! hand-off). Under `--cfg loom` they are loom's model-checked atomics, so the loom models in
//! `tests/loom.rs` exercise the same code the engine runs.

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize};
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize};

/// Busy-wait hint. Under loom it yields, so the model checker can schedule the thread being
/// waited on.
#[inline]
pub(crate) fn spin() {
    #[cfg(loom)]
    loom::thread::yield_now();
    #[cfg(not(loom))]
    std::hint::spin_loop();
}
