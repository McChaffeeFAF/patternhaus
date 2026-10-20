//! Real audio output through `cpal`: WASAPI shared mode by default on Windows; ASIO behind
//! the `asio` cargo feature (DESIGN §11).
//!
//! `cpal::Stream` is not `Send` on every backend, so a dedicated device thread owns it. The
//! thread opens the device, reports the actual sample rate and buffer size, receives the
//! [`Runtime`] compiled for them, builds and starts the stream, then waits for a stop
//! message. The data callback is the only real-time code here: it calls
//! [`Runtime::process_interleaved`] under `assert_no_alloc`.

use crate::runtime::Runtime;
use crate::{EngineConfig, EngineError, EngineResult};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::thread::JoinHandle;

pub(crate) struct DeviceHandle {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    pub sample_rate: u32,
    pub buffer_frames: u32,
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

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

/// Opens the output device. `make(sample_rate, buffer_frames)` builds the runtime for the
/// settings the device actually accepted.
pub(crate) fn open(
    config: &EngineConfig,
    make: impl FnOnce(u32, u32) -> EngineResult<Runtime> + Send + 'static,
) -> EngineResult<DeviceHandle> {
    let cfg = config.clone();
    let (ready_tx, ready_rx) = mpsc::channel::<EngineResult<(u32, u32)>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("ph-engine-device".into())
        .spawn(move || {
            let opened = (|| -> EngineResult<(cpal::Device, cpal::StreamConfig)> {
                let host = pick_host(cfg.exclusive)?;
                let device = match &cfg.device {
                    Some(name) => host
                        .output_devices()
                        .map_err(dev_err)?
                        .find(|d| d.description().is_ok_and(|x| x.name() == name))
                        .ok_or_else(|| EngineError::Device(format!("no output device `{name}`")))?,
                    None => host
                        .default_output_device()
                        .ok_or_else(|| EngineError::Device("no default output device".into()))?,
                };
                let wanted = device
                    .supported_output_configs()
                    .map_err(dev_err)?
                    .filter(|c| c.sample_format() == cpal::SampleFormat::F32 && c.channels() >= 1)
                    .find_map(|c| c.try_with_sample_rate(cfg.sample_rate));
                let supported = match wanted {
                    Some(s) => s,
                    None => {
                        let d = device.default_output_config().map_err(dev_err)?;
                        if d.sample_format() != cpal::SampleFormat::F32 {
                            return Err(EngineError::Device(format!(
                                "device does not offer 32-bit float output ({:?})",
                                d.sample_format()
                            )));
                        }
                        d
                    }
                };
                let mut sc = supported.config();
                sc.buffer_size = match supported.buffer_size() {
                    cpal::SupportedBufferSize::Range { min, max }
                        if (*min..=*max).contains(&cfg.buffer_frames) =>
                    {
                        cpal::BufferSize::Fixed(cfg.buffer_frames)
                    }
                    _ => cpal::BufferSize::Default,
                };
                Ok((device, sc))
            })();
            let (device, sc) = match opened {
                Ok(x) => x,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let frames = match sc.buffer_size {
                cpal::BufferSize::Fixed(f) => f,
                cpal::BufferSize::Default => cfg.buffer_frames,
            };
            let mut rt = match make(sc.sample_rate, frames) {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let channels = sc.channels as usize;
            let rate = sc.sample_rate as f64;
            let rig_shared = rt.rig.shared.clone();
            let stream = device.build_output_stream(
                sc,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    crate::rt::enable_ftz_daz();
                    // how far ahead of the speakers this block is (recording latency
                    // compensation); an atomic store, no allocation
                    let ts = info.timestamp();
                    let ahead = ts.playback.saturating_duration_since(ts.callback);
                    rig_shared
                        .out_latency
                        .store((ahead.as_secs_f64() * rate).round() as u32, Relaxed);
                    crate::rt::no_alloc(|| rt.process_interleaved(data, channels));
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
            let _ = ready_tx.send(Ok((sc.sample_rate, frames)));
            // run until the handle is dropped
            let _ = stop_rx.recv();
            drop(stream);
        })
        .map_err(dev_err)?;
    match ready_rx.recv() {
        Ok(Ok((sample_rate, buffer_frames))) => Ok(DeviceHandle {
            stop: Some(stop_tx),
            thread: Some(thread),
            sample_rate,
            buffer_frames,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(EngineError::Device(
                "device thread ended unexpectedly".into(),
            ))
        }
    }
}
