//! JSON-RPC 2.0 over a line-delimited stream, with methods mirroring the MCP tools.
//!
//! A request is `{"jsonrpc":"2.0","id":1,"method":"run","params":{"id":"note.add","params":{...}}}`.
//! `method` is any tool name (`run`, `run_batch`, `get_state`, `analyze`, `snapshot`, direct
//! tools like `add_notes`, ...) and `params` are that tool's arguments. The result is
//! `{ "content": [...], "isError": false, "value": <the first text block parsed as JSON> }`,
//! the same content blocks MCP returns (images as base64 PNG). Extra methods:
//! - `rpc.hello` `{ "client": "name" }`: sets the identity (default `jsonrpc`);
//! - `rpc.tools`: the tool list with schemas;
//! - `rpc.subscribe`: from now on, other authors' events arrive as `event` notifications.
//!
//! Arrays of requests are batches; requests without an `id` get no response.

use crate::agent::{Agent, Block, ToolOutput};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

struct Conn {
    guard: crate::connections::ConnGuard,
    agent: Arc<Agent>,
    client: String,
    notify: mpsc::UnboundedSender<String>,
    subscribed: bool,
}

impl Conn {
    async fn handle(&mut self, req: Value) -> Option<Value> {
        let id = req.get("id").cloned();
        let reply_id = id.clone().unwrap_or(Value::Null);
        if req["jsonrpc"] != "2.0" || !req["method"].is_string() {
            return Some(error(
                &reply_id,
                -32600,
                "invalid request: need jsonrpc \"2.0\" and a method",
            ));
        }
        let method = req["method"].as_str().unwrap_or_default().to_string();
        self.guard.touch();
        let params = req.get("params").cloned().unwrap_or(json!({}));
        let result = match method.as_str() {
            "rpc.hello" => {
                if let Some(c) = params["client"].as_str().filter(|c| !c.is_empty()) {
                    self.client = c.to_string();
                    self.guard.set_client(c);
                }
                json!({ "client": self.client })
            }
            "rpc.tools" => {
                let tools: Vec<Value> = self
                    .agent
                    .tools()
                    .into_iter()
                    .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
                    .collect();
                json!({ "tools": tools })
            }
            "rpc.subscribe" => {
                if !self.subscribed {
                    self.subscribed = true;
                    if let Ok(hub) = self.agent.studio().events() {
                        let mut rx = hub.subscribe();
                        let tx = self.notify.clone();
                        let me = ph_commands::Author::Agent(self.client.clone()).to_string();
                        tokio::spawn(async move {
                            while let Some(e) = rx.recv().await {
                                if e.author.as_deref() == Some(me.as_str()) {
                                    continue;
                                }
                                let n = json!({ "jsonrpc": "2.0", "method": "event", "params": e });
                                if tx.send(n.to_string()).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                }
                json!({ "subscribed": true })
            }
            tool => {
                self.guard.count_command();
                let agent = self.agent.clone();
                let (client, tool) = (self.client.clone(), tool.to_string());
                let out = tokio::task::spawn_blocking(move || agent.call(&client, &tool, params))
                    .await
                    .unwrap_or_else(|e| ToolOutput {
                        blocks: vec![Block::Text(format!("{{\"ok\":false,\"error\":\"{e}\"}}"))],
                        is_error: true,
                    });
                out.to_json()
            }
        };
        id.map(|id| json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }
}

/// Serves JSON-RPC until the stream closes. `first_line` is a line the caller already read
/// (the sniffed first message).
pub async fn serve_jsonrpc<R, W>(
    agent: Arc<Agent>,
    mut reader: BufReader<R>,
    mut writer: W,
    first_line: Option<String>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (ntx, mut nrx) = mpsc::unbounded_channel::<String>();
    let mut conn = Conn {
        guard: agent
            .registry()
            .open(crate::connections::Protocol::JsonRpc, "jsonrpc"),
        agent,
        client: "jsonrpc".into(),
        notify: ntx,
        subscribed: false,
    };
    let mut pending = first_line;
    loop {
        let line = match pending.take() {
            Some(l) => l,
            None => {
                let mut buf = String::new();
                tokio::select! {
                    n = reader.read_line(&mut buf) => match n {
                        Ok(0) | Err(_) => return,
                        Ok(_) => buf,
                    },
                    Some(note) = nrx.recv() => {
                        if writer.write_all(format!("{note}\n").as_bytes()).await.is_err() { return; }
                        let _ = writer.flush().await;
                        continue;
                    }
                }
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply: Option<Value> = match serde_json::from_str::<Value>(line) {
            Err(e) => Some(error(&Value::Null, -32700, format!("parse error: {e}"))),
            Ok(Value::Array(batch)) => {
                let mut out = Vec::new();
                for r in batch {
                    if let Some(x) = conn.handle(r).await {
                        out.push(x);
                    }
                }
                (!out.is_empty()).then_some(Value::Array(out))
            }
            Ok(req) => conn.handle(req).await,
        };
        if let Some(r) = reply {
            if writer.write_all(format!("{r}\n").as_bytes()).await.is_err() {
                return;
            }
            let _ = writer.flush().await;
        }
    }
}
