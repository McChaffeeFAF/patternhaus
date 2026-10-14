//! Audio input and recording (M2/T0 types; M2/T8 adds device input through cpal).
//!
//! - An [`InputSource`] delivers input frames on the audio thread. T8 implements it over
//!   the device; [`SimulatedInput`] plays a buffer, for tests.
//! - A [`Recorder`] captures one armed input into a buffer allocated up front, so capturing
//!   is real-time safe; [`Recorder::finish`] turns it into a [`RecordedTake`] off the audio
//!   thread.
//! - [`RecordedTake::sample_entry`] is the registry entry the take becomes once T8 has
//!   written its audio to `samples/` (`ph_core::Origin::Recorded`).
//! - `Engine::arm`, `record_start`, `record_stop` and `set_monitor` are the engine API; until
//!   T8 they return [`crate::EngineError::Unavailable`].
//!
//! Offline renders never read inputs: [`crate::render_offline`] is the input-free path, and
//! a monitored input is not part of the oracle comparison.

use crate::tempo::TempoMap;
use ph_core::*;

/// An audio input as the host numbers them (a device channel or channel pair).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InputId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct InputInfo {
    pub id: InputId,
    /// What the user sees, e.g. `Focusrite USB: In 1-2`. Recorded takes keep it.
    pub name: String,
    /// 1 or 2.
    pub channels: u16,
    pub sample_rate: u32,
}

/// Where a recording goes.
#[derive(Clone, Debug, PartialEq)]
pub enum RecordTarget {
    /// A new audio clip on this playlist track, at the recording start.
    Track(u32),
    /// Replaces this Sampler channel's sample.
    Channel(ChannelId),
}

/// An armed input.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub input: InputId,
    pub target: RecordTarget,
    /// Hear the input through the target while armed.
    pub monitor: bool,
}

/// Monitoring latency (input buffer plus output buffer) in seconds, for the readout.
pub fn monitor_latency(buffer_frames: u32, sample_rate: u32) -> f64 {
    2.0 * buffer_frames as f64 / sample_rate.max(1) as f64
}

/// Input frames on the audio thread.
pub trait InputSource: std::marker::Send {
    fn info(&self) -> &InputInfo;
    /// Real-time. Fills `out` (`[channel][frame]`) with the next input frames and returns
    /// how many it wrote; fewer than asked means the input had nothing more (the rest is
    /// silence).
    fn read(&mut self, out: &mut [&mut [f32]]) -> usize;
}

/// An input that plays a fixed buffer once.
pub struct SimulatedInput {
    info: InputInfo,
    data: Vec<Vec<f32>>,
    pos: usize,
}

impl SimulatedInput {
    /// `data` is `[channel][frame]`.
    pub fn new(id: InputId, name: &str, sample_rate: u32, data: Vec<Vec<f32>>) -> Self {
        Self {
            info: InputInfo {
                id,
                name: name.into(),
                channels: data.len() as u16,
                sample_rate,
            },
            data,
            pos: 0,
        }
    }
}

impl InputSource for SimulatedInput {
    fn info(&self) -> &InputInfo {
        &self.info
    }

    fn read(&mut self, out: &mut [&mut [f32]]) -> usize {
        let want = out.first().map_or(0, |c| c.len());
        let len = self.data.first().map_or(0, |c| c.len());
        let n = want.min(len - self.pos.min(len));
        for (c, dst) in out.iter_mut().enumerate() {
            match self.data.get(c).or(self.data.last()) {
                Some(src) => dst[..n].copy_from_slice(&src[self.pos..self.pos + n]),
                None => dst[..n].fill(0.0),
            }
            dst[n..].fill(0.0);
        }
        self.pos += n;
        n
    }
}

/// A finished recording, before it is written to a file.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedTake {
    pub input: InputInfo,
    pub target: RecordTarget,
    /// Song position where recording started.
    pub start: Tick,
    /// Recorded range in ticks.
    pub length: Tick,
    /// `[channel][frame]` at `input.sample_rate`.
    pub audio: Vec<Vec<f32>>,
    /// Frames dropped because the take outgrew its buffer.
    pub overflow: u64,
}

impl RecordedTake {
    pub fn frames(&self) -> u64 {
        self.audio.first().map_or(0, |c| c.len() as u64)
    }

    /// The registry entry for this take once its audio is saved at `path`
    /// (`samples/<file>`). `at` is the time in Unix seconds.
    pub fn sample_entry(&self, path: &str, at: Option<u64>) -> SampleEntry {
        SampleEntry {
            id: SampleId(0),
            path: path.into(),
            name: path.rsplit('/').next().unwrap_or(path).into(),
            frames: self.frames(),
            sample_rate: self.input.sample_rate,
            channels: self.audio.len() as u16,
            bpm: None,
            root_key: None,
            analysis: SampleAnalysis::default(),
            provenance: Provenance {
                origin: Origin::Recorded(Take {
                    input: self.input.name.clone(),
                    start: self.start,
                    length: self.length,
                }),
                at,
            },
        }
    }
}

/// Captures one armed input into a preallocated buffer.
pub struct Recorder {
    arm: Arm,
    info: InputInfo,
    start: Tick,
    start_sample: i64,
    audio: Vec<Vec<f32>>,
    len: usize,
    overflow: u64,
}

impl Recorder {
    /// Allocates room for `capacity` frames. Off the audio thread.
    pub fn new(arm: Arm, info: InputInfo, start: Tick, start_sample: i64, capacity: usize) -> Self {
        let ch = info.channels.max(1) as usize;
        Self {
            arm,
            info,
            start,
            start_sample,
            audio: vec![vec![0.0; capacity]; ch],
            len: 0,
            overflow: 0,
        }
    }

    /// Real-time. Reads up to `frames` frames from `input` into the buffer; frames that do
    /// not fit are read and dropped (counted in `overflow`). Returns frames kept.
    pub fn capture(&mut self, input: &mut dyn InputSource, frames: usize) -> usize {
        let cap = self.audio.first().map_or(0, |c| c.len());
        let keep = frames.min(cap - self.len);
        let len = self.len;
        let chans = self.audio.len().min(2);
        let mut outs: [&mut [f32]; 2] = [&mut [], &mut []];
        for (o, ch) in outs.iter_mut().zip(self.audio.iter_mut()) {
            *o = &mut ch[len..len + keep];
        }
        let n = input.read(&mut outs[..chans]);
        self.len += n;
        if frames > keep {
            // drain and drop the rest (no buffer to put it in)
            let mut scratch = [0.0f32; 256];
            let mut left = frames - keep;
            while left > 0 {
                let k = left.min(scratch.len());
                let got = input.read(&mut [&mut scratch[..k]]);
                self.overflow += got as u64;
                if got < k {
                    break;
                }
                left -= k;
            }
        }
        n
    }

    pub fn arm(&self) -> &Arm {
        &self.arm
    }

    /// The take. Off the audio thread (it trims the buffer).
    pub fn finish(mut self, tempo: &TempoMap) -> RecordedTake {
        for ch in &mut self.audio {
            ch.truncate(self.len);
        }
        let end = tempo.tick_floor(self.start_sample + self.len as i64);
        RecordedTake {
            input: self.info,
            target: self.arm.target,
            start: self.start,
            length: end.saturating_sub(self.start),
            audio: self.audio,
            overflow: self.overflow,
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    fn input() -> SimulatedInput {
        let l: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let r = l.iter().map(|v| v * 2.0).collect();
        SimulatedInput::new(InputId(1), "Sim: In 1-2", 48_000, vec![l, r])
    }

    #[test]
    fn a_simulated_input_feeds_a_recorder_into_a_take() {
        let mut inp = input();
        let info = inp.info().clone();
        let arm = Arm {
            input: info.id,
            target: RecordTarget::Track(3),
            monitor: true,
        };
        let tempo = TempoMap::constant(120.0, 48_000);
        // recording starts at beat 2 (sample 48000)
        let mut rec = Recorder::new(arm, info, 2 * PPQ, 48_000, 600);
        assert_eq!(rec.capture(&mut inp, 256), 256);
        assert_eq!(rec.capture(&mut inp, 256), 256);
        assert_eq!(rec.capture(&mut inp, 256), 88, "buffer full");
        let take = rec.finish(&tempo);
        assert_eq!(take.frames(), 600);
        assert_eq!(take.overflow, 168);
        assert_eq!(take.audio[0][599], 599.0);
        assert_eq!(take.audio[1][10], 20.0);
        // 600 samples at 120 BPM and 48 kHz (25 samples a tick) = 24 ticks
        assert_eq!((take.start, take.length), (2 * PPQ, 24));
        let e = take.sample_entry("samples/take 1.wav", Some(5));
        assert_eq!(
            (e.name.as_str(), e.frames, e.channels),
            ("take 1.wav", 600, 2)
        );
        assert!(matches!(
            e.provenance.origin,
            Origin::Recorded(Take { ref input, start, length: 24 }) if input == "Sim: In 1-2" && start == 2 * PPQ
        ));
        assert_eq!(monitor_latency(256, 48_000), 512.0 / 48_000.0);
    }

    #[test]
    fn the_engine_recording_api_is_reserved_for_t8() {
        let (engine, _dev) = crate::tests::sim(256, 0);
        assert!(engine.inputs().is_empty());
        let unavailable = |r: crate::EngineResult<()>| {
            matches!(r, Err(crate::EngineError::Unavailable { track: "T8", .. }))
        };
        assert!(unavailable(engine.arm(InputId(0), RecordTarget::Track(0))));
        assert!(unavailable(engine.set_monitor(InputId(0), true)));
        assert!(unavailable(engine.record_start()));
        assert!(unavailable(engine.record_stop().map(|_| ())));
        engine.disarm(InputId(0));
        let e = engine.record_start().unwrap_err().to_string();
        assert_eq!(e, "audio input recording is not available until M2/T8");
    }

    #[test]
    fn a_simulated_input_runs_dry_with_silence() {
        let mut inp = input();
        let mut a = vec![9.0; 1500];
        assert_eq!(inp.read(&mut [&mut a]), 1000);
        assert!(a[1000..].iter().all(|v| *v == 0.0));
        assert_eq!(inp.read(&mut [&mut a]), 0);
    }
}
