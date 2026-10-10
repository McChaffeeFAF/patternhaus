//! Real-time plumbing: the plan mailbox, the collector thread that frees retired plans,
//! FTZ/DAZ, and the `assert_no_alloc` wrapper.

use crate::plan::Plan;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering::*};
use std::thread::JoinHandle;
use std::time::Duration;

/// Runs `f` with allocation forbidden (debug builds with `AllocDisabler` installed as the
/// global allocator abort on any allocation; otherwise free).
#[inline]
pub(crate) fn no_alloc<T>(f: impl FnOnce() -> T) -> T {
    assert_no_alloc::assert_no_alloc(f)
}

/// Sets flush-to-zero and denormals-are-zero for the current thread; returns the previous
/// control state for [`restore_fp`].
pub(crate) fn enable_ftz_daz() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack));
        let new = csr | 0x8040; // FTZ (bit 15) | DAZ (bit 6)
        std::arch::asm!("ldmxcsr [{}]", in(reg) &new, options(nostack));
        csr as u64
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack));
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr | (1 << 24), options(nomem, nostack));
        fpcr
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

/// Restores the floating-point control state saved by [`enable_ftz_daz`].
pub(crate) fn restore_fp(saved: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let csr = saved as u32;
        std::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("msr fpcr, {}", in(reg) saved, options(nomem, nostack));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = saved;
}

/// Single-slot lock-free exchange carrying a new plan to the audio thread.
pub(crate) struct Mailbox(AtomicPtr<Plan>);

impl Mailbox {
    pub fn new() -> Self {
        Self(AtomicPtr::new(std::ptr::null_mut()))
    }

    /// Posts a plan; returns the previous one if the audio thread had not taken it.
    pub fn put(&self, plan: Box<Plan>) -> Option<Box<Plan>> {
        let old = self.0.swap(Box::into_raw(plan), AcqRel);
        // SAFETY: every pointer in the slot came from `Box::into_raw` and is owned by it.
        (!old.is_null()).then(|| unsafe { Box::from_raw(old) })
    }

    /// Takes the posted plan, if any. Real-time safe (one atomic swap).
    pub fn take(&self) -> Option<Box<Plan>> {
        let p = self.0.swap(std::ptr::null_mut(), AcqRel);
        (!p.is_null()).then(|| unsafe { Box::from_raw(p) })
    }
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        drop(self.take());
    }
}

/// Where the audio thread sends plans it no longer runs.
pub(crate) enum Retire {
    /// To the collector thread (real-time).
    Ring(rtrb::Producer<Box<Plan>>),
    /// Drop in place (offline rendering, not real-time).
    Drop,
}

impl Retire {
    /// Hands a plan off. `Err` gives it back when the ring is full (try again later).
    pub fn send(&mut self, plan: Box<Plan>) -> Result<(), Box<Plan>> {
        match self {
            Retire::Ring(p) => p.push(plan).map_err(|rtrb::PushError::Full(p)| p),
            Retire::Drop => {
                drop(plan);
                Ok(())
            }
        }
    }

    pub fn has_room(&self) -> bool {
        match self {
            Retire::Ring(p) => p.slots() > 0,
            Retire::Drop => true,
        }
    }
}

/// Frees retired plans (and the processors in them) off the audio thread.
pub(crate) struct Collector {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Collector {
    pub fn spawn(capacity: usize) -> (Self, Retire) {
        let (tx, mut rx) = rtrb::RingBuffer::<Box<Plan>>::new(capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::Builder::new()
            .name("ph-engine-collector".into())
            .spawn(move || {
                loop {
                    let mut any = false;
                    while let Ok(p) = rx.pop() {
                        drop(p);
                        any = true;
                    }
                    if s.load(Acquire) && rx.is_abandoned() && rx.is_empty() {
                        return;
                    }
                    if !any {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            })
            .expect("spawn collector");
        (
            Self {
                stop,
                handle: Some(handle),
            },
            Retire::Ring(tx),
        )
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.stop.store(true, Release);
        if let Some(h) = self.handle.take() {
            // the producer may still be alive (runtime owned elsewhere): don't wait forever
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}
