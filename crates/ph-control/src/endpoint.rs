//! The local endpoint (DESIGN §5: local only). A named pipe on Windows, a Unix socket
//! elsewhere. One endpoint speaks both routes: a connection whose first message is the MCP
//! `initialize` request is served as MCP; anything else is line-delimited JSON-RPC 2.0.
//!
//! The app calls [`serve_local`]`(&studio)`; `beast-studio-cli mcp --connect` is a stdio proxy
//! to the same endpoint ([`proxy_stdio`]).

use crate::agent::Agent;
use crate::connections::ConnectionInfo;
use crate::grants::Grants;
use crate::jsonrpc::serve_jsonrpc;
use crate::mcp::PatternhausServer;
use crate::userdirs;
use crate::{Studio, StudioHandle};
use rmcp::ServiceExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader};
use tokio::sync::Notify;

/// Per-user endpoint name: `\\.\pipe\beast-studio-<user>` or `$XDG_RUNTIME_DIR/beast-studio.sock`.
/// A running app serves this one; [`legacy_endpoint`] is what a build from before the rename
/// serves.
pub fn default_endpoint() -> String {
    endpoint_named(userdirs::SLUG)
}

/// The endpoint name before the rename (`patternhaus-<user>`, `patternhaus.sock`): clients
/// fall back to it when nothing listens on [`default_endpoint`].
pub fn legacy_endpoint() -> String {
    endpoint_named(userdirs::LEGACY_SLUG)
}

fn endpoint_named(slug: &str) -> String {
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
        format!(r"\\.\pipe\{slug}-{user}")
    }
    #[cfg(not(windows))]
    {
        if let Ok(d) = std::env::var("XDG_RUNTIME_DIR") {
            return format!("{d}/{slug}.sock");
        }
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        format!("/tmp/{slug}-{user}.sock")
    }
}

/// The endpoints a client tries, in order: the one named, or else the default one and then
/// the pre-rename one.
pub fn endpoint_candidates(explicit: Option<&str>) -> Vec<String> {
    match explicit {
        Some(e) => vec![e.to_string()],
        None => vec![default_endpoint(), legacy_endpoint()],
    }
}

/// Where remembered per-client grants live: `agents.json` in the config folder
/// (`%APPDATA%\Beast Studio`, `$XDG_CONFIG_HOME/beast-studio`; see [`userdirs`]).
pub fn default_grants_file() -> Option<PathBuf> {
    userdirs::grants_dir_in(&userdirs::process_env, userdirs::Era::Current)
        .map(|d| d.join("agents.json"))
}

/// A running endpoint. Dropping it closes the endpoint.
pub struct LocalServer {
    endpoint: String,
    agent: Arc<Agent>,
    stop: Arc<Notify>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LocalServer {
    /// The clients connected right now: name, scopes, when they connected, when they were
    /// last active and how many tool calls they have made.
    pub fn connections(&self) -> Vec<ConnectionInfo> {
        self.agent.connections()
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop.notify_waiters();
        self.stop.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The app's one call: serve MCP and JSON-RPC on the default endpoint, with per-client
/// grants remembered in the user's config (unknown clients start at `read`).
pub fn serve_local(studio: &Studio) -> Result<LocalServer, String> {
    serve_local_with(
        studio.handle(),
        &default_endpoint(),
        Grants::per_client(default_grants_file(), None),
    )
}

/// As [`serve_local`] with an explicit endpoint name and grant table (for a grant-prompt
/// hook, headless servers and tests).
pub fn serve_local_with(
    studio: StudioHandle,
    endpoint: &str,
    grants: Arc<Grants>,
) -> Result<LocalServer, String> {
    let agent = Agent::new(studio, grants);
    let for_server = agent.clone();
    // Start the event hub now so the first client does not pay for it.
    agent.studio().events()?;
    let stop = Arc::new(Notify::new());
    let (ep, st) = (endpoint.to_string(), stop.clone());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread = std::thread::Builder::new()
        .name("ph-endpoint".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            rt.block_on(accept_loop(ep, agent, st, ready_tx));
        })
        .map_err(|e| e.to_string())?;
    ready_rx
        .recv()
        .map_err(|_| "the endpoint thread died".to_string())??;
    Ok(LocalServer {
        endpoint: endpoint.to_string(),
        agent: for_server,
        stop,
        thread: Some(thread),
    })
}

async fn serve_connection<R, W>(agent: Arc<Agent>, read: R, write: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(read);
    let mut first = String::new();
    match reader.read_line(&mut first).await {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }
    let is_mcp = serde_json::from_str::<serde_json::Value>(first.trim())
        .map(|v| v["method"] == "initialize")
        .unwrap_or(false);
    if is_mcp {
        let Ok(server) = PatternhausServer::new(agent) else {
            return;
        };
        // Replay the line we already consumed, then the rest of the stream.
        let replay = std::io::Cursor::new(first.into_bytes()).chain(reader);
        if let Ok(running) = server.serve((replay, write)).await {
            let _ = running.waiting().await;
        }
    } else {
        serve_jsonrpc(agent, reader, write, Some(first)).await;
    }
}

#[cfg(windows)]
async fn accept_loop(
    endpoint: String,
    agent: Arc<Agent>,
    stop: Arc<Notify>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut first = true;
    let mut ready = Some(ready);
    loop {
        let server = match ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create(&endpoint)
        {
            Ok(s) => s,
            Err(e) => {
                if let Some(r) = ready.take() {
                    let _ = r.send(Err(format!("cannot open {endpoint}: {e}")));
                }
                return;
            }
        };
        first = false;
        if let Some(r) = ready.take() {
            let _ = r.send(Ok(()));
        }
        tokio::select! {
            _ = stop.notified() => return,
            c = server.connect() => {
                if c.is_err() { continue; }
                let agent = agent.clone();
                tokio::spawn(async move {
                    let (r, w) = tokio::io::split(server);
                    serve_connection(agent, r, w).await;
                });
            }
        }
    }
}

#[cfg(unix)]
async fn accept_loop(
    endpoint: String,
    agent: Arc<Agent>,
    stop: Arc<Notify>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    use tokio::net::UnixListener;
    let _ = std::fs::remove_file(&endpoint);
    let listener = match UnixListener::bind(&endpoint) {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(format!("cannot open {endpoint}: {e}")));
            return;
        }
    };
    // Owner only.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600));
    }
    let _ = ready.send(Ok(()));
    loop {
        tokio::select! {
            _ = stop.notified() => break,
            c = listener.accept() => {
                let Ok((stream, _)) = c else { continue };
                let agent = agent.clone();
                tokio::spawn(async move {
                    let (r, w) = tokio::io::split(stream);
                    serve_connection(agent, r, w).await;
                });
            }
        }
    }
    let _ = std::fs::remove_file(&endpoint);
}

/// Serves MCP over this process's stdin/stdout (headless `beast-studio-cli mcp`).
pub async fn serve_stdio(agent: Arc<Agent>) -> Result<(), String> {
    let server = PatternhausServer::new(agent)?;
    let running = server
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await
        .map_err(|e| e.to_string())?;
    running.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Connects to a running app's endpoint.
#[cfg(windows)]
pub async fn connect(
    endpoint: &str,
) -> Result<
    (
        impl AsyncRead + Unpin + Send + use<>,
        impl AsyncWrite + Unpin + Send + use<>,
    ),
    String,
> {
    let client = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .map_err(|e| format!("no running Beast Studio at {endpoint}: {e}"))?;
    Ok(tokio::io::split(client))
}

#[cfg(unix)]
pub async fn connect(
    endpoint: &str,
) -> Result<
    (
        impl AsyncRead + Unpin + Send + use<>,
        impl AsyncWrite + Unpin + Send + use<>,
    ),
    String,
> {
    let s = tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(|e| format!("no running Beast Studio at {endpoint}: {e}"))?;
    Ok(tokio::io::split(s))
}

/// A connection to the endpoint, with the stream types erased.
pub type Connection = (
    Box<dyn AsyncRead + Unpin + Send>,
    Box<dyn AsyncWrite + Unpin + Send>,
);

/// Connects to the first of `endpoints` that has a running app behind it. The error names
/// each endpoint tried.
pub async fn connect_any(endpoints: &[String]) -> Result<Connection, String> {
    let mut errors = Vec::new();
    for ep in endpoints {
        match connect(ep).await {
            Ok((r, w)) => return Ok((Box::new(r), Box::new(w))),
            Err(e) => errors.push(e),
        }
    }
    Err(if errors.is_empty() {
        "no endpoint to connect to".to_string()
    } else {
        errors.join("; ")
    })
}

/// `beast-studio-cli mcp --connect`: copies stdin to the endpoint and the endpoint to stdout,
/// so one stdio MCP registration attaches to the running app. Tries each endpoint in order
/// ([`endpoint_candidates`]).
pub async fn proxy_stdio(endpoints: &[String]) -> Result<(), String> {
    let (mut r, mut w) = connect_any(endpoints).await?;
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let up = tokio::io::copy(&mut stdin, &mut w);
    let down = tokio::io::copy(&mut r, &mut stdout);
    tokio::select! {
        r = up => r.map(|_| ()).map_err(|e| e.to_string()),
        r = down => r.map(|_| ()).map_err(|e| e.to_string()),
    }
}
