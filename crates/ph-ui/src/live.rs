//! Live engine readouts for the UI: transport position, DSP load, meters.
//!
//! The engine's `status`/`stats`/`meters` take the engine's mutex, which is held while a new
//! render plan compiles. So the UI thread never calls them: a small poller thread does, at
//! about 120 Hz, and publishes a plain copy the UI reads under a tiny lock. Preview and
//! audition messages travel the other way through the same thread, so the UI never waits on
//! the engine either.

use egui::Context;
use ph_core::{Channel, ChannelId, InstrumentSpec, Project, Tick};
use ph_engine::{Engine, EngineStats, PlayMode, TransportStatus};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Incremented on every poll, so tests and the UI can tell it is alive.
    pub polls: u64,
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
            polls: 0,
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
    /// Play a throw-away channel for the browser: the engine plays `project` (the current
    /// project plus the audition channel) for `hold`, then goes back to the real snapshot.
    Audition {
        project: Arc<Project>,
        channel: ChannelId,
        key: u8,
        hold: Duration,
    },
    Reconfigure(ph_engine::EngineConfig),
    Stop,
}

/// Handle the UI keeps: read [`Live`], send [`EngineMsg`]s.
pub struct EngineLink {
    pub live: Arc<Mutex<Live>>,
    tx: Sender<EngineMsg>,
    latest: Arc<Mutex<Arc<Project>>>,
}

impl EngineLink {
    pub fn spawn(engine: Arc<Engine>, ctx: Context, project: Arc<Project>) -> EngineLink {
        let live = Arc::new(Mutex::new(Live::default()));
        let latest = Arc::new(Mutex::new(project));
        let (tx, rx) = channel();
        let (l2, latest2) = (live.clone(), latest.clone());
        std::thread::Builder::new()
            .name("ph-ui-engine-link".into())
            .spawn(move || run(engine, ctx, rx, l2, latest2))
            .expect("spawn engine link");
        EngineLink { live, tx, latest }
    }

    pub fn snapshot(&self) -> Live {
        self.live.lock().unwrap().clone()
    }

    pub fn set_latest(&self, project: Arc<Project>) {
        *self.latest.lock().unwrap() = project;
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

    pub fn audition(&self, project: Arc<Project>, channel: ChannelId, key: u8, hold: Duration) {
        let _ = self.tx.send(EngineMsg::Audition {
            project,
            channel,
            key,
            hold,
        });
    }
}

impl Drop for EngineLink {
    fn drop(&mut self) {
        let _ = self.tx.send(EngineMsg::Stop);
    }
}

/// The project with a temporary channel playing `instrument` on the master, for auditioning a
/// browser entry. The channel takes the next free id and is never committed to the session.
pub fn audition_project(
    base: &Project,
    instrument: InstrumentSpec,
) -> (Arc<Project>, ChannelId, u8) {
    let mut p = base.clone();
    let id = ChannelId(p.next_ids.channel);
    let key = match &instrument {
        InstrumentSpec::Sampler { root_key, .. } => *root_key,
        _ => 60,
    };
    let mut channels: BTreeMap<_, _> = (*p.channels).clone();
    channels.insert(
        id,
        Channel {
            id,
            name: "audition".into(),
            color: 0x888888,
            instrument,
            volume: 0.9,
            pan: 0.0,
            muted: false,
            insert: ph_core::InsertId(0),
        },
    );
    p.channels = Arc::new(channels);
    let mut order = (*p.channel_order).clone();
    order.push(id);
    p.channel_order = Arc::new(order);
    (Arc::new(p), id, key)
}

fn run(
    engine: Arc<Engine>,
    ctx: Context,
    rx: Receiver<EngineMsg>,
    live: Arc<Mutex<Live>>,
    latest: Arc<Mutex<Arc<Project>>>,
) {
    let generation = Arc::new(AtomicU64::new(0));
    let mut last = Instant::now();
    let mut peak: Vec<[f32; 2]> = Vec::new();
    let mut idle_polls = 0u32;
    loop {
        match rx.recv_timeout(Duration::from_millis(8)) {
            Ok(EngineMsg::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Ok(EngineMsg::Preview {
                channel,
                key,
                velocity,
                on,
            }) => engine.preview_note(channel, key, velocity, on),
            Ok(EngineMsg::Reconfigure(cfg)) => {
                let _ = engine.reconfigure(cfg);
            }
            Ok(EngineMsg::Audition {
                project,
                channel,
                key,
                hold,
            }) => {
                let my = generation.fetch_add(1, Ordering::SeqCst) + 1;
                let (engine, generation, latest) =
                    (engine.clone(), generation.clone(), latest.clone());
                std::thread::spawn(move || {
                    let _ = engine.set_project(project);
                    std::thread::sleep(Duration::from_millis(200));
                    if generation.load(Ordering::SeqCst) != my {
                        return;
                    }
                    engine.preview_note(channel, key, 0.9, true);
                    std::thread::sleep(hold);
                    engine.preview_note(channel, key, 0.9, false);
                    std::thread::sleep(Duration::from_millis(900));
                    if generation.load(Ordering::SeqCst) == my {
                        let back = latest.lock().unwrap().clone();
                        let _ = engine.set_project(back);
                    }
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let status = engine.status();
        let stats = engine.stats();
        let meters = engine.meters();
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
        let playing = status.playing;
        {
            let mut l = live.lock().unwrap();
            l.status = status;
            l.stats = stats;
            l.peak.clone_from(&peak);
            l.rms = meters.rms;
            l.polls += 1;
        }
        if playing || active {
            idle_polls = 0;
            ctx.request_repaint();
        } else {
            idle_polls += 1;
            if idle_polls % 30 == 0 {
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
