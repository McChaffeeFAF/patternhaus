//! Wires the local agent endpoint (`ph_control::serve_local_with`) to the UI's Agents panel.
//!
//! - New clients trigger `GrantPrompt::decide` on an endpoint thread; we queue a
//!   [`GrantRequest`] for the panel and wait (up to [`PROMPT_TIMEOUT`]) for the human's answer.
//!   No answer, or a denial, leaves the client at the read-only default (DESIGN §5).
//! - `clients()` lists every client with a remembered grant. A live connection registry is a
//!   T7 follow-up; until then `connected_for` counts from when this app first saw the client.

use ph_commands::{Grant, Scope};
use ph_control::{
    GrantPrompt, Grants, LocalServer, Studio, default_endpoint, default_grants_file,
    serve_local_with,
};
use ph_ui::{AgentDirectory, ClientInfo, GrantRequest};
use std::collections::BTreeMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// A grant prompt waiting for the human, with the channel its answer goes back on.
type PendingGrant = (GrantRequest, Sender<Option<Vec<Scope>>>);

pub struct AppAgents {
    grants: OnceLock<Arc<Grants>>,
    pending: Mutex<Vec<PendingGrant>>,
    first_seen: Mutex<BTreeMap<String, Instant>>,
    next_id: Mutex<u64>,
    /// Keeps the endpoint running for the app's lifetime.
    server: Mutex<Option<LocalServer>>,
}

impl AppAgents {
    /// Starts the local endpoint for `studio`. On failure the app runs without agents.
    pub fn start(studio: &Studio) -> Result<Arc<AppAgents>, String> {
        let me = Arc::new(AppAgents {
            grants: OnceLock::new(),
            pending: Mutex::new(Vec::new()),
            first_seen: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            server: Mutex::new(None),
        });
        let grants = Grants::per_client(
            default_grants_file(),
            Some(me.clone() as Arc<dyn GrantPrompt>),
        );
        let _ = me.grants.set(grants.clone());
        let server = serve_local_with(studio.handle(), &default_endpoint(), grants)?;
        *me.server.lock().unwrap() = Some(server);
        Ok(me)
    }

    fn seen(&self, client: &str) -> Instant {
        *self
            .first_seen
            .lock()
            .unwrap()
            .entry(client.to_string())
            .or_insert_with(Instant::now)
    }
}

impl GrantPrompt for AppAgents {
    fn decide(&self, client: &str) -> Option<Grant> {
        self.seen(client);
        let (tx, rx) = channel();
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n - 1
        };
        let req = GrantRequest {
            id,
            client: client.to_string(),
            requested: vec![Scope::Compose, Scope::Mix],
        };
        self.pending.lock().unwrap().push((req, tx));
        let answer = rx.recv_timeout(PROMPT_TIMEOUT).ok().flatten();
        self.pending.lock().unwrap().retain(|(r, _)| r.id != id);
        answer.map(Grant)
    }
}

impl AgentDirectory for AppAgents {
    fn clients(&self) -> Vec<ClientInfo> {
        let Some(grants) = self.grants.get() else {
            return Vec::new();
        };
        grants
            .clients()
            .into_iter()
            .map(|(name, grant)| {
                let since = self.seen(&name);
                ClientInfo {
                    name,
                    scopes: grant.0,
                    connected_for: since.elapsed().as_secs(),
                }
            })
            .collect()
    }

    fn pending_grants(&self) -> Vec<GrantRequest> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }

    fn resolve_grant(&self, id: u64, granted: Option<Vec<Scope>>) {
        if let Some((_, tx)) = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .find(|(r, _)| r.id == id)
        {
            let _ = tx.send(granted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_waits_for_the_ui_answer() {
        let a = Arc::new(AppAgents {
            grants: OnceLock::new(),
            pending: Mutex::new(Vec::new()),
            first_seen: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            server: Mutex::new(None),
        });
        let asker = a.clone();
        let t = std::thread::spawn(move || asker.decide("claude"));
        let req = loop {
            if let Some(r) = a.pending_grants().pop() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(req.client, "claude");
        a.resolve_grant(req.id, Some(vec![Scope::Compose]));
        assert_eq!(t.join().unwrap(), Some(Grant(vec![Scope::Compose])));
        assert!(a.pending_grants().is_empty());
    }
}
