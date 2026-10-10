//! Plays the demo beat on the real audio device (manual check; tests never open a device).
//!
//! ```text
//! cargo run -p ph-engine --release --example play_beat
//! ```
//!
//! Loops the eight bars for two passes, printing DSP load and overloads once a second.

#[path = "common/beat.rs"]
mod beat;

use ph_engine::{Engine, EngineConfig, PlayMode, TransportCommand};
use std::sync::Arc;
use std::time::Duration;

fn main() {
    let engine = Engine::start(EngineConfig::default(), Arc::new(beat::DemoDrums))
        .expect("could not open the audio device");
    let project = Arc::new(beat::project());
    let bpm = project.settings.bpm;
    let song_secs = ph_core::ticks_to_seconds(project.song_length(), bpm);
    engine.set_project(project).expect("plan");
    engine.transport(TransportCommand::SetMode(PlayMode::Song));
    engine.transport(TransportCommand::Play);
    let seconds = (song_secs * 2.0).ceil() as u64;
    for _ in 0..seconds {
        std::thread::sleep(Duration::from_secs(1));
        let s = engine.stats();
        let st = engine.status();
        println!(
            "tick {:>6}  load {:5.1}%  overloads {}  buffer {} @ {} Hz{}",
            st.position,
            s.dsp_load * 100.0,
            s.overloads,
            s.buffer_frames,
            s.sample_rate,
            s.suggest_buffer_frames
                .map(|f| format!("  (suggest {f} frames)"))
                .unwrap_or_default()
        );
    }
    engine.transport(TransportCommand::Stop);
    std::thread::sleep(Duration::from_millis(200));
}
