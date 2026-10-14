//! The processor interface.
//!
//! Every built-in instrument and effect is a [`Processor`]. The shape mirrors CLAP
//! (descriptor, activate, process with sample-stamped events, parameters by stable id,
//! opaque state) so built-ins can later be wrapped as CLAP/VST3 plugins mechanically, and
//! external plugins can be adapted to it by `ph-plugin-host`.
//!
//! # Real-time contract
//!
//! [`Processor::process`] and [`Processor::set_param`] run on an audio thread. They must not
//! allocate, free, lock, block, do I/O, or panic. Everything that needs memory happens in
//! [`Processor::activate`] (called off the audio thread). Debug and CI builds run the engine
//! under `assert_no_alloc` to enforce this.
//!
//! # Events (contract, M2/T0)
//!
//! Events arrive in [`ProcessIo::events`], sorted by frame; at one frame the engine orders
//! them `AllNotesOff`, `NoteOff`, `Cc`/`PitchBend`, `NoteOn`, `Param`, `NoteExpression`, so a
//! bend or controller sent with a note applies before it starts and an expression applies to
//! the note it names. Handling an event is real-time code: no allocation, locks or panics.
//! A processor ignores event kinds it has no use for.
//!
//! Units: velocities and controller values are normalised 0..=1, pitch bend -1..=1, fine
//! pitch in cents, note-expression units per [`NoteExpressionKind`].
//!
//! # Parameter automation (contract, M2/T0)
//!
//! Automation reaches a processor only as [`EventKind::Param`] events with plain values (the
//! parameter's own unit, clamped by the processor). The engine sends one at every
//! automation point and at least every [`AUTOMATION_INTERVAL`] frames between points, each
//! carrying the exact curve value at its frame; the processor smooths between them as it
//! does for any parameter change. There is no separate ramp event: every processor already
//! handles `Param`, and a 32-frame staircase under the processor's own smoothing is
//! inaudible.

use serde::{Deserialize, Serialize};

/// Longest gap, in frames, between two `Param` events the engine sends for an automated
/// parameter (see the crate docs).
pub const AUTOMATION_INTERVAL: u32 = 32;

/// Index into a processor's parameter list (position in [`NodeDescriptor::params`]).
/// Real-time code uses this; saved projects and agents use [`ParamInfo::id`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParamId(pub u32);

/// Static description of a processor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeDescriptor {
    /// Stable id, e.g. `ph.synth`, `ph.sampler`, `ph.fx.compressor`.
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub audio_inputs: Vec<PortInfo>,
    pub audio_outputs: Vec<PortInfo>,
    /// Accepts note events.
    pub note_input: bool,
    pub params: Vec<ParamInfo>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Instrument,
    Effect,
    /// Mixer insert, send, master, router, etc.
    Utility,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortInfo {
    pub name: String,
    pub channels: u32,
    /// Sidechain inputs are optional; the engine passes silence when unconnected.
    pub sidechain: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    /// Stable string id used in saved projects, by agents and in automation. Never renamed;
    /// deprecate and add a new id instead.
    pub id: String,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: ParamUnit,
    /// Only whole values are meaningful (choices, switches, counts).
    pub stepped: bool,
    /// Labels for stepped choice parameters, indexed from `min`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// Can be automated sample-accurately.
    pub automatable: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamUnit {
    None,
    Decibels,
    Hertz,
    Seconds,
    Milliseconds,
    Percent,
    Semitones,
    Cents,
    Ratio,
    Pan,
    Choice,
    Switch,
}

/// Rendering quality. `High` is used for HQ export (DESIGN §4): better interpolation,
/// more oversampling. A processor may ignore it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    #[default]
    Live,
    High,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ActivateInfo {
    pub sample_rate: f64,
    /// Largest block `process` will ever be called with.
    pub max_frames: usize,
    pub quality: Quality,
    /// Deterministic seed for any randomness (DESIGN §5 determinism).
    pub seed: u64,
}

/// Per-block timing information.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProcessContext {
    pub sample_rate: f64,
    /// Frames in this block (≤ `max_frames`).
    pub frames: usize,
    /// Song position of the first frame, in samples since song start.
    pub song_pos_samples: i64,
    /// Song position of the first frame, in ticks (fractional).
    pub song_pos_ticks: f64,
    pub bpm: f64,
    pub playing: bool,
    /// False when this block is rendered ahead of the playhead (anticipative processing,
    /// DESIGN §4). Processors must not assume the block they process is "now".
    pub live: bool,
}

/// A sample-stamped event. Events in a block are sorted by `frame`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Event {
    /// Offset into the current block.
    pub frame: u32,
    pub kind: EventKind,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EventKind {
    /// `note_id` is unique per sounding note, so per-note expression and note-off can
    /// target it exactly (CLAP-style). `velocity` 0.0..=1.0.
    NoteOn {
        note_id: u32,
        key: u8,
        velocity: f32,
        pan: f32,
        /// Per-note detune in **cents** (matches `ph_core::Note::fine_pitch`).
        fine_pitch: f32,
    },
    NoteOff {
        note_id: u32,
        key: u8,
        velocity: f32,
    },
    /// Stop every voice immediately (transport stop, panic).
    AllNotesOff,
    /// Sample-accurate parameter change. The processor smooths if it needs to.
    Param { id: ParamId, value: f64 },
    /// A MIDI-style controller change (M2). `controller` is the MIDI CC number 0..=127
    /// (1 = mod wheel, 64 = sustain pedal, ...); `value` is normalised 0.0..=1.0 (the MIDI
    /// value / 127; 14-bit pairs are combined by the sender).
    Cc { controller: u8, value: f32 },
    /// Channel pitch bend (M2), normalised -1.0..=1.0 (MIDI: `(v - 8192) / 8192`, clamped).
    /// The range in semitones is the instrument's own (Haus Synth: `bend_range`).
    PitchBend { value: f32 },
    /// Per-note expression (M2, CLAP note expressions) for the voice its `NoteOn` started
    /// with `note_id`. Unknown or finished ids are ignored. Absolute, not cumulative: each
    /// event sets the current value of `kind` for that note.
    NoteExpression {
        note_id: u32,
        kind: NoteExpressionKind,
        value: f32,
    },
}

/// What a [`EventKind::NoteExpression`] changes, and its unit.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteExpressionKind {
    /// Linear gain 0.0..=4.0; 1.0 = unchanged.
    Volume,
    /// -1.0 (left) ..= 1.0 (right), added to the note's own pan. (CLAP uses 0..1; a CLAP
    /// host adapter converts.)
    Pan,
    /// Relative tuning in semitones, -120.0..=120.0.
    Tuning,
    /// 0.0..=1.0.
    Vibrato,
    /// 0.0..=1.0 (MPE timbre, CC 74).
    Brightness,
    /// 0.0..=1.0 (polyphonic aftertouch).
    Pressure,
}

/// Audio buffers for one block. Channel slices are exactly `ProcessContext::frames` long.
pub struct ProcessIo<'io, 'buf> {
    /// `[port][channel]`
    pub inputs: &'io [&'io [&'buf [f32]]],
    /// `[port][channel]`
    pub outputs: &'io mut [&'io mut [&'buf mut [f32]]],
    pub events: &'io [Event],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProcessStatus {
    /// Output is meaningful.
    Continue,
    /// Output is silent and will stay silent until new input or events arrive; the engine
    /// may skip this processor until then (instruments with no sounding voices, tails ended).
    Sleep,
}

/// A built-in instrument or effect. See the crate docs for the real-time contract.
pub trait Processor: Send {
    fn descriptor(&self) -> &NodeDescriptor;

    /// Allocate everything needed for processing. Called off the audio thread, again whenever
    /// sample rate, block size or quality change. Clears all voices and tails.
    fn activate(&mut self, info: &ActivateInfo);

    /// Real-time. Fill every output channel (write silence when quiet).
    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus;

    /// Real-time. Set a parameter immediately (used between blocks; inside a block prefer
    /// [`EventKind::Param`]). Out-of-range values are clamped.
    fn set_param(&mut self, id: ParamId, value: f64);

    fn param(&self, id: ParamId) -> f64;

    /// Real-time. Silence all voices and tails without reallocating.
    fn reset(&mut self);

    /// Extra latency this processor adds, in samples (for PDC, M4).
    fn latency(&self) -> u32 {
        0
    }

    /// Tail length in samples after input stops (reverb, delay); `u32::MAX` = infinite.
    fn tail(&self) -> u32 {
        0
    }

    /// Opaque state beyond parameters (e.g. a loaded sample path, wavetables). Off the audio
    /// thread. Parameters themselves are saved by the host by [`ParamInfo::id`].
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Restores [`Processor::save_state`] output. Off the audio thread. `assets` resolves
    /// any sample references the state names (M2: every implementor gets the resolver, so a
    /// state can switch samples, not only confirm the current one).
    fn load_state(&mut self, _state: &[u8], _assets: &dyn AssetResolver) -> Result<(), String> {
        Ok(())
    }
}

impl NodeDescriptor {
    /// Looks up a parameter by its stable string id.
    pub fn param_id(&self, id: &str) -> Option<ParamId> {
        self.params
            .iter()
            .position(|p| p.id == id)
            .map(|i| ParamId(i as u32))
    }
}

impl ParamInfo {
    /// A continuous automatable parameter.
    pub fn new(id: &str, name: &str, min: f64, max: f64, default: f64, unit: ParamUnit) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            min,
            max,
            default,
            unit,
            stepped: false,
            choices: Vec::new(),
            automatable: true,
        }
    }

    /// A stepped choice parameter.
    pub fn choice(id: &str, name: &str, choices: &[&str], default: usize) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            min: 0.0,
            max: (choices.len().max(1) - 1) as f64,
            default: default as f64,
            unit: ParamUnit::Choice,
            stepped: true,
            choices: choices.iter().map(|s| s.to_string()).collect(),
            automatable: true,
        }
    }

    pub fn clamp(&self, v: f64) -> f64 {
        let v = v.clamp(self.min, self.max);
        if self.stepped { v.round() } else { v }
    }
}

/// Decoded audio, shared between every processor that plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleData {
    pub sample_rate: f64,
    /// `[channel][frame]`, de-interleaved, -1.0..=1.0.
    pub channels: Vec<Vec<f32>>,
    /// Loop points from the file (e.g. WAV `smpl` chunk), in frames.
    pub loop_range: Option<(usize, usize)>,
    /// Root key from the file, if it carries one.
    pub root_key: Option<u8>,
}

impl SampleData {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, |c| c.len())
    }
}

/// Resolves sample references (`factory:<name>`, project-relative or allowed absolute
/// paths) to decoded audio. Implemented by the app over `ph-formats` decoding and the
/// file-access rules; processors call it only from [`Processor::activate`] or
/// [`Processor::load_state`], never on the audio thread.
pub trait AssetResolver: Send + Sync {
    fn sample(&self, reference: &str) -> Result<std::sync::Arc<SampleData>, String>;

    /// Opens `reference` for disk streaming (M2/T1): long audio-clip sources are read in
    /// blocks by the engine's prefetch thread instead of being decoded whole. `None` when this
    /// resolver cannot stream the reference (the engine then decodes it with
    /// [`AssetResolver::sample`]). Additive default: resolvers that never stream need nothing.
    fn stream(&self, reference: &str) -> Option<Result<Box<dyn SampleStream>, String>> {
        let _ = reference;
        None
    }
}

/// A seekable reader of one audio file (M2/T1 disk streaming). Opened off the audio thread;
/// read by the engine's prefetch thread, or by an offline render's own thread.
/// [`SampleStream::read`] must not allocate (buffers are allocated when the stream opens).
pub trait SampleStream: Send {
    /// Length in frames.
    fn frames(&self) -> u64;
    /// The file's own sample rate, in Hz.
    fn sample_rate(&self) -> f64;
    /// 1 = mono, 2 = stereo, ...
    fn channels(&self) -> usize;
    /// Reads frames `start..start + n` into `out[c][..n]` (`n` = the shortest `out` slice,
    /// one slice per channel, at most [`SampleStream::channels`]). Frames past the end are
    /// zero. Returns the frames read from the file.
    fn read(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, StreamError>;
}

/// Why a [`SampleStream`] read failed (no message: reads must not allocate).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// The file could not be read (moved, unplugged drive, permissions).
    Io,
    /// The file's contents are not what its header promised.
    Malformed,
}

/// Fill every output channel with silence. Real-time safe.
pub fn silence(io: &mut ProcessIo<'_, '_>) {
    for port in io.outputs.iter_mut() {
        for ch in port.iter_mut() {
            ch.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial processor proving the trait is implementable and object-safe.
    struct Gain {
        desc: NodeDescriptor,
        gain: f64,
    }

    impl Processor for Gain {
        fn descriptor(&self) -> &NodeDescriptor {
            &self.desc
        }
        fn activate(&mut self, _: &ActivateInfo) {}
        fn process(&mut self, _: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
            for (c, out) in io.outputs[0].iter_mut().enumerate() {
                for (o, i) in out.iter_mut().zip(io.inputs[0][c]) {
                    *o = *i * self.gain as f32;
                }
            }
            ProcessStatus::Continue
        }
        fn set_param(&mut self, _: ParamId, v: f64) {
            self.gain = self.desc.params[0].clamp(v);
        }
        fn param(&self, _: ParamId) -> f64 {
            self.gain
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn processor_is_object_safe_and_processes() {
        let desc = NodeDescriptor {
            id: "test.gain".into(),
            name: "Gain".into(),
            kind: NodeKind::Effect,
            audio_inputs: vec![PortInfo {
                name: "in".into(),
                channels: 1,
                sidechain: false,
            }],
            audio_outputs: vec![PortInfo {
                name: "out".into(),
                channels: 1,
                sidechain: false,
            }],
            note_input: false,
            params: vec![ParamInfo::new(
                "gain",
                "Gain",
                0.0,
                2.0,
                1.0,
                ParamUnit::Ratio,
            )],
        };
        let mut p: Box<dyn Processor> = Box::new(Gain { desc, gain: 1.0 });
        let id = p.descriptor().param_id("gain").unwrap();
        p.set_param(id, 5.0);
        assert_eq!(p.param(id), 2.0);

        let input = [1.0f32, -0.5];
        let mut out = [0.0f32; 2];
        let ins: [&[f32]; 1] = [&input];
        let in_ports: [&[&[f32]]; 1] = [&ins];
        let mut outs: [&mut [f32]; 1] = [&mut out];
        let mut out_ports: [&mut [&mut [f32]]; 1] = [&mut outs];
        let ctx = ProcessContext {
            sample_rate: 48000.0,
            frames: 2,
            song_pos_samples: 0,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        let mut io = ProcessIo {
            inputs: &in_ports,
            outputs: &mut out_ports,
            events: &[],
        };
        assert_eq!(p.process(&ctx, &mut io), ProcessStatus::Continue);
        assert_eq!(out, [2.0, -1.0]);
    }
}
