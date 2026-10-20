//! Recording and input monitoring (M2/T8): the audio-thread side ([`Rig`]), the writer thread
//! ([`Service`]) and the control side ([`RecCtl`]).
//!
//! # Threads and buffers
//!
//! ```text
//!  control thread --RigMsg ring--> audio thread (Rig) --f32 ring (one per take)--> writer thread
//!                 <------------ Trash ring (freed off the audio thread) <---------- (drops it)
//! ```
//!
//! - The **audio thread** owns the opened [`InputSource`]s. Once per chunk it reads the
//!   armed inputs that need reading (monitored, or capturing), adds monitored audio to the
//!   target insert through the chunk context (`Feeds`, summed into the insert's input), and
//!   pushes the frames to capture into the take's ring as interleaved `f32`. It allocates
//!   nothing and never waits: when the ring is full the frames are dropped and counted.
//! - The **writer thread** (`ph-record-writer`) pops the rings every few milliseconds and
//!   appends the samples to the take's WAV (32-bit float), refreshing the header about once a
//!   second so a crash leaves a playable file. It also frees everything the audio thread
//!   retires (`Trash`): sources, finished captures and start packets.
//! - The **control thread** talks to the audio thread through one `rtrb` ring of [`RigMsg`]s.
//!   The boxes inside were built here, so the audio thread only moves pointers.
//!
//! # Alignment and latency
//!
//! A chunk of `n` frames starting at transport sample `a` reads `n` input frames. Those frames
//! left the instrument about `latency` frames ago (device buffering on the way in, the
//! output buffering the performer heard the click through, plus anything queued in the input
//! ring), so the take's first frame belongs `latency` frames *before* the position it was
//! captured at. The take is stored untrimmed and the clip plays from `source_offset =
//! latency_frames`, which puts the sound back on the beat without destroying audio.
//!
//! # Count-in and punch
//!
//! A start with a count-in holds the transport and clicks the metronome for the count-in
//! length, then plays. A punch range captures only the frames of a chunk that fall inside it
//! (sample accurate; chunks are not split), and punching out ends the capture while
//! monitoring goes on.

use crate::record::*;
use ph_core::Tick;
use std::io::BufWriter;
use std::sync::atomic::Ordering::*;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Capture ring per take: seconds of audio the writer thread may lag by.
const RING_SECONDS: f64 = 3.0;
/// Messages in flight from the control thread.
const MSG_RING: usize = 64;
/// Retired boxes in flight to the writer thread.
const TRASH_RING: usize = 64;
/// The writer refreshes the WAV header at least this often.
const FLUSH_EVERY: Duration = Duration::from_millis(1000);
/// How long `record_stop` waits for the writer to finish a file.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);
/// A click lasts this long (seconds).
const CLICK_SECONDS: f64 = 0.03;

// ------------------------------------------------------------------------------ shared

/// What the audio thread publishes about a take, read by the control and writer threads.
pub(crate) struct TakeShared {
    /// Transport sample of the first captured frame; `i64::MIN` until one is.
    pub start_sample: AtomicI64,
    /// Frames pushed to the ring.
    pub frames: AtomicU64,
    /// Frames that did not fit the ring.
    pub dropped: AtomicU64,
    /// Latency compensation in frames, fixed when capturing starts.
    pub latency: AtomicU32,
    /// The audio thread pushed its last frame.
    pub done: AtomicBool,
}

impl TakeShared {
    fn new() -> Self {
        Self {
            start_sample: AtomicI64::new(i64::MIN),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            latency: AtomicU32::new(0),
            done: AtomicBool::new(false),
        }
    }
}

/// Lock-free figures between the audio thread and the control side.
pub(crate) struct RigShared {
    /// 0 idle, 1 count-in, 2 recording.
    pub phase: AtomicU8,
    /// Peak of each armed slot since the last read (f32 bits; non-negative, so the integer
    /// order is the float order).
    pub peaks: [AtomicU32; MAX_INPUTS],
    /// Frames the first capturing slot has pushed.
    pub frames: AtomicU64,
    /// Output buffering in frames, measured by the output stream (0 when unknown).
    pub out_latency: AtomicU32,
    /// Input latency of each slot, as the source reported it when it was last read.
    pub in_latency: [AtomicU32; MAX_INPUTS],
}

impl RigShared {
    fn new() -> Self {
        Self {
            phase: AtomicU8::new(0),
            peaks: Default::default(),
            frames: AtomicU64::new(0),
            out_latency: AtomicU32::new(0),
            in_latency: Default::default(),
        }
    }
}

// ------------------------------------------------------------------------------ messages

pub(crate) struct Capture {
    tx: rtrb::Producer<f32>,
    shared: Arc<TakeShared>,
    started: bool,
}

pub(crate) struct ArmPacket {
    pub slot: u8,
    /// Moved into its slot by the audio thread (the emptied packet is retired).
    pub source: Option<Box<dyn InputSource>>,
    pub monitor: bool,
    /// The mixer insert the monitored audio is added to.
    pub insert: u32,
}

pub(crate) struct StartPacket {
    /// Samples of count-in (0 = none), the beat length for its clicks and beats per bar.
    pub count_in: i64,
    pub beat: i64,
    pub per_bar: u32,
    /// Timeline samples; capturing happens only inside.
    pub punch: Option<(i64, i64)>,
    pub captures: [Option<Capture>; MAX_INPUTS],
    /// Replaces the measured latency (frames).
    pub latency_override: Option<u32>,
}

pub(crate) enum RigMsg {
    Arm(Box<ArmPacket>),
    Disarm(u8),
    Monitor(u8, bool),
    Start(Box<StartPacket>),
    Stop,
}

/// Things the audio thread retires; freed on the writer thread (held only to be dropped).
#[allow(dead_code)]
pub(crate) enum Trash {
    Source(Box<dyn InputSource>),
    Capture(Capture),
    Start(Box<StartPacket>),
    Arm(Box<ArmPacket>),
}

/// What the runtime has to do to the transport after the rig handled its messages.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Transition {
    None,
    /// Hold the transport and count in.
    CountIn,
    /// Play from here.
    Roll,
}

// ------------------------------------------------------------------------------ audio side

/// Monitored audio for one insert in the current chunk.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Feed {
    pub insert: u32,
    pub l: *const f32,
    pub r: *const f32,
}

/// The monitor feeds of a chunk: pointers into the rig's buffers, valid for the chunk.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Feeds {
    pub n: usize,
    pub items: [Feed; MAX_INPUTS],
}

// SAFETY: the pointers refer to buffers owned by the runtime, which outlive the chunk; they
// are only read while the graph runs.
unsafe impl Send for Feeds {}
unsafe impl Sync for Feeds {}

impl Feeds {
    pub const NONE: Feeds = Feeds {
        n: 0,
        items: [Feed {
            insert: u32::MAX,
            l: std::ptr::null(),
            r: std::ptr::null(),
        }; MAX_INPUTS],
    };
}

struct Slot {
    source: Option<Box<dyn InputSource>>,
    monitor: bool,
    insert: u32,
    channels: usize,
    scratch: [Vec<f32>; 2],
    cap: Option<Capture>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Phase {
    Idle,
    CountIn,
    Rolling,
}

/// Control-side ends of the rig's rings.
pub(crate) struct RigHandles {
    pub msgs: rtrb::Producer<RigMsg>,
    pub trash: rtrb::Consumer<Trash>,
    pub shared: Arc<RigShared>,
}

pub(crate) struct Rig {
    rx: rtrb::Consumer<RigMsg>,
    trash: rtrb::Producer<Trash>,
    slots: [Slot; MAX_INPUTS],
    feed_bufs: [[Vec<f32>; 2]; MAX_INPUTS],
    feed_insert: [u32; MAX_INPUTS],
    phase: Phase,
    count_total: i64,
    count_left: i64,
    beat: i64,
    per_bar: u32,
    punch: Option<(i64, i64)>,
    latency_override: Option<u32>,
    pub shared: Arc<RigShared>,
    handles: Option<RigHandles>,
}

impl Rig {
    pub fn new(max_frames: usize) -> Self {
        let (tx, rx) = rtrb::RingBuffer::new(MSG_RING);
        let (trash_tx, trash_rx) = rtrb::RingBuffer::new(TRASH_RING);
        let shared = Arc::new(RigShared::new());
        let n = max_frames.max(1);
        let slot = || Slot {
            source: None,
            monitor: false,
            insert: 0,
            channels: 1,
            scratch: [vec![0.0; n], vec![0.0; n]],
            cap: None,
        };
        Self {
            rx,
            trash: trash_tx,
            slots: [slot(), slot(), slot(), slot()],
            feed_bufs: std::array::from_fn(|_| [vec![0.0; n], vec![0.0; n]]),
            feed_insert: [u32::MAX; MAX_INPUTS],
            phase: Phase::Idle,
            count_total: 0,
            count_left: 0,
            beat: 1,
            per_bar: 4,
            punch: None,
            latency_override: None,
            shared: shared.clone(),
            handles: Some(RigHandles {
                msgs: tx,
                trash: trash_rx,
                shared,
            }),
        }
    }

    /// The control side's ends, once.
    pub fn take_handles(&mut self) -> Option<RigHandles> {
        self.handles.take()
    }

    fn bin(&mut self, item: Trash) {
        if let Err(rtrb::PushError::Full(item)) = self.trash.push(item) {
            // never free on the audio thread: leak (the writer thread is not keeping up)
            std::mem::forget(item);
        }
    }

    fn finish_cap(&mut self, i: usize) {
        if let Some(cap) = self.slots[i].cap.take() {
            cap.shared.done.store(true, Release);
            self.bin(Trash::Capture(cap));
        }
    }

    fn publish_phase(&self) {
        let p = match self.phase {
            Phase::Idle => 0,
            Phase::CountIn => 1,
            Phase::Rolling => 2,
        };
        self.shared.phase.store(p, Relaxed);
    }

    /// Applies queued messages. Real-time. The runtime follows the returned transition.
    pub fn drain(&mut self) -> Transition {
        let mut out = Transition::None;
        while let Ok(msg) = self.rx.pop() {
            match msg {
                RigMsg::Arm(mut p) => {
                    let i = (p.slot as usize).min(MAX_INPUTS - 1);
                    // a capture of the slot ends with its source
                    self.finish_cap(i);
                    if let Some(src) = p.source.take() {
                        let channels = src.info().channels.clamp(1, 2) as usize;
                        let old = self.slots[i].source.replace(src);
                        self.slots[i].monitor = p.monitor;
                        self.slots[i].insert = p.insert;
                        self.slots[i].channels = channels;
                        if let Some(old) = old {
                            self.bin(Trash::Source(old));
                        }
                    }
                    self.bin(Trash::Arm(p));
                }
                RigMsg::Disarm(slot) => {
                    let i = (slot as usize).min(MAX_INPUTS - 1);
                    self.finish_cap(i);
                    if let Some(old) = self.slots[i].source.take() {
                        self.bin(Trash::Source(old));
                    }
                    self.slots[i].monitor = false;
                }
                RigMsg::Monitor(slot, on) => {
                    let i = (slot as usize).min(MAX_INPUTS - 1);
                    if on && !self.slots[i].monitor
                        && let Some(s) = self.slots[i].source.as_mut()
                    {
                        s.flush();
                    }
                    self.slots[i].monitor = on;
                }
                RigMsg::Start(mut p) => {
                    for i in 0..MAX_INPUTS {
                        self.finish_cap(i);
                        self.slots[i].cap = p.captures[i].take();
                    }
                    self.punch = p.punch;
                    self.latency_override = p.latency_override;
                    self.beat = p.beat.max(1);
                    self.per_bar = p.per_bar.max(1);
                    self.count_total = p.count_in.max(0);
                    self.count_left = self.count_total;
                    self.shared.frames.store(0, Relaxed);
                    self.bin(Trash::Start(p));
                    if self.count_total > 0 {
                        self.phase = Phase::CountIn;
                        out = Transition::CountIn;
                    } else {
                        self.begin_roll();
                        out = Transition::Roll;
                    }
                }
                RigMsg::Stop => {
                    for i in 0..MAX_INPUTS {
                        self.finish_cap(i);
                    }
                    self.phase = Phase::Idle;
                    self.count_left = 0;
                    self.punch = None;
                    out = Transition::None;
                }
            }
        }
        self.publish_phase();
        out
    }

    fn begin_roll(&mut self) {
        self.phase = Phase::Rolling;
        // the freshest frames: whatever queued while nobody read
        for s in &mut self.slots {
            if s.cap.is_some()
                && !s.monitor
                && let Some(src) = s.source.as_mut()
            {
                src.flush();
            }
        }
    }

    /// Frames of count-in left (the chunk must not run past them).
    pub fn count_in_left(&self) -> Option<usize> {
        (self.phase == Phase::CountIn).then_some(self.count_left.max(1) as usize)
    }

    /// Adds the metronome clicks of the count-in for the next `n` frames, and advances it.
    /// Returns true when the count-in is over (the transport plays from the next chunk).
    pub fn count_in_step(&mut self, out: &mut [Vec<f32>; 2], n: usize, sample_rate: f64) -> bool {
        let elapsed = self.count_total - self.count_left;
        let len = (CLICK_SECONDS * sample_rate) as i64;
        for f in 0..n {
            let t = elapsed + f as i64;
            let beat = t / self.beat;
            let off = t % self.beat;
            if off < len {
                let freq = if (beat as u64).is_multiple_of(self.per_bar as u64) {
                    1760.0
                } else {
                    880.0
                };
                let s = off as f64 / sample_rate;
                let v = (std::f64::consts::TAU * freq * s).sin() * (-s / 0.008).exp() * 0.4;
                out[0][f] += v as f32;
                out[1][f] += v as f32;
            }
        }
        self.count_left -= n as i64;
        if self.count_left <= 0 {
            self.count_left = 0;
            self.begin_roll();
            self.publish_phase();
            return true;
        }
        false
    }

    /// Reads the inputs for the chunk `[a, a + n)`, captures what is due and returns the
    /// monitor feeds. Real-time: no allocation, no locks.
    pub fn pre_chunk(&mut self, a: i64, n: usize, playing: bool) -> Feeds {
        let mut feeds = Feeds::NONE;
        self.feed_insert = [u32::MAX; MAX_INPUTS];
        let capturing = self.phase == Phase::Rolling && playing;
        let mut punched_out = 0u8;
        for i in 0..MAX_INPUTS {
            let slot = &mut self.slots[i];
            let Some(src) = slot.source.as_mut() else {
                self.shared.peaks[i].store(0, Relaxed);
                continue;
            };
            self.shared.in_latency[i].store(src.latency_frames(), Relaxed);
            let want_capture = capturing && slot.cap.is_some();
            if !(slot.monitor || want_capture) {
                continue;
            }
            let [sl, sr] = &mut slot.scratch;
            let got = {
                let (l, r) = (&mut sl[..n], &mut sr[..n]);
                if slot.channels == 1 {
                    src.read(&mut [l])
                } else {
                    src.read(&mut [l, r])
                }
            };
            let _ = got; // missing frames are silence (the trait zero-fills)
            if slot.channels == 1 {
                sr[..n].copy_from_slice(&sl[..n]);
            }
            let mut peak = 0.0f32;
            for k in 0..n {
                peak = peak.max(sl[k].abs()).max(sr[k].abs());
            }
            self.shared.peaks[i].fetch_max(peak.to_bits(), Relaxed);
            // monitoring: add to the feed of the target insert
            if slot.monitor {
                let fi = (0..feeds.n)
                    .find(|&f| self.feed_insert[f] == slot.insert)
                    .unwrap_or_else(|| {
                        let f = feeds.n;
                        feeds.n += 1;
                        self.feed_insert[f] = slot.insert;
                        self.feed_bufs[f][0][..n].fill(0.0);
                        self.feed_bufs[f][1][..n].fill(0.0);
                        f
                    });
                let [fl, fr] = &mut self.feed_bufs[fi];
                for k in 0..n {
                    fl[k] += sl[k];
                    fr[k] += sr[k];
                }
            }
            // capture the part of the chunk inside the punch range
            if want_capture {
                let (from, to) = match self.punch {
                    Some((pin, pout)) => (
                        (pin - a).clamp(0, n as i64) as usize,
                        (pout - a).clamp(0, n as i64) as usize,
                    ),
                    None => (0, n),
                };
                if from < to {
                    let latency = match self.latency_override {
                        Some(v) => v,
                        None => src
                            .latency_frames()
                            .saturating_add(self.shared.out_latency.load(Relaxed)),
                    };
                    if let Some(cap) = slot.cap.as_mut() {
                        if !cap.started {
                            cap.started = true;
                            cap.shared.latency.store(latency, Relaxed);
                            cap.shared.start_sample.store(a + from as i64, Relaxed);
                        }
                        let want = to - from;
                        let ch = slot.channels;
                        let take = want.min(cap.tx.slots() / ch);
                        if take > 0
                            && let Ok(chunk) = cap.tx.write_chunk_uninit(take * ch)
                        {
                            let (l, r) = (&sl[from..from + take], &sr[from..from + take]);
                            if ch == 1 {
                                chunk.fill_from_iter(l.iter().copied());
                            } else {
                                chunk.fill_from_iter((0..take).flat_map(|k| [l[k], r[k]]));
                            }
                            let total =
                                cap.shared.frames.fetch_add(take as u64, Relaxed) + take as u64;
                            self.shared.frames.fetch_max(total, Relaxed);
                        }
                        if take < want {
                            cap.shared.dropped.fetch_add((want - take) as u64, Relaxed);
                        }
                    }
                }
                // punched out: the take is complete
                if let Some((_, pout)) = self.punch
                    && a + n as i64 >= pout
                {
                    punched_out |= 1 << i;
                }
            }
        }
        for i in 0..MAX_INPUTS {
            if punched_out & (1 << i) != 0 {
                self.finish_cap(i);
            }
        }
        for f in 0..feeds.n {
            feeds.items[f] = Feed {
                insert: self.feed_insert[f],
                l: self.feed_bufs[f][0].as_ptr(),
                r: self.feed_bufs[f][1].as_ptr(),
            };
        }
        feeds
    }
}

// ------------------------------------------------------------------------------ writer

/// What the writer reports when a take's file is complete.
pub(crate) struct Written {
    pub frames: u64,
    pub error: Option<String>,
}

struct Job {
    rx: rtrb::Consumer<f32>,
    shared: Arc<TakeShared>,
    wav: Option<hound::WavWriter<BufWriter<std::fs::File>>>,
    channels: u16,
    samples: u64,
    flushed: Instant,
    error: Option<String>,
    done: mpsc::Sender<Written>,
}

impl Job {
    fn pump(&mut self) -> bool {
        // read `done` first: everything pushed before it is visible after it
        let finished = self.shared.done.load(Acquire);
        let n = self.rx.slots();
        if n > 0
            && let Ok(chunk) = self.rx.read_chunk(n)
        {
            let (a, b) = chunk.as_slices();
            if let Some(w) = self.wav.as_mut() {
                for &s in a.iter().chain(b) {
                    if let Err(e) = w.write_sample(s) {
                        self.error = Some(e.to_string());
                        break;
                    }
                }
            }
            if self.error.is_some() {
                self.wav = None; // keep draining so the audio thread never blocks
            }
            self.samples += n as u64;
            chunk.commit_all();
        }
        if self.flushed.elapsed() >= FLUSH_EVERY {
            self.flushed = Instant::now();
            if let Some(w) = self.wav.as_mut()
                && let Err(e) = w.flush()
            {
                self.error = Some(e.to_string());
                self.wav = None;
            }
        }
        finished && self.rx.is_empty()
    }

    fn finish(mut self) {
        if let Some(w) = self.wav.take()
            && let Err(e) = w.finalize()
        {
            self.error.get_or_insert(e.to_string());
        }
        let _ = self.done.send(Written {
            frames: self.samples / self.channels.max(1) as u64,
            error: self.error,
        });
    }
}

enum ServiceMsg {
    Job(Box<Job>),
}

/// The writer thread: appends takes to disk and frees retired boxes.
pub(crate) struct Service {
    tx: mpsc::Sender<ServiceMsg>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Service {
    pub fn spawn(mut trash: rtrb::Consumer<Trash>) -> Service {
        let (tx, rx) = mpsc::channel::<ServiceMsg>();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ph-record-writer".into())
            .spawn(move || {
                let mut jobs: Vec<Box<Job>> = Vec::new();
                loop {
                    let stopping = s2.load(Acquire);
                    while let Ok(t) = trash.pop() {
                        drop(t);
                    }
                    while let Ok(ServiceMsg::Job(j)) = rx.try_recv() {
                        jobs.push(j);
                    }
                    let mut k = 0;
                    while k < jobs.len() {
                        let end = jobs[k].pump() || (stopping && jobs[k].rx.is_empty());
                        if end {
                            jobs.swap_remove(k).finish();
                        } else {
                            k += 1;
                        }
                    }
                    if stopping && jobs.is_empty() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            })
            .expect("spawn record writer");
        Service {
            tx,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------------------ control

struct Armed {
    slot: u8,
    info: InputInfo,
    target: RecordTarget,
    monitor: bool,
    latency: u32,
}

struct Take {
    slot: usize,
    shared: Arc<TakeShared>,
    done: mpsc::Receiver<Written>,
    path: std::path::PathBuf,
    file: String,
}

/// The control side of recording: armed inputs, the rig's message ring and the writer.
pub(crate) struct RecCtl {
    provider: Option<Box<dyn InputProvider>>,
    msgs: rtrb::Producer<RigMsg>,
    pub shared: Arc<RigShared>,
    service: Service,
    armed: Vec<Armed>,
    takes: Vec<Take>,
    max_frames: usize,
    sample_rate: u32,
}

fn err(e: impl std::fmt::Display) -> crate::EngineError {
    crate::EngineError::Device(e.to_string())
}

impl RecCtl {
    pub fn new(h: RigHandles, sample_rate: u32, max_frames: usize) -> RecCtl {
        let service = Service::spawn(h.trash);
        RecCtl {
            provider: None,
            msgs: h.msgs,
            shared: h.shared,
            service,
            armed: Vec::new(),
            takes: Vec::new(),
            max_frames,
            sample_rate,
        }
    }

    pub fn set_provider(&mut self, p: Option<Box<dyn InputProvider>>) {
        self.provider = p;
    }

    fn post(&mut self, m: RigMsg) -> crate::EngineResult<()> {
        self.msgs
            .push(m)
            .map_err(|_| err("the audio thread is not taking recorder messages"))
    }

    pub fn inputs(&mut self) -> Vec<InputInfo> {
        self.provider.as_mut().map_or(Vec::new(), |p| p.list())
    }

    pub fn is_recording(&self) -> bool {
        !self.takes.is_empty()
    }

    pub fn arm(
        &mut self,
        input: InputId,
        target: RecordTarget,
        monitor: bool,
        insert: u32,
    ) -> crate::EngineResult<()> {
        if self.is_recording() {
            return Err(err("stop recording before arming another input"));
        }
        let slot = match self.armed.iter().find(|a| a.info.id == input) {
            Some(a) => a.slot,
            None => {
                let used: Vec<u8> = self.armed.iter().map(|a| a.slot).collect();
                (0..MAX_INPUTS as u8)
                    .find(|s| !used.contains(s))
                    .ok_or_else(|| err(format!("at most {MAX_INPUTS} inputs can be armed")))?
            }
        };
        let provider = self
            .provider
            .as_mut()
            .ok_or_else(|| err("this engine has no audio inputs"))?;
        let source = provider.open(input, self.sample_rate, self.max_frames)?;
        let info = source.info().clone();
        let latency = source.latency_frames();
        self.armed.retain(|a| a.info.id != input);
        self.post(RigMsg::Arm(Box::new(ArmPacket {
            slot,
            source: Some(source),
            monitor,
            insert,
        })))?;
        self.armed.push(Armed {
            slot,
            info,
            target,
            monitor,
            latency,
        });
        Ok(())
    }

    pub fn disarm(&mut self, input: InputId) {
        if self.is_recording() {
            return;
        }
        if let Some(i) = self.armed.iter().position(|a| a.info.id == input) {
            let a = self.armed.remove(i);
            let _ = self.post(RigMsg::Disarm(a.slot));
        }
    }

    pub fn set_monitor(&mut self, input: InputId, on: bool) -> crate::EngineResult<()> {
        let a = self
            .armed
            .iter_mut()
            .find(|a| a.info.id == input)
            .ok_or_else(|| err(format!("input {} is not armed", input.0)))?;
        a.monitor = on;
        let slot = a.slot;
        self.post(RigMsg::Monitor(slot, on))
    }

    pub fn armed(&self) -> Vec<(InputInfo, RecordTarget, bool)> {
        self.armed
            .iter()
            .map(|a| (a.info.clone(), a.target.clone(), a.monitor))
            .collect()
    }

    pub fn latency(&self) -> LatencyReport {
        let input = self.armed.iter().map(|a| a.latency).max().unwrap_or(0);
        let output = self.shared.out_latency.load(Relaxed);
        LatencyReport {
            sample_rate: self.sample_rate,
            input_frames: input,
            output_frames: output,
            monitor_seconds: (input + output) as f64 / self.sample_rate.max(1) as f64,
            compensation_frames: input + output,
        }
    }

    pub fn status(&self) -> RecordStatus {
        let phase = match self.shared.phase.load(Relaxed) {
            1 => RecordPhase::CountIn,
            2 => RecordPhase::Recording,
            _ => RecordPhase::Idle,
        };
        RecordStatus {
            phase,
            inputs: self
                .armed
                .iter()
                .map(|a| {
                    (
                        a.info.id,
                        f32::from_bits(self.shared.peaks[a.slot as usize].swap(0, Relaxed)),
                    )
                })
                .collect(),
            frames: self.shared.frames.load(Relaxed),
        }
    }

    /// Starts every armed input. `count_in` and `beat` are samples; see [`StartPacket`].
    pub fn start(
        &mut self,
        opts: &RecordOptions,
        count_in: i64,
        beat: i64,
        per_bar: u32,
        punch: Option<(i64, i64)>,
    ) -> crate::EngineResult<()> {
        if self.armed.is_empty() {
            return Err(err("no input is armed (record.arm)"));
        }
        if self.is_recording() {
            return Err(err("already recording"));
        }
        std::fs::create_dir_all(&opts.dir).map_err(|e| err(format!("{}: {e}", opts.dir.display())))?;
        let multi = self.armed.len() > 1;
        let mut packet = StartPacket {
            count_in,
            beat,
            per_bar,
            punch,
            captures: [None, None, None, None],
            latency_override: opts.latency_ms.map(|ms| {
                (ms.max(0.0) / 1000.0 * self.sample_rate as f64).round() as u32
            }),
        };
        let mut takes = Vec::new();
        let ring = ((RING_SECONDS * self.sample_rate as f64) as usize).max(4096) * 2;
        for a in &self.armed {
            let (file, path, f) = create_take_file(&opts.dir, &opts.name, multi.then(|| &a.info))
                .map_err(err)?;
            let spec = hound::WavSpec {
                channels: a.info.channels.clamp(1, 2),
                sample_rate: self.sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let wav = hound::WavWriter::new(BufWriter::new(f), spec).map_err(err)?;
            let (tx, rx) = rtrb::RingBuffer::new(ring);
            let shared = Arc::new(TakeShared::new());
            let (dtx, drx) = mpsc::channel();
            let job = Job {
                rx,
                shared: shared.clone(),
                wav: Some(wav),
                channels: spec.channels,
                samples: 0,
                flushed: Instant::now(),
                error: None,
                done: dtx,
            };
            let _ = self.service.tx.send(ServiceMsg::Job(Box::new(job)));
            packet.captures[a.slot as usize] = Some(Capture {
                tx,
                shared: shared.clone(),
                started: false,
            });
            takes.push(Take {
                slot: a.slot as usize,
                shared,
                done: drx,
                path,
                file,
            });
        }
        self.post(RigMsg::Start(Box::new(packet)))?;
        self.takes = takes;
        Ok(())
    }

    /// Stops and waits for the files. `tempo` converts sample positions to ticks.
    pub fn stop(
        &mut self,
        tempo: &crate::tempo::TempoMap,
        sample_rate: u32,
    ) -> crate::EngineResult<Vec<RecordedFile>> {
        if self.takes.is_empty() {
            return Err(err("not recording"));
        }
        let posted = self.post(RigMsg::Stop);
        let takes = std::mem::take(&mut self.takes);
        // The writer finishes at the moment of the call, whether or not the audio thread has
        // run since (a device always has; a simulated one is pulled by its owner): whatever the
        // ring holds is the take.
        for t in &takes {
            t.shared.done.store(true, Release);
        }
        posted?;
        let mut out = Vec::new();
        for t in takes {
            let written = t
                .done
                .recv_timeout(FINISH_TIMEOUT)
                .map_err(|_| err("the recording writer did not finish"))?;
            let a = self
                .armed
                .iter()
                .find(|a| a.slot as usize == t.slot)
                .ok_or_else(|| err("armed input vanished"))?;
            let start_sample = t.shared.start_sample.load(Acquire);
            if start_sample == i64::MIN || written.frames == 0 {
                // nothing was captured: leave no empty file behind
                let _ = std::fs::remove_file(&t.path);
                continue;
            }
            let start = tempo.tick_floor(start_sample).max(0) as Tick;
            let end = tempo.tick_floor(start_sample + written.frames as i64) as Tick;
            out.push(RecordedFile {
                input: a.info.clone(),
                target: a.target.clone(),
                path: t.path,
                file: t.file,
                start,
                length: end.saturating_sub(start),
                frames: written.frames,
                sample_rate,
                channels: a.info.channels.clamp(1, 2),
                latency_frames: t.shared.latency.load(Relaxed) as u64,
                overflow: t.shared.dropped.load(Relaxed),
                error: written.error,
            });
        }
        Ok(out)
    }
}

/// Creates `<dir>/<name> <n>.wav`, never touching an existing file: the number moves past
/// the highest one already there and `create_new` settles any race.
fn create_take_file(
    dir: &std::path::Path,
    name: &str,
    input: Option<&InputInfo>,
) -> Result<(String, std::path::PathBuf, std::fs::File), String> {
    let mut n = 1u32;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if let Some(rest) = f.strip_prefix(name).and_then(|r| r.strip_prefix(' '))
                && let Some(num) = rest
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .and_then(|d| d.parse::<u32>().ok())
            {
                n = n.max(num + 1);
            }
        }
    }
    let suffix = input.map_or(String::new(), |i| {
        let clean: String = i
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
            .collect();
        format!(" - {}", clean.trim())
    });
    loop {
        let file = format!("{name} {n}{suffix}.wav");
        let path = dir.join(&file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((file, path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
}
