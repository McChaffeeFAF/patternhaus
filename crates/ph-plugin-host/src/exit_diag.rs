//! Opt-in crash diagnostics for process shutdown (Windows).
//!
//! Set `PATTERNHAUS_CRASH_LOG=<file>` and [`install`] adds a vectored exception handler that
//! appends one line per hardware fault to that file: the exception code, the faulting address,
//! the module that owns it, the thread, and the last [`stage`] the program reported. Nothing
//! happens without the variable. It exists to find faults that happen while the process shuts
//! down with hosted plugins still loaded.

use std::sync::atomic::{AtomicPtr, Ordering};

static STAGE: AtomicPtr<&'static str> = AtomicPtr::new(std::ptr::null_mut());

/// Records where the program is (a static label), for the crash log.
pub fn stage(label: &'static str) {
    let boxed: &'static &'static str = Box::leak(Box::new(label));
    STAGE.store(boxed as *const _ as *mut _, Ordering::SeqCst);
}

fn current_stage() -> &'static str {
    let p = STAGE.load(Ordering::SeqCst);
    if p.is_null() {
        "start"
    } else {
        // SAFETY: only leaked `&'static &'static str` values are stored
        unsafe { *p }
    }
}

/// Installs the handler when `PATTERNHAUS_CRASH_LOG` is set (Windows only; a no-op elsewhere).
pub fn install() {
    #[cfg(windows)]
    win::install();
}

#[cfg(windows)]
mod win {
    use std::io::Write;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_POINTERS,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    static PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

    pub fn install() {
        let Some(p) = std::env::var_os("PATTERNHAUS_CRASH_LOG") else {
            return;
        };
        if PATH.set(p.into()).is_ok() {
            // SAFETY: registering a handler function with the right signature, first in line
            unsafe {
                AddVectoredExceptionHandler(1, Some(handler));
            }
        }
    }

    fn module_of(addr: usize) -> String {
        let mut m: HMODULE = std::ptr::null_mut();
        // SAFETY: plain lookups; the module is not pinned or released
        unsafe {
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                addr as *const u16,
                &mut m,
            ) == 0
            {
                return "<no module>".into();
            }
            let mut buf = [0u16; 512];
            let n = GetModuleFileNameW(m, buf.as_mut_ptr(), buf.len() as u32) as usize;
            format!(
                "{}+0x{:x}",
                String::from_utf16_lossy(&buf[..n]),
                addr - m as usize
            )
        }
    }

    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        const CONTINUE_SEARCH: i32 = 0;
        // SAFETY: the system passes valid exception pointers
        let rec = unsafe { &*(*info).ExceptionRecord };
        let code = rec.ExceptionCode as u32;
        // hardware faults and fail-fast only; C++ exceptions (0xE06D7363) and debug events
        // are normal inside plugins
        if !matches!(
            code,
            0xC000_0005 | 0xC000_001D | 0xC000_0096 | 0xC000_0409 | 0xC000_0374
        ) {
            return CONTINUE_SEARCH;
        }
        let addr = rec.ExceptionAddress as usize;
        let detail = if code == 0xC000_0005 && rec.NumberParameters >= 2 {
            format!(
                " ({} 0x{:x})",
                if rec.ExceptionInformation[0] == 0 {
                    "read"
                } else {
                    "write"
                },
                rec.ExceptionInformation[1]
            )
        } else {
            String::new()
        };
        // SAFETY: plain call
        let tid = unsafe { GetCurrentThreadId() };
        let line = format!(
            "code 0x{code:08x}{detail} at {} thread {tid} stage {}\n",
            module_of(addr),
            super::current_stage()
        );
        if let Some(p) = PATH.get()
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
        {
            let _ = f.write_all(line.as_bytes());
        }
        CONTINUE_SEARCH
    }
}
