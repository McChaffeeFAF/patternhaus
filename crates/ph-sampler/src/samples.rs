//! How the engine hands the sampler its audio: preloads and streams.
//!
//! # Decision (M3/T0)
//!
//! **Instruments get their own preload type; the M2 streamer gets no preload API.**
//!
//! - [`ZoneSample`] is the preload: the first [`ZoneSample::head`] frames of one sample file,
//!   decoded in memory when the instrument compiles (off the audio thread), so a voice starts
//!   instantly. Its size is the instrument's `preload_frames` (default
//!   [`crate::DEFAULT_PRELOAD_FRAMES`]). A file no longer than that is preloaded whole
//!   ([`ZoneSample::complete`]) and never streams.
//! - The rest of a long file streams through the **existing** M2 block cache
//!   (`ph-engine/src/stream.rs`: seqlocked block slots, one prefetch thread, the
//!   `Live`/`Wait`/`Sync` policies that keep offline renders and the simulated device
//!   bit-identical). `ph-sampler` may not depend on `ph-engine`, so it sees that cache through
//!   the [`StreamSource`] trait, which mirrors the cache's real-time calls exactly
//!   (`want`/`release`/`read`). `ph-engine` implements it over its `Cache` (M3/T2).
//! - A voice calls [`StreamSource::want`] at note-on (from the frame where its preload ends),
//!   reads the preload while it lasts, then reads blocks with [`StreamSource::read`]. A block
//!   that is not there under the `Live` policy is silence and an underrun, as for audio clips.
//!   Preloads are sized so the prefetch thread has filled the first blocks long before a voice
//!   gets there (a 32k-frame preload is 0.68 s at 48 kHz).
//!
//! # What streams (M3/T2)
//!
//! [`load_samples_streamed`] opens each file with `AssetResolver::stream`. A file streams when
//! the resolver can stream it and it is longer than its head: `preload_frames` past the
//! latest zone `start` that plays it. Its head is read into memory and the rest is left to the
//! [`StreamSource`] the host attaches. A file is held **whole** instead when a zone that plays
//! it loops or plays in reverse (loops jump back and reverse starts at the end: neither suits
//! a forward stream), or when the resolver cannot stream it. [`load_samples`] (no streaming)
//! holds every file whole, as M3/T0 did.
//!
//! A voice reads its stream through a window of [`crate::render::WINDOW`] frames that it
//! refills with one [`StreamSource::read`], after telling the source where it is
//! ([`StreamSource::want`]); at note-on it asks for the frame its head ends, so the first
//! blocks are on their way while the head plays.

use ph_core::{HausInstrument, Project};
use ph_node::{AssetResolver, SampleData, SampleStream};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One sample file as the sampler holds it.
#[derive(Clone, Debug)]
pub struct ZoneSample {
    /// The asset reference it was loaded from (for warnings and identity).
    pub reference: String,
    /// The preloaded head of the file (`head.frames()` frames), at the file's own rate.
    pub head: Arc<SampleData>,
    /// Length of the whole file, in frames.
    pub frames: u64,
    /// True when `head` is the whole file: the sample never streams.
    pub complete: bool,
    /// Index of this file in the [`StreamSource`] (when it streams).
    pub stream: Option<usize>,
}

impl ZoneSample {
    /// A sample held whole in memory.
    pub fn whole(reference: &str, data: Arc<SampleData>) -> Self {
        Self {
            reference: reference.into(),
            frames: data.frames() as u64,
            head: data,
            complete: true,
            stream: None,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.head.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.head.channels.len()
    }
}

/// The real-time side of disk streaming, as the engine's block cache provides it. Every
/// method is called on the audio thread: no allocation, locks or I/O (see the module docs).
pub trait StreamSource: Send + Sync {
    /// Voice `voice` reads source `source` around `frame`, moving backwards if `reverse`.
    fn want(&self, voice: usize, source: usize, frame: u64, reverse: bool);
    /// Voice `voice` stopped reading.
    fn release(&self, voice: usize);
    /// Reads frames `start..start + n` of `source` into `out` (`n` = `out[0].len()`; a mono
    /// source fills both). Returns false after an underrun (the frames are then silence).
    fn read(&self, source: usize, start: i64, out: [&mut [f32]; 2]) -> bool;
}

/// Every sample an instrument plays, indexed by [`SampleSet::index`], plus the stream
/// source for the long ones (`None` while everything is preloaded whole).
#[derive(Clone, Default)]
pub struct SampleSet {
    pub samples: Vec<ZoneSample>,
    /// Zone id → index into `samples`; zones whose sample failed to load are absent.
    pub index: BTreeMap<ph_core::ZoneId, usize>,
    pub streams: Option<Arc<dyn StreamSource>>,
}

/// Loads every sample `inst` plays, through `assets` (off the audio thread). Each distinct
/// reference loads once. Samples that fail are reported in the warnings and leave their
/// zones silent. M3/T0 preloads every file whole (see the module docs).
pub fn load_samples(
    project: &Project,
    inst: &HausInstrument,
    assets: &dyn AssetResolver,
) -> (SampleSet, Vec<String>) {
    let mut set = SampleSet::default();
    let mut by_ref: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut warnings = Vec::new();
    for z in inst.zones.values() {
        let Some(reference) = inst.sample_reference(project, &z.sample) else {
            warnings.push(format!(
                "{}: zone {} plays a sample missing from the registry",
                inst.name, z.id
            ));
            continue;
        };
        let slot =
            by_ref
                .entry(reference.clone())
                .or_insert_with(|| match assets.sample(&reference) {
                    Ok(data) => {
                        set.samples.push(ZoneSample::whole(&reference, data));
                        Some(set.samples.len() - 1)
                    }
                    Err(e) => {
                        warnings.push(format!("{}: cannot load `{reference}`: {e}", inst.name));
                        None
                    }
                });
        if let Some(i) = *slot {
            set.index.insert(z.id, i);
        }
    }
    (set, warnings)
}

/// Loads every sample `inst` plays, streaming long files (see the module docs). Off the audio
/// thread. Returns the set (with `streams` unset), the warnings, and the opened streams in the
/// order of [`ZoneSample::stream`]; the host wraps them in a [`StreamSource`] and sets
/// [`SampleSet::streams`] (without one, streamed samples fall silent past their head).
pub fn load_samples_streamed(
    project: &Project,
    inst: &HausInstrument,
    assets: &dyn AssetResolver,
) -> (SampleSet, Vec<String>, Vec<Box<dyn SampleStream>>) {
    let preload = inst
        .settings
        .preload_frames
        .unwrap_or(crate::DEFAULT_PRELOAD_FRAMES) as u64;
    // per reference: the latest zone start, and whether some zone needs it whole
    let mut need: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    for z in inst.zones.values() {
        if let Some(r) = inst.sample_reference(project, &z.sample) {
            let e = need.entry(r).or_insert((0, false));
            e.0 = e.0.max(z.start);
            e.1 |= z.looping.is_some() || z.reverse;
        }
    }
    let mut set = SampleSet::default();
    let mut streams: Vec<Box<dyn SampleStream>> = Vec::new();
    let mut by_ref: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut warnings = Vec::new();
    for z in inst.zones.values() {
        let Some(reference) = inst.sample_reference(project, &z.sample) else {
            warnings.push(format!(
                "{}: zone {} plays a sample missing from the registry",
                inst.name, z.id
            ));
            continue;
        };
        let slot = by_ref.entry(reference.clone()).or_insert_with(|| {
            let (latest, whole) = need.get(&reference).copied().unwrap_or((0, true));
            if !whole
                && let Some(Ok(mut s)) = assets.stream(&reference)
            {
                let frames = s.frames();
                let head = (latest + preload).min(frames);
                if head < frames {
                    let chans = s.channels().clamp(1, 2);
                    let mut data = vec![vec![0.0f32; head as usize]; chans];
                    let ok = {
                        let mut outs: Vec<&mut [f32]> =
                            data.iter_mut().map(|c| &mut c[..]).collect();
                        s.read(0, &mut outs).is_ok()
                    };
                    if ok {
                        let sample_rate = s.sample_rate();
                        streams.push(s);
                        set.samples.push(ZoneSample {
                            reference: reference.clone(),
                            head: Arc::new(SampleData {
                                sample_rate,
                                channels: data,
                                loop_range: None,
                                root_key: None,
                            }),
                            frames,
                            complete: false,
                            stream: Some(streams.len() - 1),
                        });
                        return Some(set.samples.len() - 1);
                    }
                }
            }
            match assets.sample(&reference) {
                Ok(data) => {
                    set.samples.push(ZoneSample::whole(&reference, data));
                    Some(set.samples.len() - 1)
                }
                Err(e) => {
                    warnings.push(format!("{}: cannot load `{reference}`: {e}", inst.name));
                    None
                }
            }
        });
        if let Some(i) = *slot {
            set.index.insert(z.id, i);
        }
    }
    (set, warnings, streams)
}

impl SampleSet {
    /// Bytes of sample data held in memory (heads and whole files).
    pub fn resident_bytes(&self) -> usize {
        self.samples
            .iter()
            .map(|s| s.head.channels.iter().map(|c| c.len() * 4).sum::<usize>())
            .sum()
    }
}
