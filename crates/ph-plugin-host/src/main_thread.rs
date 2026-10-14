//! The plugins' main thread.
//!
//! CLAP and VST3 both have a *main thread*: instances are created, activated, saved and
//! destroyed there, and their editors live there. Patternhaus has no single UI-owned main
//! thread that every caller runs on (commands, the engine's compiler and the UI each have
//! their own), so one dedicated thread plays that role for every plugin in the process.
//! Callers marshal work to it with [`MainThread::run`] and wait for the result.
//!
//! The thread also runs idle hooks every few milliseconds (CLAP `request_callback`) and, on
//! Windows, pumps window messages so plugin editors created on it stay responsive.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

pub(crate) struct MainThread {
    tx: Mutex<Sender<Job>>,
    id: ThreadId,
}

thread_local! {
    /// Hooks run between jobs on the main thread.
    static IDLE: RefCell<Vec<fn()>> = const { RefCell::new(Vec::new()) };
}

/// How often idle hooks run when no job arrives.
const TICK: Duration = Duration::from_millis(10);

pub(crate) fn main_thread() -> &'static MainThread {
    static MAIN: OnceLock<MainThread> = OnceLock::new();
    MAIN.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let (id_tx, id_rx) = sync_channel(1);
        std::thread::Builder::new()
            .name("ph-plugin-main".into())
            .spawn(move || {
                let _ = id_tx.send(std::thread::current().id());
                run_loop(rx);
            })
            .expect("cannot start the plugin main thread");
        let id = id_rx.recv().expect("plugin main thread did not start");
        MainThread {
            tx: Mutex::new(tx),
            id,
        }
    })
}

fn run_loop(rx: Receiver<Job>) {
    loop {
        match rx.recv_timeout(TICK) {
            Ok(job) => {
                // a panicking job must not take the thread (and every plugin) down with it
                let _ = catch_unwind(AssertUnwindSafe(job));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let hooks: Vec<fn()> = IDLE.with(|h| h.borrow().clone());
        for hook in hooks {
            let _ = catch_unwind(hook);
        }
        pump_messages();
    }
}

#[cfg(windows)]
fn pump_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    // SAFETY: plain Win32 message loop on the thread that owns the windows.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(windows))]
fn pump_messages() {}

impl MainThread {
    pub fn is_current(&self) -> bool {
        std::thread::current().id() == self.id
    }

    /// Runs `f` on the main thread and waits for its result (inline when already there).
    pub fn run<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> R {
        if self.is_current() {
            return f();
        }
        let (tx, rx) = sync_channel(1);
        let job: Job = Box::new(move || {
            let _ = tx.send(f());
        });
        self.tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(job)
            .expect("the plugin main thread has stopped");
        rx.recv()
            .expect("a plugin main-thread job panicked; see the log")
    }

    /// Queues `f` without waiting.
    pub fn post(&self, f: impl FnOnce() + Send + 'static) {
        if self.is_current() {
            f();
            return;
        }
        let _ = self
            .tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(Box::new(f));
    }

    /// Registers a hook to run on the main thread between jobs (at least every 10 ms).
    pub fn add_idle(&self, hook: fn()) {
        self.run(move || {
            IDLE.with(|h| {
                let mut h = h.borrow_mut();
                if !h.contains(&hook) {
                    h.push(hook);
                }
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_jobs_on_one_thread_and_survives_a_panicking_job() {
        let m = main_thread();
        let a = m.run(|| std::thread::current().id());
        let b = m.run(|| std::thread::current().id());
        assert_eq!(a, b);
        assert_ne!(a, std::thread::current().id());
        assert!(m.run(|| main_thread().is_current()));
        // re-entrant calls run inline
        assert_eq!(m.run(|| main_thread().run(|| 5)), 5);
        let r = std::panic::catch_unwind(|| m.run(|| -> u8 { panic!("job") }));
        assert!(r.is_err());
        assert_eq!(m.run(|| 1 + 1), 2, "still alive");
    }
}
