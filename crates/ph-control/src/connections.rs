//! The live connection registry: who is connected to the endpoint right now.
//!
//! Every MCP and JSON-RPC connection registers itself when it starts and unregisters when it
//! ends (the guard is dropped with the connection). The app's Agents panel reads
//! [`Connections::list`] (through `LocalServer::connections`) to show live clients first and
//! the remembered-but-gone ones greyed out.

use ph_commands::Scope;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Which protocol a connection speaks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Protocol {
    Mcp,
    JsonRpc,
}

/// One live connection, as of the moment it was listed.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionInfo {
    /// Unique per connection for the life of the endpoint.
    pub id: u64,
    /// The client's self-reported name (`unknown` / `jsonrpc` until it says).
    pub client: String,
    pub protocol: Protocol,
    /// The client's grant right now (never prompts: unknown clients show as `read`).
    pub scopes: Vec<Scope>,
    pub connected_at: SystemTime,
    pub last_activity: SystemTime,
    /// Tool calls the connection has made.
    pub commands: u64,
}

impl ConnectionInfo {
    pub fn connected_for(&self) -> Duration {
        self.connected_at.elapsed().unwrap_or_default()
    }
    pub fn idle_for(&self) -> Duration {
        self.last_activity.elapsed().unwrap_or_default()
    }
}

struct State {
    client: String,
    last_activity: SystemTime,
    commands: u64,
}

pub(crate) struct Entry {
    id: u64,
    protocol: Protocol,
    connected_at: SystemTime,
    state: Mutex<State>,
}

/// The registry. Cheap to clone the `Arc`; one per [`crate::Agent`].
#[derive(Default)]
pub struct Connections {
    next: AtomicU64,
    live: Mutex<Vec<Arc<Entry>>>,
}

impl Connections {
    /// Registers a connection; it stays listed until the returned guard is dropped.
    pub fn open(self: &Arc<Self>, protocol: Protocol, client: &str) -> ConnGuard {
        let now = SystemTime::now();
        let entry = Arc::new(Entry {
            id: self.next.fetch_add(1, Ordering::Relaxed) + 1,
            protocol,
            connected_at: now,
            state: Mutex::new(State {
                client: client.to_string(),
                last_activity: now,
                commands: 0,
            }),
        });
        self.live.lock().unwrap().push(entry.clone());
        ConnGuard {
            registry: self.clone(),
            entry,
        }
    }

    /// The live connections, oldest first. `scope_of` supplies each client's current scopes.
    pub fn list(&self, scope_of: impl Fn(&str) -> Vec<Scope>) -> Vec<ConnectionInfo> {
        self.live
            .lock()
            .unwrap()
            .iter()
            .map(|e| {
                let s = e.state.lock().unwrap();
                ConnectionInfo {
                    id: e.id,
                    client: s.client.clone(),
                    protocol: e.protocol,
                    scopes: scope_of(&s.client),
                    connected_at: e.connected_at,
                    last_activity: s.last_activity,
                    commands: s.commands,
                }
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.live.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A registered connection. Dropping it removes the connection from the registry.
pub struct ConnGuard {
    registry: Arc<Connections>,
    entry: Arc<Entry>,
}

impl ConnGuard {
    /// The client said who it is.
    pub fn set_client(&self, name: &str) {
        let mut s = self.entry.state.lock().unwrap();
        s.client = name.to_string();
        s.last_activity = SystemTime::now();
    }

    /// Any traffic at all counts as activity.
    pub fn touch(&self) {
        self.entry.state.lock().unwrap().last_activity = SystemTime::now();
    }

    /// A tool call was made.
    pub fn count_command(&self) {
        let mut s = self.entry.state.lock().unwrap();
        s.commands += 1;
        s.last_activity = SystemTime::now();
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.registry
            .live
            .lock()
            .unwrap()
            .retain(|e| e.id != self.entry.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_register_count_and_unregister() {
        let reg = Arc::new(Connections::default());
        assert!(reg.is_empty());
        let a = reg.open(Protocol::Mcp, "unknown");
        let b = reg.open(Protocol::JsonRpc, "jsonrpc");
        a.set_client("Claude Code");
        a.count_command();
        a.count_command();
        b.touch();
        let list = reg.list(|c| {
            if c == "Claude Code" {
                vec![Scope::Compose]
            } else {
                vec![Scope::Read]
            }
        });
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].client, "Claude Code");
        assert_eq!(list[0].commands, 2);
        assert_eq!(list[0].scopes, vec![Scope::Compose]);
        assert_eq!(list[0].protocol, Protocol::Mcp);
        assert_eq!(list[1].client, "jsonrpc");
        assert_eq!(list[1].commands, 0);
        assert_ne!(list[0].id, list[1].id);
        assert!(list[0].last_activity >= list[0].connected_at);
        drop(a);
        let list = reg.list(|_| vec![]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].client, "jsonrpc");
        drop(b);
        assert!(reg.is_empty());
    }
}
