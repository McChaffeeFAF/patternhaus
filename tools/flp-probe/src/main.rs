//! `flp-probe dump <file.flp> [--preview N]`: prints the event stream of one file.
//! `flp-probe stats <file-or-dir>...`: aggregate statistics over many files (numbers only).
//! `flp-probe corpus <file-or-dir>...`: imports every file with `ph_formats::flp` and diffs the
//! result against an independent reading (numbers only; local use, never in CI).

mod corpus;

use flp_probe::{Stats, dump_line, parse, version};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str =
    "usage: flp-probe dump <file.flp> [--preview N] | flp-probe stats <file-or-dir>...
       | flp-probe corpus <file-or-dir>... (imports every file, prints aggregates)";

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        children.sort();
        for c in children {
            collect(&c, out);
        }
    } else if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("flp"))
    {
        out.push(path.to_path_buf());
    }
}

fn dump(file: &str, preview: usize) -> ExitCode {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match parse(&bytes) {
        Ok(flp) => {
            let h = flp.header;
            println!(
                "header length {} format {} channels {} ppq {}; version {}; {} events; {} trailing bytes",
                h.length,
                h.format,
                h.channels,
                h.ppq,
                version(&flp).unwrap_or_else(|| "?".into()),
                flp.events.len(),
                flp.trailing
            );
            for e in &flp.events {
                println!("{}", dump_line(e, preview));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {file}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn stats(paths: &[String]) -> ExitCode {
    let mut files = Vec::new();
    for p in paths {
        collect(Path::new(p), &mut files);
    }
    let mut stats = Stats::default();
    for f in &files {
        match std::fs::read(f).map(|b| parse(&b).map(|flp| stats.add(&flp)).is_ok()) {
            Ok(true) => {}
            _ => stats.add_failure(),
        }
    }
    print!("{}", stats.report());
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dump") if args.len() >= 2 => {
            let preview = args
                .iter()
                .position(|a| a == "--preview")
                .and_then(|i| args.get(i + 1))
                .and_then(|n| n.parse().ok())
                .unwrap_or(32);
            dump(&args[1], preview)
        }
        Some("stats") if args.len() >= 2 => stats(&args[1..]),
        Some("corpus") if args.len() >= 2 => {
            let mut files = Vec::new();
            for p in &args[1..] {
                collect(Path::new(p), &mut files);
            }
            print!("{}", corpus::run(&files));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}
