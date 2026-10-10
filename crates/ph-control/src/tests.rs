use super::*;
use ph_commands::{Grant, Scope};
use serde_json::json;
use std::time::{Duration, Instant};

fn studio(dir: &std::path::Path) -> Studio {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let config = StudioConfig {
        project_dir: dir.to_path_buf(),
        assets_dir: assets,
        library_roots: vec![],
        exports_dir: None,
        engine: EngineConfig::default(),
        audio: AudioMode::Silent,
    };
    Studio::start(config, Project::empty("t")).unwrap()
}

fn wait_for(mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn edits_reach_the_engine_and_export_renders_audio() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    let pat = s.run(&agent, "pattern.add", json!({})).unwrap().result["pattern"].clone();
    let ch = s
        .run(&agent, "channel.add", json!({
            "name": "Kick",
            "instrument": { "kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60 }
        }))
        .unwrap()
        .result["channel"]
        .clone();
    let notes: Vec<_> = (0..4)
        .map(|i| json!({ "key": 60, "start": i * 960, "length": 240, "velocity": 1.0 }))
        .collect();
    s.run(
        &agent,
        "note.add",
        json!({ "pattern": pat, "channel": ch, "notes": notes }),
    )
    .unwrap();
    s.run(
        &agent,
        "playlist.add_clip",
        json!({ "track": 0, "pattern": pat, "start": 0 }),
    )
    .unwrap();

    // the bridge delivers the snapshot
    wait_for(|| s.project().clips.len() == 1);

    let out = s
        .run(
            &agent,
            "file.export_audio",
            json!({ "path": "kick.wav", "bit_depth": 16 }),
        )
        .unwrap();
    assert!(dir.path().join("kick.wav").exists(), "{out:?}");
    let peak = out.result["peak_dbfs"].as_f64().unwrap();
    assert!(
        peak > -12.0 && peak <= 0.0,
        "factory kick audible, not clipping: {peak} dBFS"
    );
}

#[test]
fn file_access_is_confined() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    let err = s.run(
        &agent,
        "file.export_audio",
        json!({ "path": "../escape.wav" }),
    );
    assert!(
        err.is_err(),
        "climbing out of the project folder is refused"
    );
    let abs = std::env::temp_dir().join("ph-outside.wav");
    assert!(
        s.run(
            &agent,
            "file.export_midi",
            json!({ "path": abs.to_string_lossy() })
        )
        .is_err()
    );
}

#[test]
fn transport_goes_through_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let h = Caller::human();
    s.run(&h, "pattern.add", json!({})).unwrap();
    let st = s
        .run(&h, "transport.set_mode", json!({ "mode": "pattern" }))
        .unwrap();
    assert_eq!(st.result["mode"], "pattern");
    let st = s.run(&h, "transport.play", json!({})).unwrap();
    assert_eq!(st.result["playing"], true);
    // the silent pump drives the audio thread, so the engine really plays and advances
    wait_for(|| s.engine.status().playing && s.engine.status().position > 0);
    let st = s.run(&h, "transport.toggle", json!({})).unwrap();
    assert_eq!(st.result["playing"], false);
}

#[test]
fn ui_requests_are_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    s.run(
        &Caller::human(),
        "ui.open_panel",
        json!({ "panel": "mixer" }),
    )
    .unwrap();
    assert!(matches!(
        s.ui_requests.recv_timeout(Duration::from_secs(2)).unwrap(),
        UiRequest::Open { .. }
    ));
}

// ---- the endpoint: live connections, and a waiting grant prompt --------------------------

use crate::{GrantPrompt, Grants, connect, serve_local_with};
use std::sync::Arc;
use std::sync::mpsc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// A grant prompt that waits until the test lets it answer (a human thinking).
struct Slow {
    asked: mpsc::Sender<String>,
    release: std::sync::Mutex<mpsc::Receiver<()>>,
}

impl GrantPrompt for Slow {
    fn decide(&self, client: &str) -> Option<Grant> {
        let _ = self.asked.send(client.to_string());
        let _ = self.release.lock().unwrap().recv();
        Some(Grant(vec![Scope::Compose]))
    }
}

struct Rpc<R, W> {
    r: BufReader<R>,
    w: W,
    next: u64,
}

impl<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin> Rpc<R, W> {
    async fn send(&mut self, method: &str, params: serde_json::Value) -> u64 {
        self.next += 1;
        let line = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        self.w
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        self.w.flush().await.unwrap();
        self.next
    }
    async fn recv(&mut self) -> serde_json::Value {
        let mut l = String::new();
        tokio::time::timeout(Duration::from_secs(10), self.r.read_line(&mut l))
            .await
            .expect("a reply in time")
            .unwrap();
        serde_json::from_str(&l).unwrap()
    }
}

async fn client(
    endpoint: &str,
    name: &str,
) -> Rpc<impl tokio::io::AsyncRead + Unpin, impl tokio::io::AsyncWrite + Unpin> {
    let (r, w) = connect(endpoint).await.unwrap();
    let mut c = Rpc {
        r: BufReader::new(r),
        w,
        next: 0,
    };
    c.send("rpc.hello", json!({ "client": name })).await;
    assert_eq!(c.recv().await["result"]["client"], name);
    c
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_prompt_does_not_stall_other_clients_and_connections_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let (asked_tx, asked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let grants = Grants::per_client(
        None,
        Some(Arc::new(Slow {
            asked: asked_tx,
            release: std::sync::Mutex::new(release_rx),
        })),
    );
    grants.set("known", Grant(vec![Scope::Read]));
    let endpoint = if cfg!(windows) {
        format!(r"\\.\pipe\patternhaus-test-ctl-{}", std::process::id())
    } else {
        dir.path().join("ph.sock").to_string_lossy().into_owned()
    };
    let server = serve_local_with(s.handle(), &endpoint, grants.clone()).unwrap();
    assert!(server.connections().is_empty());

    // a new client calls a tool: the prompt opens and waits for the human
    let mut slow = client(&endpoint, "newcomer").await;
    slow.send("get_state", json!({ "view": "session" })).await;
    let who = tokio::task::spawn_blocking(move || asked_rx.recv_timeout(Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("the prompt opened");
    assert_eq!(who, "newcomer");

    // while it waits, two more clients connect and are served at once
    let mut a = client(&endpoint, "known").await;
    let mut b = client(&endpoint, "known").await;
    for c in [&mut a, &mut b] {
        let id = c.send("get_state", json!({ "view": "session" })).await;
        let r = c.recv().await;
        assert_eq!(r["id"], id);
        assert!(r["result"]["isError"] == false, "{r}");
    }
    let live = server.connections();
    assert_eq!(live.len(), 3, "{live:?}");
    let by = |name: &str| live.iter().filter(|c| c.client == name).collect::<Vec<_>>();
    assert_eq!(by("newcomer")[0].commands, 1, "the waiting call is counted");
    assert_eq!(
        by("newcomer")[0].scopes,
        vec![Scope::Read],
        "not granted yet"
    );
    assert_eq!(by("known").len(), 2);
    assert!(by("known").iter().all(|c| c.commands == 1));
    assert!(live.iter().all(|c| c.last_activity >= c.connected_at));

    // the human answers; the waiting client gets its reply and its new scope shows
    release_tx.send(()).unwrap();
    let r = slow.recv().await;
    assert!(r["result"]["isError"] == false, "{r}");
    assert_eq!(
        server
            .connections()
            .iter()
            .find(|c| c.client == "newcomer")
            .unwrap()
            .scopes,
        vec![Scope::Compose]
    );

    // a closed connection leaves the list
    drop(a);
    drop(b);
    for _ in 0..100 {
        if server.connections().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let left = server.connections();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].client, "newcomer");
}
