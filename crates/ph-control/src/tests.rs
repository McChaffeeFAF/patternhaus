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

#[test]
fn agents_read_the_ui_hint_through_get_state() {
    let dir = tempfile::tempdir().unwrap();
    let s = studio(dir.path());
    let agent = Agent::new(s.handle(), Grants::fixed(Grant(vec![Scope::Full])));
    let read = |a: &Arc<Agent>| {
        a.call("claude", "get_state", json!({ "view": "ui" }))
            .to_json()
    };
    let none = read(&agent);
    assert_eq!(none["value"]["hint"], serde_json::Value::Null);
    s.set_ui_hint(Some(UiHint {
        name: "Volume".into(),
        value: "-3.0 dB".into(),
        gestures: "drag; double-click resets".into(),
    }));
    let some = read(&agent);
    assert_eq!(some["value"]["hint"]["name"], "Volume");
    assert_eq!(some["value"]["hint"]["value"], "-3.0 dB");
    s.set_ui_hint(None);
    assert_eq!(read(&agent)["value"]["hint"], serde_json::Value::Null);
}

fn wav(freq: f32) -> Vec<u8> {
    use ph_formats::encode::{DITHER_SEED, WavFormat, encode_wav};
    let tone: Vec<f32> = (0..2000)
        .map(|i| (i as f32 * freq * 0.01).sin() * 0.5)
        .collect();
    encode_wav(&[tone], 44_100, WavFormat::Pcm16, DITHER_SEED).unwrap()
}

#[test]
fn sample_import_copies_renames_dedupes_and_is_one_undo_step() {
    let proj = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let s = studio(proj.path());
    let human = Caller::human();
    let a = outside.path().join("kick.wav");
    let b = outside.path().join("sub");
    std::fs::create_dir(&b).unwrap();
    let b = b.join("kick.wav"); // same name, different bytes
    std::fs::write(&a, wav(1.0)).unwrap();
    std::fs::write(&b, wav(2.0)).unwrap();
    let paths = |ps: &[&std::path::Path]| json!({ "paths": ps.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>() });
    let out = s
        .run(&human, "sample.import", paths(&[&a, &b, &a]))
        .unwrap();
    assert_eq!(
        out.result["references"],
        json!([
            "samples/kick.wav",
            "samples/kick (2).wav",
            "samples/kick.wav"
        ]),
        "clash gets (2); identical bytes reuse the file"
    );
    assert_eq!(out.result["channels"].as_array().unwrap().len(), 3);
    assert_eq!(
        std::fs::read_dir(proj.path().join("samples"))
            .unwrap()
            .count(),
        2
    );
    wait_for(|| s.project().channels.len() == 3);
    let names: Vec<_> = s
        .project()
        .channel_order
        .iter()
        .map(|c| s.project().channels[c].name.clone())
        .collect();
    assert_eq!(names, ["kick", "kick", "kick"]);
    // one undo step removes all three; the copied files stay
    s.run(&human, "history.undo", json!({})).unwrap();
    wait_for(|| s.project().channels.is_empty());
    assert!(proj.path().join("samples/kick.wav").exists());

    // replace one channel's sample
    let ch = s.run(&human, "sample.import", paths(&[&a])).unwrap().result["channels"][0].clone();
    let out = s
        .run(
            &human,
            "sample.import",
            json!({ "paths": [b.to_string_lossy()], "channel": ch }),
        )
        .unwrap();
    assert_eq!(out.result["references"], json!(["samples/kick (2).wav"]));
    wait_for(|| {
        matches!(
            s.project().channels.values().next().map(|c| &c.instrument),
            Some(ph_core::InstrumentSpec::Sampler { sample: Some(r), .. }) if r == "samples/kick (2).wav"
        )
    });
    // a channel needs exactly one path
    assert!(
        s.run(
            &human,
            "sample.import",
            json!({ "paths": [a.to_string_lossy(), b.to_string_lossy()], "channel": ch })
        )
        .is_err()
    );
}

#[test]
fn sample_import_skips_and_names_undecodable_files() {
    let proj = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let s = studio(proj.path());
    let good = outside.path().join("good.wav");
    let bad = outside.path().join("bad.wav");
    std::fs::write(&good, wav(1.0)).unwrap();
    std::fs::write(&bad, b"not audio at all").unwrap();
    let human = Caller::human();
    let out = s
        .run(
            &human,
            "sample.import",
            json!({ "paths": [bad.to_string_lossy(), good.to_string_lossy()] }),
        )
        .unwrap();
    assert_eq!(out.result["channels"].as_array().unwrap().len(), 1);
    assert_eq!(
        out.result["skipped"][0]["path"],
        bad.to_string_lossy().as_ref()
    );
    assert!(!proj.path().join("samples/bad.wav").exists(), "not copied");
    // all bad: an error that names the file
    let err = s
        .run(
            &human,
            "sample.import",
            json!({ "paths": [bad.to_string_lossy()] }),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("bad.wav"), "{err}");
}

#[test]
fn sample_import_by_an_agent_is_confined_to_the_allowed_roots() {
    let proj = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let s = studio(proj.path());
    let agent = Caller::agent("claude", Grant(vec![Scope::Full]));
    let far = outside.path().join("far.wav");
    std::fs::write(&far, wav(1.0)).unwrap();
    assert!(
        s.run(
            &agent,
            "sample.import",
            json!({ "paths": [far.to_string_lossy()] })
        )
        .is_err()
    );
    assert!(!proj.path().join("samples/far.wav").exists());
    // inside the project folder is fine
    std::fs::write(proj.path().join("near.wav"), wav(3.0)).unwrap();
    let out = s
        .run(&agent, "sample.import", json!({ "paths": ["near.wav"] }))
        .unwrap();
    assert_eq!(out.result["references"], json!(["samples/near.wav"]));
    // and a human may open/import MIDI from anywhere, an agent may not
    let mid = outside.path().join("x.mid");
    let bytes = {
        let human = Caller::human();
        let pat = s.run(&human, "pattern.add", json!({})).unwrap().result["pattern"].clone();
        let ch = s
            .run(
                &human,
                "channel.add",
                json!({"name":"k","instrument":{"kind":"synth","preset":null}}),
            )
            .unwrap()
            .result["channel"]
            .clone();
        s.run(&human, "note.add", json!({"pattern":pat,"channel":ch,"notes":[{"key":60,"start":0,"length":240,"velocity":1.0}]})).unwrap();
        wait_for(|| s.project().patterns.values().any(|p| !p.notes.is_empty()));
        s.run(
            &human,
            "file.export_midi",
            json!({"path":"m.mid","pattern":pat}),
        )
        .unwrap();
        std::fs::read(proj.path().join("m.mid")).unwrap()
    };
    std::fs::write(&mid, bytes).unwrap();
    assert!(
        s.run(
            &agent,
            "file.import_midi",
            json!({ "path": mid.to_string_lossy() })
        )
        .is_err()
    );
    let out = s
        .run(
            &Caller::human(),
            "file.import_midi",
            json!({ "path": mid.to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(out.result["notes"], 1);
}

/// Opening a project from another folder must move the session to that folder: saving
/// afterwards writes the opened project, never over the project the app started with, and the
/// opened project's own samples resolve from its folder.
#[test]
fn opening_another_project_folder_moves_saves_and_samples_there() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    // project B on disk, with a project-relative sample in its own samples/
    std::fs::create_dir_all(b.path().join("samples")).unwrap();
    let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    ph_formats::encode::write_wav(
        &b.path().join("samples/tone.wav"),
        &[tone.clone(), tone],
        48_000,
        ph_formats::encode::WavFormat::Pcm16,
    )
    .unwrap();
    let mut pb = Project::empty("B");
    let mut ed = ph_core::Edit::new(&pb);
    ed.add_channel(
        "Tone",
        ph_core::InstrumentSpec::Sampler {
            sample: Some("samples/tone.wav".into()),
            root_key: 60,
            params: Default::default(),
        },
    )
    .unwrap();
    pb = ed.finish().0;
    ph_formats::save_project(&pb, b.path()).unwrap();

    // the app starts in A, saves A
    let s = studio(a.path());
    let h = Caller::human();
    s.run(&h, "project.rename", json!({ "title": "A" }))
        .unwrap();
    s.run(&h, "file.save", json!({})).unwrap();
    let a_before = std::fs::read(a.path().join("project.json")).unwrap();

    // open B (absolute path, as a drop or File > Open gives it), edit, save
    let req = ph_commands::Request {
        confirm: true,
        ..ph_commands::Request::new("file.open", json!({ "path": b.path().to_string_lossy() }))
    };
    s.session.execute(&h, req).unwrap();
    s.run(&h, "project.set_tempo", json!({ "bpm": 99 }))
        .unwrap();
    s.run(&h, "file.save", json!({})).unwrap();

    assert_eq!(
        std::fs::read(a.path().join("project.json")).unwrap(),
        a_before,
        "saving the opened project must not overwrite the project the app started with"
    );
    let saved_b = ph_formats::load_project(b.path()).unwrap();
    assert_eq!(
        saved_b.settings.bpm, 99.0,
        "the save went to the opened project's folder"
    );

    // B's project-relative sample loads (no engine warning about it)
    wait_for(|| s.project().settings.bpm == 99.0);
    let w = ph_engine::project_warnings(&s.project(), &*s.handle().assets);
    assert!(
        w.is_empty(),
        "opened project's samples resolve from its own folder: {w:?}"
    );
}
