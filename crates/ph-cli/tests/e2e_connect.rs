//! The live route: an app (here, an in-process studio) serves the local endpoint;
//! `patternhaus-cli mcp --connect` is a stdio proxy to it. Checks identity and grants,
//! live events for edits by someone else, and the JSON-RPC route on the same endpoint.

#![allow(deprecated)]

use ph_commands::{Caller, Grant, Scope};
use ph_control::{Grants, HeadlessOptions, serve_local_with, start_headless};
use rmcp::model::*;
use rmcp::service::{NotificationContext, RoleClient};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct Watcher {
    updated: Arc<Mutex<Vec<String>>>,
    logs: Arc<Mutex<Vec<Value>>>,
}

impl ClientHandler for Watcher {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("proxy-agent", "1.0"),
        )
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.updated.lock().unwrap().push(params.uri);
    }

    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.logs.lock().unwrap().push(params.data);
    }
}

async fn tool(
    c: &rmcp::service::RunningService<RoleClient, Watcher>,
    name: &str,
    args: Value,
) -> (bool, Vec<String>) {
    let r = c
        .call_tool(
            CallToolRequestParams::new(name.to_string())
                .with_arguments(args.as_object().cloned().unwrap_or_default()),
        )
        .await
        .unwrap();
    let v = serde_json::to_value(&r).unwrap();
    let texts = v["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str().map(String::from))
        .collect();
    (r.is_error.unwrap_or(false), texts)
}

async fn wait_for<F: FnMut() -> bool>(mut f: F) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_attaches_to_the_running_app_with_grants_and_live_events() {
    let dir = tempfile::tempdir().unwrap();
    let (studio, _own_agent) = start_headless(HeadlessOptions {
        project_dir: dir.path().join("app"),
        assets_dir: None,
        audio: ph_control::AudioMode::Silent,
        projects_dir: None,
        grant: Grant::full(),
        plugins: Default::default(),
    })
    .unwrap();

    // The app: per-client grants remembered in a settings file; unknown clients start at read.
    let grants_file = dir.path().join("agents.json");
    let grants = Grants::per_client(Some(grants_file.clone()), None);
    let endpoint = if cfg!(windows) {
        format!(r"\\.\pipe\patternhaus-test-{}", std::process::id())
    } else {
        dir.path().join("ph.sock").to_string_lossy().into_owned()
    };
    let server = serve_local_with(studio.handle(), &endpoint, grants.clone()).unwrap();

    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_patternhaus-cli"));
    cmd.args(["mcp", "--connect", "--endpoint", &endpoint]);
    let watcher = Watcher::default();
    let c = watcher
        .clone()
        .serve(TokioChildProcess::new(cmd).unwrap())
        .await
        .unwrap();

    // identity and the read default
    let (err, t) = tool(&c, "get_state", json!({ "view": "session" })).await;
    assert!(!err);
    assert!(
        t[0].contains("\"you\":\"agent:proxy-agent\"") && t[0].contains("\"grant\":\"read\""),
        "{t:?}"
    );
    let (err, t) = tool(&c, "add_pattern", json!({ "name": "Nope" })).await;
    assert!(err && t[0].contains("Compose scope"), "{t:?}");

    // the human raises the grant (the app's prompt would do this); no reconnect needed
    grants.set("proxy-agent", Grant(vec![Scope::Compose]));
    let (err, t) = tool(&c, "add_pattern", json!({ "name": "Mine" })).await;
    assert!(!err, "{t:?}");
    assert!(
        std::fs::read_to_string(&grants_file)
            .unwrap()
            .contains("proxy-agent")
    );

    // live events: subscribe, then someone else (the human at the keyboard) edits
    c.subscribe(SubscribeRequestParams::new("patternhaus://events"))
        .await
        .unwrap();
    c.set_level(SetLevelRequestParams::new(LoggingLevel::Info))
        .await
        .unwrap();
    studio
        .run(
            &Caller::human(),
            "project.set_tempo",
            json!({ "bpm": 90.0 }),
        )
        .unwrap();
    wait_for(|| !watcher.updated.lock().unwrap().is_empty()).await;
    assert_eq!(watcher.updated.lock().unwrap()[0], "patternhaus://events");
    wait_for(|| !watcher.logs.lock().unwrap().is_empty()).await;
    let ev = watcher.logs.lock().unwrap()[0].clone();
    assert_eq!(ev["kind"], "edit");
    assert_eq!(ev["author"], "human");
    assert!(
        ev["data"]["changes"][0].as_str().unwrap().contains("tempo"),
        "{ev}"
    );

    // the resource carries the same events, and the next tool result mentions them
    let res = c
        .read_resource(ReadResourceRequestParams::new("patternhaus://events"))
        .await
        .unwrap();
    let text = serde_json::to_string(&res).unwrap();
    assert!(text.contains("human edited"), "{text}");
    studio
        .run(
            &Caller::human(),
            "project.set_tempo",
            json!({ "bpm": 100.0 }),
        )
        .unwrap();
    wait_for(|| studio.handle().events().unwrap().last_seq() >= 2).await;
    let (_, t) = tool(&c, "get_state", json!({})).await;
    assert!(
        t.iter()
            .any(|x| x.contains("while_you_worked") && x.contains("tempo")),
        "{t:?}"
    );

    // The agent's own edits are not echoed back to it.
    let before = watcher.updated.lock().unwrap().len();
    tool(&c, "add_pattern", json!({ "name": "Own" })).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(watcher.updated.lock().unwrap().len(), before);

    c.cancel().await.ok();

    // -- JSON-RPC on the same endpoint ---------------------------------------------------
    let (r, mut w) = ph_control::connect(&endpoint).await.unwrap();
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut r = BufReader::new(r);
    for req in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "rpc.hello", "params": { "client": "script" } }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "get_state", "params": { "view": "session" } }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "run", "params": { "id": "pattern.add" } }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "nope" }),
        json!({ "jsonrpc": "2.0", "method": "rpc.tools" }),
    ] {
        w.write_all(
            format!(
                "{req}
"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    }
    w.flush().await.unwrap();
    let mut got = Vec::new();
    for _ in 0..4 {
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        got.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    assert_eq!(got[0]["result"]["client"], "script");
    assert_eq!(got[1]["result"]["value"]["you"], "agent:script");
    assert_eq!(got[1]["result"]["value"]["grant"], "read");
    assert_eq!(got[2]["result"]["isError"], true); // read grant may not add patterns
    assert_eq!(got[3]["result"]["isError"], true); // unknown tool is a readable error
    drop(server);
}
