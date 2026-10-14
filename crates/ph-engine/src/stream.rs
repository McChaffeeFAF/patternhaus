//! Disk streaming for audio clips (M2/T1).
//!
//! Short samples (under [`STREAM_MIN_SECONDS`]) are decoded whole when the plan compiles and
//! read straight from memory. Long ones are read from disk in blocks of [`BLOCK_FRAMES`]
//! into a per-track [`Cache`]: a fixed ring of block slots allocated at compile time.
//!
//! - **Audio thread** (the clip voices): publishes where each voice is reading
//!   ([`Cache::want`]) and copies frames out of ready slots ([`Cache::read`]). It never
//!   blocks, locks, allocates or does I/O under [`StreamPolicy::Live`]: a block that is not
//!   there yet is silence and counts as an **underrun** (`EngineStats::underruns`).
//! - **Prefetch thread** ([`Prefetcher`], one per engine): keeps the blocks around and ahead
//!   of every voice (in its playing direction) loaded, evicting blocks no voice needs.
//!
//! Slots are seqlocks: the writer makes the sequence odd, writes the key and the samples,
//! then makes it even again; a reader copies only when the sequence was even and unchanged
//! around its copy and the key is the block it wants. Samples are stored as `AtomicU32` bit
//! patterns so racing reads are well defined (and discarded). The protocol has a loom
//! model (`loom_models::stream_slot_seqlock`).
//!
//! Offline renders read the same slots under [`StreamPolicy::Sync`]: there is no prefetch
//! thread and a missing block is loaded by the reading thread itself, so offline output
//! never depends on timing. The simulated device ([`crate::Engine::start_simulated`]) uses
//! [`StreamPolicy::Wait`]: the real prefetch thread, but a reader that waits for it, so the
//! oracle tests compare equal data.

use crate::sync::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, fence};
use ph_node::SampleStream;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::{Arc, Mutex, Weak};

/// Frames per cached block.
pub const BLOCK_FRAMES: usize = 8192;
/// Blocks kept loaded ahead of each voice (about 1.4 s at 48 kHz).
pub const LOOKAHEAD_BLOCKS: usize = 8;
/// Sources at least this long are streamed from disk; shorter ones are preloaded whole.
pub const STREAM_MIN_SECONDS: f64 = 10.0;
/// How long a [`StreamPolicy::Wait`] reader waits for the prefetch thread before giving up
/// (and counting an underrun).
const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How a reader treats a block that is not loaded yet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamPolicy {
    /// Real device: never wait; silence and count an underrun.
    Live,
    /// Simulated device: wake the prefetch thread and wait for it (not real-time).
    Wait,
    /// Offline: no prefetch thread; the reading thread loads the block itself.
    Sync,
}

/// One long file, open for reading. Shared by every cache that plays it.
pub(crate) struct Source {
    stream: Mutex<Box<dyn SampleStream>>,
    pub frames: u64,
    pub channels: usize,
    pub sample_rate: f64,
}

impl Source {
    pub fn new(stream: Box<dyn SampleStream>) -> Self {
        Self {
            frames: stream.frames(),
            channels: stream.channels().max(1),
            sample_rate: stream.sample_rate(),
            stream: Mutex::new(stream),
        }
    }

    /// Reads `out[c].len()` frames from `start` (zero past the end or on error).
    fn read(&self, start: u64, out: &mut [&mut [f32]]) {
        let ok = match self.stream.lock() {
            Ok(mut s) => s.read(start, out).is_ok(),
            Err(_) => false,
        };
        if !ok {
            for c in out.iter_mut() {
                c.fill(0.0);
            }
        }
    }
}

const NONE: u64 = u64::MAX;

fn key(source: usize, block: u64) -> u64 {
    ((source as u64) << 40) | block
}

/// One cached block: a seqlock around two channels of `block` frames.
pub(crate) struct Slot {
    seq: AtomicU32,
    key: AtomicU64,
    data: Box<[AtomicU32]>,
}

impl Slot {
    pub(crate) fn new(block: usize) -> Self {
        Self {
            seq: AtomicU32::new(0),
            key: AtomicU64::new(NONE),
            data: (0..2 * block).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    /// The stable key, if no write is in progress.
    fn stable_key(&self) -> Option<u64> {
        let s = self.seq.load(Acquire);
        if s & 1 == 1 {
            return None;
        }
        let k = self.key.load(Relaxed);
        fence(Acquire);
        (self.seq.load(Relaxed) == s).then_some(k)
    }

    /// Single writer: replaces the block with `l`/`r` under the seqlock.
    pub(crate) fn write(&self, k: u64, l: &[f32], r: &[f32]) {
        let s = self.seq.load(Relaxed);
        self.seq.store(s.wrapping_add(1), Relaxed);
        fence(Release);
        self.key.store(k, Relaxed);
        let block = self.data.len() / 2;
        for i in 0..block {
            self.data[i].store(l.get(i).copied().unwrap_or(0.0).to_bits(), Relaxed);
            self.data[block + i].store(r.get(i).copied().unwrap_or(0.0).to_bits(), Relaxed);
        }
        self.seq.store(s.wrapping_add(2), Release);
    }

    /// Copies frames `from..from + n` of block `k` into `out` at `at`; false if the slot does
    /// not hold `k` or was rewritten during the copy (then `out` holds garbage to discard).
    pub(crate) fn copy(
        &self,
        k: u64,
        from: usize,
        n: usize,
        out: [&mut [f32]; 2],
        at: usize,
    ) -> bool {
        let s = self.seq.load(Acquire);
        if s & 1 == 1 || self.key.load(Relaxed) != k {
            return false;
        }
        let block = self.data.len() / 2;
        let [l, r] = out;
        for i in 0..n {
            l[at + i] = f32::from_bits(self.data[from + i].load(Relaxed));
            r[at + i] = f32::from_bits(self.data[block + from + i].load(Relaxed));
        }
        fence(Acquire);
        self.seq.load(Relaxed) == s
    }
}

/// Where one voice reads: a source, a frame, a direction.
struct Want {
    /// Source index + 1; 0 = idle.
    source: AtomicU32,
    frame: AtomicU64,
    reverse: AtomicBool,
}

/// The streaming cache of one audio track (all its voices and long sources).
pub(crate) struct Cache {
    sources: Vec<Arc<Source>>,
    slots: Box<[Slot]>,
    wants: Box<[Want]>,
    block: usize,
    lookahead: usize,
    policy: StreamPolicy,
    underruns: Arc<AtomicU64>,
    /// Sync policy: next slot to evict.
    victim: AtomicUsize,
    waker: Option<Arc<PrefetchShared>>,
}

impl Cache {
    /// A cache for `voices` voices over `sources`. Allocates (control thread).
    pub fn new(
        sources: Vec<Arc<Source>>,
        voices: usize,
        policy: StreamPolicy,
        underruns: Arc<AtomicU64>,
    ) -> Self {
        Self::with_geometry(
            sources,
            voices,
            policy,
            underruns,
            BLOCK_FRAMES,
            LOOKAHEAD_BLOCKS,
        )
    }

    pub fn with_geometry(
        sources: Vec<Arc<Source>>,
        voices: usize,
        policy: StreamPolicy,
        underruns: Arc<AtomicU64>,
        block: usize,
        lookahead: usize,
    ) -> Self {
        let voices = voices.max(1);
        // each voice needs its block, one behind and `lookahead` ahead; plus spare slots
        let n = voices * (lookahead + 2) + 2;
        Self {
            sources,
            slots: (0..n).map(|_| Slot::new(block)).collect(),
            wants: (0..voices)
                .map(|_| Want {
                    source: AtomicU32::new(0),
                    frame: AtomicU64::new(0),
                    reverse: AtomicBool::new(false),
                })
                .collect(),
            block,
            lookahead,
            policy,
            underruns,
            victim: AtomicUsize::new(0),
            waker: None,
        }
    }

    /// Real-time: voice `v` reads `source` around `frame`, moving backwards if `reverse`.
    pub fn want(&self, v: usize, source: usize, frame: u64, reverse: bool) {
        if let Some(w) = self.wants.get(v) {
            w.frame.store(frame, Relaxed);
            w.reverse.store(reverse, Relaxed);
            w.source.store(source as u32 + 1, Release);
        }
    }

    /// Real-time: voice `v` stopped reading.
    pub fn release(&self, v: usize) {
        if let Some(w) = self.wants.get(v) {
            w.source.store(0, Release);
        }
    }

    fn find(&self, k: u64) -> Option<&Slot> {
        self.slots.iter().find(|s| s.stable_key() == Some(k))
    }

    /// Reads frames `start..start + n` of `source` into `out` (`n` = `out[0].len()`); a mono
    /// source fills both channels, frames outside the file are zero. Real-time under
    /// [`StreamPolicy::Live`]. Returns false after an underrun.
    pub fn read(&self, source: usize, start: i64, out: [&mut [f32]; 2]) -> bool {
        let [l, r] = out;
        let n = l.len().min(r.len());
        let Some(src) = self.sources.get(source) else {
            l[..n].fill(0.0);
            r[..n].fill(0.0);
            return true;
        };
        let mut ok = true;
        let mut i = 0usize;
        while i < n {
            let f = start + i as i64;
            if f < 0 || f as u64 >= src.frames {
                // outside the file: silence up to the file (or the end of the request)
                let next = if f < 0 {
                    (n as i64).min(-start) as usize
                } else {
                    n
                };
                l[i..next].fill(0.0);
                r[i..next].fill(0.0);
                i = next;
                continue;
            }
            let b = f as u64 / self.block as u64;
            let off = (f as u64 % self.block as u64) as usize;
            let in_file = (src.frames - f as u64) as usize;
            let k_n = (self.block - off).min(n - i).min(in_file);
            let k = key(source, b);
            if !self.copy_block(source, b, k, off, k_n, [&mut *l, &mut *r], i) {
                l[i..i + k_n].fill(0.0);
                r[i..i + k_n].fill(0.0);
                ok = false;
            }
            i += k_n;
        }
        if !ok {
            self.underruns.fetch_add(1, Relaxed);
        }
        ok
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_block(
        &self,
        source: usize,
        b: u64,
        k: u64,
        off: usize,
        n: usize,
        out: [&mut [f32]; 2],
        at: usize,
    ) -> bool {
        let [l, r] = out;
        if let Some(slot) = self.find(k)
            && slot.copy(k, off, n, [&mut *l, &mut *r], at)
        {
            return true;
        }
        match self.policy {
            StreamPolicy::Live => false,
            StreamPolicy::Sync => {
                self.load_sync(source, b);
                self.find(k)
                    .is_some_and(|s| s.copy(k, off, n, [&mut *l, &mut *r], at))
            }
            StreamPolicy::Wait => {
                let t0 = std::time::Instant::now();
                loop {
                    if let Some(w) = &self.waker {
                        w.wake();
                    }
                    if let Some(slot) = self.find(k)
                        && slot.copy(k, off, n, [&mut *l, &mut *r], at)
                    {
                        return true;
                    }
                    if t0.elapsed() > WAIT_LIMIT {
                        return false;
                    }
                    std::thread::yield_now();
                }
            }
        }
    }

    /// Sync policy: the reading thread loads block `b` itself into the next slot in turn
    /// (no other writer exists). Uses a stack-free path: the slot is filled straight from
    /// the source through a fixed-size bounce buffer.
    fn load_sync(&self, source: usize, b: u64) {
        let i = self.victim.fetch_add(1, Relaxed) % self.slots.len();
        let mut bounce = [[0.0f32; SYNC_BOUNCE]; 2];
        let slot = &self.slots[i];
        let src = &self.sources[source];
        let s = slot.seq.load(Relaxed);
        slot.seq.store(s.wrapping_add(1), Relaxed);
        fence(Release);
        slot.key.store(key(source, b), Relaxed);
        let start = b * self.block as u64;
        let mut done = 0usize;
        while done < self.block {
            let n = (self.block - done).min(SYNC_BOUNCE);
            let [bl, br] = &mut bounce;
            read_pair(src, start + done as u64, &mut bl[..n], &mut br[..n]);
            for j in 0..n {
                slot.data[done + j].store(bl[j].to_bits(), Relaxed);
                slot.data[self.block + done + j].store(br[j].to_bits(), Relaxed);
            }
            done += n;
        }
        slot.seq.store(s.wrapping_add(2), Release);
    }

    /// Prefetch thread: loads every block a voice needs that is missing, nearest first.
    /// `l`/`r` are the thread's scratch buffers (at least one block long).
    fn fill(&self, l: &mut [f32], r: &mut [f32]) {
        let mut needed: Vec<(u64, u64)> = Vec::new(); // (distance rank, key)
        for w in self.wants.iter() {
            let s = w.source.load(Acquire);
            if s == 0 {
                continue;
            }
            let source = (s - 1) as usize;
            let Some(src) = self.sources.get(source) else {
                continue;
            };
            let blocks = src.frames.div_ceil(self.block as u64);
            let b0 = w.frame.load(Relaxed) / self.block as u64;
            let reverse = w.reverse.load(Relaxed);
            for d in 0..=(self.lookahead as i64 + 1) {
                // one block behind the direction of play, then ahead
                let step = if d == 0 {
                    0
                } else if d == 1 {
                    -1
                } else {
                    d - 1
                };
                let b = if reverse {
                    b0 as i64 - step
                } else {
                    b0 as i64 + step
                };
                if b >= 0 && (b as u64) < blocks {
                    needed.push((d as u64, key(source, b as u64)));
                }
            }
        }
        needed.sort_unstable();
        let keys: Vec<u64> = needed.iter().map(|n| n.1).collect();
        for &(_, k) in &needed {
            if self.find(k).is_some() {
                continue;
            }
            // a slot holding nothing needed (or nothing at all)
            let Some(slot) = self.slots.iter().find(|s| match s.stable_key() {
                Some(NONE) => true,
                Some(other) => !keys.contains(&other),
                None => false,
            }) else {
                return;
            };
            let source = (k >> 40) as usize;
            let b = k & ((1 << 40) - 1);
            let n = self.block;
            read_pair(
                &self.sources[source],
                b * n as u64,
                &mut l[..n],
                &mut r[..n],
            );
            slot.write(k, &l[..n], &r[..n]);
        }
    }
}

/// Bounce buffer of a synchronous load, in frames.
const SYNC_BOUNCE: usize = 1024;

/// Reads a stereo pair from `src` (a mono source fills both).
fn read_pair(src: &Source, start: u64, l: &mut [f32], r: &mut [f32]) {
    if src.channels == 1 {
        src.read(start, &mut [&mut *l]);
        r.copy_from_slice(l);
    } else {
        src.read(start, &mut [&mut *l, &mut *r]);
    }
}

/// State shared between the prefetch thread and its owner.
pub(crate) struct PrefetchShared {
    caches: Mutex<Vec<Weak<Cache>>>,
    stop: AtomicBool,
    thread: std::sync::OnceLock<std::thread::Thread>,
}

impl PrefetchShared {
    /// Wakes the prefetch thread (real-time safe: no allocation, no lock).
    pub fn wake(&self) {
        if let Some(t) = self.thread.get() {
            t.unpark();
        }
    }
}

/// The prefetch thread. One per engine, started on first use.
pub struct Prefetcher {
    shared: Arc<PrefetchShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Default for Prefetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Prefetcher {
    /// A prefetcher whose thread starts with the first registered cache.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(PrefetchShared {
                caches: Mutex::new(Vec::new()),
                stop: AtomicBool::new(false),
                thread: std::sync::OnceLock::new(),
            }),
            thread: None,
        }
    }

    /// Hands a new cache to the thread (control thread; allocates).
    pub(crate) fn register(&mut self, mut cache: Cache) -> Arc<Cache> {
        cache.waker = Some(self.shared.clone());
        let cache = Arc::new(cache);
        if let Ok(mut list) = self.shared.caches.lock() {
            list.retain(|w| w.strong_count() > 0);
            list.push(Arc::downgrade(&cache));
        }
        if self.thread.is_none() {
            let s = self.shared.clone();
            self.thread = std::thread::Builder::new()
                .name("ph-prefetch".into())
                .spawn(move || run(s))
                .ok();
        }
        self.shared.wake();
        cache
    }

    /// Caches still alive.
    pub fn active(&self) -> usize {
        self.shared
            .caches
            .lock()
            .map_or(0, |l| l.iter().filter(|w| w.strong_count() > 0).count())
    }
}

fn run(s: Arc<PrefetchShared>) {
    let _ = s.thread.set(std::thread::current());
    let mut l = vec![0.0f32; BLOCK_FRAMES];
    let mut r = vec![0.0f32; BLOCK_FRAMES];
    while !s.stop.load(Acquire) {
        let live: Vec<Arc<Cache>> = match s.caches.lock() {
            Ok(list) => list.iter().filter_map(Weak::upgrade).collect(),
            Err(_) => Vec::new(),
        };
        for c in &live {
            if l.len() < c.block {
                l.resize(c.block, 0.0);
                r.resize(c.block, 0.0);
            }
            c.fill(&mut l, &mut r);
        }
        // drop the strong references here, off the audio thread
        drop(live);
        std::thread::park_timeout(std::time::Duration::from_millis(2));
    }
}

impl Drop for Prefetcher {
    fn drop(&mut self) {
        self.shared.stop.store(true, Release);
        self.shared.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;
    use ph_node::StreamError;

    /// Left = frame index, right = −index; optionally slow (sleeps per read).
    pub(crate) struct Ramp {
        pub frames: u64,
        pub slow: std::time::Duration,
    }

    impl SampleStream for Ramp {
        fn frames(&self) -> u64 {
            self.frames
        }
        fn sample_rate(&self) -> f64 {
            48_000.0
        }
        fn channels(&self) -> usize {
            2
        }
        fn read(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, StreamError> {
            if !self.slow.is_zero() {
                std::thread::sleep(self.slow);
            }
            let n = out[0].len();
            for i in 0..n {
                let f = start + i as u64;
                let v = if f < self.frames { f as f32 } else { 0.0 };
                out[0][i] = v;
                if out.len() > 1 {
                    out[1][i] = -v;
                }
            }
            Ok(n.min(self.frames.saturating_sub(start) as usize))
        }
    }

    fn src(frames: u64, slow_ms: u64) -> Arc<Source> {
        Arc::new(Source::new(Box::new(Ramp {
            frames,
            slow: std::time::Duration::from_millis(slow_ms),
        })))
    }

    fn check(l: &[f32], r: &[f32], start: i64, frames: u64) {
        for i in 0..l.len() {
            let f = start + i as i64;
            let want = if f >= 0 && (f as u64) < frames {
                f as f32
            } else {
                0.0
            };
            assert_eq!((l[i], r[i]), (want, -want), "frame {f}");
        }
    }

    #[test]
    fn sync_reads_any_range_exactly() {
        let u = Arc::new(AtomicU64::new(0));
        let c = Cache::with_geometry(
            vec![src(10_000, 0)],
            1,
            StreamPolicy::Sync,
            u.clone(),
            256,
            2,
        );
        for (start, n) in [
            (-100i64, 300usize),
            (0, 256),
            (1000, 2000),
            (9_900, 300),
            (20_000, 10),
        ] {
            let (mut l, mut r) = (vec![7.0; n], vec![7.0; n]);
            assert!(c.read(0, start, [&mut l, &mut r]));
            check(&l, &r, start, 10_000);
        }
        assert_eq!(u.load(Relaxed), 0);
    }

    #[test]
    fn the_prefetcher_follows_voices_forwards_and_backwards() {
        let u = Arc::new(AtomicU64::new(0));
        let mut pf = Prefetcher::new();
        let c = pf.register(Cache::with_geometry(
            vec![src(100_000, 0)],
            2,
            StreamPolicy::Wait,
            u.clone(),
            512,
            4,
        ));
        // forwards from 0 and backwards from the end, 300 frames at a time
        let (mut l, mut r) = (vec![0.0; 300], vec![0.0; 300]);
        let mut fwd = 0i64;
        let mut back = 99_700i64;
        for _ in 0..200 {
            c.want(0, 0, fwd as u64, false);
            c.want(1, 0, back as u64, true);
            assert!(c.read(0, fwd, [&mut l, &mut r]));
            check(&l, &r, fwd, 100_000);
            assert!(c.read(0, back, [&mut l, &mut r]));
            check(&l, &r, back, 100_000);
            fwd += 300;
            back -= 300;
        }
        assert_eq!(u.load(Relaxed), 0);
        assert_eq!(pf.active(), 1);
        drop(c);
        // the prefetch thread may hold it for one more pass
        for _ in 0..500 {
            if pf.active() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(pf.active(), 0, "dropped caches leave the prefetch thread");
    }

    #[test]
    fn a_live_reader_never_waits_and_counts_underruns() {
        let u = Arc::new(AtomicU64::new(0));
        let mut pf = Prefetcher::new();
        // every block read takes 50 ms: the reader is always ahead of the disk
        let c = pf.register(Cache::with_geometry(
            vec![src(1_000_000, 50)],
            1,
            StreamPolicy::Live,
            u.clone(),
            512,
            2,
        ));
        let (mut l, mut r) = (vec![1.0; 256], vec![1.0; 256]);
        c.want(0, 0, 500_000, false);
        let t0 = std::time::Instant::now();
        assert!(!c.read(0, 500_000, [&mut l, &mut r]));
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(20),
            "the reader waited"
        );
        assert!(
            l.iter().chain(&r).all(|v| *v == 0.0),
            "missing data is silence"
        );
        assert_eq!(u.load(Relaxed), 1);
        // given time, the prefetcher catches up
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if c.read(0, 500_000, [&mut l, &mut r]) {
                check(&l, &r, 500_000, 1_000_000);
                return;
            }
        }
        panic!("the prefetcher never delivered");
    }
}
