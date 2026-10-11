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

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::PatternId;
    use serde_json::json;

    fn spawn() -> SessionHandle {
        SessionHandle::spawn(
            Session::new(Arc::new(Registry::with_builtins()), Project::empty("q")),
            NullHost::default(),
        )
    }

    fn setup(h: &SessionHandle) -> (u32, u32) {
        let who = Caller::human();
        let pat = h
            .execute(&who, Request::new("pattern.add", json!({})))
            .unwrap()
            .result["pattern"]
            .as_u64()
            .unwrap() as u32;
        let ch = h
            .execute(
                &who,
                Request::new(
                    "channel.add",
                    json!({ "name": "K", "instrument": { "kind": "synth", "preset": null } }),
                ),
            )
            .unwrap()
            .result["channel"]
            .as_u64()
            .unwrap() as u32;
        (pat, ch)
    }

    #[test]
    fn many_threads_are_serialized_in_arrival_order() {
        let h = spawn();
        let (pat, ch) = setup(&h);
        let changes = h.subscribe().unwrap();
        const THREADS: usize = 8;
        const EACH: usize = 25;
        let workers: Vec<_> = (0..THREADS)
            .map(|t| {
                let h = h.clone();
                std::thread::spawn(move || {
                    let who = if t % 2 == 0 {
                        Caller::human()
                    } else {
                        Caller::agent(&format!("bot{t}"), Grant(vec![Scope::Compose]))
                    };
                    (0..EACH)
                        .map(|i| {
                            let notes = [json!({ "key": 30 + t, "start": i * 240, "length": 240 })];
                            h.execute(
                                &who,
                                Request::new(
                                    "note.add",
                                    json!({ "pattern": pat, "channel": ch, "notes": notes }),
                                ),
                            )
                            .unwrap()
                            .txn
                            .unwrap()
                        })
                        .collect::<Vec<u64>>()
                })
            })
            .collect();
        let per_thread: Vec<Vec<u64>> = workers.into_iter().map(|w| w.join().unwrap()).collect();

        // Each thread saw its own requests applied in the order it sent them.
        for txns in &per_thread {
            assert!(txns.windows(2).all(|w| w[0] < w[1]), "{txns:?}");
        }
        // No transaction was lost or duplicated.
        let mut all: Vec<u64> = per_thread.concat();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), THREADS * EACH);
        let project = h.project().unwrap();
        assert_eq!(
            project.pattern(PatternId(pat)).unwrap().note_count(),
            THREADS * EACH
        );
        assert_eq!(h.history().unwrap().len(), 2 + THREADS * EACH);

        // The subscriber heard every change, in order, each with a consistent snapshot.
        let heard: Vec<Change> = changes.try_iter().collect();
        assert_eq!(heard.len(), THREADS * EACH);
        assert!(heard.windows(2).all(|w| w[0].txn < w[1].txn));
        for (i, c) in heard.iter().enumerate() {
            let notes = c.project.pattern(PatternId(pat)).unwrap().note_count();
            assert_eq!(notes, i + 1, "snapshot {i} matches its change");
            assert_eq!(c.paths.len(), 1);
        }
        assert!(Arc::ptr_eq(&heard.last().unwrap().project, &project));
    }

    /// M1 criterion 3: a human and an agent edit at the same time from their own threads;
    /// undoing the agent's work reverts only the agent's notes, and an undo that would
    /// overwrite the human's later edit stops and names the conflict.
    #[test]
    fn concurrent_human_and_agent_edits_undo_per_author() {
        let h = spawn();
        let (pat, ch) = setup(&h);
        let human = Caller::human();
        let claude = Caller::agent("claude", Grant(vec![Scope::Compose]));
        let edit = |who: Caller, key: u32| {
            let h = h.clone();
            std::thread::spawn(move || {
                for i in 0..20 {
                    let notes = [json!({ "key": key, "start": i * 240, "length": 240 })];
                    h.execute(
                        &who,
                        Request::new(
                            "note.add",
                            json!({ "pattern": pat, "channel": ch, "notes": notes }),
                        ),
                    )
                    .unwrap();
                }
            })
        };
        let (a, b) = (edit(human.clone(), 60), edit(claude.clone(), 72));
        a.join().unwrap();
        b.join().unwrap();
        let notes = |h: &SessionHandle| {
            let p = h.project().unwrap();
            let mut keys: Vec<u8> = p
                .pattern(PatternId(pat))
                .unwrap()
                .notes
                .values()
                .flat_map(|m| m.values().map(|n| n.key))
                .collect();
            keys.sort_unstable();
            keys
        };
        assert_eq!(notes(&h).len(), 40, "both authors' edits landed");

        // undo everything the agent did: only the human's notes remain
        h.undo_author(&claude.author, &human.author, UndoMode::Stop)
            .unwrap();
        assert_eq!(
            notes(&h),
            vec![60u8; 20],
            "the agent's notes are gone, mine stay"
        );

        // the agent adds a note, the human then edits it: undoing the agent conflicts
        let out = h
            .execute(
                &claude,
                Request::new(
                    "note.add",
                    json!({ "pattern": pat, "channel": ch, "notes": [{ "key": 84, "start": 0, "length": 240 }] }),
                ),
            )
            .unwrap();
        let id = out.result["notes"][0].as_u64().expect("note id");
        h.execute(
            &human,
            Request::new(
                "note.update",
                json!({ "pattern": pat, "channel": ch, "updates": [{ "id": id, "velocity": 0.3 }] }),
            ),
        )
        .unwrap();
        let before = h.project().unwrap();
        let r = h.undo(&claude.author, UndoMode::Stop);
        assert!(
            matches!(r, Err(CmdError::UndoConflict(ref c)) if c[0].changed_by.contains(&Author::Human)),
            "{r:?}"
        );
        assert_eq!(
            *h.project().unwrap(),
            *before,
            "a stopped undo changes nothing"
        );
    }

    #[test]
    fn undo_and_batch_go_through_the_same_queue() {
        let h = spawn();
        let (pat, ch) = setup(&h);
        let changes = h.subscribe().unwrap();
        let claude = Caller::agent("claude", Grant(vec![Scope::Compose]));
        let out = h
            .batch(
                &claude,
                vec![
                    Request::new("project.set_tempo", json!({ "bpm": 90 })),
                    Request::new(
                        "note.add",
                        json!({ "pattern": pat, "channel": ch, "notes": [{ "key": 40, "start": 0, "length": 240 }] }),
                    ),
                ],
            )
            .unwrap();
        assert_eq!(out[0].txn, out[1].txn);
        let c = changes.recv().unwrap();
        assert_eq!(c.author, claude.author);
        assert_eq!(c.txn, out[0].txn.unwrap());
        assert_eq!(c.paths.len(), 2);
        assert!(c.summary.unwrap().contains("tempo"));

        let report = h.undo(&claude.author, UndoMode::Stop).unwrap().unwrap();
        let c = changes.recv().unwrap();
        assert_eq!(c.txn, report.txn);
        assert_eq!(c.project.settings.bpm, 130.0);
        assert!(h.redo(&claude.author, UndoMode::Stop).unwrap().is_some());
        assert_eq!(changes.recv().unwrap().project.settings.bpm, 90.0);

        // A rejected request publishes nothing and the queue keeps working.
        assert!(
            h.execute(
                &claude,
                Request::new("project.set_tempo", json!({ "bpm": 1 }))
            )
            .is_err()
        );
        assert!(changes.try_recv().is_err());
    }

    #[test]
    fn a_panicking_request_does_not_kill_the_queue() {
        let h = spawn();
        let pending = h.submit(|_, _| -> u32 { panic!("boom") });
        assert!(pending.wait().is_err());
        assert_eq!(h.call(|_, _| 7).unwrap(), 7);
    }

    #[test]
    fn replacing_the_project_is_published_and_dropped_subscribers_are_forgotten() {
        let h = spawn();
        let kept = h.subscribe().unwrap();
        drop(h.subscribe().unwrap());
        h.call(|s, _| s.reset(Project::empty("other"))).unwrap();
        let c = kept.recv().unwrap();
        assert_eq!((c.txn, c.project.settings.title.as_str()), (0, "other"));
        // Still fine after the dead subscriber is pruned.
        let who = Caller::human();
        h.execute(
            &who,
            Request::new("project.set_tempo", json!({ "bpm": 99 })),
        )
        .unwrap();
        assert_eq!(kept.recv().unwrap().project.settings.bpm, 99.0);
    }

    #[test]
    fn handles_are_send_and_clone() {
        fn assert_send<T: Send + Clone + 'static>() {}
        assert_send::<SessionHandle>();
    }
}
