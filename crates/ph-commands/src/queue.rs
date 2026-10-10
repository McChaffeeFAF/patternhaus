//! The co-editing queue (DESIGN §3, §5).
//!
//! A [`SessionHandle`] is a cloneable, `Send` handle to a [`Session`] that lives on its own
//! control thread. The human's UI and every connected agent hold a clone and send requests
//! from their own threads; the control thread processes them **one at a time in arrival
//! order** and answers each on its own one-shot channel. Readers never lock the project:
//! after each request that changed it, the control thread publishes a [`Change`] carrying an
//! `Arc<Project>` snapshot to every subscriber (the engine, the UI, agent event streams).
//!
//! Nothing here runs on an audio thread, and there is no async runtime: plain
//! `std::sync::mpsc` channels and one thread.

use crate::*;
use ph_core::{ObjectPath, Project};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// One published change to the project.
#[derive(Clone, Debug)]
pub struct Change {
    /// The last history transaction the request produced; `0` when the project was replaced
    /// wholesale (`file.open`), which also clears the history.
    pub txn: u64,
    /// Who made it (for undo, the author who undid). `human` for a wholesale replacement.
    pub author: Author,
    /// Every object path the request touched; empty for a wholesale replacement.
    pub paths: Vec<ObjectPath>,
    /// Musician-readable summary, if the commands gave one.
    pub summary: Option<String>,
    /// The project after the request. Cheap to hold: collections are shared.
    pub project: Arc<Project>,
}

type Job = Box<dyn FnOnce(&mut Session, &mut dyn Host) + Send>;

enum Msg {
    Job(Job),
    Subscribe(Sender<Change>),
}

/// A reply that has not arrived yet.
pub struct Pending<R>(Receiver<R>);

impl<R> Pending<R> {
    /// Blocks until the control thread has processed the request.
    pub fn wait(self) -> CmdResult<R> {
        self.0.recv().map_err(|_| closed())
    }

    /// The reply if it has arrived; `Ok(None)` while the request is still queued.
    pub fn try_wait(&self) -> CmdResult<Option<R>> {
        match self.0.try_recv() {
            Ok(r) => Ok(Some(r)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(closed()),
        }
    }
}

fn closed() -> CmdError {
    CmdError::Other("the session is closed, or the request panicked".into())
}

/// Cloneable handle to a session running on its control thread.
#[derive(Clone)]
pub struct SessionHandle {
    tx: Sender<Msg>,
}

impl SessionHandle {
    /// Moves `session` and `host` onto a new control thread. The thread ends when the last
    /// handle is dropped.
    pub fn spawn(session: Session, host: impl Host + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        thread::Builder::new()
            .name("ph-session".into())
            .spawn(move || control_loop(session, Box::new(host), rx))
            .expect("spawn the control thread");
        Self { tx }
    }

    /// Queues `f` to run on the control thread with the session and host; the reply arrives
    /// in order with every other request. Anything the session can do can be sent this way.
    pub fn submit<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Session, &mut dyn Host) -> R + Send + 'static,
    ) -> Pending<R> {
        let (reply, rx) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |s, h| {
            let _ = reply.send(f(s, h));
        });
        // If the thread is gone the job (and its reply sender) is dropped, so `wait` errors.
        let _ = self.tx.send(Msg::Job(job));
        Pending(rx)
    }

    /// [`submit`](Self::submit) and wait.
    pub fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Session, &mut dyn Host) -> R + Send + 'static,
    ) -> CmdResult<R> {
        self.submit(f).wait()
    }

    pub fn submit_execute(&self, caller: Caller, req: Request) -> Pending<CmdResult<Executed>> {
        self.submit(move |s, h| s.execute(&caller, req, h))
    }

    pub fn execute(&self, caller: &Caller, req: Request) -> CmdResult<Executed> {
        self.submit_execute(caller.clone(), req).wait()?
    }

    pub fn submit_batch(
        &self,
        caller: Caller,
        reqs: Vec<Request>,
    ) -> Pending<CmdResult<Vec<Executed>>> {
        self.submit(move |s, h| s.batch(&caller, reqs, h))
    }

    /// One transaction, one undo step.
    pub fn batch(&self, caller: &Caller, reqs: Vec<Request>) -> CmdResult<Vec<Executed>> {
        self.submit_batch(caller.clone(), reqs).wait()?
    }

    pub fn undo(&self, author: &Author, mode: UndoMode) -> CmdResult<Option<UndoReport>> {
        let a = author.clone();
        self.call(move |s, _| s.undo_with(&a, mode))?
    }

    pub fn redo(&self, author: &Author, mode: UndoMode) -> CmdResult<Option<UndoReport>> {
        let a = author.clone();
        self.call(move |s, _| s.redo_with(&a, mode))?
    }

    pub fn undo_txn(&self, txn: u64, by: &Author, mode: UndoMode) -> CmdResult<UndoReport> {
        let by = by.clone();
        self.call(move |s, _| s.undo_txn_with(txn, &by, mode))?
    }

    pub fn undo_author(
        &self,
        target: &Author,
        by: &Author,
        mode: UndoMode,
    ) -> CmdResult<Vec<UndoReport>> {
        let (target, by) = (target.clone(), by.clone());
        self.call(move |s, _| s.undo_author(&target, &by, mode))?
    }

    /// The current snapshot (as of every request queued before this one).
    pub fn project(&self) -> CmdResult<Arc<Project>> {
        self.call(|s, _| s.project().clone())
    }

    /// A copy of the history log.
    pub fn history(&self) -> CmdResult<Vec<HistoryEntry>> {
        self.call(|s, _| s.history().to_vec())
    }

    /// Receives a [`Change`] after every request that changes the project, in order. The
    /// channel is unbounded: drain it, or drop the receiver to unsubscribe.
    pub fn subscribe(&self) -> CmdResult<Receiver<Change>> {
        let (tx, rx) = mpsc::channel();
        // Subscribing is itself queued, so it takes effect between two requests.
        self.tx.send(Msg::Subscribe(tx)).map_err(|_| closed())?;
        // Wait for the control thread to have registered it, so no change slips past.
        self.call(|_, _| ())?;
        Ok(rx)
    }
}

fn control_loop(mut session: Session, mut host: Box<dyn Host + Send>, rx: Receiver<Msg>) {
    let mut subscribers: Vec<Sender<Change>> = Vec::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Subscribe(tx) => subscribers.push(tx),
            Msg::Job(job) => {
                let before_len = session.history().len();
                let before_project = session.project().clone();
                let ran = catch_unwind(AssertUnwindSafe(|| job(&mut session, &mut *host)));
                // A panicking command leaves the session as it was (a batch only commits at
                // the end), and its caller sees a closed reply; keep serving everyone else.
                if ran.is_err() || subscribers.is_empty() {
                    continue;
                }
                let h = session.history();
                let replaced =
                    !Arc::ptr_eq(&before_project, session.project()) && h.len() <= before_len;
                if h.len() > before_len {
                    let new = &h[before_len..];
                    let mut paths = Vec::new();
                    for e in new {
                        for p in e.patches.paths() {
                            if !paths.contains(&p) {
                                paths.push(p);
                            }
                        }
                    }
                    let summary: Vec<_> = new.iter().filter_map(|e| e.summary.clone()).collect();
                    let last = new.last().expect("non-empty");
                    publish(
                        &mut subscribers,
                        Change {
                            txn: last.txn,
                            author: last.author.clone(),
                            paths,
                            summary: (!summary.is_empty()).then(|| summary.join("; ")),
                            project: session.project().clone(),
                        },
                    );
                } else if replaced {
                    publish(
                        &mut subscribers,
                        Change {
                            txn: 0,
                            author: Author::Human,
                            paths: Vec::new(),
                            summary: Some("project replaced".into()),
                            project: session.project().clone(),
                        },
                    );
                }
            }
        }
    }
}

fn publish(subscribers: &mut Vec<Sender<Change>>, change: Change) {
    subscribers.retain(|s| s.send(change.clone()).is_ok());
}
