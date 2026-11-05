//! Where the real DSP goes (M3/T2). Nothing here is called yet: the M3/T0 processor plays
//! the minimal path (one-shot playback, linear interpolation, no envelope, no loop, no
//! filter) so the engine wiring and the oracle test exist. T2 replaces these bodies and
//! calls them from `HausSampler::render_voice` and the event handlers. Each must stay
//! real-time safe: no allocation, locks, I/O or panics once implemented.

use crate::plan::{Plan, PlanLoop};
use crate::voice::Voice;

/// Next read position for a looping voice: forward or ping-pong, the crossfade region, and
/// leaving the loop on release when it is a sustain loop (`exit_on_release`).
pub fn loop_position(_voice: &mut Voice, _l: &PlanLoop, _released: bool) -> f64 {
    todo!("M3/T2: loop playback with crossfade")
}

/// DAHDSR amp and filter envelopes per voice (group `amp_env`, `filter_env`).
pub fn envelope_gain(_voice: &Voice, _plan: &Plan, _frame: u64) -> f32 {
    todo!("M3/T2: envelopes")
}

/// The group filter (kind, cutoff, resonance, key tracking, envelope depth).
pub fn filter(_voice: &mut Voice, _plan: &Plan, _l: &mut [f32], _r: &mut [f32]) {
    todo!("M3/T2: group filter")
}

/// The modulation matrix: sources (velocity, key, CC, bend, pressure, LFO, envelope,
/// random-per-note) to targets (pitch, volume, pan, cutoff, resonance, sample start).
pub fn modulate(_voice: &mut Voice, _plan: &Plan) {
    todo!("M3/T2: modulation matrix")
}

/// Velocity and controller crossfades between layers (equal power or linear gain).
pub fn crossfade_gain(_plan: &Plan, _zone: usize, _velocity: f32, _cc: &[f32; 128]) -> f32 {
    todo!("M3/T2: crossfades")
}

/// Legato and portamento for mono and legato groups: glide the playing voice instead of
/// retriggering.
pub fn legato(_voice: &mut Voice, _target_step: f64, _glide_frames: u32) {
    todo!("M3/T2: legato and portamento")
}

/// Streams the part of a sample past its preload through `StreamSource::read`.
pub fn read_streamed(_voice: &Voice, _out: [&mut [f32]; 2]) -> bool {
    todo!("M3/T2: streamed playback")
}

/// Per-group insert chains of built-in effects (instances allocated in `activate`).
pub fn group_effects(_plan: &Plan, _group: usize, _l: &mut [f32], _r: &mut [f32]) {
    todo!("M3/T2: per-group effects")
}
