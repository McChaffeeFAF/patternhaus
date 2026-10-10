//! Who may do what: scope grants for connected clients (DESIGN §5).
//!
//! Each MCP/JSON-RPC client becomes `Caller::agent(<client name>)`. Its [`Grant`] comes from
//! a [`Grants`] table:
//! - **headless** sessions use a fixed grant from `--grant` (default `full`: the user started
//!   the server on purpose);
//! - **in-app** sessions look the client name up in a settings file, ask the app through a
//!   [`GrantPrompt`] hook for names it has never seen, and fall back to `read`.
//!
//! The table is consulted on every call, so the app can change a live connection's grant with
//! [`Grants::set`].

use ph_commands::{Grant, Scope};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Hook for the app (T5) to ask the human about a client it has not seen before. Called on
/// the connection's thread when the client first calls a tool; it may block on a dialog.
/// Return `None` to leave the client at the default grant (`read`).
pub trait GrantPrompt: Send + Sync {
    fn decide(&self, client: &str) -> Option<Grant>;
}

pub struct Grants {
    default: Grant,
    fixed: bool,
    file: Option<PathBuf>,
    prompt: Option<Arc<dyn GrantPrompt>>,
    table: Mutex<BTreeMap<String, Grant>>,
}

/// `compose,mix` to a grant. Accepts `read`, `compose`, `mix`, `full`.
pub fn parse_grant(s: &str) -> Result<Grant, String> {
    let mut v = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        v.push(match part {
            "read" => Scope::Read,
            "compose" => Scope::Compose,
            "mix" => Scope::Mix,
            "full" => Scope::Full,
            other => {
                return Err(format!(
                    "unknown scope {other:?}: use read, compose, mix, full"
                ));
            }
        });
    }
    if v.is_empty() {
        v.push(Scope::Read);
    }
    Ok(Grant(v))
}

pub fn grant_text(g: &Grant) -> String {
    g.0.iter()
        .map(|s| format!("{s:?}").to_lowercase())
        .collect::<Vec<_>>()
        .join(",")
}

impl Grants {
    /// Every client gets `grant` (headless sessions).
    pub fn fixed(grant: Grant) -> Arc<Grants> {
        Arc::new(Grants {
            default: grant,
            fixed: true,
            file: None,
            prompt: None,
            table: Mutex::default(),
        })
    }

    /// In-app: remembered per client name in `file`; unknown clients go through `prompt`,
    /// then get `read`.
    pub fn per_client(file: Option<PathBuf>, prompt: Option<Arc<dyn GrantPrompt>>) -> Arc<Grants> {
        let table = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                serde_json::from_value::<BTreeMap<String, Grant>>(v["clients"].clone()).ok()
            })
            .unwrap_or_default();
        Arc::new(Grants {
            default: Grant(vec![Scope::Read]),
            fixed: false,
            file,
            prompt,
            table: Mutex::new(table),
        })
    }

    pub fn is_fixed(&self) -> bool {
        self.fixed
    }

    /// The grant for `client` right now.
    pub fn grant_for(&self, client: &str) -> Grant {
        if self.fixed {
            return self.default.clone();
        }
        if let Some(g) = self.table.lock().unwrap().get(client) {
            return g.clone();
        }
        match self.prompt.as_ref().and_then(|p| p.decide(client)) {
            Some(g) => {
                self.set(client, g.clone());
                g
            }
            None => self.default.clone(),
        }
    }

    /// Every client with a remembered grant, by name. For the app's Agents panel.
    pub fn clients(&self) -> Vec<(String, Grant)> {
        self.table
            .lock()
            .unwrap()
            .iter()
            .map(|(n, g)| (n.clone(), g.clone()))
            .collect()
    }

    /// Sets and remembers a client's grant (persisted when there is a settings file).
    pub fn set(&self, client: &str, grant: Grant) {
        let mut t = self.table.lock().unwrap();
        t.insert(client.to_string(), grant);
        if let Some(f) = &self.file {
            let doc = serde_json::json!({ "clients": &*t });
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(f, serde_json::to_string_pretty(&doc).unwrap_or_default());
        }
    }

    /// Clients with a remembered grant.
    pub fn known(&self) -> BTreeMap<String, Grant> {
        self.table.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_remember() {
        assert_eq!(
            parse_grant("compose, mix").unwrap(),
            Grant(vec![Scope::Compose, Scope::Mix])
        );
        assert!(parse_grant("root").is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("agents.json");
        let g = Grants::per_client(Some(file.clone()), None);
        assert_eq!(g.grant_for("Claude Code"), Grant(vec![Scope::Read]));
        g.set("Claude Code", Grant(vec![Scope::Compose]));
        let again = Grants::per_client(Some(file), None);
        assert_eq!(again.grant_for("Claude Code"), Grant(vec![Scope::Compose]));
        assert_eq!(again.grant_for("Other"), Grant(vec![Scope::Read]));
        assert_eq!(Grants::fixed(Grant::full()).grant_for("x"), Grant::full());
    }

    struct Always(Grant);
    impl GrantPrompt for Always {
        fn decide(&self, _: &str) -> Option<Grant> {
            Some(self.0.clone())
        }
    }

    #[test]
    fn prompt_hook_decides_new_clients_once() {
        let g = Grants::per_client(None, Some(Arc::new(Always(Grant(vec![Scope::Mix])))));
        assert_eq!(g.grant_for("a"), Grant(vec![Scope::Mix]));
        assert_eq!(g.known().len(), 1);
    }
}
