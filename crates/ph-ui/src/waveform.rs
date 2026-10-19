//! Waveforms for the playlist and the audio editor (M2/T6).
//!
//! # Peaks
//!
//! A file is decoded **off the UI thread** and reduced to a pyramid of min/max pairs: level 0
//! has one pair per [`BASE_BLOCK`] frames, every next level four times coarser. Drawing picks
//! the level whose blocks are no wider than one pixel, so a column costs a handful of lookups
//! at any zoom and nothing is ever read from the file while painting. The pyramid is the
//! per-zoom-level cache; [`ColumnCache`] adds the finished columns of a drawn clip, so a still
//! frame repaints without even those lookups.
//!
//! # Loading
//!
//! [`WaveCache::request`] starts at most [`MAX_LOADERS`] worker threads; finished files are
//! collected by [`WaveCache::poll`] (once per frame) and a repaint is requested when one
//! arrives. The editor asks for the decoded samples as well (`keep_data`).

use ph_node::SampleData;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Frames per bin on level 0.
pub const BASE_BLOCK: usize = 64;
const LEVEL_FACTOR: usize = 4;
/// Files decoded at the same time.
pub const MAX_LOADERS: usize = 2;

/// One level of the pyramid: per channel, the min and max of every `block` frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub block: usize,
    pub min: Vec<Vec<f32>>,
    pub max: Vec<Vec<f32>>,
}

impl Level {
    pub fn bins(&self) -> usize {
        self.min.first().map_or(0, Vec::len)
    }
}

/// The min/max pyramid of one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Peaks {
    pub sample_rate: u32,
    pub frames: usize,
    pub channels: usize,
    pub levels: Vec<Level>,
}

impl Peaks {
    pub fn build(channels: &[Vec<f32>], sample_rate: u32) -> Peaks {
        let frames = channels.first().map_or(0, Vec::len);
        let mut levels = Vec::new();
        let mut base = Level {
            block: BASE_BLOCK,
            min: Vec::new(),
            max: Vec::new(),
        };
        for ch in channels {
            let (mut mn, mut mx) = (Vec::new(), Vec::new());
            for chunk in ch.chunks(BASE_BLOCK) {
                let (a, b) = chunk
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
                mn.push(a);
                mx.push(b);
            }
            base.min.push(mn);
            base.max.push(mx);
        }
        levels.push(base);
        while levels.last().is_some_and(|l| l.bins() > 8) {
            let prev = levels.last().unwrap();
            let mut next = Level {
                block: prev.block * LEVEL_FACTOR,
                min: Vec::new(),
                max: Vec::new(),
            };
            for c in 0..prev.min.len() {
                next.min.push(
                    prev.min[c]
                        .chunks(LEVEL_FACTOR)
                        .map(|k| k.iter().copied().fold(f32::MAX, f32::min))
                        .collect(),
                );
                next.max.push(
                    prev.max[c]
                        .chunks(LEVEL_FACTOR)
                        .map(|k| k.iter().copied().fold(f32::MIN, f32::max))
                        .collect(),
                );
            }
            levels.push(next);
        }
        Peaks {
            sample_rate,
            frames,
            channels: channels.len(),
            levels,
        }
    }

    /// The level to use for `frames_per_pixel`: the coarsest whose blocks fit in a pixel.
    pub fn level_for(&self, frames_per_pixel: f64) -> usize {
        let mut best = 0;
        for (i, l) in self.levels.iter().enumerate() {
            if (l.block as f64) <= frames_per_pixel.max(1.0) {
                best = i;
            }
        }
        best
    }

    /// Min and max over frames `f0..f1` (clamped to the file) on `level`, over the channels in
    /// `chans` (`None` = all of them mixed as an envelope). `None` outside the file.
    pub fn range(
        &self,
        level: usize,
        f0: f64,
        f1: f64,
        chans: Option<usize>,
    ) -> Option<(f32, f32)> {
        let l = self.levels.get(level)?;
        if f1 <= 0.0 || f0 >= self.frames as f64 || l.bins() == 0 {
            return None;
        }
        let b0 = (f0.max(0.0) / l.block as f64).floor() as usize;
        let b1 = ((f1 / l.block as f64).ceil() as usize)
            .min(l.bins())
            .max(b0 + 1);
        let (mut mn, mut mx) = (f32::MAX, f32::MIN);
        let range = match chans {
            Some(c) => c..c + 1,
            None => 0..l.min.len(),
        };
        for c in range {
            if let (Some(a), Some(b)) = (
                l.min[c].get(b0..b1.min(l.min[c].len())),
                l.max[c].get(b0..b1.min(l.max[c].len())),
            ) {
                for (lo, hi) in a.iter().zip(b) {
                    mn = mn.min(*lo);
                    mx = mx.max(*hi);
                }
            }
        }
        (mn <= mx).then_some((mn, mx))
    }

    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }
}

/// What a worker produced.
pub struct Loaded {
    pub peaks: Peaks,
    /// The decoded samples, when the editor asked for them.
    pub data: Option<Arc<SampleData>>,
}

/// Where references resolve (set from the app's paths; updated when another project opens).
#[derive(Clone, Debug, Default)]
pub struct Roots {
    pub project_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub library_roots: Vec<PathBuf>,
}

impl Roots {
    fn resolve(&self, reference: &str) -> Result<PathBuf, String> {
        let mut r = ph_engine::assets::FileResolver::new()
            .with_project_dir(&self.project_dir)
            .with_assets_dir(&self.assets_dir);
        for root in &self.library_roots {
            r = r.allow_root(root);
        }
        r.resolve_path(reference)
    }
}

type Done = (String, bool, Result<Loaded, String>);

/// Decoded-in-the-background peaks (and, on request, samples) by asset reference.
pub struct WaveCache {
    entries: HashMap<String, Result<Arc<Loaded>, String>>,
    inflight: HashSet<String>,
    queue: VecDeque<(String, bool)>,
    running: usize,
    tx: Sender<Done>,
    rx: Mutex<Receiver<Done>>,
    wake: Option<egui::Context>,
    pub roots: Roots,
    /// Bumped whenever a file arrives, so cached columns know to redraw.
    pub generation: u64,
}

impl WaveCache {
    pub fn new(roots: Roots, wake: Option<egui::Context>) -> Self {
        let (tx, rx) = channel();
        Self {
            entries: HashMap::new(),
            inflight: HashSet::new(),
            queue: VecDeque::new(),
            running: 0,
            tx,
            rx: Mutex::new(rx),
            wake,
            roots,
            generation: 0,
        }
    }

    /// The loaded peaks, if they are ready. Starts loading the file the first time.
    pub fn get(&mut self, reference: &str) -> Option<Arc<Loaded>> {
        self.request(reference, false)
    }

    /// As [`WaveCache::get`]; with `keep_data` the decoded samples must be there too (peaks
    /// that are already loaded stay usable until they are).
    pub fn request(&mut self, reference: &str, keep_data: bool) -> Option<Arc<Loaded>> {
        let have = match self.entries.get(reference) {
            Some(Ok(l)) => Some(l.clone()),
            _ => None,
        };
        if let Some(l) = &have
            && (!keep_data || l.data.is_some())
        {
            return have;
        }
        let failed = matches!(self.entries.get(reference), Some(Err(_)));
        if !failed && !self.inflight.contains(reference) {
            self.inflight.insert(reference.to_string());
            self.queue.push_back((reference.to_string(), keep_data));
            self.pump();
        }
        have
    }

    /// Why a file could not be loaded.
    pub fn failure(&self, reference: &str) -> Option<&str> {
        match self.entries.get(reference) {
            Some(Err(e)) => Some(e),
            _ => None,
        }
    }

    pub fn is_ready(&self, reference: &str) -> bool {
        matches!(self.entries.get(reference), Some(Ok(_)))
    }

    /// Files being decoded or waiting to be.
    pub fn busy(&self) -> usize {
        self.inflight.len()
    }

    /// Forget a file (it was replaced on disk).
    pub fn forget(&mut self, reference: &str) {
        self.entries.remove(reference);
    }

    fn pump(&mut self) {
        while self.running < MAX_LOADERS
            && let Some((reference, keep)) = self.queue.pop_front()
        {
            let roots = self.roots.clone();
            let tx = self.tx.clone();
            let wake = self.wake.clone();
            self.running += 1;
            let r2 = reference.clone();
            let spawned = std::thread::Builder::new()
                .name("ph-waveform".into())
                .spawn(move || {
                    let r = (|| {
                        let path = roots.resolve(&reference)?;
                        let d = ph_formats::decode::decode_file(&path)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        let peaks = Peaks::build(&d.channels, d.sample_rate.round() as u32);
                        Ok(Loaded {
                            peaks,
                            data: keep.then(|| Arc::new(d)),
                        })
                    })();
                    let _ = tx.send((reference, keep, r));
                    if let Some(c) = wake {
                        c.request_repaint();
                    }
                });
            if spawned.is_err() {
                self.running -= 1;
                self.inflight.remove(&r2);
                self.entries
                    .insert(r2, Err("could not start a loader thread".into()));
            }
        }
    }

    /// Collects finished files. Call once per frame; returns whether anything arrived.
    pub fn poll(&mut self) -> bool {
        let mut got = false;
        let items: Vec<Done> = {
            let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
            rx.try_iter().collect()
        };
        for (reference, _keep, r) in items {
            self.running = self.running.saturating_sub(1);
            self.inflight.remove(&reference);
            got = true;
            self.entries.insert(reference, r.map(Arc::new));
        }
        if got {
            self.generation += 1;
        }
        self.pump();
        got
    }
}

/// The waveform cache the app and its panels share.
pub type SharedWaves = Arc<Mutex<WaveCache>>;

/// Finished columns of drawn waveforms, so a still frame does not recompute them.
#[derive(Default)]
pub struct ColumnCache {
    map: HashMap<u64, (u64, Vec<(f32, f32)>)>,
}

impl ColumnCache {
    /// The columns for `id` under `key`, computed by `make` when the key changed.
    pub fn get(
        &mut self,
        id: u64,
        key: u64,
        make: impl FnOnce() -> Vec<(f32, f32)>,
    ) -> &[(f32, f32)] {
        let e = self.map.entry(id).or_insert_with(|| (u64::MAX, Vec::new()));
        if e.0 != key {
            *e = (key, make());
        }
        &e.1
    }

    /// Drops what no clip uses any more.
    pub fn retain(&mut self, live: impl Fn(u64) -> bool) {
        self.map.retain(|id, _| live(*id));
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A hash of `f64`s and integers for [`ColumnCache`] keys.
pub fn key_of(parts: &[u64]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        h ^= *p;
        h = h.wrapping_mul(0x100_0000_01b3);
        h ^= h >> 29;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.01).sin()).collect()
    }

    #[test]
    fn pyramid_levels_agree_with_the_raw_samples() {
        let x = sine(100_000);
        let p = Peaks::build(&[x.clone(), x.iter().map(|v| -v).collect()], 48_000);
        assert_eq!(p.channels, 2);
        assert!(p.levels.len() >= 4);
        for w in p.levels.windows(2) {
            assert_eq!(w[1].block, w[0].block * 4);
        }
        // the coarsest level's single range is the file's min/max
        let top = p.levels.len() - 1;
        let (mn, mx) = p.range(top, 0.0, p.frames as f64, Some(0)).unwrap();
        let raw_max = x.iter().copied().fold(f32::MIN, f32::max);
        let raw_min = x.iter().copied().fold(f32::MAX, f32::min);
        assert_eq!((mn, mx), (raw_min, raw_max));
        // a fine range matches the samples it covers (block aligned)
        let (mn, mx) = p.range(0, 6400.0, 6464.0, Some(0)).unwrap();
        let slice = &x[6400..6464];
        assert_eq!(mx, slice.iter().copied().fold(f32::MIN, f32::max));
        assert_eq!(mn, slice.iter().copied().fold(f32::MAX, f32::min));
        // outside the file there is nothing
        assert!(p.range(0, 200_000.0, 300_000.0, None).is_none());
        assert!(p.range(0, -50.0, -1.0, None).is_none());
    }

    #[test]
    fn level_choice_follows_the_zoom() {
        let p = Peaks::build(&[sine(200_000)], 48_000);
        assert_eq!(p.level_for(1.0), 0);
        assert_eq!(p.level_for(63.0), 0);
        assert_eq!(p.levels[p.level_for(300.0)].block, 256);
        assert_eq!(p.levels[p.level_for(1100.0)].block, 1024);
        assert_eq!(p.level_for(1e12), p.levels.len() - 1);
    }

    #[test]
    fn column_cache_recomputes_only_when_the_key_changes() {
        let mut c = ColumnCache::default();
        let mut calls = 0;
        for key in [1u64, 1, 1, 2, 2] {
            c.get(7, key, || {
                calls += 1;
                vec![(0.0, 1.0)]
            });
        }
        assert_eq!(calls, 2);
        c.retain(|_| false);
        assert!(c.is_empty());
        assert_ne!(key_of(&[1, 2]), key_of(&[2, 1]));
    }
}
