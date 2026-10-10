//! `cargo xtask <check-layers|check-provenance|check|golden|perf|notices>`
//!
//! - `check`: layering and provenance (CI, every OS).
//! - `golden [--update]`: the golden render tests (`crates/ph-control/tests/golden.rs`);
//!   `--update` rewrites `tests/projects/` and `tests/golden/` (say why in the PR).
//! - `perf [--report|--update]`: runs the reference benchmark and compares it with
//!   `perf/baseline-7800x3d.json`; fails on a regression of more than 10% (the local gate on
//!   the owner's machine). `--report` only prints (CI runners are noisy); `--update`
//!   records the current numbers as the baseline.
//! - `notices [--check]`: regenerates `THIRD_PARTY_NOTICES.md` with cargo-about;
//!   `--check` fails if the committed file is out of date.
//!
//! check-layers enforces DESIGN §2: a crate may depend only on the `ph-*` crates its layer
//! allows. Adding a crate means adding it to `LAYERS` (and to the design doc's diagram).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tasks::{golden, notices, perf};

mod tasks;

/// crate -> `ph-*` crates it may depend on (directly). Anything else is a layering error.
const LAYERS: &[(&str, &[&str])] = &[
    // pure data and interfaces
    ("ph-core", &[]),
    ("ph-node", &[]),
    ("ph-dsp", &[]),
    // DSP building blocks
    ("ph-instruments", &["ph-core", "ph-node", "ph-dsp"]),
    ("ph-effects", &["ph-core", "ph-node", "ph-dsp"]),
    ("ph-sampler", &["ph-core", "ph-node", "ph-dsp", "ph-script"]),
    ("ph-sampler-formats", &["ph-core", "ph-sampler"]),
    ("ph-script", &[]),
    ("ph-formats", &["ph-core", "ph-node"]),
    ("ph-audio-io", &[]),
    ("ph-plugin-host", &["ph-core", "ph-node"]),
    ("ph-plugin-server", &["ph-node", "ph-plugin-host"]),
    // engine
    (
        "ph-engine",
        &[
            "ph-core",
            "ph-node",
            "ph-dsp",
            "ph-instruments",
            "ph-effects",
            "ph-sampler",
            "ph-audio-io",
            "ph-plugin-host",
            "ph-formats",
        ],
    ),
    // commands and perception
    ("ph-commands", &["ph-core", "ph-formats"]),
    ("ph-analysis", &["ph-core", "ph-node", "ph-dsp"]),
    // front ends
    (
        "ph-control",
        &[
            "ph-core",
            "ph-commands",
            "ph-analysis",
            "ph-engine",
            "ph-formats",
            "ph-node",
        ],
    ),
    (
        "ph-ui",
        &[
            "ph-core",
            "ph-commands",
            "ph-analysis",
            "ph-engine",
            "ph-control",
            "ph-formats",
            "ph-node",
        ],
    ),
    (
        "ph-cli",
        &[
            "ph-core",
            "ph-commands",
            "ph-control",
            "ph-engine",
            "ph-formats",
            "ph-analysis",
            "ph-node",
        ],
    ),
    ("ph-app", &["*"]),
];

const USAGE: &str = "usage: cargo xtask <check-layers|check-provenance|check|golden [--update]|perf [--report|--update]|notices [--check]>";

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let task = args.first().cloned().unwrap_or_default();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let result = match task.as_str() {
        "check-layers" => check_layers(&root),
        "check-provenance" => check_provenance(&root),
        "check" => check_layers(&root).and_then(|_| check_provenance(&root)),
        "golden" => golden(&root, flag("--update")),
        "perf" => perf(&root, flag("--report"), flag("--update")),
        "notices" => notices(&root, flag("--check")),
        _ => Err(vec![USAGE.into()]),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for e in errors {
                eprintln!("error: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

fn check_layers(root: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .map_err(|e| vec![e.to_string()])?
        .filter_map(|e| e.ok().map(|e| e.path().join("Cargo.toml")))
        .filter(|p| p.exists())
        .collect();
    crates.sort();
    for manifest in crates {
        let text = std::fs::read_to_string(&manifest).map_err(|e| vec![e.to_string()])?;
        let doc: toml::Table = text
            .parse()
            .map_err(|e| vec![format!("{}: {e}", manifest.display())])?;
        let name = doc["package"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let Some((_, allowed)) = LAYERS.iter().find(|(n, _)| *n == name) else {
            errors.push(format!(
                "{name} is not in xtask LAYERS; place it in a layer (and in DESIGN §2)"
            ));
            continue;
        };
        // dev-dependencies may reach further (tests can wire things together)
        let Some(deps) = doc.get("dependencies").and_then(|d| d.as_table()) else {
            continue;
        };
        for dep in deps.keys().filter(|d| d.starts_with("ph-")) {
            if !allowed.contains(&"*") && !allowed.contains(&dep.as_str()) {
                errors.push(format!(
                    "{name} must not depend on {dep} (allowed: {allowed:?})"
                ));
            }
        }
    }
    if errors.is_empty() {
        println!("layering ok");
        Ok(())
    } else {
        Err(errors)
    }
}

/// Every file under `assets/` (except PROVENANCE.toml itself) needs an entry in
/// `assets/PROVENANCE.toml` (DESIGN §7).
fn check_provenance(root: &Path) -> Result<(), Vec<String>> {
    let assets = root.join("assets");
    if !assets.exists() {
        println!("provenance ok (no assets yet)");
        return Ok(());
    }
    let table: toml::Table = std::fs::read_to_string(assets.join("PROVENANCE.toml"))
        .map_err(|e| vec![format!("assets/PROVENANCE.toml: {e}")])?
        .parse()
        .map_err(|e| vec![format!("assets/PROVENANCE.toml: {e}")])?;
    let files = table
        .get("file")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let listed: Vec<String> = files
        .iter()
        .filter_map(|f| f.get("path").and_then(|p| p.as_str()).map(String::from))
        .collect();
    let mut errors = Vec::new();
    for f in &files {
        let ok = f.get("license").is_some()
            && (f.get("generator").is_some() || f.get("source").is_some());
        if !ok {
            errors.push(format!(
                "provenance entry {f:?} needs license and generator or source"
            ));
        }
    }
    let mut stack = vec![assets.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| vec![e.to_string()])?
            .flatten()
        {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p
                .strip_prefix(&assets)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel != "PROVENANCE.toml" && !listed.contains(&rel) {
                errors.push(format!("assets/{rel} has no provenance entry"));
            }
        }
    }
    if errors.is_empty() {
        println!("provenance ok ({} files)", listed.len());
        Ok(())
    } else {
        Err(errors)
    }
}
