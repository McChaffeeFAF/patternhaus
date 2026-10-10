//! The local endpoint (DESIGN §5: local only). A named pipe on Windows, a Unix socket
//! elsewhere. One endpoint speaks both routes: a connection whose first message is the MCP
//! `initialize` request is served as MCP; anything else is line-delimited JSON-RPC 2.0.
//!
//! The app calls [`serve_local`]`(&studio)`; `patternhaus-cli mcp --connect` is a stdio proxy
//! to the same endpoint ([`proxy_stdio`]).

use crate::agent::Agent;
use crate::grants::Grants;
use crate::jsonrpc::serve_jsonrpc;
use crate::mcp::PatternhausServer;
use crate::{Studio, StudioHandle};
use rmcp::ServiceExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader};
use tokio::sync::Notify;

/// Per-user endpoint name: `\\.\pipe\patternhaus-<user>` or `$XDG_RUNTIME_DIR/patternhaus.sock`.
pub fn default_endpoint() -> String {
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
        format!(r"\\.\pipe\patternhaus-{user}")
    }
    #[cfg(not(windows))]
    {
        if let Ok(d) = std::env::var("XDG_RUNTIME_DIR") {
            return format!("{d}/patternhaus.sock");
        }
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        format!("/tmp/patternhaus-{user}.sock")
    }
}

/// Where remembered per-client grants live: `<config dir>/patternhaus/agents.json`.
pub fn default_grants_file() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|b| b.join("patternhaus").join("agents.json"))
}

/// A running endpoint. Dropping it closes the endpoint.
pub struct LocalServer {
    endpoint: String,
    stop: Arc<Notify>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LocalServer {
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

/// Serves MCP over this process's stdin/stdout (headless `patternhaus-cli mcp`).
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
        impl AsyncRead + Unpin + Send,
        impl AsyncWrite + Unpin + Send,
    ),
    String,
> {
    let client = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .map_err(|e| format!("no running Patternhaus at {endpoint}: {e}"))?;
    Ok(tokio::io::split(client))
}

#[cfg(unix)]
pub async fn connect(
    endpoint: &str,
) -> Result<
    (
        impl AsyncRead + Unpin + Send,
        impl AsyncWrite + Unpin + Send,
    ),
    String,
> {
    let s = tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(|e| format!("no running Patternhaus at {endpoint}: {e}"))?;
    Ok(tokio::io::split(s))
}

/// `patternhaus-cli mcp --connect`: copies stdin to the endpoint and the endpoint to stdout,
/// so one stdio MCP registration attaches to the running app.
pub async fn proxy_stdio(endpoint: &str) -> Result<(), String> {
    let (mut r, mut w) = connect(endpoint).await?;
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let up = tokio::io::copy(&mut stdin, &mut w);
    let down = tokio::io::copy(&mut r, &mut stdout);
    tokio::select! {
        r = up => r.map(|_| ()).map_err(|e| e.to_string()),
        r = down => r.map(|_| ()).map_err(|e| e.to_string()),
    }
}
