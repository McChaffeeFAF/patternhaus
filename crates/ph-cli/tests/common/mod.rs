//! An MCP client that spawns `patternhaus-cli mcp --headless` over stdio, shared by the e2e
//! and acceptance tests.
#![allow(dead_code)] // each test binary uses a different subset

use base64::Engine as _;
use rmcp::model::*;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, ServiceExt};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct Agent;

impl ClientHandler for Agent {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("e2e-agent", "1.0"),
        )
    }
}

pub type Client = RunningService<RoleClient, Agent>;

pub async fn connect(project: &Path, extra: &[&str]) -> Client {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_patternhaus-cli"));
    cmd.args(["mcp", "--headless", "--project"])
        .arg(project)
        .args(extra);
    Agent
        .serve(TokioChildProcess::new(cmd).expect("spawn patternhaus-cli"))
        .await
        .expect("MCP initialize")
}

/// [`connect`] with extra environment for the server process (M2: a private plugin cache and
/// the test plugins' scan helper, so a test never touches the user's own plugin cache).
pub async fn connect_with_env(project: &Path, extra: &[&str], env: &[(&str, &Path)]) -> Client {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_patternhaus-cli"));
    cmd.args(["mcp", "--headless", "--project"])
        .arg(project)
        .args(extra);
    for (k, v) in env {
        cmd.env(k, v);
    }
    Agent
        .serve(TokioChildProcess::new(cmd).expect("spawn patternhaus-cli"))
        .await
        .expect("MCP initialize")
}

pub struct Reply {
    pub json: Value,
    pub is_error: bool,
    pub images: Vec<Vec<u8>>,
    pub texts: Vec<String>,
}

pub async fn call(c: &Client, tool: &str, args: Value) -> Reply {
    let obj = args.as_object().cloned().unwrap_or_default();
    let res = c
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(obj))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    let v = serde_json::to_value(&res).unwrap();
    let mut r = Reply {
        json: Value::Null,
        is_error: res.is_error.unwrap_or(false),
        images: vec![],
        texts: vec![],
    };
    for b in v["content"].as_array().unwrap() {
        match b["type"].as_str() {
            Some("text") => {
                let t = b["text"].as_str().unwrap().to_string();
                if r.json.is_null() {
                    r.json = serde_json::from_str(&t).unwrap_or(Value::Null);
                }
                r.texts.push(t);
            }
            Some("image") => {
                assert_eq!(b["mimeType"], "image/png");
                r.images.push(
                    base64::engine::general_purpose::STANDARD
                        .decode(b["data"].as_str().unwrap())
                        .unwrap(),
                );
            }
            _ => {}
        }
    }
    r
}

pub async fn ok(c: &Client, tool: &str, args: Value) -> Value {
    let r = call(c, tool, args.clone()).await;
    assert!(!r.is_error, "{tool} {args}: {}", r.texts.join("\n"));
    r.json
}

pub fn id(v: &Value, key: &str) -> u64 {
    v["results"][0]["result"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("no {key} in {v}"))
}

pub fn assert_png(bytes: &[u8], min_w: u32) -> (u32, u32) {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let r = dec.read_info().expect("a valid PNG");
    let (w, h) = (r.info().width, r.info().height);
    assert!(w >= min_w && h >= 200, "{w}x{h}");
    (w, h)
}

pub fn screenshot_dir() -> Option<PathBuf> {
    std::env::var_os("PH_SAVE_SCREENSHOTS")
        .map(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/M1"))
}
