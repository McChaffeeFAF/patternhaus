//! Audio clips in the engine (M2/T0 interfaces; M2/T1 implements playback).
//!
//! - [`plan_audio_clips`] compiles the model's audio clips into [`AudioClipPlan`]s: song
//!   positions in samples, the source range in source frames, the speed, gain and fades.
//!   Control thread.
//! - [`Prefetcher`] is the streaming side: a thread off the audio thread keeps one lock-free
//!   ring buffer per open stream filled ahead of playback; the audio thread only reads its
//!   [`StreamReader`]. (M2/T0 streams from decoded audio held by the `AssetResolver`; T1 may
//!   read from disk instead behind the same interface.)
//! - [`Stretcher`] is the time-stretch / pitch-shift engine interface, shaped after
//!   Signalsmith Stretch (MIT), which T1 binds. [`stretcher_for`] is where T1 plugs it in.
//! - [`AudioClipNode`] is the per-track playback node T1 fills in; in T0 it outputs silence
//!   and is not yet part of the render plan, so M1 renders are unchanged.

use crate::tempo::TempoMap;
use crate::{EngineError, EngineResult};
use ph_core::*;
use ph_node::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// One audio clip, ready to play.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioClipPlan {
    pub clip: ClipId,
    pub track: u32,
    pub sample: SampleId,
    /// Asset reference of the registry entry.
    pub path: String,
    /// Song samples covered: `start..end`.
    pub start: i64,
    pub end: i64,
    /// First source frame played (the start of the played range, also when reversed).
    pub source_start: u64,
    /// Source frames consumed per output frame, sample-rate conversion included:
    /// `AudioClip::time_ratio × source rate / engine rate`. Uses the tempo at the clip
    /// start; T1 follows tempo changes inside a synced clip.
    pub speed: f64,
    /// Transposition left to a pitch shifter, in semitones: 0 for `Resample` (the speed
    /// carries it), the clip's pitch for `Stretch` and `Slice`.
    pub transpose: f64,
    /// Linear gain from `gain_db`.
    pub gain: f32,
    /// Fade lengths in output samples, with their curves.
    pub fade_in: (u64, FadeCurve),
    pub fade_out: (u64, FadeCurve),
    pub reverse: bool,
    pub stretch: StretchMode,
}

impl AudioClipPlan {
    /// Source frames the clip plays (`speed × length`, rounded up, clipped to the file).
    pub fn source_frames(&self, file_frames: u64) -> u64 {
        let want = ((self.end - self.start) as f64 * self.speed).ceil() as u64;
        want.min(file_frames.saturating_sub(self.source_start))
    }

    /// Fade gain at output sample `i` of the clip (0-based), fade-in and fade-out combined.
    pub fn fade_gain(&self, i: u64) -> f64 {
        let len = (self.end - self.start) as u64;
        let mut g = 1.0;
        let (fi, ci) = self.fade_in;
        if fi > 0 && i < fi {
            g *= ci.gain(i as f64 / fi as f64);
        }
        let (fo, co) = self.fade_out;
        if fo > 0 && i + fo >= len {
            g *= co.gain(len.saturating_sub(i) as f64 / fo as f64);
        }
        g
    }
}

/// Compiles every audio clip on an unmuted track, sorted by (start, clip id), plus a warning
/// for each clip that cannot play as asked (a missing registry entry, a synced clip whose
/// sample has no tempo). Off the audio thread.
pub fn plan_audio_clips(project: &Project, tempo: &TempoMap) -> (Vec<AudioClipPlan>, Vec<String>) {
    let rate = tempo.sample_rate() as f64;
    let mut plans = Vec::new();
    let mut warnings = Vec::new();
    for c in project.clips.values() {
        let Some(a) = c.audio() else { continue };
        if project
            .tracks
            .get(c.track as usize)
            .is_some_and(|t| t.muted)
        {
            continue;
        }
        let Ok(entry) = project.sample(a.sample) else {
            warnings.push(format!(
                "audio clip {}: sample {} is not registered",
                c.id, a.sample
            ));
            continue;
        };
        if a.tempo_sync && entry.bpm.is_none() {
            warnings.push(format!(
                "audio clip {}: {} has no tempo, so it plays unsynced",
                c.id, entry.name
            ));
        }
        let start = tempo.sample_at(c.start);
        let end = tempo.sample_at(c.start + c.length);
        let to_samples = |t: Tick| (tempo.sample_at(c.start + t.min(c.length)) - start) as u64;
        let ratio = a.time_ratio(entry, tempo.bpm_at(c.start));
        plans.push(AudioClipPlan {
            clip: c.id,
            track: c.track,
            sample: a.sample,
            path: entry.path.clone(),
            start,
            end,
            source_start: a.source_offset,
            speed: ratio * entry.sample_rate as f64 / rate,
            transpose: match a.stretch {
                StretchMode::Resample => 0.0,
                StretchMode::Stretch | StretchMode::Slice => a.pitch(),
            },
            gain: 10f32.powf(a.gain_db / 20.0),
            fade_in: (to_samples(a.fade_in.length), a.fade_in.curve),
            fade_out: (to_samples(a.fade_out.length), a.fade_out.curve),
            reverse: a.reverse,
            stretch: a.stretch,
        });
    }
    plans.sort_by_key(|p| (p.start, p.clip));
    (plans, warnings)
}

/// A time-stretch and pitch-shift engine (shaped after Signalsmith Stretch). One instance
/// per playing clip voice.
pub trait Stretcher: std::marker::Send {
    /// Allocates for `channels` at `sample_rate`, for blocks of up to `max_frames` output
    /// frames. Off the audio thread.
    fn configure(&mut self, channels: usize, sample_rate: f64, max_frames: usize);
    /// Real-time. Forgets all buffered audio (seek, loop).
    fn reset(&mut self);
    /// Input frames to feed ahead of the first output frame.
    fn input_latency(&self) -> usize;
    /// Output frames before the input's first frame appears.
    fn output_latency(&self) -> usize;
    /// Real-time. Pitch shift applied from now on, in semitones.
    fn set_transpose(&mut self, semitones: f64);
    /// Real-time. Consumes all of `input` (`[channel][frame]`) and produces all of `output`;
    /// the time ratio of this call is `input frames / output frames`.
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]);
}

/// The stretcher for a mode. M2/T1 provides them (Signalsmith Stretch for `Stretch`, the
/// slicer for `Slice`, a resampler for `Resample`); until then every mode is unavailable.
pub fn stretcher_for(mode: StretchMode) -> EngineResult<Box<dyn Stretcher>> {
    let _ = mode;
    Err(EngineError::Unavailable {
        feature: "audio clip playback (stretching and resampling)",
        track: "T1",
    })
}

/// Ring capacity per stream, in frames, unless the caller asks for another.
pub const DEFAULT_PREFETCH_FRAMES: usize = 1 << 16;

/// One stream as the prefetch thread sees it.
struct Feed {
    data: Arc<SampleData>,
    /// Next source frame to push (counting down when reversed).
    next: u64,
    /// Frames still to push.
    left: u64,
    reverse: bool,
    prod: rtrb::Producer<f32>,
    closed: Arc<AtomicBool>,
}

impl Feed {
    /// Pushes as many whole frames as fit. Returns false when the feed is done.
    fn pump(&mut self) -> bool {
        let ch = self.data.channels.len().max(1);
        let fit = (self.prod.slots() / ch) as u64;
        let n = fit.min(self.left) as usize;
        if n > 0
            && let Ok(mut chunk) = self.prod.write_chunk(n * ch)
        {
            let (a, b) = chunk.as_mut_slices();
            for (k, slot) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                let (i, c) = (k / ch, k % ch);
                let f = if self.reverse {
                    self.next - i as u64
                } else {
                    self.next + i as u64
                } as usize;
                *slot = self.data.channels[c].get(f).copied().unwrap_or(0.0);
            }
            chunk.commit_all();
            self.left -= n as u64;
            if self.reverse {
                self.next = self.next.saturating_sub(n as u64);
            } else {
                self.next += n as u64;
            }
        }
        self.left > 0 && !self.closed.load(Ordering::Acquire)
    }
}

/// The audio-thread end of a stream. Dropping it closes the stream (do that off the audio
/// thread, like any plan data).
pub struct StreamReader {
    cons: rtrb::Consumer<f32>,
    channels: usize,
    left: u64,
    underruns: u64,
    closed: Arc<AtomicBool>,
}

impl StreamReader {
    /// Real-time. Reads the next frames into `out` (`[channel][frame]`, all the same
    /// length; output channels beyond the source's repeat its last one, so a mono source
    /// fills both sides). Frames the prefetcher has not delivered yet are silence and count as an
    /// underrun; frames past the stream's end are silence and do not. Returns frames read.
    pub fn read(&mut self, out: &mut [&mut [f32]]) -> usize {
        let want = out.first().map_or(0, |c| c.len());
        let ch = self.channels;
        let avail = self.cons.slots() / ch;
        let n = want.min(avail);
        if n > 0
            && let Ok(chunk) = self.cons.read_chunk(n * ch)
        {
            let (a, b) = chunk.as_slices();
            for (k, v) in a.iter().chain(b.iter()).enumerate() {
                let (i, c) = (k / ch, k % ch);
                for (o, dst) in out.iter_mut().enumerate() {
                    if o.min(ch - 1) == c {
                        dst[i] = *v;
                    }
                }
            }
            chunk.commit_all();
        }
        for dst in out.iter_mut() {
            dst[n..].fill(0.0);
        }
        if n < want && (self.left as usize) > n {
            self.underruns += 1;
        }
        self.left = self.left.saturating_sub(n as u64);
        n
    }

    /// Blocks that came up short before the stream's end.
    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Every frame of the stream has been read.
    pub fn finished(&self) -> bool {
        self.left == 0
    }
}

impl Drop for StreamReader {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

struct Shared {
    feeds: Mutex<Vec<Feed>>,
    stop: AtomicBool,
}

/// The prefetch thread. One per engine.
pub struct Prefetcher {
    assets: Arc<dyn AssetResolver>,
    ring_frames: usize,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Prefetcher {
    /// Starts the thread. `ring_frames` is the read-ahead per stream.
    pub fn start(assets: Arc<dyn AssetResolver>, ring_frames: usize) -> Self {
        let shared = Arc::new(Shared {
            feeds: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("ph-prefetch".into())
            .spawn(move || {
                while !s.stop.load(Ordering::Acquire) {
                    if let Ok(mut feeds) = s.feeds.lock() {
                        feeds.retain_mut(Feed::pump);
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
            .expect("spawn the prefetch thread");
        Self {
            assets,
            ring_frames: ring_frames.max(64),
            shared,
            thread: Some(thread),
        }
    }

    /// Opens a stream of `frames` source frames of `path` from `start` (backwards from
    /// `start + frames − 1` when `reverse`). Resolves the asset and pre-fills the ring on the
    /// calling thread, so playback can start at once. Off the audio thread.
    pub fn open(
        &self,
        path: &str,
        start: u64,
        frames: u64,
        reverse: bool,
    ) -> EngineResult<StreamReader> {
        let data = self.assets.sample(path).map_err(EngineError::Asset)?;
        let total = data.frames() as u64;
        let frames = frames.min(total.saturating_sub(start));
        let channels = data.channels.len().max(1);
        let (prod, cons) = rtrb::RingBuffer::new(self.ring_frames * channels);
        let closed = Arc::new(AtomicBool::new(false));
        let mut feed = Feed {
            data,
            next: if reverse {
                (start + frames).saturating_sub(1)
            } else {
                start
            },
            left: frames,
            reverse,
            prod,
            closed: closed.clone(),
        };
        if feed.pump()
            && let Ok(mut feeds) = self.shared.feeds.lock()
        {
            feeds.push(feed);
        }
        Ok(StreamReader {
            cons,
            channels,
            left: frames,
            underruns: 0,
            closed,
        })
    }

    /// Open streams still being filled.
    pub fn active(&self) -> usize {
        self.shared.feeds.lock().map_or(0, |f| f.len())
    }
}

impl Drop for Prefetcher {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The playback node for one playlist track's audio clips (M2/T1). In T0 it only holds its
/// plans and outputs silence; it is not part of the render plan yet.
pub struct AudioClipNode {
    desc: NodeDescriptor,
    plans: Vec<AudioClipPlan>,
}

impl AudioClipNode {
    pub fn new(track: u32, plans: Vec<AudioClipPlan>) -> Self {
        Self {
            desc: NodeDescriptor {
                id: "ph.audio_clips".into(),
                name: format!("Audio clips, track {}", track + 1),
                kind: NodeKind::Utility,
                audio_inputs: Vec::new(),
                audio_outputs: vec![PortInfo {
                    name: "out".into(),
                    channels: 2,
                    sidechain: false,
                }],
                note_input: false,
                params: Vec::new(),
            },
            plans: plans.into_iter().filter(|p| p.track == track).collect(),
        }
    }

    pub fn plans(&self) -> &[AudioClipPlan] {
        &self.plans
    }
}

impl Processor for AudioClipNode {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.desc
    }
    fn activate(&mut self, _: &ActivateInfo) {}
    fn process(&mut self, _: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        silence(io);
        ProcessStatus::Sleep
    }
    fn set_param(&mut self, _: ParamId, _: f64) {}
    fn param(&self, _: ParamId) -> f64 {
        0.0
    }
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Assets(Arc<SampleData>);
    impl AssetResolver for Assets {
        fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
            if r == "samples/ramp.wav" {
                Ok(self.0.clone())
            } else {
                Err(format!("no {r}"))
            }
        }
    }

    /// Stereo: left counts up 0, 1, 2, ...; right is its negative.
    fn ramp(frames: usize) -> Arc<SampleData> {
        let l: Vec<f32> = (0..frames).map(|i| i as f32).collect();
        let r = l.iter().map(|v| -v).collect();
        Arc::new(SampleData {
            sample_rate: 44_100.0,
            channels: vec![l, r],
            loop_range: None,
            root_key: None,
        })
    }

    fn project() -> (Project, ClipId) {
        let mut ed = Edit::new(&Project::empty("a"));
        let s = ed
            .add_sample(SampleEntry {
                id: SampleId(0),
                path: "samples/ramp.wav".into(),
                name: "ramp.wav".into(),
                frames: 1000,
                sample_rate: 44_100,
                channels: 2,
                bpm: Some(SampleBpm {
                    value: 100.0,
                    source: BpmSource::User,
                }),
                root_key: None,
                analysis: Default::default(),
                provenance: Default::default(),
            })
            .unwrap();
        let mut a = AudioClip::new(s);
        a.tempo_sync = true;
        a.gain_db = -6.0;
        a.fade_in = Fade {
            length: PPQ,
            curve: FadeCurve::Linear,
        };
        a.stretch = StretchMode::Stretch;
        a.pitch_semitones = 3;
        a.source_offset = 100;
        let c = ed.add_audio_clip(0, PPQ, 4 * PPQ, a).unwrap();
        (ed.finish().0, c)
    }

    #[test]
    fn plans_carry_units_the_player_needs() {
        let (p, c) = project();
        let tempo = TempoMap::constant(125.0, 48_000);
        let (plans, warnings) = plan_audio_clips(&p, &tempo);
        assert!(warnings.is_empty());
        let pl = &plans[0];
        assert_eq!(pl.clip, c);
        assert_eq!((pl.start, pl.end), (23_040, 115_200));
        assert_eq!(pl.source_start, 100);
        // 125 / 100 BPM, and 44.1 kHz source into a 48 kHz engine
        assert!((pl.speed - 1.25 * 44_100.0 / 48_000.0).abs() < 1e-12);
        assert_eq!(pl.transpose, 3.0);
        assert!((pl.gain - 0.501_187).abs() < 1e-5);
        assert_eq!(pl.fade_in, (23_040, FadeCurve::Linear));
        assert_eq!(pl.fade_gain(0), 0.0);
        assert_eq!(pl.fade_gain(11_520), 0.5);
        assert_eq!(pl.fade_gain(23_040), 1.0);
        assert_eq!(pl.source_frames(1000), 900);
        // no tempo: warned, plays unsynced
        let mut ed = Edit::new(&p);
        ed.set_sample_bpm(SampleId(0), None).unwrap();
        let (plans, warnings) = plan_audio_clips(ed.project(), &tempo);
        assert_eq!(warnings.len(), 1);
        assert!((plans[0].speed - 44_100.0 / 48_000.0).abs() < 1e-12);
        // the node holds its track's plans and is silent until T1
        let node = AudioClipNode::new(0, plans);
        assert_eq!(node.plans().len(), 1);
        assert!(stretcher_for(StretchMode::Stretch).is_err());
    }

    fn read_all(r: &mut StreamReader, block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut rr) = (Vec::new(), Vec::new());
        for _ in 0..10_000 {
            if r.finished() {
                break;
            }
            let (mut a, mut b) = (vec![0.0; block], vec![0.0; block]);
            let n = r.read(&mut [&mut a, &mut b]);
            l.extend_from_slice(&a[..n]);
            rr.extend_from_slice(&b[..n]);
            if n < block {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        (l, rr)
    }

    #[test]
    fn the_prefetcher_streams_forwards_and_backwards() {
        let pf = Prefetcher::start(Arc::new(Assets(ramp(10_000))), 256);
        let mut fwd = pf.open("samples/ramp.wav", 100, 5000, false).unwrap();
        let (l, r) = read_all(&mut fwd, 100);
        assert_eq!(l.len(), 5000);
        assert!(l.iter().enumerate().all(|(i, v)| *v == (100 + i) as f32));
        assert!(r.iter().zip(&l).all(|(a, b)| *a == -b));
        let mut back = pf.open("samples/ramp.wav", 9_990, 50, true).unwrap();
        let (l, _) = read_all(&mut back, 64);
        assert_eq!(l, (0..10).map(|i| (9_999 - i) as f32).collect::<Vec<_>>());
        // reading past the end is silence, not an underrun
        let mut o = [1.0f32; 8];
        assert_eq!(back.read(&mut [&mut o]), 0);
        assert_eq!(o, [0.0; 8]);
        assert_eq!(back.underruns(), 0);
        assert!(pf.open("samples/missing.wav", 0, 10, false).is_err());
        drop((fwd, back));
        for _ in 0..500 {
            if pf.active() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            pf.active(),
            0,
            "closed streams are dropped by the prefetch thread"
        );
    }

    #[test]
    fn a_reader_counts_underruns_when_the_prefetcher_falls_behind() {
        let pf = Prefetcher::start(Arc::new(Assets(ramp(100_000))), 64);
        let mut r = pf.open("samples/ramp.wav", 0, 100_000, false).unwrap();
        // the ring holds 64 frames; asking for 1000 at once must come up short
        let mut a = vec![0.0; 1000];
        let n = r.read(&mut [&mut a]);
        assert!(n <= 64);
        assert_eq!(r.underruns(), 1);
        assert!(a[n..].iter().all(|v| *v == 0.0));
    }
}
