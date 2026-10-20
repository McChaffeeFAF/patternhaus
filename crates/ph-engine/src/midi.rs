//! MIDI input (M2/T8): devices, notes to the selected channel, controllers, MIDI learn and
//! note recording.
//!
//! # Paths
//!
//! ```text
//!  device ──callback (midir thread)──> raw bytes ──mpsc──> router thread (ph-midi-router)
//!                                                              │
//!        notes ───────────────────────────────────────────────>│ Engine::preview_note   (sound at once)
//!        CC / bend / channel pressure (unmapped) ─────────────>│ Engine::preview_midi    (ph-node events)
//!        CC (mapped or learning) ─────────────────────────────>│ events queue ──> the UI / host applies it
//!        notes while recording ───────────────────────────────>│ note buffer ──> `record.stop` writes the pattern
//! ```
//!
//! - The device callback only forwards three bytes; everything else happens on the router
//!   thread, which is not real-time but answers a note in well under a millisecond: it calls
//!   `Engine::preview_note`, the same preview path as the typing keyboard, so the note sounds
//!   at the start of the next audio block. Channel pressure, pitch bend and controllers go to
//!   the same channel as `ph-node` events (`Cc`, `PitchBend`, `NoteExpression::Pressure`),
//!   which the Haus Synth's mod wheel and bend follow.
//! - A mapped controller is not sent to the instrument: it becomes a [`MidiEvent::Control`]
//!   (target and a 0..1 value) for the front end to apply as a knob move. That is also what
//!   lets armed automation recording write it: the move arrives like a drag.
//! - Devices are polled once a second (and on [`MidiService::refresh`]); new ports connect,
//!   vanished ports drop (hot-plug).
//! - Mappings are per user: a JSON file in the config folder (see [`MappingStore`]), not in the
//!   project.
//! - Hardware sits behind [`MidiBackend`]. `MidirBackend` is the real one; [`FakeMidi`] and
//!   [`NoMidi`] are for tests and headless sessions. Tests never open a real device.

use crate::Engine;
use ph_core::{ChannelId, ParamPath, Tick};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering::*};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

// ------------------------------------------------------------------------------ messages

/// A parsed channel message. MIDI channels are ignored (omni).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MidiMessage {
    NoteOn { key: u8, velocity: f32 },
    NoteOff { key: u8 },
    /// Controller `controller`, value 0..=1.
    Cc { controller: u8, value: f32 },
    /// -1..=1.
    PitchBend { value: f32 },
    /// Channel pressure (aftertouch), 0..=1.
    ChannelPressure { value: f32 },
}

/// Parses a short channel message. A note-on with velocity 0 is a note-off.
pub fn parse(bytes: &[u8]) -> Option<MidiMessage> {
    let status = *bytes.first()?;
    let d1 = bytes.get(1).copied().unwrap_or(0) & 0x7f;
    let d2 = bytes.get(2).copied().unwrap_or(0) & 0x7f;
    Some(match status & 0xf0 {
        0x80 => MidiMessage::NoteOff { key: d1 },
        0x90 if d2 == 0 => MidiMessage::NoteOff { key: d1 },
        0x90 => MidiMessage::NoteOn {
            key: d1,
            velocity: d2 as f32 / 127.0,
        },
        0xb0 => MidiMessage::Cc {
            controller: d1,
            value: d2 as f32 / 127.0,
        },
        0xd0 => MidiMessage::ChannelPressure {
            value: d1 as f32 / 127.0,
        },
        0xe0 => {
            let raw = ((d2 as i32) << 7 | d1 as i32) - 8192;
            MidiMessage::PitchBend {
                value: (raw as f32 / 8192.0).clamp(-1.0, 1.0),
            }
        }
        _ => return None,
    })
}

/// What a controller, bend or pressure message becomes in the engine.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MidiMod {
    Cc { controller: u8, value: f32 },
    Bend { value: f32 },
    Pressure { value: f32 },
}

impl Engine {
    /// Sends a controller, pitch bend or channel pressure to one channel's instrument (the
    /// MIDI keyboard's wheels), at the start of the next block. Held preview notes get the
    /// pressure as note expression.
    pub fn preview_midi(&self, channel: ChannelId, what: MidiMod) {
        use crate::runtime::ModKind;
        let what = match what {
            MidiMod::Cc { controller, value } => ModKind::Cc {
                controller: controller.min(127),
                value: value.clamp(0.0, 1.0),
            },
            MidiMod::Bend { value } => ModKind::Bend {
                value: value.clamp(-1.0, 1.0),
            },
            MidiMod::Pressure { value } => ModKind::Pressure {
                value: value.clamp(0.0, 1.0),
            },
        };
        self.send_command(crate::runtime::Command::PreviewMod { channel, what });
    }

    /// The playhead in ticks as the audio thread last published it. Lock-free (the other
    /// readouts take the engine's mutex, which a plan compile holds).
    pub fn song_position(&self) -> Tick {
        self.shared.position.load(Relaxed)
    }

    /// Whether the transport was playing at the last block (lock-free).
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Relaxed)
    }
}

// ------------------------------------------------------------------------------ backends

/// A MIDI input port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiPort {
    /// The name the system gives the port; also its id.
    pub name: String,
}

/// Delivers a short message from a device. Called on the device's thread.
pub type MidiSink = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Keeps a connection open; dropping it closes the port.
pub trait MidiConnection: std::marker::Send {}

/// Where MIDI comes from. Only the router thread calls it.
pub trait MidiBackend: std::marker::Send {
    /// The input ports present now.
    fn ports(&mut self) -> Vec<MidiPort>;
    /// Connects to a port; messages arrive at `sink` until the connection is dropped.
    fn connect(
        &mut self,
        port: &MidiPort,
        sink: MidiSink,
    ) -> Result<Box<dyn MidiConnection>, String>;
}

/// No MIDI hardware (headless sessions).
pub struct NoMidi;

impl MidiBackend for NoMidi {
    fn ports(&mut self) -> Vec<MidiPort> {
        Vec::new()
    }
    fn connect(&mut self, _: &MidiPort, _: MidiSink) -> Result<Box<dyn MidiConnection>, String> {
        Err("no MIDI backend".into())
    }
}

/// A scripted MIDI source for tests: ports come and go, messages are injected.
#[derive(Clone, Default)]
pub struct FakeMidi {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    ports: Vec<MidiPort>,
    sinks: BTreeMap<String, MidiSink>,
    connects: usize,
}

struct FakeConnection {
    name: String,
    inner: Arc<Mutex<FakeState>>,
}

impl MidiConnection for FakeConnection {}

impl Drop for FakeConnection {
    fn drop(&mut self) {
        self.inner.lock().unwrap().sinks.remove(&self.name);
    }
}

impl FakeMidi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_port(&self, name: &str) {
        let mut s = self.inner.lock().unwrap();
        if !s.ports.iter().any(|p| p.name == name) {
            s.ports.push(MidiPort { name: name.into() });
        }
    }

    pub fn remove_port(&self, name: &str) {
        self.inner.lock().unwrap().ports.retain(|p| p.name != name);
    }

    /// Injects a message as if the device at `port` sent it. False if nothing is connected.
    pub fn send(&self, port: &str, bytes: &[u8]) -> bool {
        let sink = self.inner.lock().unwrap().sinks.get(port).cloned();
        match sink {
            Some(s) => {
                s(bytes);
                true
            }
            None => false,
        }
    }

    /// Ports the service is connected to right now.
    pub fn connected(&self) -> Vec<String> {
        self.inner.lock().unwrap().sinks.keys().cloned().collect()
    }

    /// How many connections were ever made.
    pub fn connect_count(&self) -> usize {
        self.inner.lock().unwrap().connects
    }
}

impl MidiBackend for FakeMidi {
    fn ports(&mut self) -> Vec<MidiPort> {
        self.inner.lock().unwrap().ports.clone()
    }
    fn connect(
        &mut self,
        port: &MidiPort,
        sink: MidiSink,
    ) -> Result<Box<dyn MidiConnection>, String> {
        let mut s = self.inner.lock().unwrap();
        if !s.ports.contains(port) {
            return Err(format!("no port {}", port.name));
        }
        s.sinks.insert(port.name.clone(), sink);
        s.connects += 1;
        Ok(Box::new(FakeConnection {
            name: port.name.clone(),
            inner: self.inner.clone(),
        }))
    }
}

/// The system's MIDI inputs through `midir` (WinMM on Windows, CoreMIDI, ALSA).
pub struct MidirBackend;

struct MidirConnection {
    _conn: midir::MidiInputConnection<()>,
}

impl MidiConnection for MidirConnection {}

impl MidiBackend for MidirBackend {
    fn ports(&mut self) -> Vec<MidiPort> {
        let Ok(input) = midir::MidiInput::new("Patternhaus ports") else {
            return Vec::new();
        };
        input
            .ports()
            .iter()
            .filter_map(|p| input.port_name(p).ok())
            .map(|name| MidiPort { name })
            .collect()
    }

    fn connect(
        &mut self,
        port: &MidiPort,
        sink: MidiSink,
    ) -> Result<Box<dyn MidiConnection>, String> {
        let input = midir::MidiInput::new("Patternhaus").map_err(|e| e.to_string())?;
        let found = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).is_ok_and(|n| n == port.name))
            .ok_or_else(|| format!("MIDI input `{}` is gone", port.name))?;
        let conn = input
            .connect(
                &found,
                "patternhaus-in",
                move |_stamp, bytes, _| sink(bytes),
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(Box::new(MidirConnection { _conn: conn }))
    }
}

// ------------------------------------------------------------------------------ mappings

/// One controller mapped to one parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    /// CC number 0..=127.
    pub controller: u8,
    /// Only this device; `None` = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub target: ParamPath,
}

#[derive(Serialize, Deserialize)]
struct MappingFile {
    version: u32,
    mappings: Vec<Mapping>,
}

/// The user's mappings, kept in memory and (when it has a path) in a JSON file in the config
/// folder, rewritten after every change.
#[derive(Default)]
pub struct MappingStore {
    path: Option<PathBuf>,
    mappings: Vec<Mapping>,
}

impl MappingStore {
    /// In memory only (tests, headless sessions without a config folder).
    pub fn memory() -> Self {
        Self::default()
    }

    /// `<config>/midi-mappings.json`; a missing or unreadable file starts empty (a broken
    /// file is left alone until the first change replaces it).
    pub fn open(config_dir: &Path) -> Self {
        let path = config_dir.join("midi-mappings.json");
        let mappings = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<MappingFile>(&b).ok())
            .map(|f| f.mappings)
            .unwrap_or_default();
        Self {
            path: Some(path),
            mappings,
        }
    }

    pub fn list(&self) -> &[Mapping] {
        &self.mappings
    }

    /// The mapping for a message from `device`: the one for that device first, else the one
    /// for any device.
    pub fn find(&self, controller: u8, device: &str) -> Option<&Mapping> {
        self.mappings
            .iter()
            .find(|m| m.controller == controller && m.device.as_deref() == Some(device))
            .or_else(|| {
                self.mappings
                    .iter()
                    .find(|m| m.controller == controller && m.device.is_none())
            })
    }

    /// Adds or replaces the mapping of (controller, device). Returns the mapping it replaced.
    pub fn set(&mut self, m: Mapping) -> std::io::Result<Option<Mapping>> {
        let old = match self
            .mappings
            .iter()
            .position(|x| x.controller == m.controller && x.device == m.device)
        {
            Some(i) => Some(std::mem::replace(&mut self.mappings[i], m)),
            None => {
                self.mappings.push(m);
                None
            }
        };
        self.save()?;
        Ok(old)
    }

    /// Removes the mapping of (controller, device). Returns it.
    pub fn remove(
        &mut self,
        controller: u8,
        device: Option<&str>,
    ) -> std::io::Result<Option<Mapping>> {
        let Some(i) = self
            .mappings
            .iter()
            .position(|x| x.controller == controller && x.device.as_deref() == device)
        else {
            return Ok(None);
        };
        let old = self.mappings.remove(i);
        self.save()?;
        Ok(Some(old))
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = MappingFile {
            version: 1,
            mappings: self.mappings.clone(),
        };
        let json = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
        // write beside, then rename: a crash never leaves half a file
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

// ------------------------------------------------------------------------------ service

/// Something the front end has to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum MidiEvent {
    /// A mapped controller moved: set `target` to `value` (0..=1 of its range).
    Control {
        target: ParamPath,
        value: f64,
        controller: u8,
        device: String,
    },
    /// MIDI learn caught a controller and mapped it.
    Learned {
        controller: u8,
        device: String,
        target: ParamPath,
    },
    /// Ports appeared or vanished.
    Devices,
    /// A note went on or off (activity lights).
    Note { on: bool, key: u8 },
}

/// A note played while recording, in transport ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedNote {
    pub key: u8,
    pub velocity: f32,
    pub start: Tick,
    pub length: Tick,
}

/// An input port with its connection state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MidiInputInfo {
    pub name: String,
    pub connected: bool,
}

enum Msg {
    Raw(Arc<str>, [u8; 3], usize),
    Scan,
    /// Replace the backend: connections to the old one close.
    Backend(Box<dyn MidiBackend>),
    Stop,
}

#[derive(Default)]
struct State {
    inputs: Vec<MidiInputInfo>,
    store: MappingStore,
    learning: Option<ParamPath>,
    events: VecDeque<MidiEvent>,
    /// Note recording: `Some` while armed; the keys currently held and the finished notes.
    recording: bool,
    held: BTreeMap<u8, (Tick, f32)>,
    notes: Vec<RecordedNote>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

const NO_CHANNEL: u32 = u32::MAX;
const SCAN_EVERY: Duration = Duration::from_millis(1000);
/// Events kept for a front end that is not draining (a headless session).
const EVENT_CAP: usize = 4096;

struct Shared {
    state: Mutex<State>,
    target: AtomicU32,
    tx: Mutex<mpsc::Sender<Msg>>,
}

/// The MIDI side of the application: owns the router thread and the scan timer.
pub struct MidiService {
    shared: Arc<Shared>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl MidiService {
    /// Starts the service on `backend`, routing into `engine`.
    pub fn start(
        engine: Arc<Engine>,
        backend: Box<dyn MidiBackend>,
        store: MappingStore,
    ) -> MidiService {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                store,
                ..State::default()
            }),
            target: AtomicU32::new(NO_CHANNEL),
            tx: Mutex::new(tx.clone()),
        });
        let mut threads = Vec::new();
        let sh = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("ph-midi-router".into())
                .spawn(move || route(engine, backend, sh, rx, tx))
                .expect("spawn midi router"),
        );
        let sh = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("ph-midi-scan".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(SCAN_EVERY);
                        if sh.tx.lock().unwrap().send(Msg::Scan).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn midi scan"),
        );
        let svc = MidiService {
            shared,
            threads: Mutex::new(threads),
        };
        svc.refresh();
        svc
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Looks for new and vanished ports now (instead of at the next second).
    pub fn refresh(&self) {
        let _ = self.shared.tx.lock().unwrap().send(Msg::Scan);
    }

    /// The input ports found by the last scan.
    pub fn inputs(&self) -> Vec<MidiInputInfo> {
        self.state().inputs.clone()
    }

    /// The channel notes, bend, pressure and unmapped controllers go to (the selected one).
    pub fn set_target_channel(&self, channel: Option<ChannelId>) {
        self.shared
            .target
            .store(channel.map_or(NO_CHANNEL, |c| c.0), Relaxed);
    }

    pub fn target_channel(&self) -> Option<ChannelId> {
        match self.shared.target.load(Relaxed) {
            NO_CHANNEL => None,
            c => Some(ChannelId(c)),
        }
    }

    /// Called (from the router thread) whenever an event is queued, to wake a UI.
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.state().waker = waker;
    }

    /// Replaces where MIDI comes from (the front end switches a headless service to the
    /// system's devices). Connections to the old backend close.
    pub fn set_backend(&self, backend: Box<dyn MidiBackend>) {
        let _ = self.shared.tx.lock().unwrap().send(Msg::Backend(backend));
    }

    /// Replaces the mapping store (the front end points it at the config folder).
    pub fn set_store(&self, store: MappingStore) {
        self.state().store = store;
    }

    /// The next controller moved is mapped to `target`; `None` cancels.
    pub fn learn(&self, target: Option<ParamPath>) {
        self.state().learning = target;
    }

    pub fn learning(&self) -> Option<ParamPath> {
        self.state().learning.clone()
    }

    pub fn mappings(&self) -> Vec<Mapping> {
        self.state().store.list().to_vec()
    }

    pub fn map(&self, m: Mapping) -> std::io::Result<Option<Mapping>> {
        self.state().store.set(m)
    }

    pub fn unmap(
        &self,
        controller: u8,
        device: Option<&str>,
    ) -> std::io::Result<Option<Mapping>> {
        self.state().store.remove(controller, device)
    }

    /// Takes the events queued since the last call.
    pub fn drain_events(&self) -> Vec<MidiEvent> {
        self.state().events.drain(..).collect()
    }

    /// Starts collecting played notes (clears earlier ones).
    pub fn start_note_recording(&self) {
        let mut s = self.state();
        s.recording = true;
        s.held.clear();
        s.notes.clear();
    }

    pub fn is_note_recording(&self) -> bool {
        self.state().recording
    }

    /// Stops collecting and returns the notes. Keys still held end at `end` (a tick; the
    /// song position when recording stopped). Notes are in the order they started.
    pub fn finish_note_recording(&self, end: Tick) -> Vec<RecordedNote> {
        let mut s = self.state();
        s.recording = false;
        let held: Vec<_> = s.held.iter().map(|(k, v)| (*k, *v)).collect();
        s.held.clear();
        for (key, (start, velocity)) in held {
            s.notes.push(RecordedNote {
                key,
                velocity,
                start,
                length: end.saturating_sub(start).max(1),
            });
        }
        let mut notes = std::mem::take(&mut s.notes);
        notes.sort_by_key(|n| (n.start, n.key));
        notes
    }
}

impl Drop for MidiService {
    fn drop(&mut self) {
        let _ = self.shared.tx.lock().unwrap().send(Msg::Stop);
        // the scan thread notices the closed channel on its next tick; do not wait for it
        let mut threads = self.threads.lock().unwrap();
        if let Some(router) = threads.first()
            && router.thread().id() != std::thread::current().id()
        {
            let router = threads.remove(0);
            let _ = router.join();
        }
    }
}

fn push_event(shared: &Shared, e: MidiEvent) {
    let waker = {
        let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.events.len() >= EVENT_CAP {
            s.events.pop_front();
        }
        s.events.push_back(e);
        s.waker.clone()
    };
    if let Some(w) = waker {
        w();
    }
}

fn route(
    engine: Arc<Engine>,
    mut backend: Box<dyn MidiBackend>,
    shared: Arc<Shared>,
    rx: mpsc::Receiver<Msg>,
    tx: mpsc::Sender<Msg>,
) {
    let mut open: BTreeMap<String, Box<dyn MidiConnection>> = BTreeMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Stop => return,
            Msg::Backend(b) => {
                open.clear();
                backend = b;
                let _ = tx.send(Msg::Scan);
            }
            Msg::Scan => {
                let ports = backend.ports();
                let mut changed = false;
                // ports that vanished
                let gone: Vec<String> = open
                    .keys()
                    .filter(|n| !ports.iter().any(|p| &p.name == *n))
                    .cloned()
                    .collect();
                for n in gone {
                    open.remove(&n);
                    changed = true;
                }
                // ports that appeared
                for p in &ports {
                    if open.contains_key(&p.name) {
                        continue;
                    }
                    let name: Arc<str> = p.name.as_str().into();
                    let tx = tx.clone();
                    let sink: MidiSink = Arc::new(move |bytes: &[u8]| {
                        let mut b = [0u8; 3];
                        let n = bytes.len().min(3);
                        b[..n].copy_from_slice(&bytes[..n]);
                        let _ = tx.send(Msg::Raw(name.clone(), b, n));
                    });
                    if let Ok(conn) = backend.connect(p, sink) {
                        open.insert(p.name.clone(), conn);
                        changed = true;
                    }
                }
                let listing: Vec<MidiInputInfo> = ports
                    .iter()
                    .map(|p| MidiInputInfo {
                        name: p.name.clone(),
                        connected: open.contains_key(&p.name),
                    })
                    .collect();
                let differs = {
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    let differs = s.inputs != listing;
                    s.inputs = listing;
                    differs
                };
                if changed || differs {
                    push_event(&shared, MidiEvent::Devices);
                }
            }
            Msg::Raw(device, bytes, n) => {
                if let Some(m) = parse(&bytes[..n]) {
                    handle(&engine, &shared, &device, m);
                }
            }
        }
    }
}

fn handle(engine: &Engine, shared: &Shared, device: &str, m: MidiMessage) {
    let target = match shared.target.load(Relaxed) {
        NO_CHANNEL => None,
        c => Some(ChannelId(c)),
    };
    match m {
        MidiMessage::NoteOn { key, velocity } => {
            if let Some(c) = target {
                engine.preview_note(c, key, velocity, true);
            }
            note_recording(engine, shared, key, Some(velocity));
            push_event(shared, MidiEvent::Note { on: true, key });
        }
        MidiMessage::NoteOff { key } => {
            if let Some(c) = target {
                engine.preview_note(c, key, 0.0, false);
            }
            note_recording(engine, shared, key, None);
            push_event(shared, MidiEvent::Note { on: false, key });
        }
        MidiMessage::Cc { controller, value } => {
            // learning, then mapped controllers; everything else is the instrument's
            let outcome = {
                let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(t) = s.learning.take() {
                    let mapping = Mapping {
                        controller,
                        device: Some(device.to_string()),
                        target: t.clone(),
                    };
                    let _ = s.store.set(mapping);
                    Some(MidiEvent::Learned {
                        controller,
                        device: device.to_string(),
                        target: t,
                    })
                } else {
                    s.store
                        .find(controller, device)
                        .map(|mp| MidiEvent::Control {
                            target: mp.target.clone(),
                            value: value as f64,
                            controller,
                            device: device.to_string(),
                        })
                }
            };
            match outcome {
                Some(e) => push_event(shared, e),
                None => {
                    if let Some(c) = target {
                        engine.preview_midi(c, MidiMod::Cc { controller, value });
                    }
                }
            }
        }
        MidiMessage::PitchBend { value } => {
            if let Some(c) = target {
                engine.preview_midi(c, MidiMod::Bend { value });
            }
        }
        MidiMessage::ChannelPressure { value } => {
            if let Some(c) = target {
                engine.preview_midi(c, MidiMod::Pressure { value });
            }
        }
    }
}

fn note_recording(engine: &Engine, shared: &Shared, key: u8, on: Option<f32>) {
    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if !s.recording {
        return;
    }
    let now = engine.song_position();
    match on {
        Some(v) => {
            // a key pressed again while held ends the earlier note first
            if let Some((start, vel)) = s.held.insert(key, (now, v)) {
                s.notes.push(RecordedNote {
                    key,
                    velocity: vel,
                    start,
                    length: now.saturating_sub(start).max(1),
                });
            }
        }
        None => {
            if let Some((start, velocity)) = s.held.remove(&key) {
                s.notes.push(RecordedNote {
                    key,
                    velocity,
                    start,
                    length: now.saturating_sub(start).max(1),
                });
            }
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    #[test]
    fn channel_messages_parse() {
        assert_eq!(
            parse(&[0x93, 60, 127]),
            Some(MidiMessage::NoteOn {
                key: 60,
                velocity: 1.0
            })
        );
        assert_eq!(parse(&[0x90, 60, 0]), Some(MidiMessage::NoteOff { key: 60 }));
        assert_eq!(parse(&[0x81, 61, 64]), Some(MidiMessage::NoteOff { key: 61 }));
        assert_eq!(
            parse(&[0xb0, 1, 127]),
            Some(MidiMessage::Cc {
                controller: 1,
                value: 1.0
            })
        );
        assert_eq!(
            parse(&[0xd5, 127]),
            Some(MidiMessage::ChannelPressure { value: 1.0 })
        );
        // bend: centre 0x2000, full up 0x3fff, full down 0
        assert_eq!(
            parse(&[0xe0, 0x00, 0x40]),
            Some(MidiMessage::PitchBend { value: 0.0 })
        );
        let Some(MidiMessage::PitchBend { value }) = parse(&[0xe0, 0x7f, 0x7f]) else {
            panic!()
        };
        assert!(value > 0.999 && value <= 1.0);
        assert_eq!(
            parse(&[0xe0, 0x00, 0x00]),
            Some(MidiMessage::PitchBend { value: -1.0 })
        );
        assert_eq!(parse(&[0xf8]), None, "timing clock is not a channel message");
        assert_eq!(parse(&[]), None);
    }

    #[test]
    fn mappings_prefer_the_device_and_persist_per_user() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = MappingStore::open(dir.path());
        assert!(s.list().is_empty());
        let t1 = ParamPath::ChannelVolume(ChannelId(1));
        let t2 = ParamPath::ChannelPan(ChannelId(2));
        s.set(Mapping {
            controller: 7,
            device: None,
            target: t1.clone(),
        })
        .unwrap();
        s.set(Mapping {
            controller: 7,
            device: Some("Keys".into()),
            target: t2.clone(),
        })
        .unwrap();
        assert_eq!(s.find(7, "Keys").unwrap().target, t2);
        assert_eq!(s.find(7, "Pads").unwrap().target, t1);
        assert!(s.find(8, "Keys").is_none());
        // replacing keeps one mapping per (controller, device)
        let old = s
            .set(Mapping {
                controller: 7,
                device: None,
                target: t2.clone(),
            })
            .unwrap();
        assert_eq!(old.unwrap().target, t1);
        assert_eq!(s.list().len(), 2);
        // the file is in the config folder and reloads
        let again = MappingStore::open(dir.path());
        assert_eq!(again.list(), s.list());
        assert!(dir.path().join("midi-mappings.json").exists());
        let mut again = again;
        assert!(again.remove(7, Some("Keys")).unwrap().is_some());
        assert!(again.remove(7, Some("Keys")).unwrap().is_none());
        assert_eq!(MappingStore::open(dir.path()).list().len(), 1);
    }

    #[test]
    fn a_broken_mapping_file_starts_empty_and_is_replaced_by_the_first_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("midi-mappings.json"), b"{ not json").unwrap();
        let mut s = MappingStore::open(dir.path());
        assert!(s.list().is_empty());
        s.set(Mapping {
            controller: 1,
            device: None,
            target: ParamPath::Tempo,
        })
        .unwrap();
        assert_eq!(MappingStore::open(dir.path()).list().len(), 1);
    }
}
