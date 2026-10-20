//! Audio input through `cpal` (M2/T8): WASAPI shared mode by default on Windows, ASIO behind
//! the `asio` cargo feature, like the output (`device.rs`).
//!
//! - Inputs are listed as channel pairs of each input device: `Focusrite USB: In 1-2`. A
//!   device with an odd channel count ends in a mono input.
//! - `cpal::Stream` is not `Send` on every backend, so each opened input has a thread that
//!   owns the stream, like the output's device thread. Its data callback copies the chosen
//!   pair into a lock-free ring ([`rtrb`]); the audio thread reads that ring through
//!   [`RingInput`]. The callback never blocks: when the ring is full the newest frames are
//!   dropped.
//! - The input latency is measured from the callback timestamps (callback instant minus
//!   capture instant) and reported by [`InputSource::latency_frames`] together with the frames
//!   waiting in the ring.
//! - Never exercised by tests (they must not open a real device); it compiles against, and is
//!   checked on the owner's hardware through `docs/manual-tests/M2-recording-midi.md`.
//!
//! WASAPI **exclusive** mode: cpal 0.18 offers no exclusive-mode switch for WASAPI (the output
//! side says so too, see `device.rs`), so exclusive input exists only in the `asio` build. A
//! follow-up can add it when cpal does (or through a small WASAPI binding of our own).

use crate::record::*;
use crate::{EngineError, EngineResult};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::*;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

/// Seconds of audio the ring between the input callback and the audio thread holds.
const RING_SECONDS: f64 = 0.5;
/// Queued frames beyond this many blocks are dropped (the input runs ahead of the output).
const MAX_BACKLOG_BLOCKS: usize = 4;

fn dev_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::Device(e.to_string())
}

fn pick_host(exclusive: bool) -> EngineResult<cpal::Host> {
    #[cfg(all(feature = "asio", target_os = "windows"))]
    if exclusive {
        return cpal::host_from_id(cpal::HostId::Asio).map_err(dev_err);
    }
    if exclusive {
        return Err(EngineError::Device(
            "exclusive mode needs the `asio` build (cpal has no WASAPI exclusive mode)".into(),
        ));
    }
    Ok(cpal::default_host())
}

/// The device inputs, as channel pairs. Ids are `device index << 4 | pair`.
pub(crate) struct CpalInputs {
    exclusive: bool,
    /// Device names in the order of the last listing (ids index into it).
    devices: Vec<String>,
}

impl CpalInputs {
    pub fn new(exclusive: bool) -> Self {
        Self {
            exclusive,
            devices: Vec::new(),
        }
    }
}

fn pair_name(device: &str, channels: u16, pair: u16) -> String {
    let c0 = pair * 2;
    if c0 + 1 < channels {
        format!("{device}: In {}-{}", c0 + 1, c0 + 2)
    } else {
        format!("{device}: In {}", c0 + 1)
    }
}

impl InputProvider for CpalInputs {
    fn list(&mut self) -> Vec<InputInfo> {
        let mut out = Vec::new();
        self.devices.clear();
        let Ok(host) = pick_host(self.exclusive) else {
            return out;
        };
        let Ok(devices) = host.input_devices() else {
            return out;
        };
        for d in devices {
            let Ok(desc) = d.description() else { continue };
            let name = desc.name().to_string();
            let Ok(configs) = d.supported_input_configs() else {
                continue;
            };
            let mut channels = 0u16;
            let mut rate = 48_000;
            for c in configs.filter(|c| c.sample_format() == cpal::SampleFormat::F32) {
                channels = channels.max(c.channels());
                rate = c.max_sample_rate();
            }
            if channels == 0 {
                continue;
            }
            let idx = self.devices.len() as u32;
            self.devices.push(name.clone());
            for pair in 0..channels.div_ceil(2).min(8) {
                let width = if pair * 2 + 1 < channels { 2 } else { 1 };
                out.push(InputInfo {
                    id: InputId(idx << 4 | pair as u32),
                    name: pair_name(&name, channels, pair),
                    channels: width,
                    sample_rate: rate,
                });
            }
        }
        out
    }

    fn open(
        &mut self,
        id: InputId,
        sample_rate: u32,
        max_frames: usize,
    ) -> EngineResult<Box<dyn InputSource>> {
        if self.devices.is_empty() {
            self.list();
        }
        let (dev, pair) = ((id.0 >> 4) as usize, (id.0 & 15) as u16);
        let name = self
            .devices
            .get(dev)
            .cloned()
            .ok_or_else(|| EngineError::Device(format!("no audio input {}", id.0)))?;
        RingInput::open(self.exclusive, id, &name, pair, sample_rate, max_frames)
            .map(|s| Box::new(s) as Box<dyn InputSource>)
    }
}

/// What the input callback publishes to the reader.
struct InState {
    /// Callback instant minus capture instant of the latest callback, in frames.
    latency: AtomicU32,
}

/// Owns the thread that owns the stream; dropping it stops the stream.
struct StreamGuard {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// An open device input: a ring the callback fills and the audio thread drains.
pub(crate) struct RingInput {
    info: InputInfo,
    rx: rtrb::Consumer<f32>,
    ch: usize,
    state: Arc<InState>,
    max_backlog: usize,
    _guard: StreamGuard,
}

impl RingInput {
    fn open(
        exclusive: bool,
        id: InputId,
        device_name: &str,
        pair: u16,
        sample_rate: u32,
        max_frames: usize,
    ) -> EngineResult<RingInput> {
        let name = device_name.to_string();
        let (ready_tx, ready_rx) = mpsc::channel::<EngineResult<(u16, u16, rtrb::Consumer<f32>, Arc<InState>)>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("ph-engine-input".into())
            .spawn(move || {
                let opened = (|| {
                    let host = pick_host(exclusive)?;
                    let device = host
                        .input_devices()
                        .map_err(dev_err)?
                        .find(|d| d.description().is_ok_and(|x| x.name() == name))
                        .ok_or_else(|| EngineError::Device(format!("no input device `{name}`")))?;
                    let supported = device
                        .supported_input_configs()
                        .map_err(dev_err)?
                        .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
                        .filter(|c| c.channels() > pair * 2)
                        .find_map(|c| c.try_with_sample_rate(sample_rate))
                        .ok_or_else(|| {
                            EngineError::Device(format!(
                                "{name} does not offer 32-bit float input at {sample_rate} Hz (set the device to {sample_rate} Hz)"
                            ))
                        })?;
                    let mut sc = supported.config();
                    sc.buffer_size = match supported.buffer_size() {
                        cpal::SupportedBufferSize::Range { min, max }
                            if (*min..=*max).contains(&(max_frames as u32)) =>
                        {
                            cpal::BufferSize::Fixed(max_frames as u32)
                        }
                        _ => cpal::BufferSize::Default,
                    };
                    Ok((device, sc))
                })();
                let (device, sc): (cpal::Device, cpal::StreamConfig) = match opened {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let channels = sc.channels as usize;
                let sc_channels = sc.channels;
                let c0 = pair as usize * 2;
                let width = if c0 + 1 < channels { 2 } else { 1 };
                let ring = ((RING_SECONDS * sample_rate as f64) as usize).max(4096) * width;
                let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(ring);
                let state = Arc::new(InState {
                    latency: AtomicU32::new(0),
                });
                let st = state.clone();
                let rate = sample_rate as f64;
                let stream = device.build_input_stream(
                    sc,
                    move |data: &[f32], info: &cpal::InputCallbackInfo| {
                        crate::rt::enable_ftz_daz();
                        let ts = info.timestamp();
                        let lat = ts.callback.saturating_duration_since(ts.capture).as_secs_f64();
                        st.latency.store((lat * rate).round() as u32, Relaxed);
                        for frame in data.chunks_exact(channels) {
                            if tx.slots() < width {
                                break; // full: drop the newest frames, never block
                            }
                            let _ = tx.push(frame[c0]);
                            if width == 2 {
                                let _ = tx.push(frame[c0 + 1]);
                            }
                        }
                    },
                    |_err| {},
                    None,
                );
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(dev_err(e)));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(dev_err(e)));
                    return;
                }
                let _ = ready_tx.send(Ok((width as u16, sc_channels, rx, state)));
                // run until the guard is dropped
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(dev_err)?;
        let guard = StreamGuard {
            stop: Some(stop_tx),
            thread: Some(thread),
        };
        match ready_rx.recv() {
            Ok(Ok((width, total, rx, state))) => Ok(RingInput {
                info: InputInfo {
                    id,
                    name: pair_name(device_name, total, pair),
                    channels: width,
                    sample_rate,
                },
                rx,
                ch: width as usize,
                state,
                max_backlog: max_frames * MAX_BACKLOG_BLOCKS,
                _guard: guard,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(EngineError::Device(
                "input thread ended unexpectedly".into(),
            )),
        }
    }

    fn skip(&mut self, frames: usize) {
        let n = (frames * self.ch).min(self.rx.slots());
        if n > 0
            && let Ok(chunk) = self.rx.read_chunk(n)
        {
            chunk.commit_all();
        }
    }
}

impl InputSource for RingInput {
    fn info(&self) -> &InputInfo {
        &self.info
    }

    fn read(&mut self, out: &mut [&mut [f32]]) -> usize {
        let want = out.first().map_or(0, |c| c.len());
        let queued = self.rx.slots() / self.ch;
        // the input ran ahead of the output: drop the oldest audio rather than grow latency
        if queued > want + self.max_backlog {
            self.skip(queued - want - self.max_backlog / 2);
        }
        let n = (self.rx.slots() / self.ch).min(want);
        if n > 0
            && let Ok(chunk) = self.rx.read_chunk(n * self.ch)
        {
            let (a, b) = chunk.as_slices();
            let mut it = a.iter().chain(b);
            for k in 0..n {
                for c in 0..self.ch {
                    let v = it.next().copied().unwrap_or(0.0);
                    if let Some(dst) = out.get_mut(c) {
                        dst[k] = v;
                    }
                }
            }
            chunk.commit_all();
        }
        for dst in out.iter_mut() {
            dst[n..].fill(0.0);
        }
        n
    }

    fn latency_frames(&self) -> u32 {
        self.state.latency.load(Relaxed) + (self.rx.slots() / self.ch) as u32
    }

    fn flush(&mut self) {
        let n = self.rx.slots();
        if n > 0
            && let Ok(chunk) = self.rx.read_chunk(n)
        {
            chunk.commit_all();
        }
    }
}
