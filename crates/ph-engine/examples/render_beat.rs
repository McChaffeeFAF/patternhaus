//! Owner checkpoint 1: render the demo beat to `beat.wav`.
//!
//! ```text
//! cargo run -p ph-engine --release --example render_beat [-- out.wav]
//! ```
//!
//! Eight bars at 124 BPM (generated drums, Haus Synth bass and chords, EQ + compressor on
//! inserts, a reverb send), rendered offline at 48 kHz in HQ mode with the reverb tail, and
//! written as a 24-bit WAV (default: `target/beat.wav` in the workspace).

#[path = "common/beat.rs"]
mod beat;

use ph_engine::{PlayMode, RenderSettings, render_offline};
use ph_formats::encode::{WavFormat, write_wav};
use ph_node::Quality;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/beat.wav"));
    let project = beat::project();
    let settings = RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: 48_000,
        quality: Quality::High,
        include_tail: true,
        block_frames: 512,
    };
    let t = Instant::now();
    let audio = render_offline(&project, &settings, &beat::DemoDrums).expect("render failed");
    let took = t.elapsed();
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("create output folder");
    }
    write_wav(&out, &audio.channels, audio.sample_rate, WavFormat::Pcm24).expect("write WAV");
    let peak = audio
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    let secs = audio.frames() as f64 / audio.sample_rate as f64;
    let shown = out.canonicalize().unwrap_or(out);
    let shown = shown.display().to_string();
    // drop Windows' verbatim prefix from canonical paths
    println!("wrote {}", shown.trim_start_matches(r"\\?\"));
    println!(
        "duration {secs:.3} s ({} frames at {} Hz), peak {peak:.4} ({:.2} dBFS), rendered in {:.2} s ({:.0}x real time)",
        audio.frames(),
        audio.sample_rate,
        20.0 * peak.max(1e-9).log10(),
        took.as_secs_f64(),
        secs / took.as_secs_f64()
    );
}
