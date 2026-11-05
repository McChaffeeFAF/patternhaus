//! The Haus Sampler engine (M3): `ph.haus_sampler`, a [`Processor`] that plays a
//! [`HausInstrument`] from the project model.
//!
//! - [`Plan`] is the immutable real-time plan an instrument compiles to (zone lookup by key,
//!   then velocity, articulation, controller conditions and round robin at note-on).
//! - [`samples`] is how the engine hands it audio: preloads now, the M2 disk streamer later
//!   (the decision and its reasons are in that module's docs).
//! - [`voice`] is the voice model: trigger, release, steal, round robin, and the seeding rule
//!   that keeps live output bit-identical to offline renders.
//! - [`t2`] marks where the real DSP goes (envelopes, loops, filter, modulation, crossfades,
//!   legato, streaming, group effects).
//!
//! # What plays in M3/T0
//!
//! The minimal path: each zone the note selects plays its sample once from `start` to `end`
//! (backwards if `reverse`), resampled to the note's pitch with linear interpolation, at
//! instrument × group × zone level times velocity, balanced by pan. Note-off stops a voice at
//! once unless the zone is one-shot. Release-trigger zones fire on note-off, quieter by
//! `release_decay_db` per second held. Keyswitches and controller switches select
//! articulations; mono and legato groups cut their previous voice. No envelopes, loops,
//! filters or modulation yet (M3/T2).
//!
//! # Multi-out
//!
//! Output port 0 is the main stereo output. A group with `output: n` (n ≥ 1) plays into port
//! `n` (named `out <n + 1>`); the descriptor lists ports up to the highest bus a group uses.
//! A host that passes fewer ports than that (the M3/T0 engine passes only port 0, until the
//! mixer routes extra outputs, M3/T8) gets those buses mixed into port 0, so nothing is lost.

pub mod effects;
pub mod env;
pub mod plan;
pub mod render;
pub mod samples;
pub mod voice;

pub use plan::Plan;
pub use samples::{SampleSet, StreamSource, ZoneSample, load_samples, load_samples_streamed};

use effects::{Chains, GroupFx};
use env::EnvShape;
use ph_core::{HausInstrument, InstrumentId, Polyphony, Project, ZoneTrigger, midi_velocity};
use ph_dsp::interp::SincInterpolator;
use ph_node::*;
use render::{CTRL, Controls, WINDOW};
use std::sync::Arc;
use voice::{Rng, RoundRobin, Trigger, VoicePool};

/// Stable processor id.
pub const ID: &str = "ph.haus_sampler";

/// Frames of each sample preloaded when the instrument does not say (`preload_frames`):
/// 0.68 s at 48 kHz.
pub const DEFAULT_PRELOAD_FRAMES: u32 = 32_768;

/// Most voices one sampler node plays (the instrument's `voice_limit` is capped to it).
pub const MAX_VOICES: usize = 1024;

const P_VOLUME: usize = 0;
const P_PAN: usize = 1;
const P_TRANSPOSE: usize = 2;

/// The parameter list (stable ids): `volume` (dB), `pan`, `transpose` (semitones).
fn params() -> Vec<ParamInfo> {
    vec![
        ParamInfo::new("volume", "Volume", -60.0, 12.0, 0.0, ParamUnit::Decibels),
        ParamInfo::new("pan", "Pan", -1.0, 1.0, 0.0, ParamUnit::Pan),
        ParamInfo {
            stepped: true,
            ..ParamInfo::new(
                "transpose",
                "Transpose",
                -48.0,
                48.0,
                0.0,
                ParamUnit::Semitones,
            )
        },
    ]
}

/// The node descriptor for an instrument whose groups use buses up to `max_output`.
pub fn descriptor(max_output: u32) -> NodeDescriptor {
    let mut outputs = vec![PortInfo {
        name: "main".into(),
        channels: 2,
        sidechain: false,
    }];
    for n in 1..=max_output.min(ph_core::MAX_OUTPUT_BUSES - 1) {
        outputs.push(PortInfo {
            name: format!("out {}", n + 1),
            channels: 2,
            sidechain: false,
        });
    }
    NodeDescriptor {
        id: ID.into(),
        name: "Haus Sampler".into(),
        kind: NodeKind::Instrument,
        audio_inputs: Vec::new(),
        audio_outputs: outputs,
        note_input: true,
        params: params(),
    }
}

/// Builds the sampler for project instrument `id`: loads its samples through `assets`, every
/// file whole, and compiles the plan (off the audio thread). The warnings name samples that
/// failed to load (their zones stay silent). Hosts that stream use [`create_streamed`].
pub fn create(
    project: &Project,
    id: InstrumentId,
    assets: &dyn AssetResolver,
) -> Result<(HausSampler, Vec<String>), String> {
    let inst = project
        .instrument(id)
        .map_err(|e| format!("Haus Sampler: {e}"))?;
    let (samples, warnings) = load_samples(project, inst, assets);
    Ok((HausSampler::new(inst, samples), warnings))
}

/// [`create`] with disk streaming: long files keep only their head in memory
/// ([`load_samples_streamed`]). `attach` gets the opened streams (indexed by
/// [`ZoneSample::stream`]) and the number of voice slots that will read them, and returns the
/// host's [`StreamSource`] over them; it is not called when nothing streams. Off the audio
/// thread.
pub fn create_streamed(
    project: &Project,
    id: InstrumentId,
    assets: &dyn AssetResolver,
    attach: &mut dyn FnMut(Vec<Box<dyn SampleStream>>, usize) -> Option<Arc<dyn StreamSource>>,
) -> Result<(HausSampler, Vec<String>), String> {
    let inst = project
        .instrument(id)
        .map_err(|e| format!("Haus Sampler: {e}"))?;
    let (mut samples, warnings, streams) = load_samples_streamed(project, inst, assets);
    if !streams.is_empty() {
        samples.streams = attach(streams, voice_slots(inst));
    }
    Ok((HausSampler::new(inst, samples), warnings))
}

/// Voice slots a sampler for `inst` has (its voice limit, capped at [`MAX_VOICES`], plus
/// the spares stolen voices fade in): what a [`StreamSource`] must serve.
pub fn voice_slots(inst: &HausInstrument) -> usize {
    VoicePool::capacity_for((inst.settings.voice_limit.max(1) as usize).min(MAX_VOICES))
}

/// Steal and choke fades, seconds.
pub const FADE_SECONDS: f64 = 0.003;

/// `ph.haus_sampler`.
pub struct HausSampler {
    desc: NodeDescriptor,
    plan: Arc<Plan>,
    samples: SampleSet,
    params: [f64; 3],
    sample_rate: f64,
    sinc: Option<SincInterpolator>,
    pool: VoicePool,
    rr: RoundRobin,
    rng: Rng,
    ctl: Controls,
    articulation: usize,
    /// Per key: (velocity, frame it started) while held, for release triggers.
    held: [Option<(f32, u64)>; 128],
    /// Frames since the last reset.
    clock: u64,
    /// Per group, the round-robin position of the note being triggered (`u32::MAX` = not
    /// drawn yet), whether its mono voices were cut, and whether a legato note glided its
    /// voices instead of starting new ones. Sized in `new`.
    rr_pos: Box<[u32]>,
    cut: Box<[bool]>,
    glided: Box<[bool]>,
    /// Per group: (amp, filter) envelope shapes at the sample rate (set in `activate`).
    shapes: Box<[(EnvShape, EnvShape)]>,
    /// Stream windows, `2 × WINDOW` frames per voice slot (empty when nothing streams).
    win: Box<[f32]>,
    /// Per-group insert chains.
    chains: Chains,
    /// Mono fallback for a one-channel port.
    scratch: Box<[f32]>,
}

impl HausSampler {
    /// A sampler over a compiled instrument and its loaded samples.
    pub fn new(inst: &HausInstrument, samples: SampleSet) -> Self {
        let plan = Arc::new(Plan::compile(inst, &samples));
        let groups = plan.groups.len();
        let desc = descriptor(plan.max_output);
        let defaults = desc.params.iter().map(|p| p.default).collect::<Vec<_>>();
        let mut s = Self {
            desc,
            pool: VoicePool::new(plan.voice_limit.min(MAX_VOICES)),
            rr: RoundRobin::new(groups),
            rng: Rng::new(0),
            articulation: plan.default_articulation,
            plan,
            samples,
            params: [defaults[0], defaults[1], defaults[2]],
            sample_rate: 48_000.0,
            sinc: None,
            ctl: Controls::default(),
            held: [None; 128],
            clock: 0,
            rr_pos: vec![u32::MAX; groups].into_boxed_slice(),
            cut: vec![false; groups].into_boxed_slice(),
            glided: vec![false; groups].into_boxed_slice(),
            shapes: Box::new([]),
            win: Box::new([]),
            chains: Chains::default(),
            scratch: Box::new([]),
        };
        s.sync_params();
        s
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The voices (active or not), for inspection.
    pub fn voices(&self) -> &[voice::Voice] {
        &self.pool.voices
    }

    /// Voices sounding now (fading ones included).
    pub fn active_voices(&self) -> usize {
        self.pool.active()
    }

    /// The articulation in effect (index into the instrument's articulations).
    pub fn articulation(&self) -> usize {
        self.articulation
    }

    /// Bytes of sample data this sampler holds in memory (heads and whole files).
    pub fn resident_bytes(&self) -> usize {
        self.samples.resident_bytes() + self.win.len() * 4
    }

    /// Installs group `group`'s insert chain (index into [`Plan::groups`]): effect
    /// processors and their dry/wet mix, in order. The host builds them (see
    /// [`effects`]). Call before `activate`, off the audio thread.
    pub fn set_group_effects(&mut self, group: usize, chain: Vec<(Box<dyn Processor>, f32)>) {
        let n = self.plan.groups.len();
        self.chains.chains.resize_with(n, Vec::new);
        if let Some(c) = self.chains.chains.get_mut(group) {
            *c = chain
                .into_iter()
                .map(|(fx, mix)| GroupFx { fx, mix })
                .collect();
        }
    }

    fn sync_params(&mut self) {
        self.ctl.volume = 10f32.powf(self.params[P_VOLUME] as f32 / 20.0);
        self.ctl.pan = self.params[P_PAN] as f32;
    }

    /// Back to the state right after `activate`: no voices, round robins and RNG at the
    /// seed, controllers at 0, default articulation, effect tails cleared. Real-time.
    fn clear(&mut self) {
        if let Some(s) = self.samples.streams.as_deref() {
            for (i, v) in self.pool.voices.iter_mut().enumerate() {
                if v.active {
                    render::finish(v, i, Some(s));
                }
            }
        }
        self.pool.reset();
        self.rr.reset();
        self.rng.reset();
        self.ctl.cc = [0.0; 128];
        self.ctl.bend = 0.0;
        self.ctl.aftertouch = 0.0;
        self.held = [None; 128];
        self.articulation = self.plan.default_articulation;
        self.clock = 0;
        self.chains.reset();
    }

    /// Starts the zones of `trigger` for one note. Real-time.
    #[allow(clippy::too_many_arguments)]
    fn start_note(
        &mut self,
        note_id: u32,
        key: u8,
        velocity: f32,
        pan: f32,
        fine: f32,
        trigger: ZoneTrigger,
        held_seconds: f64,
    ) {
        let vel7 = midi_velocity(velocity);
        self.rr_pos.fill(u32::MAX);
        self.cut.fill(false);
        self.glided.fill(false);
        let plan = self.plan.clone();
        // one random draw per note, only when something uses it (keeps the round-robin
        // sequence of instruments without random sources as it was)
        let random = if plan.uses_random {
            self.rng.unit()
        } else {
            0.0
        };
        for &zi in plan.zones_for(key, trigger) {
            let z = &plan.zones[zi as usize];
            if vel7 < z.vel_lo || vel7 > z.vel_hi || !plan.group_plays(z.group, self.articulation) {
                continue;
            }
            if !z
                .cc_conditions
                .iter()
                .all(|c| (c.lo..=c.hi).contains(&self.ctl.cc[c.controller as usize & 127]))
            {
                continue;
            }
            let gi = z.group;
            let g = &plan.groups[gi];
            if let Some(p) = z.round_robin {
                if self.rr_pos[gi] == u32::MAX {
                    self.rr_pos[gi] = self.rr.advance(gi, g.rr_len, g.round_robin, &mut self.rng);
                }
                if self.rr_pos[gi] != p {
                    continue;
                }
            }
            let Some(si) = z.sample else {
                continue;
            };
            let Some(s) = self.samples.samples.get(si) else {
                continue;
            };
            if z.end <= z.start {
                continue;
            }
            let vel_xfade =
                render::crossfade_gain(&z.crossfades, velocity, &self.ctl.cc, Some(true));
            if vel_xfade <= 0.0 {
                continue;
            }
            let semis = key as f64 - z.root_key as f64
                + z.transpose
                + plan.transpose
                + self.params[P_TRANSPOSE]
                + fine as f64 / 100.0;
            let ratio = s.sample_rate() / self.sample_rate;
            let glide_frames = (g.portamento as f64 * self.sample_rate).round();
            // mono and legato groups: one note at a time
            let mut glide_from = None;
            if g.polyphony != Polyphony::Poly && trigger == ZoneTrigger::Attack {
                if self.glided[gi] {
                    continue;
                }
                if !self.cut[gi] {
                    self.cut[gi] = true;
                    let held = self
                        .pool
                        .voices
                        .iter()
                        .any(|v| v.counts() && v.group == gi && v.held);
                    if g.polyphony == Polyphony::Legato && held {
                        // glide the held voices to the new note; no new voices
                        self.glided[gi] = true;
                        for v in self
                            .pool
                            .voices
                            .iter_mut()
                            .filter(|v| v.counts() && v.group == gi && v.held)
                        {
                            let vz = &plan.zones[v.zone];
                            let old = v.key as f64 - vz.root_key as f64;
                            let new = key as f64 - vz.root_key as f64;
                            let base = v.step / (old / 12.0).exp2();
                            v.step = base * (new / 12.0).exp2();
                            v.glide += old - new;
                            v.glide_step = if glide_frames >= 1.0 {
                                v.glide / glide_frames
                            } else {
                                v.glide
                            };
                            v.note_id = note_id;
                            v.key = key;
                        }
                        continue;
                    }
                    // the newest sounding voice is where a mono glide starts from
                    if g.portamento > 0.0
                        && let Some(v) = self
                            .pool
                            .voices
                            .iter()
                            .filter(|v| v.counts() && v.group == gi)
                            .max_by_key(|v| v.order)
                    {
                        let vz = &plan.zones[v.zone];
                        let from = v.key as f64 - vz.root_key as f64 + v.glide;
                        glide_from = Some(from - (key as f64 - z.root_key as f64));
                    }
                    self.pool.choke_group(gi);
                }
            }
            let mut step = (semis / 12.0).exp2() * ratio;
            let pos = if z.reverse {
                step = -step;
                (z.end - 1) as f64
            } else {
                z.start as f64
            };
            let mut level = plan.gain * g.gain * z.gain * velocity;
            if trigger == ZoneTrigger::Release && z.release_decay_db > 0.0 {
                level *= 10f32.powf(-z.release_decay_db * held_seconds as f32 / 20.0);
            }
            let note_pan = plan.pan + z.pan + pan;
            let Some(slot) = self.pool.trigger(
                &plan,
                Trigger {
                    note_id,
                    key,
                    zone: zi as usize,
                    group: gi,
                    bus: g.output,
                    pos,
                    step,
                    gain: [level, level],
                },
            ) else {
                continue;
            };
            let (amp, fenv) = self.shapes.get(gi).copied().unwrap_or_default();
            let v = &mut self.pool.voices[slot];
            v.velocity = velocity;
            v.random = random;
            v.pressure = 0.0;
            v.level = level;
            v.note_pan = note_pan;
            v.vel_xfade = vel_xfade;
            v.amp.start(amp);
            v.fenv.start(fenv);
            v.lfo_phase = [0.0; plan::MAX_LFOS];
            v.lfo_rng = (random.to_bits() as u64) << 32 | zi as u64;
            for h in v.lfo_hold.iter_mut() {
                *h = (render::splitmix(&mut v.lfo_rng) >> 40) as f32 / (1u64 << 23) as f32 - 1.0;
            }
            v.age = 0;
            for f in v.filt.iter_mut() {
                f.reset();
            }
            v.glide = glide_from.unwrap_or(0.0);
            v.glide_step = if glide_frames >= 1.0 {
                v.glide / glide_frames
            } else {
                v.glide
            };
            v.ctrl = 0;
            v.looping = z.looping.is_some() && trigger == ZoneTrigger::Attack;
            if trigger == ZoneTrigger::Release {
                v.held = false;
            }
            v.fixed = render::modulate(&g.routes, true, v, &self.ctl, &[0.0; plan::MAX_LFOS]);
            let offset = render::start_offset(&g.routes, v, &self.ctl);
            if offset > 0.0 {
                v.pos = if z.reverse {
                    (v.pos - offset).max(z.start as f64)
                } else {
                    (v.pos + offset).min((z.end - 1) as f64)
                };
            }
            // start fetching the stream where the head ends
            if let (Some(src), Some(streams)) = (s.stream, self.samples.streams.as_deref())
                && !s.complete
            {
                streams.want(slot, src, s.head.frames() as u64, false);
                v.win_start = i64::MIN;
            }
        }
    }

    fn event(&mut self, e: &Event) {
        match e.kind {
            EventKind::NoteOn {
                note_id,
                key,
                velocity,
                pan,
                fine_pitch,
            } => {
                if let Some(a) = self.plan.keyswitch(key) {
                    self.articulation = a;
                    return;
                }
                if let Some(h) = self.held.get_mut(key as usize) {
                    *h = Some((velocity, self.clock + e.frame as u64));
                }
                self.start_note(
                    note_id,
                    key,
                    velocity,
                    pan,
                    fine_pitch,
                    ZoneTrigger::Attack,
                    0.0,
                );
            }
            EventKind::NoteOff { note_id, key, .. } => {
                let plan = self.plan.clone();
                self.pool.release(&plan, note_id);
                if let Some((velocity, t0)) = self.held.get_mut(key as usize).and_then(|h| h.take())
                {
                    let held =
                        (self.clock + e.frame as u64).saturating_sub(t0) as f64 / self.sample_rate;
                    self.start_note(note_id, key, velocity, 0.0, 0.0, ZoneTrigger::Release, held);
                }
            }
            EventKind::AllNotesOff => self.clear(),
            EventKind::Param { id, value } => self.set_param(id, value),
            EventKind::Cc { controller, value } => {
                let c = controller as usize & 127;
                self.ctl.cc[c] = value;
                for (i, a) in self.plan.articulations.iter().enumerate() {
                    if let ph_core::ArticulationTrigger::Cc {
                        controller: ac,
                        lo,
                        hi,
                    } = a.trigger
                        && ac as usize == c
                        && (lo..=hi).contains(&value)
                    {
                        self.articulation = i;
                    }
                }
            }
            EventKind::PitchBend { value } => self.ctl.bend = value.clamp(-1.0, 1.0),
            EventKind::NoteExpression {
                note_id,
                kind: NoteExpressionKind::Pressure,
                value,
            } => {
                let value = value.clamp(0.0, 1.0);
                self.ctl.aftertouch = value;
                for v in self
                    .pool
                    .voices
                    .iter_mut()
                    .filter(|v| v.active && v.note_id == note_id)
                {
                    v.pressure = value;
                }
            }
            EventKind::NoteExpression { .. } => {}
        }
    }

    /// Renders every active voice into its port (or its group's chain buffer) over
    /// `from..to`. Real-time.
    fn render(&mut self, out: &mut [&mut [&mut [f32]]], from: usize, to: usize) {
        let ports = out.len();
        let streams = self.samples.streams.as_deref();
        let sinc = self.sinc.as_ref();
        for (slot, v) in self.pool.voices.iter_mut().enumerate() {
            if !v.active {
                continue;
            }
            let z = &self.plan.zones[v.zone];
            let Some(s) = z.sample.and_then(|s| self.samples.samples.get(s)) else {
                v.active = false;
                continue;
            };
            let win: &mut [f32] = if self.win.is_empty() {
                &mut []
            } else {
                &mut self.win[slot * 2 * WINDOW..(slot + 1) * 2 * WINDOW]
            };
            if let Some(b) = self.chains.buffer_of.get(v.group).copied().flatten() {
                let (l, r) = self.chains.buffer(b, to);
                render::render_voice(
                    v, slot, &self.plan, s, streams, win, sinc, &self.ctl, l, r, from, to,
                );
                continue;
            }
            let port = if (v.bus as usize) < ports {
                v.bus as usize
            } else {
                0
            };
            let Some(o) = out.get_mut(port) else {
                continue;
            };
            match &mut o[..] {
                [l, r, ..] => render::render_voice(
                    v, slot, &self.plan, s, streams, win, sinc, &self.ctl, l, r, from, to,
                ),
                [m] => {
                    let r = &mut self.scratch[..to];
                    render::render_voice(
                        v, slot, &self.plan, s, streams, win, sinc, &self.ctl, m, r, from, to,
                    );
                }
                [] => {}
            }
        }
    }
}

impl Processor for HausSampler {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sample_rate = info.sample_rate;
        self.ctl.sample_rate = info.sample_rate;
        self.rng = Rng::new(info.seed);
        self.pool = VoicePool::new(self.plan.voice_limit.min(MAX_VOICES));
        self.pool.fade_frames = (FADE_SECONDS * info.sample_rate).round().max(1.0) as u32;
        self.sinc = match info.quality {
            Quality::Live => None,
            Quality::High => Some(SincInterpolator::new(
                render::SINC_HALF_WIDTH,
                render::SINC_MAX_STRETCH,
            )),
        };
        self.shapes = self
            .plan
            .groups
            .iter()
            .map(|g| {
                (
                    EnvShape::new(&g.amp_env, info.sample_rate),
                    g.filter_env
                        .as_ref()
                        .map(|e| EnvShape::new(e, info.sample_rate))
                        .unwrap_or_default(),
                )
            })
            .collect();
        let streamed = self.samples.streams.is_some()
            && self.samples.samples.iter().any(|s| !s.complete);
        self.win = if streamed {
            vec![0.0; self.pool.voices.len() * 2 * WINDOW].into_boxed_slice()
        } else {
            Box::new([])
        };
        self.scratch = vec![0.0; info.max_frames.max(1)].into_boxed_slice();
        self.chains.activate(self.plan.groups.len(), info);
        self.clear();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        silence(io);
        let n = ctx.frames;
        let fx = !self.chains.is_empty();
        if fx {
            self.chains.begin(n);
        }
        let mut at = 0usize;
        for e in io.events {
            let f = (e.frame as usize).min(n);
            if f > at {
                self.render(io.outputs, at, f);
                at = f;
            }
            self.event(e);
        }
        if at < n {
            self.render(io.outputs, at, n);
        }
        let mut tail = false;
        if fx {
            let ports = io.outputs.len();
            for g in 0..self.plan.groups.len() {
                if self.chains.buffer_of.get(g).copied().flatten().is_none() {
                    continue;
                }
                let bus = self.plan.groups[g].output as usize;
                let port = if bus < ports { bus } else { 0 };
                let Some(o) = io.outputs.get_mut(port) else {
                    continue;
                };
                tail |= match &mut o[..] {
                    [l, r, ..] => self.chains.run(g, ctx, l, r),
                    [m] => {
                        let r = &mut self.scratch[..n];
                        self.chains.run(g, ctx, m, r)
                    }
                    [] => false,
                };
            }
        }
        self.clock += n as u64;
        if tail || self.pool.voices.iter().any(|v| v.active) {
            ProcessStatus::Continue
        } else {
            ProcessStatus::Sleep
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        if let Some(p) = self.desc.params.get(id.0 as usize) {
            self.params[id.0 as usize] = p.clamp(value);
            self.sync_params();
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.params.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    fn reset(&mut self) {
        self.clear();
    }
}

/// Control updates happen every [`CTRL`] frames of a voice (re-exported for hosts and tests).
pub const CONTROL_FRAMES: u32 = CTRL;

#[cfg(test)]
mod tests;
