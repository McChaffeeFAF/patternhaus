//! The native crash guard (DESIGN §6 crash isolation, as far as the platform allows).
//!
//! [`guarded`] runs a closure that calls into plugin code. A hardware fault inside it (access
//! violation, illegal instruction, division by zero, bus error) comes back as a
//! [`NativeFault`] instead of killing the process:
//!
//! - **Windows (MSVC):** structured exception handling in a small C shim (`guard.c`,
//!   `__try`/`__except`). The frames between the fault and the shim are unwound by the
//!   system. C++ exceptions, which include Rust panics, are not caught here: they reach
//!   `catch_unwind` in [`crate::PluginNode`].
//! - **Linux and macOS:** `SIGSEGV`, `SIGBUS`, `SIGILL` and `SIGFPE` handlers jump back to the
//!   shim with `siglongjmp` when the faulting thread is inside a guarded call, and chain to the
//!   previous handler otherwise. The abandoned frames are not unwound.
//! - **Other targets** (e.g. Windows GNU): the call is unguarded ([`supported`] is false).
//!
//! Either way the plugin that faulted is suspect: its node is killed for good and the instance
//! is leaked, never called again. Real-time: a guarded call allocates nothing and makes no
//! system call (`sigsetjmp` without the signal mask; SEH is table-based).

use std::ffi::{c_int, c_ulong, c_void};

unsafe extern "C-unwind" {
    fn ph_guard_call(
        f: unsafe extern "C-unwind" fn(*mut c_void),
        ctx: *mut c_void,
        code: *mut c_ulong,
    ) -> c_int;
    fn ph_guard_install();
    fn ph_guard_supported() -> c_int;
}

/// A hardware fault caught by [`guarded`]: the Windows exception code or the Unix signal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NativeFault {
    pub code: u64,
}

impl NativeFault {
    pub fn describe(&self) -> String {
        let name = if cfg!(windows) {
            match self.code {
                0xC000_0005 => "access violation",
                0xC000_001D => "illegal instruction",
                0xC000_0096 => "privileged instruction",
                0xC000_0094 => "integer division by zero",
                0xC000_0095 => "integer overflow",
                0xC000_008C => "array bounds exceeded",
                0x8000_0002 => "misaligned data",
                0xC000_0006 => "in-page error",
                0xC000_008E => "floating-point division by zero",
                0xC000_0090 => "invalid floating-point operation",
                _ => "native exception",
            }
        } else {
            match self.code {
                11 => "segmentation fault (SIGSEGV)",
                7 | 10 => "bus error (SIGBUS)",
                4 => "illegal instruction (SIGILL)",
                8 => "arithmetic fault (SIGFPE)",
                _ => "signal",
            }
        };
        format!("{name} (code {:#x})", self.code)
    }
}

/// Whether native faults are caught on this platform.
pub fn supported() -> bool {
    // SAFETY: no arguments, no state.
    unsafe { ph_guard_supported() != 0 }
}

/// Installs the Unix signal handlers (once per process; a no-op elsewhere). Call it off the
/// audio thread before the first [`guarded`] call; format hosts do so when they are created.
pub fn install() {
    // SAFETY: idempotent, guarded by pthread_once in the shim.
    unsafe { ph_guard_install() }
}

struct Ctx<F, R> {
    f: Option<F>,
    out: Option<R>,
}

unsafe extern "C-unwind" fn trampoline<F: FnOnce() -> R, R>(ctx: *mut c_void) {
    // SAFETY: `ctx` is the `Ctx<F, R>` passed by `guarded`, alive for the whole call.
    let ctx = unsafe { &mut *(ctx as *mut Ctx<F, R>) };
    if let Some(f) = ctx.f.take() {
        ctx.out = Some(f());
    }
}

/// Runs `f`, turning a native fault inside it into `Err`. Panics propagate unchanged.
///
/// After an `Err`, whatever `f` was doing was abandoned mid-way (on Unix without running
/// destructors): treat every object `f` touched as poisoned and leak it.
pub fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, NativeFault> {
    fn run<F: FnOnce() -> R, R>(f: F) -> Result<R, NativeFault> {
        let mut ctx = Ctx::<F, R> {
            f: Some(f),
            out: None,
        };
        let mut code: c_ulong = 0;
        // SAFETY: the trampoline matches `ctx`'s type and `ctx` outlives the call.
        let faulted = unsafe {
            ph_guard_call(
                trampoline::<F, R>,
                &mut ctx as *mut Ctx<F, R> as *mut c_void,
                &mut code,
            )
        };
        match (faulted, ctx.out.take()) {
            (0, Some(r)) => Ok(r),
            _ => {
                // the closure may be half-run: never drop what it captured
                std::mem::forget(ctx.f.take());
                Err(NativeFault { code: code as u64 })
            }
        }
    }
    run(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guarded_call_returns_its_value_and_panics_pass_through() {
        install();
        assert_eq!(guarded(|| 6 * 7), Ok(42));
        let p = std::panic::catch_unwind(|| guarded(|| -> u32 { panic!("boom") }));
        assert!(p.is_err(), "a panic is not a native fault");
        // still usable afterwards
        assert_eq!(guarded(|| "ok"), Ok("ok"));
    }

    #[test]
    fn an_access_violation_is_caught() {
        install();
        if !supported() {
            return;
        }
        let r = guarded(|| {
            let p = std::hint::black_box(8usize) as *mut u32;
            // SAFETY: deliberately not: this is the fault under test
            unsafe { std::ptr::write_volatile(p, 1) };
            1
        });
        let fault = r.unwrap_err();
        assert!(
            fault.describe().contains("access violation") || fault.describe().contains("SIG"),
            "{}",
            fault.describe()
        );
        // nested guards and later calls still work
        assert_eq!(guarded(|| guarded(|| 3)), Ok(Ok(3)));
    }
}
