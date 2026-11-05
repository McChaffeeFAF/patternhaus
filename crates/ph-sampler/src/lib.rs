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

pub mod plan;
pub mod samples;
pub mod t2;
pub mod voice;

pub use plan::Plan;
pub use samples::{SampleSet, StreamSource, ZoneSample, load_samples};

use ph_core::{HausInstrument, InstrumentId, Project, ZoneTrigger, midi_velocity};
use ph_node::*;
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

/// Builds the sampler for project instrument `id`: loads its samples through `assets` and
/// compiles the plan (off the audio thread). The warnings name samples that failed to load
/// (their zones stay silent).
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

/// `ph.haus_sampler`.
pub struct HausSampler {
    desc: NodeDescriptor,
    plan: Arc<Plan>,
    samples: SampleSet,
    params: [f64; 3],
    sample_rate: f64,
    pool: VoicePool,
    rr: RoundRobin,
    rng: Rng,
    cc: [f32; 128],
    articulation: usize,
    /// Per key: (velocity, frame it started) while held, for release triggers.
    held: [Option<(f32, u64)>; 128],
    /// Frames since the last reset.
    clock: u64,
    /// Per group, the round-robin position of the note being triggered (`u32::MAX` = not
    /// drawn yet) and whether its mono voices were cut. Sized in `new`.
    rr_pos: Box<[u32]>,
    cut: Box<[bool]>,
}

impl HausSampler {
    /// A sampler over a compiled instrument and its loaded samples.
    pub fn new(inst: &HausInstrument, samples: SampleSet) -> Self {
        let plan = Arc::new(Plan::compile(inst, &samples));
        let groups = plan.groups.len();
        let desc = descriptor(plan.max_output);
        let defaults = desc.params.iter().map(|p| p.default).collect::<Vec<_>>();
        Self {
            desc,
            pool: VoicePool::new(plan.voice_limit.min(MAX_VOICES)),
            rr: RoundRobin::new(groups),
            rng: Rng::new(0),
            articulation: plan.default_articulation,
            plan,
            samples,
            params: [defaults[0], defaults[1], defaults[2]],
            sample_rate: 48_000.0,
            cc: [0.0; 128],
            held: [None; 128],
            clock: 0,
            rr_pos: vec![u32::MAX; groups].into_boxed_slice(),
            cut: vec![false; groups].into_boxed_slice(),
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Voices sounding now.
    pub fn active_voices(&self) -> usize {
        self.pool.active()
    }

    /// The articulation in effect (index into the instrument's articulations).
    pub fn articulation(&self) -> usize {
        self.articulation
    }

    /// Back to the state right after `activate`: no voices, round robins and RNG at the
    /// seed, controllers at 0, default articulation. Real-time.
    fn clear(&mut self) {
        self.pool.reset();
        self.rr.reset();
        self.rng.reset();
        self.cc = [0.0; 128];
        self.held = [None; 128];
        self.articulation = self.plan.default_articulation;
        self.clock = 0;
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
        let plan = self.plan.clone();
        for &zi in plan.zones_for(key, trigger) {
            let z = &plan.zones[zi as usize];
            if vel7 < z.vel_lo || vel7 > z.vel_hi || !plan.group_plays(z.group, self.articulation) {
                continue;
            }
            if !z
                .cc_conditions
                .iter()
                .all(|c| (c.lo..=c.hi).contains(&self.cc[c.controller as usize & 127]))
            {
                continue;
            }
            let g = &plan.groups[z.group];
            if let Some(p) = z.round_robin {
                if self.rr_pos[z.group] == u32::MAX {
                    self.rr_pos[z.group] =
                        self.rr
                            .advance(z.group, g.rr_len, g.round_robin, &mut self.rng);
                }
                if self.rr_pos[z.group] != p {
                    continue;
                }
            }
            let Some(s) = z.sample.and_then(|s| self.samples.samples.get(s)) else {
                continue;
            };
            if z.end <= z.start {
                continue;
            }
            if g.polyphony != ph_core::Polyphony::Poly && !self.cut[z.group] {
                self.cut[z.group] = true;
                self.pool.choke_group(z.group); // T2: legato glides instead
            }
            let semis = key as f64 - z.root_key as f64
                + z.transpose
                + plan.transpose
                + self.params[P_TRANSPOSE]
                + fine as f64 / 100.0;
            let mut step = (semis / 12.0).exp2() * s.sample_rate() / self.sample_rate;
            let pos = if z.reverse {
                step = -step;
                (z.end - 1) as f64
            } else {
                z.start as f64
            };
            let mut gain = plan.gain
                * g.gain
                * z.gain
                * velocity
                * 10f32.powf(self.params[P_VOLUME] as f32 / 20.0);
            if trigger == ZoneTrigger::Release && z.release_decay_db > 0.0 {
                gain *= 10f32.powf(-z.release_decay_db * held_seconds as f32 / 20.0);
            }
            let p = (plan.pan + z.pan + pan + self.params[P_PAN] as f32).clamp(-1.0, 1.0);
            let gains = [gain * (1.0 - p).min(1.0), gain * (1.0 + p).min(1.0)];
            self.pool.trigger(
                &plan,
                Trigger {
                    note_id,
                    key,
                    zone: zi as usize,
                    group: z.group,
                    bus: g.output,
                    pos,
                    step,
                    gain: gains,
                },
            );
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
                self.cc[c] = value;
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
            // T2: pitch bend and note expressions through the modulation matrix
            EventKind::PitchBend { .. } | EventKind::NoteExpression { .. } => {}
        }
    }

    /// Renders every active voice into `out[port][channel][from..to]`. Real-time.
    fn render(&mut self, out: &mut [&mut [&mut [f32]]], from: usize, to: usize) {
        let ports = out.len();
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            let z = &self.plan.zones[v.zone];
            let Some(s) = z.sample.and_then(|s| self.samples.samples.get(s)) else {
                v.active = false;
                continue;
            };
            let data = &s.head.channels;
            let len = data.first().map_or(0, |c| c.len()) as f64;
            let (lo, hi) = (z.start as f64, (z.end as f64).min(len));
            let port = if (v.bus as usize) < ports {
                v.bus as usize
            } else {
                0
            };
            for i in from..to {
                if v.pos < lo || v.pos > hi - 1.0 {
                    // past the end (or, streamed, past the preload: T2 reads on)
                    v.active = false;
                    break;
                }
                let i0 = v.pos as usize;
                let frac = (v.pos - i0 as f64) as f32;
                let i1 = (i0 + 1).min(hi as usize - 1);
                let at = |c: &Vec<f32>| c[i0] + (c[i1] - c[i0]) * frac;
                let l = at(&data[0]);
                let r = data.get(1).map_or(l, at);
                let o = &mut out[port];
                o[0][i] += l * v.gain[0];
                if o.len() > 1 {
                    o[1][i] += r * v.gain[1];
                }
                v.pos += v.step;
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
        self.rng = Rng::new(info.seed);
        self.pool = VoicePool::new(self.plan.voice_limit.min(MAX_VOICES));
        self.clear();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        silence(io);
        let n = ctx.frames;
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
        self.clock += n as u64;
        if self.pool.voices.iter().any(|v| v.active) {
            ProcessStatus::Continue
        } else {
            ProcessStatus::Sleep
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        if let Some(p) = self.desc.params.get(id.0 as usize) {
            self.params[id.0 as usize] = p.clamp(value);
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.params.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    fn reset(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests;
