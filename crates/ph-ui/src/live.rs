//! Live engine readouts for the UI: transport position, DSP load, meters.
//!
//! The engine's `status`/`stats`/`meters` take the engine's mutex, which is held while a new
//! render plan compiles. So the UI thread never calls them: a small poller thread does, at
//! about 120 Hz, and publishes a plain copy the UI reads under a tiny lock. Preview and
//! audition messages travel the other way through the same thread, so the UI never waits on
//! the engine either.

use egui::Context;
use ph_core::{ChannelId, InstrumentSpec, Tick};
use ph_engine::{Engine, EngineStats, PlayMode, TransportStatus};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A copy of what the engine reports, refreshed by the poller.
#[derive(Clone, Debug)]
pub struct Live {
    pub status: TransportStatus,
    pub stats: EngineStats,
    /// Per insert `[left, right]` with a short peak hold and decay, linear.
    pub peak: Vec<[f32; 2]>,
    pub rms: Vec<[f32; 2]>,
    /// The master's recent peak levels (newest last), for the top bar's scope.
    pub scope: Vec<f32>,
    /// Incremented on every poll, so tests and the UI can tell it is alive.
    pub polls: u64,
    /// The recorder's phase and input levels (M2/T8).
    pub record: ph_engine::record::RecordStatus,
    /// An audio recording is running.
    pub recording: bool,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            status: TransportStatus {
                playing: false,
                mode: PlayMode::Song,
                position: 0,
                loop_range: None,
                metronome: false,
            },
            stats: EngineStats::default(),
            peak: Vec::new(),
            rms: Vec::new(),
            scope: Vec::new(),
            polls: 0,
            record: Default::default(),
            recording: false,
        }
    }
}

pub enum EngineMsg {
    Preview {
        channel: ChannelId,
        key: u8,
        velocity: f32,
        on: bool,
    },
    /// Browser click-to-preview: plays a sample, or a preset's note, on the engine's
    /// audition voice (outside the project; `Engine::audition`).
    Audition(Audition),
    Reconfigure(ph_engine::EngineConfig),
    Stop,
}

/// Handle the UI keeps: read [`Live`], send [`EngineMsg`]s.
pub struct EngineLink {
    pub live: Arc<Mutex<Live>>,
    tx: Sender<EngineMsg>,
}

impl EngineLink {
    pub fn spawn(engine: Arc<Engine>, ctx: Context) -> EngineLink {
        let live = Arc::new(Mutex::new(Live::default()));
        let (tx, rx) = channel();
        let l2 = live.clone();
        std::thread::Builder::new()
            .name("ph-ui-engine-link".into())
            .spawn(move || run(engine, ctx, rx, l2))
            .expect("spawn engine link");
        EngineLink { live, tx }
    }

    pub fn snapshot(&self) -> Live {
        self.live.lock().unwrap().clone()
    }

    pub fn preview(&self, channel: ChannelId, key: u8, velocity: f32, on: bool) {
        let _ = self.tx.send(EngineMsg::Preview {
            channel,
            key,
            velocity,
            on,
        });
    }

    pub fn reconfigure(&self, cfg: ph_engine::EngineConfig) {
        let _ = self.tx.send(EngineMsg::Reconfigure(cfg));
    }

    pub fn audition(&self, what: Audition) {
        let _ = self.tx.send(EngineMsg::Audition(what));
    }
}

impl Drop for EngineLink {
    fn drop(&mut self) {
        let _ = self.tx.send(EngineMsg::Stop);
    }
}

/// What the browser previews.
#[derive(Clone, Debug, PartialEq)]
pub enum Audition {
    /// A sample reference (`factory:...`, project-relative or absolute).
    Sample(String),
    /// An instrument (a Haus Synth preset) playing one note for `hold`.
    Instrument {
        instrument: InstrumentSpec,
        key: u8,
        hold: Duration,
    },
    /// Decoded audio the caller holds, such as the audio editor's selection.
    Data(Arc<ph_node::SampleData>),
}

/// Samples in the scope trace.
pub const SCOPE_LEN: usize = 96;

/// Preview velocity for the browser.
const AUDITION_VELOCITY: f32 = 0.9;

fn play_audition(engine: &Arc<Engine>, what: Audition) {
    match what {
        Audition::Sample(reference) => {
            let _ = engine.audition(&reference, AUDITION_VELOCITY);
        }
        Audition::Data(d) => engine.audition_data(&d, AUDITION_VELOCITY),
        Audition::Instrument {
            instrument,
            key,
            hold,
        } => {
            // rendering the note takes a few milliseconds: keep the link thread responsive
            let engine = engine.clone();
            std::thread::spawn(move || {
                let _ = engine.audition_instrument(
                    &instrument,
                    key,
                    AUDITION_VELOCITY,
                    hold.as_secs_f64(),
                );
            });
        }
    }
}

fn run(engine: Arc<Engine>, ctx: Context, rx: Receiver<EngineMsg>, live: Arc<Mutex<Live>>) {
    let mut last = Instant::now();
    let mut peak: Vec<[f32; 2]> = Vec::new();
    let mut idle_polls = 0u32;
    let mut scope: std::collections::VecDeque<f32> = std::collections::VecDeque::new();
    let mut scope_at = Instant::now();
    let mut next_poll = Instant::now();
    loop {
        // `thread::sleep` is high resolution on Windows; channel timeouts are not (15 ms ticks).
        // Messages (typing-keyboard notes) are served every 2 ms, readouts at their own pace.
        std::thread::sleep(Duration::from_millis(2));
        loop {
            match rx.try_recv() {
                Ok(EngineMsg::Stop) | Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                Ok(EngineMsg::Preview {
                    channel,
                    key,
                    velocity,
                    on,
                }) => engine.preview_note(channel, key, velocity, on),
                Ok(EngineMsg::Reconfigure(cfg)) => {
                    let _ = engine.reconfigure(cfg);
                }
                Ok(EngineMsg::Audition(what)) => play_audition(&engine, what),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        let now = Instant::now();
        if now < next_poll {
            continue;
        }
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let status = engine.status();
        let stats = engine.stats();
        let meters = engine.meters();
        let record = engine.record_status();
        let recording = engine.is_recording();
        // hold the peak briefly and let it fall ~ 20 dB/s
        let fall = 10f32.powf(-20.0 * dt / 20.0);
        peak.resize(meters.peak.len(), [0.0; 2]);
        let mut active = false;
        for (h, new) in peak.iter_mut().zip(&meters.peak) {
            for c in 0..2 {
                h[c] = new[c].max(h[c] * fall);
                active |= h[c] > 1e-4;
            }
        }
        let playing = status.playing || recording;
        // a trace of the master level, 30 samples a second; it runs out when the sound stops
        if scope_at.elapsed() >= Duration::from_millis(33) {
            scope_at = Instant::now();
            if playing || active {
                if scope.len() >= SCOPE_LEN {
                    scope.pop_front();
                }
                scope.push_back(peak.first().map_or(0.0, |p| p[0].max(p[1])));
            } else if !scope.is_empty() {
                scope.pop_front();
            }
        }
        {
            let mut l = live.lock().unwrap();
            l.status = status;
            l.stats = stats;
            l.peak.clone_from(&peak);
            l.rms = meters.rms;
            l.scope = scope.iter().copied().collect();
            l.polls += 1;
            // keep the highest level seen since the UI last looked
            let held: Vec<_> = record
                .inputs
                .iter()
                .map(|(id, p)| {
                    let old = l
                        .record
                        .inputs
                        .iter()
                        .find(|(i, _)| i == id)
                        .map_or(0.0, |(_, p)| *p);
                    (*id, p.max(old * 0.9))
                })
                .collect();
            l.record = ph_engine::record::RecordStatus {
                inputs: held,
                ..record.clone()
            };
            l.recording = recording;
        }
        // poll fast (faster than any display) while something moves, slowly when idle
        next_poll = Instant::now() + Duration::from_millis(if playing || active { 3 } else { 40 });
        if playing || active {
            idle_polls = 0;
            ctx.request_repaint();
        } else {
            idle_polls += 1;
            if idle_polls.is_multiple_of(6) {
                ctx.request_repaint(); // keep DSP load / notices fresh when idle
            }
        }
    }
}

/// Position in a pattern loop: the playhead shown inside a pattern view.
pub fn pattern_position(
    status: &TransportStatus,
    pattern_len: Tick,
    pattern: ph_core::PatternId,
) -> Option<Tick> {
    match status.mode {
        PlayMode::Pattern(p) if p == pattern && pattern_len > 0 => {
            Some(status.position % pattern_len)
        }
        _ => None,
    }
}
