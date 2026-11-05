//! M3 criterion 3: the importer hook, end to end. A made-up text instrument format (`.phtxt`)
//! has an importer that lives only in this test. It is registered at runtime, with no change
//! to Patternhaus, and then, through MCP alone:
//! - `sampler.import` reads a `.phtxt` file, with its import report;
//! - the instrument plays in a part and renders;
//! - `sampler.save_native` writes it as a Haus instrument folder, `sampler.load` puts that
//!   back on the channel, the project is saved and reopened, and the render is identical;
//! - a studio **without** the test importer opens the saved project and renders it
//!   identically too (after the import nothing depends on the importer);
//! - a file carrying the format's protection marker is refused in plain words.
//!
//! The CLI entry point (`ph_cli::run_from` with `Options { importers }`) is checked too: that
//! is how a separately built binary runs the normal CLI with its own formats.

use ph_sampler_formats::build::phtxt::{EXAMPLE, PhTxt};
use ph_sampler_formats::{ImportError, ImportInput, Importers};
use rmcp::model::*;
use rmcp::service::{RoleClient, RunningService};
use rmcp::{ClientHandler, ServiceExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ------------------------------------------------------------------------- the test format
// (`ph_sampler_formats::build::phtxt`, shared with the ph-ui drop test)

fn importers() -> Importers {
    let mut set = Importers::builtin();
    set.register(PhTxt).expect("phtxt is a new format");
    set
}

/// `<dir>/lib/toy.phtxt` with two zones over two generated tones, and a locked file.
fn write_library(dir: &Path) -> PathBuf {
    let lib = dir.join("lib");
    std::fs::create_dir_all(lib.join("samples")).unwrap();
    for (name, hz) in [("low.wav", 220.0f32), ("high.wav", 440.0)] {
        let tone: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.4)
            .collect();
        ph_formats::encode::write_wav(
            &lib.join("samples").join(name),
            &[&tone],
            48_000,
            ph_formats::encode::WavFormat::Pcm16,
        )
        .unwrap();
    }
    std::fs::write(lib.join("toy.phtxt"), EXAMPLE).unwrap();
    std::fs::write(lib.join("locked.phtxt"), "LOCKED\nname Secret\n").unwrap();
    lib
}

// ------------------------------------------------------------------------- MCP plumbing

#[derive(Clone, Default)]
struct Client;

impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("hook-agent", "1.0"),
        )
    }
}

type Mcp = RunningService<RoleClient, Client>;

/// A headless studio on `project` with `importers`, served over an in-process MCP pipe.
async fn open(project: &Path, importers: Importers) -> (Mcp, ph_control::Studio) {
    let (studio, agent) = ph_control::start_headless(ph_control::HeadlessOptions {
        project_dir: project.to_path_buf(),
        assets_dir: None,
        audio: ph_control::AudioMode::Silent,
        projects_dir: None,
        grant: ph_commands::Grant::full(),
        plugins: Default::default(),
        importers,
    })
    .unwrap();
    let (server_end, client_end) = tokio::io::duplex(1 << 20);
    let server = ph_control::PatternhausServer::new(agent).unwrap();
    tokio::spawn(async move {
        if let Ok(running) = server.serve(tokio::io::split(server_end)).await {
            let _ = running.waiting().await;
        }
    });
    let client = Client
        .serve(tokio::io::split(client_end))
        .await
        .expect("MCP initialize");
    (client, studio)
}

async fn call(c: &Mcp, tool: &str, args: Value) -> (bool, Value, String) {
    let r = c
        .call_tool(
            CallToolRequestParams::new(tool.to_string())
                .with_arguments(args.as_object().cloned().unwrap_or_default()),
        )
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    let v = serde_json::to_value(&r).unwrap();
    let text = v["content"][0]["text"].as_str().unwrap_or("").to_string();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (r.is_error.unwrap_or(false), json, text)
}

async fn ok(c: &Mcp, tool: &str, args: Value) -> Value {
    let (err, json, text) = call(c, tool, args.clone()).await;
    assert!(!err, "{tool} {args}: {text}");
    json
}

/// `run` one command; its result.
async fn run(c: &Mcp, id: &str, params: Value) -> Value {
    let v = ok(
        c,
        "run",
        json!({ "id": id, "params": params, "confirm": true }),
    )
    .await;
    v["results"][0]["result"].clone()
}

/// Renders the song as 32-bit float and returns the samples.
async fn render(c: &Mcp, name: &str) -> Vec<Vec<f32>> {
    let r = ok(
        c,
        "render",
        json!({ "name": name, "format": "wav", "bit_depth": 32 }),
    )
    .await;
    let path = PathBuf::from(r["path"].as_str().expect("render path"));
    ph_formats::decode::decode_file(&path).unwrap().channels
}

fn peak(chs: &[Vec<f32>]) -> f32 {
    chs.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()))
}

// ------------------------------------------------------------------------- the proof

#[tokio::test(flavor = "multi_thread")]
async fn importer_hook_end_to_end_over_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song");
    write_library(&song);
    let (c, studio) = open(&song, importers()).await;
    assert!(studio.importers().extensions().contains(&"phtxt"));

    // import through the registered importer, with its report
    let imported = run(&c, "sampler.import", json!({ "path": "lib/toy.phtxt" })).await;
    let report = &imported["report"];
    assert_eq!(report["format"], "phtxt", "{imported}");
    assert_eq!(
        (report["groups"].as_u64(), report["zones"].as_u64()),
        (Some(1), Some(2))
    );
    assert_eq!(report["notes"][0]["what"], "fx", "{report}");
    assert_eq!(
        report["unknown"][0],
        json!({ "what": "shimmer", "count": 1 })
    );
    assert_eq!(
        imported["collected"], false,
        "plain files are linked in place"
    );
    let channel = imported["channel"].as_u64().unwrap();
    let zones = ok(&c, "get_state", json!({ "view": "zones" })).await;
    assert_eq!(
        zones["instruments"][0]["source"]["format"], "phtxt",
        "{zones}"
    );

    // play it: a two-bar part across both zones
    ok(&c, "set_tempo", json!({ "bpm": 120.0 })).await;
    let pat = ok(&c, "add_pattern", json!({ "name": "Part" })).await;
    let pattern = pat["results"][0]["result"]["pattern"].as_u64().unwrap();
    let notes: Vec<Value> = [48u8, 55, 60, 64, 67, 72, 76, 79]
        .iter()
        .enumerate()
        .map(|(i, k)| json!({ "key": k, "start": i as u64 * 960, "length": 720, "velocity": 0.8 }))
        .collect();
    ok(
        &c,
        "add_notes",
        json!({ "pattern": pattern, "channel": channel, "notes": notes }),
    )
    .await;
    ok(
        &c,
        "add_clip",
        json!({ "track": 0, "pattern": pattern, "start": 0 }),
    )
    .await;
    let first = render(&c, "imported").await;
    assert!(peak(&first) > 0.05, "the imported instrument plays");

    // save as native, put the native instrument on the channel, save and reopen
    let saved = run(
        &c,
        "sampler.save_native",
        json!({ "instrument": 0, "path": "native/Toy Keys" }),
    )
    .await;
    assert_eq!(saved["zones"], 2, "{saved}");
    assert!(song.join("native/Toy Keys/instrument.toml").is_file());
    let loaded = run(
        &c,
        "sampler.load",
        json!({ "path": "native/Toy Keys", "channel": channel }),
    )
    .await;
    assert_eq!(loaded["zones"], 2, "{loaded}");
    run(&c, "file.save", json!({})).await;
    run(&c, "file.open", json!({ "path": song.to_string_lossy() })).await;
    let reopened = render(&c, "reopened").await;
    assert_eq!(
        reopened, first,
        "save as native and reopen renders identically"
    );

    // the hook's refusal path, in plain words
    let (err, _, text) = call(
        &c,
        "run",
        json!({ "id": "sampler.import", "params": { "path": "lib/locked.phtxt" } }),
    )
    .await;
    assert!(err, "{text}");
    assert!(
        text.contains(
            "encrypted or licence-locked PhTxt instrument (it starts with the LOCKED marker)"
        ) && text.contains("does not open protected libraries"),
        "{text}"
    );
    drop(c);
    drop(studio);

    // a Patternhaus without the test importer: the project still opens and renders the same,
    // and .phtxt is not a format it knows
    let (c, studio) = open(&song, Importers::builtin()).await;
    assert!(!studio.importers().extensions().contains(&"phtxt"));
    let plain = render(&c, "without-importer").await;
    assert_eq!(plain, first, "the saved project needs no importer");
    let (err, _, text) = call(
        &c,
        "run",
        json!({ "id": "sampler.import", "params": { "path": "lib/toy.phtxt" } }),
    )
    .await;
    assert!(err && text.contains("not an instrument format"), "{text}");
}

/// The CLI entry point a separately built binary calls: `ph_cli::run_from` with its own
/// importers runs a script that imports a `.phtxt` file and saves the project.
#[test]
fn the_cli_entry_point_runs_with_registered_importers() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song");
    write_library(&song);
    let script = dir.path().join("import.json");
    std::fs::write(
        &script,
        json!([
            { "command": "sampler.import", "params": { "path": "lib/toy.phtxt" } },
            { "command": "file.save" }
        ])
        .to_string(),
    )
    .unwrap();
    let args = |s: &Path| {
        vec![
            "patternhaus-cli".to_string(),
            "script".into(),
            s.to_string_lossy().into_owned(),
            "--separate".into(),
            "--project".into(),
            song.to_string_lossy().into_owned(),
        ]
    };
    // the stock CLI (built-in importers only) refuses the format and saves nothing
    let stock = ph_cli::run_from(ph_cli::Options::default(), args(&script));
    assert_ne!(stock, std::process::ExitCode::SUCCESS);
    assert!(!song.join("project.json").exists());

    let code = ph_cli::run_from(
        ph_cli::Options {
            importers: importers(),
        },
        args(&script),
    );
    assert_eq!(code, std::process::ExitCode::SUCCESS);
    let project = ph_formats::load_project(&song).unwrap();
    let inst = project.instruments.values().next().expect("an instrument");
    assert_eq!(inst.name, "Toy Keys");
    assert_eq!(inst.source.as_ref().unwrap().format, "phtxt");
}

#[test]
fn registered_importers_keep_their_contract() {
    // the hook's own rules: no takeover of a built-in extension, panics contained
    let mut set = importers();
    assert!(set.register(PhTxt).is_err(), "a format registers once");
    assert_eq!(
        set.importer_for(Path::new("x.PHTXT")).unwrap().format(),
        "phtxt"
    );
    let r = ph_sampler_formats::import_with(
        &PhTxt,
        ImportInput::Bytes {
            name: "x.phtxt",
            bytes: b"zone sample=a.wav",
        },
    );
    assert!(matches!(r, Err(ImportError::Malformed { .. })), "{r:?}");
    let _ = Arc::new(PhTxt); // shared registration also works
    set = Importers::builtin();
    set.register_shared(Arc::new(PhTxt)).unwrap();
    assert!(set.formats().contains(&"phtxt"));
}
