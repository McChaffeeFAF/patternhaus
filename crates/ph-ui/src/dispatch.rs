//! Sends commands to the session without ever blocking the UI thread.
//!
//! Every UI action is a [`Request`] submitted through `SessionHandle::submit_*` as
//! `Caller::human()`. Replies are collected with [`Dispatcher::poll`] once per frame.

use ph_commands::{Caller, CmdResult, Executed, Pending, Request, SessionHandle};

/// What to do with a command's reply beyond reporting errors.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Follow {
    None,
    /// Select the notes in `result["notes"]` once they appear in the snapshot.
    SelectNotes,
}

pub struct Done {
    pub request: Request,
    pub result: CmdResult<Executed>,
    pub follow: Follow,
}

enum Kind {
    One(Pending<CmdResult<Executed>>),
    Batch(Pending<CmdResult<Vec<Executed>>>),
}

struct Item {
    request: Request,
    follow: Follow,
    kind: Kind,
}

pub struct Dispatcher {
    session: SessionHandle,
    caller: Caller,
    pending: Vec<Item>,
    /// Count of commands sent, for tests and the status line.
    pub sent: u64,
}

impl Dispatcher {
    pub fn new(session: SessionHandle) -> Self {
        Self {
            session,
            caller: Caller::human(),
            pending: Vec::new(),
            sent: 0,
        }
    }

    pub fn session(&self) -> &SessionHandle {
        &self.session
    }

    pub fn send(&mut self, request: Request) {
        self.send_follow(request, Follow::None);
    }

    pub fn send_follow(&mut self, request: Request, follow: Follow) {
        self.sent += 1;
        let kind = Kind::One(
            self.session
                .submit_execute(self.caller.clone(), request.clone()),
        );
        self.pending.push(Item {
            request,
            follow,
            kind,
        });
    }

    /// Several commands as one transaction (one undo step).
    pub fn send_batch(&mut self, requests: Vec<Request>) {
        let Some(first) = requests.first().cloned() else {
            return;
        };
        self.sent += requests.len() as u64;
        let kind = Kind::Batch(self.session.submit_batch(self.caller.clone(), requests));
        self.pending.push(Item {
            request: first,
            follow: Follow::None,
            kind,
        });
    }

    /// Replies that have arrived since the last call. Never blocks.
    pub fn poll(&mut self) -> Vec<Done> {
        let mut done = Vec::new();
        let mut keep = Vec::new();
        for item in self.pending.drain(..) {
            let reply: Option<CmdResult<Executed>> = match &item.kind {
                Kind::One(p) => match p.try_wait() {
                    Ok(None) => None,
                    Ok(Some(r)) => Some(r),
                    Err(e) => Some(Err(e)),
                },
                Kind::Batch(p) => match p.try_wait() {
                    Ok(None) => None,
                    Ok(Some(r)) => Some(r.and_then(|mut v| {
                        v.pop()
                            .ok_or_else(|| ph_commands::CmdError::Other("empty batch".into()))
                    })),
                    Err(e) => Some(Err(e)),
                },
            };
            match reply {
                None => keep.push(item),
                Some(result) => done.push(Done {
                    request: item.request,
                    result,
                    follow: item.follow,
                }),
            }
        }
        self.pending = keep;
        done
    }

    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }
}
