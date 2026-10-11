//! `golden`, `perf` and `notices`: tasks that drive cargo itself.

use std::path::Path;
use std::process::Command;

fn run(cmd: &mut Command) -> Result<(), Vec<String>> {
    let status = cmd.status().map_err(|e| vec![format!("{cmd:?}: {e}")])?;
    if status.success() {
        Ok(())
    } else {
        Err(vec![format!("{cmd:?} failed ({status})")])
    }
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

pub fn golden(root: &Path, update: bool) -> Result<(), Vec<String>> {
    let mut cmd = cargo();
    cmd.current_dir(root).args([
        "test",
        "-p",
        "ph-control",
        "--test",
        "golden",
        "--release",
        "--",
        "--nocapture",
        "--test-threads=1",
    ]);
    if update {
        cmd.env("PH_GOLDEN_UPDATE", "1");
    }
    run(&mut cmd)?;
    if update {
        println!(
            "golden references rewritten in tests/projects and tests/golden; say why in the PR"
        );
    }
    Ok(())
}

/// Metrics the perf gate compares (lower is better). Worst-block figures are printed but
/// not gated: one preempted block on a desktop says nothing about the engine.
const PERF_GATED: &[&str] = &[
    "all_cores_dsp_load_mean_pct",
    "all_cores_dsp_load_median_pct",
    "one_thread_dsp_load_mean_pct",
    "one_thread_dsp_load_median_pct",
];
/// A gated metric fails above baseline × this (a regression of more than 10%).
const PERF_TOLERANCE: f64 = 1.10;

fn read_json(p: &Path) -> Result<serde_json::Value, Vec<String>> {
    let text = std::fs::read_to_string(p).map_err(|e| vec![format!("{}: {e}", p.display())])?;
    serde_json::from_str(&text).map_err(|e| vec![format!("{}: {e}", p.display())])
}

pub fn perf(root: &Path, report_only: bool, update: bool) -> Result<(), Vec<String>> {
    let out = root.join("target").join("perf-current.json");
    std::fs::create_dir_all(root.join("target")).map_err(|e| vec![e.to_string()])?;
    let mut cmd = cargo();
    cmd.current_dir(root)
        .args(["bench", "-p", "ph-engine", "--bench", "reference"])
        .env("PH_PERF_OUT", &out);
    run(&mut cmd)?;
    let current = read_json(&out)?;
    let baseline_path = root.join("perf").join("baseline-7800x3d.json");
    if update {
        std::fs::create_dir_all(root.join("perf")).map_err(|e| vec![e.to_string()])?;
        std::fs::copy(&out, &baseline_path).map_err(|e| vec![e.to_string()])?;
        println!("baseline written to {}", baseline_path.display());
        return Ok(());
    }
    let baseline = read_json(&baseline_path)?;
    let mut errors = Vec::new();
    println!(
        "{:<34} {:>10} {:>10} {:>8}",
        "metric", "baseline", "current", "change"
    );
    for (key, value) in current.as_object().into_iter().flatten() {
        let (Some(now), Some(base)) = (value.as_f64(), baseline[key].as_f64()) else {
            continue;
        };
        let change = if base > 0.0 {
            (now / base - 1.0) * 100.0
        } else {
            0.0
        };
        let gated = PERF_GATED.contains(&key.as_str());
        println!(
            "{key:<34} {base:>10.2} {now:>10.2} {change:>+7.1}%{}",
            if gated { "" } else { "  (not gated)" }
        );
        if gated && now > base * PERF_TOLERANCE {
            errors.push(format!(
                "{key}: {now:.2} vs baseline {base:.2} ({change:+.1}%, limit +10%)"
            ));
        }
    }
    if report_only {
        if !errors.is_empty() {
            println!("perf report only, not gating: {}", errors.join("; "));
        }
        return Ok(());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// `THIRD_PARTY_NOTICES.md` from cargo-about (`about.toml`, `about.hbs`), for the default
/// (MIT/Apache) build. The `asio` build's extra GPL notice is in `docs/building-asio.md`.
pub fn notices(root: &Path, check: bool) -> Result<(), Vec<String>> {
    let target = root.join("THIRD_PARTY_NOTICES.md");
    let fresh = root.join("target").join("THIRD_PARTY_NOTICES.generated.md");
    std::fs::create_dir_all(root.join("target")).map_err(|e| vec![e.to_string()])?;
    let mut cmd = cargo();
    cmd.current_dir(root)
        .args([
            "about",
            "generate",
            "--workspace",
            "--locked",
            "about.hbs",
            "-o",
        ])
        .arg(&fresh);
    run(&mut cmd)?;
    let generated = std::fs::read_to_string(&fresh)
        .map_err(|e| vec![e.to_string()])?
        .replace("\r\n", "\n");
    if check {
        let committed = std::fs::read_to_string(&target)
            .map_err(|e| vec![format!("{}: {e}", target.display())])?
            .replace("\r\n", "\n");
        if committed != generated {
            return Err(vec![
                "THIRD_PARTY_NOTICES.md is out of date: run `cargo xtask notices`".into(),
            ]);
        }
        return Ok(());
    }
    std::fs::write(&target, generated).map_err(|e| vec![e.to_string()])?;
    println!("wrote {}", target.display());
    Ok(())
}
