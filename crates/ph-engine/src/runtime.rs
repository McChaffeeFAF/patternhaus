//! The audio-thread side: transport, event scheduling, plan swaps and graph execution.
//!
//! [`Runtime::process`] is the whole device callback. It never allocates, locks or does
//! I/O (tests run it under `assert_no_alloc` while another thread swaps plans every
//! block). Per callback:
//!
//! 1. install a new plan if one was posted (move reused state across, apply queued
//!    parameter changes, retire the old plan to the collector);
//! 2. apply queued transport and preview-note commands;
//! 3. render in chunks of at most `max_frames`, split at loop and range boundaries:
//!    schedule events (serial), run the graph (serial or on the worker pool), copy the
//!    master out and add the metronome;
//! 4. add the browser's audition voice (outside the graph, after the master; see
//!    `audition.rs`);
//! 5. publish position, meters, timing and load.
//!
//! Offline rendering drives the same `Runtime` (see `offline.rs`), so real-time and
//! offline output are the same computation.

use crate::audition::{Player, Port};
use crate::plan::{
    ACTIVE_CAP, Active, AutoKind, ChannelState, EFFECT_EVENT_CAP, EVENT_CAP, EVENT_RESERVE,
    NodeDef, NodeState, NoteEv, PREVIEW_CAP, Plan, Preview, Transfer, channel_gain, strip_gain,
};
use crate::pool::{Job, Pool, reset_sched};
use crate::rt::{Mailbox, Retire};
use crate::stats::{Shared, store_f32};
use crate::{PlayMode, TransportCommand};
use ph_core::{ChannelId, PPQ, PatternId, Tick};
use ph_node::{Event, EventKind, ProcessContext, ProcessIo, ProcessStatus};
use std::sync::Arc;
use std::sync::atomic::Ordering::*;
use std::time::Instant;

/// Messages from the control side to the audio thread (SPSC ring).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Transport(TransportCommand),
    Preview {
        channel: ChannelId,
        key: u8,
        velocity: f32,
        on: bool,
    },
}

/// Per-block context shared by every node of a chunk.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkCtx {
    pub frames: usize,
    pub pos: i64,
    pub ticks: f64,
    pub bpm: f64,
    pub playing: bool,
    pub sample_rate: f64,
    pub meters: bool,
    /// Song mode (audio clips play only on the song timeline).
    pub song: bool,
    /// Timeline sample where an offline range ends (audio clips are silent from there).
    pub limit: i64,
    /// The song loop region in samples while the transport loops (audio-clip pre-warm).
    pub looping: Option<(i64, i64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Transport {
    pub playing: bool,
    pub mode: PlayMode,
    /// Timeline position in samples (song or pattern timeline).
    pub pos: i64,
    pub loop_ticks: Option<(Tick, Tick)>,
    pub metronome: bool,
    /// Offline: loop nothing (render a range once).
    pub looping: bool,
    /// Offline: no note starts at or after this sample; notes still sounding are released.
    pub end: Option<i64>,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            playing: false,
            mode: PlayMode::Song,
            pos: 0,
            loop_ticks: None,
            metronome: false,
            looping: true,
            end: None,
        }
    }
}

/// Overload policy knobs.
pub(crate) struct Shedding {
    /// Highest level allowed. Levels above 1 change the audio, so the simulated device and
    /// offline renders cap at 1 (meters only) to keep the oracle exact.
    pub max_level: u8,
    pub level: u8,
    hot: u32,
    cool: u32,
    recent_overloads: f32,
}

pub(crate) struct Runtime {
    pub plan: Box<Plan>,
    deferred: Option<Box<Plan>>,
    mailbox: Arc<Mailbox<Plan>>,
    retire: Retire,
    cmds: rtrb::Consumer<Command>,
    pub shared: Arc<Shared>,
    pool: Option<Pool>,
    job: Box<Job>,
    pub tr: Transport,
    resync: bool,
    ended: bool,
    out: [Vec<f32>; 2],
    scratch: [Vec<f32>; 2],
    pub shed: Shedding,
    load: f32,
    buffer_frames: u32,
    audition_port: Arc<Port>,
    audition: Player,
}

// SAFETY: the job's raw plan pointer only refers to `self.plan` during `run_graph`.
unsafe impl Send for Runtime {}

impl Runtime {
    #[allow(clippy::too_many_arguments)] // internal constructor, one call site per engine kind
    pub fn new(
        plan: Box<Plan>,
        mailbox: Arc<Mailbox<Plan>>,
        retire: Retire,
        cmds: rtrb::Consumer<Command>,
        shared: Arc<Shared>,
        workers: usize,
        max_shed: u8,
        audition_port: Arc<Port>,
    ) -> Self {
        let n = plan.settings.max_frames;
        let pool = (workers > 0).then(|| Pool::new(workers));
        let buffer_frames = n as u32;
        let ctx = ChunkCtx {
            frames: 0,
            pos: 0,
            ticks: 0.0,
            bpm: 120.0,
            playing: false,
            sample_rate: plan.settings.sample_rate as f64,
            meters: true,
            song: true,
            limit: i64::MAX,
            looping: None,
        };
        Self {
            plan,
            deferred: None,
            mailbox,
            retire,
            cmds,
            shared,
            pool,
            job: Box::new(Job {
                plan: std::ptr::null(),
                ctx,
            }),
            tr: Transport::default(),
            resync: true,
            ended: false,
            out: [vec![0.0; n], vec![0.0; n]],
            scratch: [vec![0.0; n], vec![0.0; n]],
            shed: Shedding {
                max_level: max_shed,
                level: 0,
                hot: 0,
                cool: 0,
                recent_overloads: 0.0,
            },
            load: 0.0,
            buffer_frames,
            audition_port,
            audition: Player::new(),
        }
    }

    #[cfg(test)]
    pub fn workers(&self) -> usize {
        self.pool.as_ref().map_or(0, |p| p.workers())
    }

    /// The device callback: renders `left.len()` frames (any count; chunked internally).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let t0 = Instant::now();
        let frames = left.len().min(right.len());
        self.poll_plan();
        self.drain_commands();
        self.audition.poll(&self.audition_port);
        let mut done = 0;
        while done < frames {
            let n = self.next_chunk(frames - done);
            self.run_chunk(n);
            left[done..done + n].copy_from_slice(&self.out[0][..n]);
            right[done..done + n].copy_from_slice(&self.out[1][..n]);
            self.audition
                .mix_into(&mut left[done..done + n], &mut right[done..done + n]);
            done += n;
        }
        self.publish(t0, frames);
    }

    /// Renders into an interleaved device buffer with `channels` channels.
    pub fn process_interleaved(&mut self, data: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        let total = data.len() / channels;
        let max = self.plan.settings.max_frames;
        // `take` leaves an empty Vec behind: no allocation
        let mut l = std::mem::take(&mut self.scratch[0]);
        let mut r = std::mem::take(&mut self.scratch[1]);
        let mut done = 0;
        while done < total {
            let n = (total - done).min(max).min(l.len());
            self.process(&mut l[..n], &mut r[..n]);
            for i in 0..n {
                let f = &mut data[(done + i) * channels..(done + i + 1) * channels];
                if channels == 1 {
                    f[0] = 0.5 * (l[i] + r[i]);
                } else {
                    f[0] = l[i];
                    f[1] = r[i];
                    for x in &mut f[2..] {
                        *x = 0.0;
                    }
                }
            }
            done += n;
        }
        self.scratch = [l, r];
    }

    // ------------------------------------------------------------------ plan swap

    fn poll_plan(&mut self) {
        if self.deferred.is_none() {
            self.deferred = self.mailbox.take();
        }
        if self.deferred.is_some() && self.retire.has_room() {
            let new = self.deferred.take().expect("checked");
            self.install(new);
        }
    }

    /// Moves reused state from the running plan into `new`, applies its parameter
    /// changes, and retires the old plan.
    fn install(&mut self, mut new: Box<Plan>) {
        debug_assert_eq!(new.base, self.plan.id, "plan compiled against another plan");
        let old = &self.plan;
        for t in &new.transfers {
            // SAFETY: between blocks the audio thread has exclusive access to both plans.
            unsafe {
                match *t {
                    Transfer::Channel { from, to } => {
                        if let (NodeState::Channel(a), NodeState::Channel(b)) =
                            (old.state_mut(from), new.state_mut(to))
                        {
                            std::mem::swap(a, b);
                        }
                    }
                    Transfer::Bus { from, to } => {
                        if let (NodeState::Insert(a), NodeState::Insert(b)) =
                            (old.state_mut(from), new.state_mut(to))
                        {
                            std::mem::swap(&mut a.bus, &mut b.bus);
                        }
                    }
                    Transfer::Effect { from, to } => {
                        if let (NodeState::Insert(a), NodeState::Insert(b)) =
                            (old.state_mut(from.0), new.state_mut(to.0))
                            && let (Some(x), Some(y)) =
                                (a.effects.get_mut(from.1), b.effects.get_mut(to.1))
                        {
                            std::mem::swap(x, y);
                        }
                    }
                    Transfer::AudioTrack { from, to } => {
                        if let (NodeState::AudioTrack(a), NodeState::AudioTrack(b)) =
                            (old.state_mut(from), new.state_mut(to))
                        {
                            std::mem::swap(a, b);
                        }
                    }
                }
            }
        }
        for p in &new.params {
            // SAFETY: as above.
            unsafe {
                match (new.state_mut(p.node), p.slot) {
                    (NodeState::Channel(Some(c)), None) => {
                        if let Some(proc) = c.proc.as_mut() {
                            proc.set_param(p.id, p.value);
                        }
                    }
                    (NodeState::Insert(s), Some(k)) => {
                        if let Some(proc) = s.effects.get_mut(k).and_then(|e| e.proc.as_mut()) {
                            proc.set_param(p.id, p.value);
                        }
                    }
                    _ => {}
                }
            }
        }
        std::mem::swap(&mut self.plan, &mut new);
        if self.shed.level >= 3 {
            self.apply_voice_shedding(true);
        }
        if let Err(p) = self.retire.send(new) {
            // checked `has_room` before; keep it rather than free it here
            self.deferred_retire_fallback(p);
        }
        self.resync = true;
    }

    fn deferred_retire_fallback(&mut self, p: Box<Plan>) {
        // Cannot happen (room was checked). Leak rather than free on the audio thread.
        std::mem::forget(p);
    }

    // ------------------------------------------------------------------ commands

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmds.pop() {
            self.apply(cmd);
        }
    }

    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Transport(t) => self.apply_transport(t),
            Command::Preview {
                channel,
                key,
                velocity,
                on,
            } => {
                if let Some(i) = self.plan.channel_node(channel)
                    // SAFETY: between blocks, audio thread only.
                    && let Some(st) = unsafe { self.plan.channel_mut(i) }
                    && st.preview.len() < PREVIEW_CAP
                {
                    st.preview.push(Preview {
                        key: key.min(127),
                        velocity: velocity.clamp(0.0, 1.0),
                        on,
                    });
                }
            }
        }
    }

    fn apply_transport(&mut self, t: TransportCommand) {
        match t {
            TransportCommand::Play => {
                if !self.tr.playing {
                    self.tr.playing = true;
                    self.resync = true;
                }
            }
            TransportCommand::Stop => {
                self.tr.playing = false;
                self.tr.pos = match (self.tr.mode, self.tr.loop_ticks) {
                    (PlayMode::Song, Some((a, _))) => self.plan.tempo.sample_at(a),
                    _ => 0,
                };
                self.all_notes_off();
            }
            TransportCommand::Seek(t) => {
                self.tr.pos = self.plan.tempo.sample_at(t);
                self.all_notes_off();
            }
            TransportCommand::SetMode(m) => {
                if m != self.tr.mode {
                    self.tr.mode = m;
                    self.tr.pos = 0;
                    self.all_notes_off();
                }
            }
            TransportCommand::SetLoop(r) => {
                self.tr.loop_ticks = r.filter(|(a, b)| b > a);
            }
            TransportCommand::SetMetronome(on) => self.tr.metronome = on,
        }
        self.resync = true;
    }

    fn all_notes_off(&mut self) {
        for i in 0..self.plan.n_channels {
            // SAFETY: between blocks.
            if let Some(st) = unsafe { self.plan.channel_mut(i) } {
                st.active.clear();
                st.all_off = true;
            }
        }
        self.ended = false;
    }

    fn release_all(&mut self) {
        for i in 0..self.plan.n_channels {
            // SAFETY: between blocks.
            if let Some(st) = unsafe { self.plan.channel_mut(i) } {
                st.release_all = true;
            }
        }
    }

    // ------------------------------------------------------------------ chunking

    /// The loop region in timeline samples, if the transport loops.
    fn loop_samples(&self) -> Option<(i64, i64)> {
        if !self.tr.looping {
            return None;
        }
        match self.tr.mode {
            PlayMode::Pattern(id) => Some((0, self.pattern_len(id))),
            PlayMode::Song => match self.tr.loop_ticks {
                Some((a, b)) => Some((self.plan.tempo.sample_at(a), self.plan.tempo.sample_at(b))),
                None if self.plan.song_len > 0 => Some((0, self.plan.song_len)),
                None => None,
            },
        }
    }

    fn pattern_len(&self, id: PatternId) -> i64 {
        self.plan
            .pattern(id)
            .map_or(self.plan.bar_samples, |p| p.len_samples)
            .max(1)
    }

    /// Wraps at loop end / ends a range, and returns the next chunk length.
    fn next_chunk(&mut self, remaining: usize) -> usize {
        let mut n = remaining.min(self.plan.settings.max_frames);
        if !self.tr.playing {
            return n;
        }
        if let Some((a, b)) = self.loop_samples()
            && b > a
        {
            if self.tr.pos >= b {
                self.tr.pos = a;
                self.release_all();
                self.resync = true;
            }
            n = n.min((b - self.tr.pos) as usize);
        }
        if let Some(end) = self.tr.end {
            if self.tr.pos >= end {
                if !self.ended {
                    self.ended = true;
                    self.release_all();
                }
            } else {
                n = n.min((end - self.tr.pos) as usize);
            }
        }
        n.max(1)
    }

    fn run_chunk(&mut self, n: usize) {
        let a = self.tr.pos;
        let b = a + n as i64;
        self.schedule_events(a, b);
        let ticks = self.plan.tempo.tick_at(a);
        let ctx = ChunkCtx {
            frames: n,
            pos: a,
            ticks,
            bpm: self.plan.tempo.bpm_at(ticks.max(0.0) as Tick),
            playing: self.tr.playing,
            sample_rate: self.plan.settings.sample_rate as f64,
            meters: self.shed.level < 1,
            song: self.tr.mode == PlayMode::Song,
            limit: self.tr.end.unwrap_or(i64::MAX),
            looping: if self.tr.mode == PlayMode::Song {
                self.loop_samples().filter(|(a, b)| b > a)
            } else {
                None
            },
        };
        self.run_graph(ctx);
        let m = self.plan.master();
        // SAFETY: the graph is finished; nothing else touches the plan now.
        if let NodeState::Insert(s) = unsafe { self.plan.state(m) }
            && let Some(bus) = s.bus.as_ref()
        {
            self.out[0][..n].copy_from_slice(&bus.out[0][..n]);
            self.out[1][..n].copy_from_slice(&bus.out[1][..n]);
        } else {
            self.out[0][..n].fill(0.0);
            self.out[1][..n].fill(0.0);
        }
        if self.tr.metronome && self.tr.playing {
            self.metronome(a, n);
        }
        if self.tr.playing {
            self.tr.pos = b;
        }
    }

    fn run_graph(&mut self, ctx: ChunkCtx) {
        let plan: &Plan = &self.plan;
        match &self.pool {
            Some(pool) if plan.nodes.len() > 2 => {
                reset_sched(plan);
                self.job.plan = plan as *const Plan;
                self.job.ctx = ctx;
                pool.run(&self.job);
                self.job.plan = std::ptr::null();
            }
            _ => {
                for i in 0..plan.nodes.len() {
                    // SAFETY: serial, in topological order.
                    unsafe { process_node(plan, i, &ctx) };
                }
            }
        }
    }

    // ------------------------------------------------------------------ events

    fn schedule_events(&mut self, a: i64, b: i64) {
        let plan: &Plan = &self.plan;
        let nch = plan.n_channels;
        let playing = self.tr.playing;
        let end = self.tr.end;
        let resync = std::mem::take(&mut self.resync);
        let mode = self.tr.mode;
        let stream_of = |i: usize| -> &[NoteEv] {
            match mode {
                PlayMode::Song => &plan.song[i],
                PlayMode::Pattern(id) => plan.pattern(id).map_or(&[][..], |p| &p.notes[i]),
            }
        };
        // pass 1: transport flushes, previews and note-ons
        for i in 0..nch {
            // SAFETY: serial phase, audio thread only.
            let Some(st) = (unsafe { plan.channel_mut(i) }) else {
                continue;
            };
            st.events.clear();
            if st.all_off {
                st.all_off = false;
                push_event(st, 0, EventKind::AllNotesOff);
            }
            if st.release_all {
                st.release_all = false;
                let mut k = 0;
                while k < st.active.len() {
                    if st.active[k].preview {
                        k += 1;
                    } else {
                        let n = st.active.swap_remove(k);
                        push_off(st, 0, n);
                    }
                }
            }
            for p in 0..st.preview.len() {
                let pv = st.preview[p];
                if pv.on {
                    note_on(st, 0, a, i64::MAX, pv.key, pv.velocity, 0.0, 0.0, true);
                } else if let Some(n) = st.active.iter_mut().find(|n| n.preview && n.key == pv.key)
                {
                    n.end = a;
                }
            }
            st.preview.clear();
            let stream = stream_of(i);
            if resync {
                st.cursor = stream.partition_point(|e| e.start < a);
            }
            if playing {
                while st.cursor < stream.len() && stream[st.cursor].start < b {
                    let e = stream[st.cursor];
                    st.cursor += 1;
                    if e.start < a || end.is_some_and(|x| e.start >= x) {
                        continue;
                    }
                    note_on(
                        st,
                        (e.start - a) as u32,
                        e.start,
                        e.end,
                        e.key,
                        e.velocity,
                        e.pan,
                        e.fine_pitch,
                        false,
                    );
                }
            }
        }
        // pass 1b: automation (M2/T7). Song mode only; anywhere else automated parameters
        // return to the project's values.
        if !plan.autos.is_empty() {
            let active = playing && mode == PlayMode::Song;
            schedule_automation(plan, a, b, active, resync);
        }
        // pass 2: choke groups across channels — a note-on cuts the group's other channels
        for g in 1..plan.choke.len() {
            let members = &plan.choke[g];
            if members.len() < 2 {
                continue;
            }
            for &x in members {
                // SAFETY: serial phase; `x` and `y` are distinct nodes.
                let Some(sx) = (unsafe { plan.channel_mut(x) }) else {
                    continue;
                };
                for k in 0..sx.events.len() {
                    let EventKind::NoteOn { .. } = sx.events[k].kind else {
                        continue;
                    };
                    let at = a + sx.events[k].frame as i64;
                    for &y in members {
                        if y == x {
                            continue;
                        }
                        if let Some(sy) = unsafe { plan.channel_mut(y) } {
                            for n in sy.active.iter_mut() {
                                if n.start < at && n.end > at {
                                    n.end = at;
                                }
                            }
                        }
                    }
                }
            }
        }
        // pass 3: note-offs due in this chunk, then sort
        for i in 0..nch {
            // SAFETY: serial phase.
            let Some(st) = (unsafe { plan.channel_mut(i) }) else {
                continue;
            };
            let mut k = 0;
            while k < st.active.len() {
                if st.active[k].end < b {
                    let n = st.active.swap_remove(k);
                    let f = (n.end - a).max(0) as u32;
                    push_off(st, f, n);
                } else {
                    k += 1;
                }
            }
            st.events.sort_unstable_by_key(event_order);
        }
    }

    // ------------------------------------------------------------------ metronome

    fn metronome(&mut self, a: i64, n: usize) {
        let tempo = &self.plan.tempo;
        let sr = self.plan.settings.sample_rate as f64;
        let ts = self.plan.time_sig;
        let beat = (PPQ * 4 / ts.denominator.max(1) as Tick).max(1);
        let per_bar = ts.numerator.max(1) as u64;
        let len = (0.03 * sr) as i64;
        let first = tempo.tick_floor((a - len).max(0)) / beat;
        let mut k = first;
        loop {
            let s = tempo.sample_at(k * beat);
            if s >= a + n as i64 {
                break;
            }
            let freq = if k.is_multiple_of(per_bar) {
                1760.0
            } else {
                880.0
            };
            let from = (s.max(a) - a) as usize;
            let to = ((s + len).min(a + n as i64) - a).max(0) as usize;
            for f in from..to {
                let t = (a + f as i64 - s) as f64 / sr;
                let v = (std::f64::consts::TAU * freq * t).sin() * (-t / 0.008).exp() * 0.4;
                self.out[0][f] += v as f32;
                self.out[1][f] += v as f32;
            }
            k += 1;
        }
    }

    // ------------------------------------------------------------------ status

    fn publish(&mut self, t0: Instant, frames: usize) {
        let sh = &self.shared;
        let tick = self.plan.tempo.tick_floor(self.tr.pos);
        sh.position.store(tick, Relaxed);
        sh.playing.store(self.tr.playing, Relaxed);
        // per-node timing
        let board = &self.plan.board;
        for i in 0..self.plan.nodes.len() {
            let stat = self.plan.nodes[i].stat;
            // SAFETY: between blocks.
            match unsafe { self.plan.state_mut(i) } {
                NodeState::Channel(Some(c)) => store_f32(&board.entries[stat].micros, c.micros),
                NodeState::Insert(s) => {
                    if let (Some(b), NodeDef::Insert(d)) =
                        (s.bus.as_deref_mut(), &self.plan.nodes[i].def)
                    {
                        store_f32(&board.entries[stat].micros, b.micros);
                        if b.metered > 0
                            && let Some(m) = sh.meters.get(d.insert as usize)
                        {
                            for c in 0..2 {
                                m.peak[c].fetch_max(b.peak[c].to_bits(), Relaxed);
                                let rms = (b.sumsq[c] / b.metered as f64).sqrt() as f32;
                                store_f32(&m.rms[c], rms);
                            }
                            b.peak = [0.0; 2];
                            b.sumsq = [0.0; 2];
                            b.metered = 0;
                        }
                    }
                    for (k, e) in s.effects.iter().enumerate() {
                        if let Some(entry) = board.entries.get(stat + 1 + k) {
                            store_f32(&entry.micros, e.micros);
                        }
                    }
                }
                NodeState::AudioTrack(Some(t)) => store_f32(&board.entries[stat].micros, t.micros),
                NodeState::Channel(None) | NodeState::AudioTrack(None) => {}
            }
        }
        if frames == 0 {
            return;
        }
        let budget = frames as f64 / self.plan.settings.sample_rate as f64;
        let used = t0.elapsed().as_secs_f64();
        let load = (used / budget) as f32;
        self.load += (load - self.load) * 0.1;
        store_f32(&sh.dsp_load, self.load);
        let overloaded = load > 1.0;
        let s = &mut self.shed;
        s.recent_overloads *= 0.995;
        if overloaded {
            sh.overloads.fetch_add(1, Relaxed);
            s.recent_overloads += 1.0;
            if s.recent_overloads > 4.0 {
                let suggest = (self.buffer_frames * 2).min(4096);
                sh.suggest_frames.store(suggest, Relaxed);
            }
        }
        if load > 0.9 {
            s.hot += 1;
            s.cool = 0;
        } else if load < 0.5 {
            s.cool += 1;
            s.hot = 0;
        }
        let before = s.level;
        if s.hot >= 4 && s.level < s.max_level {
            s.level += 1;
            s.hot = 0;
        } else if s.cool >= 400 && s.level > 0 {
            s.level -= 1;
            s.cool = 0;
        }
        let level = s.level;
        if level != before {
            sh.shedding.store(level, Relaxed);
            if level >= 3 && before < 3 {
                self.apply_voice_shedding(true);
            } else if level < 3 && before >= 3 {
                self.apply_voice_shedding(false);
            }
        }
        // level 2 (oversampling): live plans are always activated at `Quality::Live`, the
        // lowest oversampling, so there is nothing further to drop.
    }

    /// Level 3: halve every instrument's voice limit (or restore it).
    fn apply_voice_shedding(&mut self, on: bool) {
        for i in 0..self.plan.n_channels {
            // SAFETY: between blocks.
            let Some(st) = (unsafe { self.plan.channel_mut(i) }) else {
                continue;
            };
            let (Some(id), Some(p)) = (st.poly, st.proc.as_mut()) else {
                continue;
            };
            if on && st.poly_saved.is_none() {
                let cur = p.param(id);
                st.poly_saved = Some(cur);
                p.set_param(id, (cur / 2.0).floor().max(1.0));
            } else if !on && let Some(v) = st.poly_saved.take() {
                p.set_param(id, v);
            }
        }
    }
}

/// Fills this block's automation: per-frame buffers for gains, `Param` events for processor
/// parameters (see [`crate::automation::BoundTarget::schedule`]). Serial phase.
fn schedule_automation(plan: &Plan, a: i64, b: i64, active: bool, resync: bool) {
    for &(node, slot) in &plan.effect_events {
        // SAFETY: serial phase, audio thread only.
        if let NodeState::Insert(s) = unsafe { plan.state_mut(node) }
            && let Some(e) = s.effects.get_mut(slot)
        {
            e.events.clear();
        }
    }
    let frames = (b - a).max(0) as usize;
    for t in &plan.autos {
        // SAFETY: serial phase; one target at a time.
        let rt = unsafe { t.rt_mut() };
        match &t.kind {
            AutoKind::Buffer => {
                let n = frames.min(rt.buf.len());
                rt.has = t.bound.fill(&plan.tempo, a, active, &mut rt.buf[..n]);
            }
            AutoKind::Param {
                node,
                slot,
                id,
                model,
            } => {
                // SAFETY: as above.
                let events: Option<&mut Vec<Event>> = match (unsafe { plan.state_mut(*node) }, slot)
                {
                    (NodeState::Channel(Some(c)), None) => Some(&mut c.events),
                    (NodeState::Insert(s), Some(k)) => s.effects.get_mut(*k).map(|e| &mut e.events),
                    _ => None,
                };
                let Some(events) = events else { continue };
                let cap = if slot.is_some() {
                    EFFECT_EVENT_CAP
                } else {
                    EVENT_CAP - EVENT_RESERVE
                };
                let id = *id;
                t.bound.schedule(
                    &plan.tempo,
                    *model,
                    a,
                    b,
                    active,
                    resync,
                    &mut rt.last,
                    |frame, value| {
                        if events.len() < cap {
                            events.push(Event {
                                frame,
                                kind: EventKind::Param { id, value },
                            });
                        }
                    },
                );
            }
            AutoKind::Tempo | AutoKind::Unresolved => {}
        }
    }
    for &(node, slot) in &plan.effect_events {
        // SAFETY: as above.
        if let NodeState::Insert(s) = unsafe { plan.state_mut(node) }
            && let Some(e) = s.effects.get_mut(slot)
        {
            e.events.sort_unstable_by_key(event_order);
        }
    }
}

fn event_order(e: &Event) -> (u32, u8, u8, u32) {
    match e.kind {
        // the relative order of the M1 kinds is unchanged (ph-node crate docs)
        EventKind::AllNotesOff => (e.frame, 0, 0, 0),
        EventKind::NoteOff { note_id, key, .. } => (e.frame, 1, key, note_id),
        EventKind::Cc { controller, .. } => (e.frame, 2, 0, controller as u32),
        EventKind::PitchBend { .. } => (e.frame, 2, 1, 0),
        EventKind::NoteOn { note_id, key, .. } => (e.frame, 3, key, note_id),
        EventKind::Param { id, .. } => (e.frame, 4, 0, id.0),
        EventKind::NoteExpression { note_id, .. } => (e.frame, 5, 0, note_id),
    }
}

fn push_event(st: &mut ChannelState, frame: u32, kind: EventKind) -> bool {
    if st.events.len() < EVENT_CAP {
        st.events.push(Event { frame, kind });
        true
    } else {
        false
    }
}

fn push_off(st: &mut ChannelState, frame: u32, n: Active) {
    push_event(
        st,
        frame,
        EventKind::NoteOff {
            note_id: n.id,
            key: n.key,
            velocity: 0.0,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn note_on(
    st: &mut ChannelState,
    frame: u32,
    start: i64,
    end: i64,
    key: u8,
    velocity: f32,
    pan: f32,
    fine_pitch: f32,
    preview: bool,
) {
    // keep room for the note-off
    if st.events.len() + 2 > EVENT_CAP {
        return;
    }
    if st.active.len() >= ACTIVE_CAP {
        let oldest = st.active.remove(0);
        push_off(st, frame, oldest);
    }
    let id = st.next_id;
    st.next_id = st.next_id.wrapping_add(1);
    push_event(
        st,
        frame,
        EventKind::NoteOn {
            note_id: id,
            key,
            velocity,
            pan,
            fine_pitch,
        },
    );
    st.active.push(Active {
        id,
        key,
        start,
        end,
        preview,
    });
}

/// Processes one node for the current chunk.
///
/// # Safety
/// The caller must own node `i` for this chunk (scheduler or serial order), and every
/// node it reads from must have completed.
pub(crate) unsafe fn process_node(plan: &Plan, i: usize, ctx: &ChunkCtx) {
    let node = &plan.nodes[i];
    let n = ctx.frames;
    let pctx = ProcessContext {
        sample_rate: ctx.sample_rate,
        frames: n,
        song_pos_samples: ctx.pos,
        song_pos_ticks: ctx.ticks,
        bpm: ctx.bpm,
        playing: ctx.playing,
        live: node.live,
    };
    let t0 = Instant::now();
    match (&node.def, unsafe { plan.state_mut(i) }) {
        (NodeDef::Channel(def), NodeState::Channel(Some(st))) => {
            let st: &mut ChannelState = st;
            let [l, r] = &mut st.out;
            let (l, r) = (&mut l[..n], &mut r[..n]);
            match st.proc.as_mut() {
                Some(p) => {
                    let mut ch: [&mut [f32]; 2] = [l, r];
                    let mut ports: [&mut [&mut [f32]]; 1] = [&mut ch];
                    let mut io = ProcessIo {
                        inputs: &[],
                        outputs: &mut ports,
                        events: &st.events,
                    };
                    let _: ProcessStatus = p.process(&pctx, &mut io);
                }
                None => {
                    l.fill(0.0);
                    r.fill(0.0);
                }
            }
            // SAFETY: the scheduling phase is over; automation buffers are read-only now.
            let (av, ap) = unsafe {
                (
                    def.auto_vol.and_then(|i| plan.autos[i].frames()),
                    def.auto_pan.and_then(|i| plan.autos[i].frames()),
                )
            };
            let [l, r] = &mut st.out;
            if av.is_none() && ap.is_none() {
                st.ramp[0].set(def.gain[0]);
                st.ramp[1].set(def.gain[1]);
                for k in 0..n {
                    l[k] *= st.ramp[0].next();
                    r[k] *= st.ramp[1].next();
                }
            } else {
                // automated volume and/or pan: a gain target per frame, smoothed by the ramps
                let mut cache = (f32::NAN, f32::NAN, def.gain);
                for k in 0..n {
                    let vol = pick(av, k, def.volume);
                    let pan = pick(ap, k, def.pan);
                    if vol.to_bits() != cache.0.to_bits() || pan.to_bits() != cache.1.to_bits() {
                        cache = (vol, pan, channel_gain(vol, pan, def.muted));
                    }
                    st.ramp[0].set(cache.2[0]);
                    st.ramp[1].set(cache.2[1]);
                    l[k] *= st.ramp[0].next();
                    r[k] *= st.ramp[1].next();
                }
            }
            st.micros += (t0.elapsed().as_secs_f32() * 1e6 - st.micros) * 0.05;
        }
        (NodeDef::AudioTrack, NodeState::AudioTrack(Some(st))) => {
            st.render(ctx.pos, n, ctx.playing && ctx.song, ctx.limit, ctx.looping);
            st.micros += (t0.elapsed().as_secs_f32() * 1e6 - st.micros) * 0.05;
        }
        (NodeDef::Insert(def), NodeState::Insert(st)) => {
            let Some(bus) = st.bus.as_deref_mut() else {
                return;
            };
            // sum inputs in their fixed order, 64-bit
            let [al, ar] = &mut bus.acc;
            al[..n].fill(0.0);
            ar[..n].fill(0.0);
            for (ei, e) in node.inputs.iter().enumerate() {
                // SAFETY: sources completed before this node became ready.
                let Some((sl, sr)) = (unsafe { output_of(plan, e.src) }) else {
                    continue;
                };
                // an automated send level is smoothed per frame (the edge ramp)
                if let Some(ai) = e.auto
                    && let Some(ramp) = st.edge_ramps.get_mut(ei)
                {
                    // SAFETY: the scheduling phase is over.
                    let f = unsafe { plan.autos[ai].frames() };
                    for k in 0..n {
                        let target = match f {
                            Some(f) if !f[k].is_nan() => f[k].clamp(0.0, 2.0) as f32,
                            _ => e.gain,
                        };
                        ramp.set(target);
                        let g = ramp.next() as f64;
                        al[k] += sl[k] as f64 * g;
                        ar[k] += sr[k] as f64 * g;
                    }
                    continue;
                }
                let g = e.gain as f64;
                for k in 0..n {
                    al[k] += sl[k] as f64 * g;
                    ar[k] += sr[k] as f64 * g;
                }
            }
            for k in 0..n {
                bus.buf[0][k] = al[k] as f32;
                bus.buf[1][k] = ar[k] as f32;
            }
            // SAFETY: as above.
            let sc = def.sidechain.and_then(|s| unsafe { output_of(plan, s) });
            for (k, slot) in def.slots.iter().enumerate() {
                let Some(es) = st.effects.get_mut(k) else {
                    break;
                };
                let Some(p) = es.proc.as_mut() else {
                    continue;
                };
                if slot.bypass {
                    continue;
                }
                let te = Instant::now();
                {
                    let [bl, br] = &bus.buf;
                    let main: [&[f32]; 2] = [&bl[..n], &br[..n]];
                    let side: [&[f32]; 2] = match sc {
                        Some((x, y)) => [&x[..n], &y[..n]],
                        None => [&bus.zeros[..n], &bus.zeros[..n]],
                    };
                    let ports: [&[&[f32]]; 2] = [&main, &side];
                    let [wl, wr] = &mut bus.wet;
                    let mut ch: [&mut [f32]; 2] = [&mut wl[..n], &mut wr[..n]];
                    let mut outs: [&mut [&mut [f32]]; 1] = [&mut ch];
                    let mut io = ProcessIo {
                        inputs: &ports[..slot.inputs],
                        outputs: &mut outs,
                        events: &es.events,
                    };
                    let _ = p.process(&pctx, &mut io);
                }
                es.mix.set(slot.mix);
                if es.mix.settled() && es.mix.cur == 1.0 {
                    std::mem::swap(&mut bus.buf, &mut bus.wet);
                } else {
                    for k in 0..n {
                        let m = es.mix.next();
                        for c in 0..2 {
                            let d = bus.buf[c][k];
                            bus.buf[c][k] = d + (bus.wet[c][k] - d) * m;
                        }
                    }
                }
                es.micros += (te.elapsed().as_secs_f32() * 1e6 - es.micros) * 0.05;
            }
            // SAFETY: the scheduling phase is over.
            let (av, ap, am) = unsafe {
                (
                    def.auto_vol.and_then(|i| plan.autos[i].frames()),
                    def.auto_pan.and_then(|i| plan.autos[i].frames()),
                    def.auto_mute.and_then(|i| plan.autos[i].frames()),
                )
            };
            if av.is_none() && ap.is_none() && am.is_none() {
                bus.ramp[0].set(def.gain[0]);
                bus.ramp[1].set(def.gain[1]);
                for k in 0..n {
                    bus.out[0][k] = bus.buf[0][k] * bus.ramp[0].next();
                    bus.out[1][k] = bus.buf[1][k] * bus.ramp[1].next();
                }
            } else {
                let mut cache = (f32::NAN, f32::NAN, false, def.gain);
                for k in 0..n {
                    let vol = pick(av, k, def.volume);
                    let pan = pick(ap, k, def.pan);
                    let muted = match am {
                        Some(f) if !f[k].is_nan() => f[k] >= 0.5,
                        _ => def.muted,
                    };
                    if vol.to_bits() != cache.0.to_bits()
                        || pan.to_bits() != cache.1.to_bits()
                        || muted != cache.2
                    {
                        cache = (vol, pan, muted, strip_gain(vol, pan, muted));
                    }
                    bus.ramp[0].set(cache.3[0]);
                    bus.ramp[1].set(cache.3[1]);
                    bus.out[0][k] = bus.buf[0][k] * bus.ramp[0].next();
                    bus.out[1][k] = bus.buf[1][k] * bus.ramp[1].next();
                }
            }
            if ctx.meters {
                for c in 0..2 {
                    let mut peak = bus.peak[c];
                    let mut sq = 0.0f64;
                    for &v in &bus.out[c][..n] {
                        peak = peak.max(v.abs());
                        sq += v as f64 * v as f64;
                    }
                    bus.peak[c] = peak;
                    bus.sumsq[c] += sq;
                }
                bus.metered += n;
            }
            bus.micros += (t0.elapsed().as_secs_f32() * 1e6 - bus.micros) * 0.05;
        }
        _ => {}
    }
}

/// Frame `k` of an automation buffer, or `fallback` (the model value) where no clip
/// covers the frame.
#[inline]
fn pick(frames: Option<&[f64]>, k: usize, fallback: f32) -> f32 {
    match frames {
        Some(f) if !f[k].is_nan() => f[k] as f32,
        _ => fallback,
    }
}

/// Output buffers of a finished node.
///
/// # Safety
/// Node `i` must have completed for this chunk and not be written concurrently.
unsafe fn output_of(plan: &Plan, i: usize) -> Option<(&[f32], &[f32])> {
    match unsafe { plan.state(i) } {
        NodeState::Channel(Some(c)) => Some((&c.out[0], &c.out[1])),
        NodeState::Insert(s) => s.bus.as_ref().map(|b| (&b.out[0][..], &b.out[1][..])),
        NodeState::AudioTrack(Some(t)) => Some((&t.out[0], &t.out[1])),
        NodeState::Channel(None) | NodeState::AudioTrack(None) => None,
    }
}
