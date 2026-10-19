//! Render plans: the compiled, audio-thread-ready form of a project snapshot.
//!
//! A plan is a DAG of nodes in topological order. M1's default layout is FL-style:
//!
//! ```text
//! channel (instrument → strip: volume, pan, mute) ─┐
//! channel ─────────────────────────────────────────┼→ insert (sum → effect slots → volume,
//!                                                  │   pan, mute → meter) ─┬→ sends ─→ insert …
//!                     insert (sidechain source) ───┘ (sidechain port)      └→ master → device
//! ```
//!
//! Every node lists its inputs in a fixed order, and a node sums them itself, in that
//! order, into 64-bit accumulators. The sum therefore never depends on which worker thread
//! finished first, which is what makes multi-threaded output bit-identical to
//! single-threaded output.
//!
//! Plans are compiled off the audio thread ([`compile`]) against a [`Shadow`] of the plan
//! the audio thread currently runs. Unchanged channels and effects are not rebuilt: the new
//! plan names them in [`Transfer`]s and the audio thread moves their state (processor,
//! sounding voices, buffers, ramps) across at the swap. Parameter-only edits become
//! [`ParamSet`]s applied at the swap. Only new or changed instruments/effects get fresh,
//! already-activated instances.

use crate::audio_clips::{AudioClipPlan, plan_audio_clips};
use crate::automation::AutomationEvaluator;
use crate::player::{AudioCtx, AudioTrackState, TrackKey, build_track};
use crate::plugins::{LivePlugin, PluginEnv, PluginSlot, effect_plugin, instrument_plugin};
use crate::sched::Sched;
use crate::stats::StatsBoard;
use crate::tempo::TempoMap;
use crate::{EngineError, EngineResult};
use ph_core::{
    ChannelId, EffectSpec, InstrumentSpec, ObjectPath, PatternId, Project, STEP, Tick,
    TimeSignature,
};
use ph_node::{ActivateInfo, AssetResolver, Event, NodeDescriptor, ParamId, Processor, Quality};
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Events one channel can receive in one block (excess note-ons are dropped).
pub(crate) const EVENT_CAP: usize = 1024;
/// Notes one channel can hold sounding at once (the oldest is released past this).
pub(crate) const ACTIVE_CAP: usize = 256;
/// Preview note commands queued per channel between blocks.
pub(crate) const PREVIEW_CAP: usize = 64;
/// Length of gain/mix ramps in frames (click-free volume, pan, mute and mix changes).
pub(crate) const RAMP_FRAMES: u32 = 64;
/// Choke groups are numbered 1..=16 (sampler `choke_group`; 0 = none).
pub(crate) const CHOKE_GROUPS: usize = 16;
/// Tempo automation becomes a tempo map with one constant step per this many ticks (a 64th
/// note), so tempo-synced audio clips and notes follow tempo changes.
pub(crate) const TEMPO_RESOLUTION: Tick = ph_core::PPQ / 16;

/// Everything that requires re-activating processors when it changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub sample_rate: u32,
    pub max_frames: usize,
    pub quality: Quality,
}

/// A scheduled note in a channel's stream, stamped in timeline samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NoteEv {
    pub start: i64,
    pub end: i64,
    pub key: u8,
    pub velocity: f32,
    pub pan: f32,
    pub fine_pitch: f32,
}

/// Linear per-sample ramp toward a target. Per-sample, so block-size independent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ramp {
    pub cur: f32,
    target: f32,
    step: f32,
    left: u32,
}

impl Ramp {
    pub fn new(v: f32) -> Self {
        Self {
            cur: v,
            target: v,
            step: 0.0,
            left: 0,
        }
    }

    #[inline]
    pub fn set(&mut self, t: f32) {
        if t != self.target {
            self.target = t;
            self.step = (t - self.cur) / RAMP_FRAMES as f32;
            self.left = RAMP_FRAMES;
        }
    }

    #[inline]
    pub fn settled(&self) -> bool {
        self.left == 0
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            if self.left == 0 {
                self.cur = self.target;
            } else {
                self.cur += self.step;
            }
        }
        self.cur
    }
}

/// A sounding note of a channel.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Active {
    pub id: u32,
    pub key: u8,
    pub start: i64,
    /// Timeline sample of its note-off (`i64::MAX` for held preview notes).
    pub end: i64,
    pub preview: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Preview {
    pub key: u8,
    pub velocity: f32,
    pub on: bool,
}

/// Audio-thread state of a channel; moves between plans while the channel is unchanged.
pub(crate) struct ChannelState {
    pub proc: Option<Box<dyn Processor>>,
    pub out: [Vec<f32>; 2],
    pub events: Vec<Event>,
    pub active: Vec<Active>,
    pub preview: Vec<Preview>,
    pub next_id: u32,
    pub ramp: [Ramp; 2],
    pub all_off: bool,
    pub release_all: bool,
    pub cursor: usize,
    pub micros: f32,
    /// Voice-limit parameter and its value before overload shedding lowered it.
    pub poly: Option<ParamId>,
    pub poly_saved: Option<f64>,
}

/// Audio-thread state of a mixer insert's bus (not its effects).
pub(crate) struct BusState {
    pub acc: [Vec<f64>; 2],
    pub buf: [Vec<f32>; 2],
    pub wet: [Vec<f32>; 2],
    pub out: [Vec<f32>; 2],
    pub zeros: Vec<f32>,
    pub ramp: [Ramp; 2],
    pub micros: f32,
    /// Meter accumulation since the last publish.
    pub peak: [f32; 2],
    pub sumsq: [f64; 2],
    pub metered: usize,
}

pub(crate) struct EffectState {
    pub proc: Option<Box<dyn Processor>>,
    pub mix: Ramp,
    pub micros: f32,
}

pub(crate) struct InsertState {
    pub bus: Option<Box<BusState>>,
    pub effects: Vec<EffectState>,
}

pub(crate) enum NodeState {
    Channel(Option<Box<ChannelState>>),
    Insert(InsertState),
    /// A playlist track's audio clips (M2/T1); `None` once moved to a newer plan.
    AudioTrack(Option<Box<AudioTrackState>>),
}

pub(crate) struct ChannelDef {
    /// Strip gains `[left, right]` (volume × pan balance; 0 when muted).
    pub gain: [f32; 2],
    pub choke: u8,
}

pub(crate) struct SlotDef {
    pub bypass: bool,
    pub mix: f32,
    /// Number of input ports the processor declares (1, or 2 with a sidechain).
    pub inputs: usize,
}

pub(crate) struct InsertDef {
    pub insert: u16,
    pub gain: [f32; 2],
    pub slots: Vec<SlotDef>,
    /// Node whose output feeds the effects' sidechain port.
    pub sidechain: Option<usize>,
}

pub(crate) enum NodeDef {
    Channel(ChannelDef),
    Insert(InsertDef),
    /// Plays one playlist track's audio clips into the master insert.
    AudioTrack,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Edge {
    pub src: usize,
    pub gain: f32,
}

pub(crate) struct Node {
    pub def: NodeDef,
    /// Main inputs, summed in this order.
    pub inputs: Vec<Edge>,
    pub dependents: Vec<u32>,
    pub indegree: u32,
    /// Rendered at device latency (true) or ahead of the playhead (anticipative, M4).
    pub live: bool,
    /// Index of this node's entry in the stats board; effect slots follow it.
    pub stat: usize,
    pub state: UnsafeCell<NodeState>,
}

/// Moves state from the running plan into the new one at the swap (audio thread).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Transfer {
    Channel {
        from: usize,
        to: usize,
    },
    Bus {
        from: usize,
        to: usize,
    },
    Effect {
        from: (usize, usize),
        to: (usize, usize),
    },
    AudioTrack {
        from: usize,
        to: usize,
    },
}

/// A parameter change for a reused processor, applied at the swap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParamSet {
    pub node: usize,
    /// `None` = the channel's instrument, `Some(i)` = effect slot `i`.
    pub slot: Option<usize>,
    pub id: ParamId,
    pub value: f64,
}

pub(crate) struct PatternStream {
    pub id: PatternId,
    pub len_samples: i64,
    /// Indexed by channel node.
    pub notes: Vec<Vec<NoteEv>>,
}

pub(crate) struct Plan {
    pub id: u64,
    /// Id of the plan this one was compiled against (its transfers refer to that plan).
    pub base: u64,
    pub settings: Settings,
    pub tempo: TempoMap,
    pub time_sig: TimeSignature,
    pub nodes: Vec<Node>,
    /// Channel nodes are `0..n_channels`; the master is the last node.
    pub n_channels: usize,
    /// Sorted by channel id.
    pub channel_lookup: Vec<(ChannelId, usize)>,
    /// Channel nodes per choke group (index = group; 0 unused).
    pub choke: Vec<Vec<usize>>,
    /// Song-mode streams, indexed by channel node.
    pub song: Vec<Vec<NoteEv>>,
    pub song_len: i64,
    /// Sorted by pattern id.
    pub patterns: Vec<PatternStream>,
    /// Fallback loop length for a pattern that no longer exists (one bar).
    pub bar_samples: i64,
    pub transfers: Vec<Transfer>,
    pub params: Vec<ParamSet>,
    pub roots: Vec<u32>,
    pub sched: Sched,
    pub board: Arc<StatsBoard>,
}

// SAFETY: node states sit in `UnsafeCell`s. The scheduler hands each node to exactly one
// thread per block, a node only reads the outputs of nodes that completed before it
// became ready (ordered by the acquire/release dependency counters), and the audio thread
// touches states only between blocks while no worker holds the plan. Processors are
// `Send`.
unsafe impl Sync for Plan {}
unsafe impl Send for Plan {}

impl Plan {
    pub fn master(&self) -> usize {
        self.nodes.len() - 1
    }

    pub fn pattern(&self, id: PatternId) -> Option<&PatternStream> {
        self.patterns
            .binary_search_by_key(&id, |p| p.id)
            .ok()
            .map(|i| &self.patterns[i])
    }

    pub fn channel_node(&self, id: ChannelId) -> Option<usize> {
        self.channel_lookup
            .binary_search_by_key(&id, |c| c.0)
            .ok()
            .map(|i| self.channel_lookup[i].1)
    }

    /// # Safety
    /// The caller must have exclusive access to node `i` (see the `Sync` impl).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn state_mut(&self, i: usize) -> &mut NodeState {
        unsafe { &mut *self.nodes[i].state.get() }
    }

    /// # Safety
    /// Node `i` must not be written concurrently.
    pub unsafe fn state(&self, i: usize) -> &NodeState {
        unsafe { &*self.nodes[i].state.get() }
    }

    /// # Safety
    /// As [`Self::state_mut`].
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn channel_mut(&self, i: usize) -> Option<&mut ChannelState> {
        match unsafe { self.state_mut(i) } {
            NodeState::Channel(c) => c.as_deref_mut(),
            NodeState::Insert(_) | NodeState::AudioTrack(_) => None,
        }
    }
}

// ------------------------------------------------------------------------------- shadow

/// What the control side remembers about a plan it handed to the audio thread, to decide
/// what the next plan can reuse.
#[derive(Clone)]
pub(crate) struct Shadow {
    pub id: u64,
    pub settings: Option<Settings>,
    pub channels: BTreeMap<ChannelId, ChannelShadow>,
    pub inserts: BTreeMap<u16, InsertShadow>,
    pub tracks: BTreeMap<u32, TrackShadow>,
    /// Plugin nodes of the plan (M2/T3), for the control side.
    pub plugins: BTreeMap<PluginSlot, LivePlugin>,
}

/// An audio-track node: reused while its clips (and the tempo map) are unchanged.
#[derive(Clone)]
pub(crate) struct TrackShadow {
    pub node: usize,
    pub key: Arc<TrackKey>,
}

#[derive(Clone)]
pub(crate) struct ChannelShadow {
    pub node: usize,
    /// Reuse key: instrument kind plus anything that needs a new instance (sample, preset).
    pub identity: Option<String>,
    pub params: BTreeMap<String, f64>,
    pub desc: Option<Arc<NodeDescriptor>>,
}

#[derive(Clone)]
pub(crate) struct InsertShadow {
    pub node: usize,
    pub slots: Vec<SlotShadow>,
}

#[derive(Clone)]
pub(crate) struct SlotShadow {
    pub identity: Option<String>,
    pub params: BTreeMap<String, f64>,
    pub desc: Option<Arc<NodeDescriptor>>,
}

impl Shadow {
    pub fn empty() -> Self {
        Self {
            id: 0,
            settings: None,
            channels: BTreeMap::new(),
            inserts: BTreeMap::new(),
            tracks: BTreeMap::new(),
            plugins: BTreeMap::new(),
        }
    }
}

/// Extra compile inputs that are not part of the project (session state).
#[derive(Clone, Debug, Default)]
pub(crate) struct CompileOptions {
    /// Hook for solo (session-local, set by the UI later): when non-empty, channels not in
    /// the list are muted in the strip.
    pub solo_channels: Vec<ChannelId>,
    /// Plugin hosting (M2/T3); `None` = plugin specs play silence / pass through.
    pub plugins: Option<PluginEnv>,
}

fn instrument_identity(spec: &InstrumentSpec) -> Option<String> {
    match spec {
        InstrumentSpec::Sampler { sample, slices, .. } => Some(format!(
            "sampler:{}{}",
            sample.as_deref().unwrap_or(""),
            // slices are baked into the node, so editing them rebuilds it
            if slices.is_empty() {
                String::new()
            } else {
                format!(
                    ":slices={}",
                    slices
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        )),
        InstrumentSpec::Synth { preset, .. } => {
            Some(format!("synth:{}", preset.as_deref().unwrap_or("")))
        }
        InstrumentSpec::Plugin {
            format,
            plugin_id,
            state,
            ..
        } => Some(format!(
            "plugin:{format:?}:{plugin_id}:{}",
            state.as_deref().unwrap_or("")
        )),
        InstrumentSpec::MultiSampler { .. } => None,
    }
}

fn instrument_params(spec: &InstrumentSpec) -> BTreeMap<String, f64> {
    match spec {
        InstrumentSpec::Sampler {
            root_key, params, ..
        } => {
            let mut p = params.clone();
            p.insert("root_key".into(), *root_key as f64);
            p
        }
        InstrumentSpec::Synth { params, .. } => params.clone(),
        InstrumentSpec::MultiSampler { params, .. } => params.clone(),
        InstrumentSpec::Plugin { params, .. } => params.clone(),
    }
}

fn effect_identity(spec: &EffectSpec) -> Option<String> {
    match spec {
        EffectSpec::Builtin { name, .. } => {
            Some(name.strip_prefix("ph.fx.").unwrap_or(name).to_string())
        }
        EffectSpec::Plugin {
            format,
            plugin_id,
            state,
            ..
        } => Some(format!(
            "plugin:{format:?}:{plugin_id}:{}",
            state.as_deref().unwrap_or("")
        )),
    }
}

fn effect_params(spec: &EffectSpec) -> BTreeMap<String, f64> {
    match spec {
        EffectSpec::Builtin { params, .. } => params.clone(),
        EffectSpec::Plugin { params, .. } => params.clone(),
    }
}

/// Parameter changes turning `old` into `new` (removed keys go back to their default).
fn param_diff(
    old: &BTreeMap<String, f64>,
    new: &BTreeMap<String, f64>,
    desc: &NodeDescriptor,
) -> Vec<(ParamId, f64)> {
    let mut out = Vec::new();
    for (k, v) in new {
        if old.get(k) != Some(v)
            && let Some(id) = desc.param_id(k)
        {
            out.push((id, *v));
        }
    }
    for k in old.keys() {
        if !new.contains_key(k)
            && let Some(id) = desc.param_id(k)
        {
            out.push((id, desc.params[id.0 as usize].default));
        }
    }
    out
}

/// Deterministic per-object seed (same object → same seed in every render).
fn seed(kind: u64, a: u64, b: u64) -> u64 {
    let mut x = kind
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(a.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(b.wrapping_mul(0x94d0_49bb_1331_11eb));
    x ^= x >> 31;
    x = x.wrapping_mul(0xd6e8_feb8_6659_fd93);
    x ^ (x >> 32)
}

/// Strip gains from volume, balance-law pan and mute.
/// Exponent of the channel volume knob: `gain = v ^ k`, fitted so FL's two documented
/// anchors hold exactly: 100% = 0 dB, and the 78% default = -5.2 dB
/// (`docs/formats/fl-behaviour-notes.md` §1; the shape between them is owner check OC-1..3).
pub(crate) const CHANNEL_VOLUME_EXPONENT: f32 = 2.409;

/// A channel strip: FL's channel volume curve and its default Circular pan law
/// (constant power, -3 dB per side at centre, 0 dB on the panned side at the extremes;
/// fl-behaviour-notes §2).
pub(crate) fn channel_gain(volume: f32, pan: f32, muted: bool) -> [f32; 2] {
    if muted {
        return [0.0, 0.0];
    }
    let g = volume.clamp(0.0, 1.0).powf(CHANNEL_VOLUME_EXPONENT);
    let theta = (pan.clamp(-1.0, 1.0) as f64 + 1.0) * std::f64::consts::FRAC_PI_4;
    [g * theta.cos() as f32, g * theta.sin() as f32]
}

/// A mixer insert: linear fader gain and a balance pan. FL's fader scale and mixer pan
/// law are owner checks OC-6..OC-9 (fl-behaviour-notes §3, §4), so this stays as it was.
fn strip_gain(volume: f32, pan: f32, muted: bool) -> [f32; 2] {
    if muted {
        return [0.0, 0.0];
    }
    let v = volume.max(0.0);
    let p = pan.clamp(-1.0, 1.0);
    [v * (1.0 - p).min(1.0), v * (1.0 + p).min(1.0)]
}

fn zeros(n: usize) -> [Vec<f32>; 2] {
    [vec![0.0; n], vec![0.0; n]]
}

fn new_channel_state(proc: Option<Box<dyn Processor>>, n: usize, gain: [f32; 2]) -> ChannelState {
    let poly = proc
        .as_ref()
        .and_then(|p| p.descriptor().param_id("polyphony"));
    ChannelState {
        proc,
        out: zeros(n),
        events: Vec::with_capacity(EVENT_CAP),
        active: Vec::with_capacity(ACTIVE_CAP),
        preview: Vec::with_capacity(PREVIEW_CAP),
        next_id: 0,
        ramp: [Ramp::new(gain[0]), Ramp::new(gain[1])],
        all_off: false,
        release_all: false,
        cursor: 0,
        micros: 0.0,
        poly,
        poly_saved: None,
    }
}

fn new_bus(n: usize, gain: [f32; 2]) -> BusState {
    BusState {
        acc: [vec![0.0; n], vec![0.0; n]],
        buf: zeros(n),
        wet: zeros(n),
        out: zeros(n),
        zeros: vec![0.0; n],
        ramp: [Ramp::new(gain[0]), Ramp::new(gain[1])],
        micros: 0.0,
        peak: [0.0; 2],
        sumsq: [0.0; 2],
        metered: 0,
    }
}

enum Key {
    Channel(usize),
    Track(u32),
    Insert(u16),
}

/// Compiles a project into a plan, reusing what it can from `prev` (the plan the audio
/// thread runs). Never touches the audio thread.
pub(crate) fn compile(
    project: &Project,
    prev: &Shadow,
    settings: Settings,
    opts: &CompileOptions,
    assets: &dyn AssetResolver,
    audio: &mut AudioCtx,
    id: u64,
) -> EngineResult<(Box<Plan>, Shadow)> {
    let n = settings.max_frames.max(1);
    let reuse = prev.settings == Some(settings);
    // tempo automation (if any) is part of the tempo map: notes and synced audio follow it
    let tempo = AutomationEvaluator::new(project).tempo_map(
        project.settings.bpm,
        settings.sample_rate,
        TEMPO_RESOLUTION,
    );
    // audio clips, by playlist track
    let (clip_plans, _) = plan_audio_clips(project, &tempo);
    let mut by_track: BTreeMap<u32, Vec<AudioClipPlan>> = BTreeMap::new();
    for p in clip_plans {
        by_track.entry(p.track).or_default().push(p);
    }
    let n_inserts = project.mixer.len();
    if n_inserts == 0 {
        return Err(EngineError::Plan("project has no master insert".into()));
    }

    // ---- channels, in rack order (then any not listed, by id)
    let mut chans: Vec<&ph_core::Channel> = project.channels_in_order().collect();
    for (cid, ch) in project.channels.iter() {
        if !project.channel_order.contains(cid) {
            chans.push(ch);
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    chans.retain(|c| seen.insert(c.id));
    let n_ch = chans.len();
    let target = |c: &ph_core::Channel| -> u16 {
        if (c.insert.0 as usize) < n_inserts {
            c.insert.0 as u16
        } else {
            0
        }
    };

    // ---- inserts in use: fed by a channel, transitively through sends/routing
    let mut used = vec![false; n_inserts];
    used[0] = true;
    let mut stack: Vec<usize> = chans.iter().map(|c| target(c) as usize).collect();
    while let Some(i) = stack.pop() {
        if used[i] {
            continue;
        }
        used[i] = true;
        for s in &project.mixer[i].sends {
            let t = s.to.0 as usize;
            if t < n_inserts && t != i {
                stack.push(t);
            }
        }
    }
    let ins_list: Vec<u16> = (1..n_inserts as u16)
        .filter(|i| used[*i as usize])
        .collect();

    // ---- edges between inserts (main and sidechain), master excluded as a source
    // main inputs of insert x: (source insert, gain), in source order
    let mut ins_inputs: BTreeMap<u16, Vec<(u16, f32)>> = BTreeMap::new();
    let mut sidechain: BTreeMap<u16, u16> = BTreeMap::new();
    for &y in &ins_list {
        let ins = &project.mixer[y as usize];
        if ins.route_to_master {
            ins_inputs.entry(0).or_default().push((y, 1.0));
        }
        for s in &ins.sends {
            let t = s.to.0 as usize;
            if t < n_inserts && t != y as usize && used[t] {
                ins_inputs.entry(t as u16).or_default().push((y, s.level));
            }
        }
    }
    for &x in &ins_list {
        if let Some(s) = project.mixer[x as usize].sidechain_from {
            let s = s.0 as usize;
            if s != 0 && s < n_inserts && s != x as usize && used[s] {
                sidechain.insert(x, s as u16);
            }
        }
    }
    // the inputs of each insert are ordered by source insert number, then declaration
    for v in ins_inputs.values_mut() {
        v.sort_by_key(|e| e.0);
    }

    // ---- topological order of the non-master inserts (Kahn, smallest number first)
    let mut indeg: BTreeMap<u16, u32> = ins_list.iter().map(|i| (*i, 0)).collect();
    let mut outs: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
    for (&x, srcs) in &ins_inputs {
        if x == 0 {
            continue;
        }
        for (y, _) in srcs {
            *indeg.get_mut(&x).unwrap() += 1;
            outs.entry(*y).or_default().push(x);
        }
    }
    for (&x, &y) in &sidechain {
        *indeg.get_mut(&x).unwrap() += 1;
        outs.entry(y).or_default().push(x);
    }
    let mut ready: std::collections::BTreeSet<u16> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| *i)
        .collect();
    let mut order = Vec::with_capacity(ins_list.len() + 1);
    while let Some(&x) = ready.iter().next() {
        ready.remove(&x);
        order.push(x);
        for y in outs.get(&x).into_iter().flatten() {
            let d = indeg.get_mut(y).unwrap();
            *d -= 1;
            if *d == 0 {
                ready.insert(*y);
            }
        }
    }
    if order.len() != ins_list.len() {
        let stuck: Vec<String> = indeg
            .iter()
            .filter(|(i, d)| **d > 0 && !order.contains(i))
            .map(|(i, _)| i.to_string())
            .collect();
        return Err(EngineError::Plan(format!(
            "mixer routing has a feedback loop through inserts {}",
            stuck.join(", ")
        )));
    }
    order.push(0);

    // ---- node keys and indices: channels, audio tracks, inserts
    let n_tr = by_track.len();
    let mut keys: Vec<Key> = (0..n_ch).map(Key::Channel).collect();
    keys.extend(by_track.keys().map(|t| Key::Track(*t)));
    keys.extend(order.iter().map(|i| Key::Insert(*i)));
    let ins_node: BTreeMap<u16, usize> = order
        .iter()
        .enumerate()
        .map(|(k, i)| (*i, n_ch + n_tr + k))
        .collect();

    let solo = &opts.solo_channels;
    let mut board = StatsBoard::default();
    let mut nodes = Vec::with_capacity(keys.len());
    let mut transfers = Vec::new();
    let mut params = Vec::new();
    let mut shadow = Shadow {
        id,
        settings: Some(settings),
        channels: BTreeMap::new(),
        inserts: BTreeMap::new(),
        tracks: BTreeMap::new(),
        plugins: BTreeMap::new(),
    };
    let info = |seed: u64| ActivateInfo {
        sample_rate: settings.sample_rate as f64,
        max_frames: n,
        quality: settings.quality,
        seed,
    };

    for (idx, key) in keys.iter().enumerate() {
        match key {
            Key::Channel(ci) => {
                let ch = chans[*ci];
                let identity = instrument_identity(&ch.instrument);
                let ps = instrument_params(&ch.instrument);
                let muted = ch.muted || (!solo.is_empty() && !solo.contains(&ch.id));
                let gain = channel_gain(ch.volume, ch.pan, muted);
                let choke = match &ch.instrument {
                    InstrumentSpec::Sampler { params, .. } => params
                        .get("choke_group")
                        .map(|g| g.round().clamp(0.0, CHOKE_GROUPS as f64) as u8)
                        .unwrap_or(0),
                    _ => 0,
                };
                let old = prev.channels.get(&ch.id).filter(|o| {
                    reuse && o.identity.is_some() && o.identity == identity && o.desc.is_some()
                });
                let (state, desc) = if let Some(o) = old {
                    transfers.push(Transfer::Channel {
                        from: o.node,
                        to: idx,
                    });
                    let slot = PluginSlot::Channel(ch.id);
                    if let Some(lp) = prev.plugins.get(&slot) {
                        shadow.plugins.insert(slot, lp.clone());
                    }
                    let desc = o.desc.clone().expect("filtered");
                    for (pid, v) in param_diff(&o.params, &ps, &desc) {
                        params.push(ParamSet {
                            node: idx,
                            slot: None,
                            id: pid,
                            value: v,
                        });
                    }
                    (None, Some(desc))
                } else {
                    let proc = match instrument_plugin(&ch.instrument) {
                        Some((format, id, state_ref)) => {
                            let slot = PluginSlot::Channel(ch.id);
                            crate::plugins::create(
                                opts.plugins.as_ref(),
                                slot,
                                format,
                                id,
                                state_ref,
                                &ps,
                                &info(seed(1, ch.id.0 as u64, 0)),
                                assets,
                            )
                            .ok()
                            .map(|(p, live)| {
                                shadow.plugins.insert(slot, live);
                                p
                            })
                        }
                        None => ph_instruments::create(&ch.instrument, assets)
                            .ok()
                            .map(|mut p| {
                                p.activate(&info(seed(1, ch.id.0 as u64, 0)));
                                p
                            }),
                    };
                    let desc = proc.as_ref().map(|p| Arc::new(p.descriptor().clone()));
                    (Some(Box::new(new_channel_state(proc, n, gain))), desc)
                };
                shadow.channels.insert(
                    ch.id,
                    ChannelShadow {
                        node: idx,
                        identity,
                        params: ps,
                        desc,
                    },
                );
                let stat = board.push(ch.name.clone(), Some(ObjectPath::Channel(ch.id)));
                nodes.push(Node {
                    def: NodeDef::Channel(ChannelDef { gain, choke }),
                    inputs: Vec::new(),
                    dependents: Vec::new(),
                    indegree: 0,
                    live: true,
                    stat,
                    state: UnsafeCell::new(NodeState::Channel(state)),
                });
            }
            Key::Track(t) => {
                let plans = &by_track[t];
                let key = Arc::new(TrackKey::new(plans.clone(), tempo.clone()));
                let old = prev.tracks.get(t).filter(|o| reuse && *o.key == *key);
                let state = if let Some(o) = old {
                    transfers.push(Transfer::AudioTrack {
                        from: o.node,
                        to: idx,
                    });
                    None
                } else {
                    let mut warnings = Vec::new();
                    Some(Box::new(build_track(
                        *t,
                        plans,
                        &tempo,
                        settings.sample_rate,
                        n,
                        settings.quality,
                        audio,
                        assets,
                        &mut warnings,
                    )))
                };
                shadow.tracks.insert(*t, TrackShadow { node: idx, key });
                let name = project
                    .tracks
                    .get(*t as usize)
                    .map_or_else(|| format!("Track {}", t + 1), |tr| tr.name.clone());
                let stat = board.push(format!("{name} (audio clips)"), Some(ObjectPath::Track(*t)));
                nodes.push(Node {
                    def: NodeDef::AudioTrack,
                    inputs: Vec::new(),
                    dependents: Vec::new(),
                    indegree: 0,
                    live: true,
                    stat,
                    state: UnsafeCell::new(NodeState::AudioTrack(state)),
                });
            }
            Key::Insert(x) => {
                let ins = &project.mixer[*x as usize];
                let gain = strip_gain(ins.volume, ins.pan, ins.muted);
                // inputs: channels (rack order), audio tracks into the master (by track),
                // then inserts (by number)
                let mut inputs: Vec<Edge> = (0..n_ch)
                    .filter(|c| target(chans[*c]) == *x)
                    .map(|c| Edge { src: c, gain: 1.0 })
                    .collect();
                if *x == 0 {
                    inputs.extend((n_ch..n_ch + n_tr).map(|src| Edge { src, gain: 1.0 }));
                }
                for (y, g) in ins_inputs.get(x).into_iter().flatten() {
                    inputs.push(Edge {
                        src: ins_node[y],
                        gain: *g,
                    });
                }
                let sc = sidechain.get(x).map(|s| ins_node[s]);
                let old = prev.inserts.get(x).filter(|_| reuse);
                let bus = if let Some(o) = old {
                    transfers.push(Transfer::Bus {
                        from: o.node,
                        to: idx,
                    });
                    None
                } else {
                    Some(Box::new(new_bus(n, gain)))
                };
                let stat = board.push(
                    ins.name.clone(),
                    Some(ObjectPath::Insert(ph_core::InsertId(*x as u32))),
                );
                let mut slots = Vec::new();
                let mut effects = Vec::new();
                let mut slot_shadows = Vec::new();
                for (si, slot) in ins.effects.iter().enumerate() {
                    let identity = effect_identity(&slot.effect);
                    let ps = effect_params(&slot.effect);
                    let old_slot = old.and_then(|o| o.slots.get(si)).filter(|s| {
                        s.identity.is_some() && s.identity == identity && s.desc.is_some()
                    });
                    let mix = slot.mix.clamp(0.0, 1.0);
                    let pslot = PluginSlot::Effect {
                        insert: *x as u32,
                        slot: si as u32,
                    };
                    let (proc, desc) = if let Some(os) = old_slot {
                        transfers.push(Transfer::Effect {
                            from: (old.unwrap().node, si),
                            to: (idx, si),
                        });
                        if let Some(lp) = prev.plugins.get(&pslot) {
                            shadow.plugins.insert(pslot, lp.clone());
                        }
                        let desc = os.desc.clone().expect("filtered");
                        for (pid, v) in param_diff(&os.params, &ps, &desc) {
                            params.push(ParamSet {
                                node: idx,
                                slot: Some(si),
                                id: pid,
                                value: v,
                            });
                        }
                        (None, Some(desc))
                    } else {
                        let p = match effect_plugin(&slot.effect) {
                            Some((format, id, state_ref)) => crate::plugins::create(
                                opts.plugins.as_ref(),
                                pslot,
                                format,
                                id,
                                state_ref,
                                &ps,
                                &info(seed(2, *x as u64, si as u64)),
                                assets,
                            )
                            .ok()
                            .map(|(p, live)| {
                                shadow.plugins.insert(pslot, live);
                                p
                            }),
                            None => ph_effects::create(&slot.effect).ok().map(|mut p| {
                                p.activate(&info(seed(2, *x as u64, si as u64)));
                                p
                            }),
                        };
                        let d = p.as_ref().map(|p| Arc::new(p.descriptor().clone()));
                        (p, d)
                    };
                    let inputs = desc
                        .as_ref()
                        .map_or(1, |d| d.audio_inputs.len().clamp(1, 2));
                    let label = match &slot.effect {
                        EffectSpec::Builtin { name, .. } => format!("{} / {name}", ins.name),
                        EffectSpec::Plugin { plugin_id, .. } => {
                            format!("{} / {plugin_id}", ins.name)
                        }
                    };
                    board.push(
                        label,
                        Some(ObjectPath::Insert(ph_core::InsertId(*x as u32))),
                    );
                    slots.push(SlotDef {
                        bypass: slot.bypass,
                        mix,
                        inputs,
                    });
                    effects.push(EffectState {
                        proc,
                        mix: Ramp::new(mix),
                        micros: 0.0,
                    });
                    slot_shadows.push(SlotShadow {
                        identity,
                        params: ps,
                        desc,
                    });
                }
                shadow.inserts.insert(
                    *x,
                    InsertShadow {
                        node: idx,
                        slots: slot_shadows,
                    },
                );
                nodes.push(Node {
                    def: NodeDef::Insert(InsertDef {
                        insert: *x,
                        gain,
                        slots,
                        sidechain: sc,
                    }),
                    inputs,
                    dependents: Vec::new(),
                    indegree: 0,
                    live: true,
                    stat,
                    state: UnsafeCell::new(NodeState::Insert(InsertState { bus, effects })),
                });
            }
        }
    }

    // ---- dependency counts
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        for e in &node.inputs {
            deps[i].push(e.src);
        }
        if let NodeDef::Insert(d) = &node.def
            && let Some(s) = d.sidechain
        {
            deps[i].push(s);
        }
        deps[i].sort_unstable();
        deps[i].dedup();
    }
    for (i, d) in deps.iter().enumerate() {
        nodes[i].indegree = d.len() as u32;
        for s in d {
            nodes[*s].dependents.push(i as u32);
        }
    }
    let roots: Vec<u32> = (0..nodes.len())
        .filter(|i| nodes[*i].indegree == 0)
        .map(|i| i as u32)
        .collect();

    // ---- note streams
    let swing = (project.settings.swing.clamp(0.0, 1.0) * 1000.0).round() as u128;
    let stamp = |t: Tick| -> i64 {
        // off-beat 16ths move later by swing × half a step (max: a dotted-16th feel)
        if swing > 0 && t % (2 * STEP) == STEP {
            tempo.sample_at_frac(t as u128 * 2000 + swing * STEP as u128, 2000)
        } else {
            tempo.sample_at(t)
        }
    };
    // note (start, end) in timeline ticks, clipped to `window_end`
    let place = |start: Tick, end: Tick, window_end: Tick| -> Option<(i64, i64)> {
        if start >= window_end {
            return None;
        }
        let s = stamp(start);
        let e_tick = end.min(window_end);
        let swung = s != tempo.sample_at(start);
        let e = if e_tick == window_end {
            tempo.sample_at(window_end)
        } else if swung {
            // keep the note's length when its start is swung
            tempo.sample_at(e_tick) + (s - tempo.sample_at(start))
        } else {
            tempo.sample_at(e_tick)
        };
        let e = e.min(tempo.sample_at(window_end));
        Some((s, e.max(s + 1)))
    };
    let mut patterns = Vec::new();
    let mut pattern_len: BTreeMap<PatternId, Tick> = BTreeMap::new();
    for (pid, pat) in project.patterns.iter() {
        let len = project.pattern_length(*pid).unwrap_or(0).max(1);
        pattern_len.insert(*pid, len);
        let notes = chans
            .iter()
            .map(|c| {
                let mut v: Vec<NoteEv> = pat
                    .notes
                    .get(&c.id)
                    .into_iter()
                    .flat_map(|m| m.values())
                    .filter(|n| !n.muted)
                    .filter_map(|n| {
                        let (s, e) = place(n.start, n.start + n.length, len)?;
                        Some(note_ev(s, e, n))
                    })
                    .collect();
                sort_stream(&mut v);
                v
            })
            .collect();
        patterns.push(PatternStream {
            id: *pid,
            len_samples: tempo.sample_at(len),
            notes,
        });
    }
    let mut clips: Vec<&ph_core::Clip> = project
        .clips
        .values()
        .filter(|c| {
            !project
                .tracks
                .get(c.track as usize)
                .is_some_and(|t| t.muted)
        })
        .collect();
    clips.sort_by_key(|c| (c.start, c.id));
    let song = chans
        .iter()
        .map(|ch| {
            let mut v = Vec::new();
            for clip in &clips {
                // audio and automation clips are not note sources (M2/T1, T7)
                let Some(pid) = clip.pattern() else {
                    continue;
                };
                let Some(pat) = project.patterns.get(&pid) else {
                    continue;
                };
                let plen = pattern_len.get(&pid).copied().unwrap_or(0);
                let win_end = clip.start + clip.length;
                for n in pat.notes.get(&ch.id).into_iter().flat_map(|m| m.values()) {
                    if n.muted
                        || n.start < clip.offset
                        || n.start >= clip.offset + clip.length
                        || n.start >= plen
                    {
                        continue;
                    }
                    let st = clip.start + (n.start - clip.offset);
                    if let Some((s, e)) = place(st, st + n.length, win_end) {
                        v.push(note_ev(s, e, n));
                    }
                }
            }
            sort_stream(&mut v);
            v
        })
        .collect();

    let mut choke = vec![Vec::new(); CHOKE_GROUPS + 1];
    for (i, node) in nodes.iter().enumerate() {
        if let NodeDef::Channel(c) = &node.def
            && c.choke > 0
        {
            choke[c.choke as usize].push(i);
        }
    }
    let mut channel_lookup: Vec<(ChannelId, usize)> =
        chans.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    channel_lookup.sort_by_key(|c| c.0);

    let count = nodes.len();
    let plan = Plan {
        id,
        base: prev.id,
        settings,
        bar_samples: tempo.sample_at(project.settings.time_signature.ticks_per_bar()),
        song_len: tempo.sample_at(project.song_length()),
        tempo,
        time_sig: project.settings.time_signature,
        nodes,
        n_channels: n_ch,
        channel_lookup,
        choke,
        song,
        patterns,
        transfers,
        params,
        roots,
        sched: Sched::new(count),
        board: Arc::new(board),
    };
    Ok((Box::new(plan), shadow))
}

fn note_ev(start: i64, end: i64, n: &ph_core::Note) -> NoteEv {
    NoteEv {
        start,
        end,
        key: n.key.min(127),
        velocity: n.velocity.clamp(0.0, 1.0),
        pan: n.pan.clamp(-1.0, 1.0),
        fine_pitch: n.fine_pitch,
    }
}

fn sort_stream(v: &mut [NoteEv]) {
    // stable: equal (start, key, end) keep their source order (clip, then note id)
    v.sort_by_key(|e| (e.start, e.key, e.end));
}
