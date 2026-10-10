use soundgen::{assets_dir, check, measure, recipes, render_all, render_i24};
use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("render") => match render_all(&assets_dir(), &assets_dir()) {
            Ok(()) => {
                println!("rendered {} sounds", recipes::all().len());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some("check") => match check(&assets_dir()) {
            Ok(n) => {
                println!("soundgen check ok ({n} generated files identical)");
                ExitCode::SUCCESS
            }
            Err(errors) => {
                for e in errors {
                    eprintln!("error: {e}");
                }
                eprintln!("run `cargo run -p soundgen -- render` and commit the result");
                ExitCode::FAILURE
            }
        },
        Some("report") => {
            println!(
                "{:<34} {:>7} {:>8} {:>8} {:>8}",
                "sound", "len s", "peak dB", "dc dB", "centroid"
            );
            for r in recipes::all() {
                let x: Vec<f64> = render_i24(&r)
                    .iter()
                    .map(|s| *s as f64 / 8_388_607.0)
                    .collect();
                println!(
                    "{:<34} {:>7.2} {:>8.2} {:>8.1} {:>8.0}",
                    r.path,
                    x.len() as f64 / 48_000.0,
                    measure::db(measure::peak(&x)),
                    measure::dc_db(&x),
                    measure::centroid_hz(&x)
                );
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: soundgen render | check | report");
            ExitCode::FAILURE
        }
    }
}
