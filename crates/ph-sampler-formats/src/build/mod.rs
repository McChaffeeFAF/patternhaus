//! Writers for generated fixtures (`docs/formats/sampler-fixtures.md` rule 1): small,
//! byte-level builders of the binary formats, written from our own specs, used by the
//! fixture tests, the fuzz seeds and the command tests. Nothing here reads a real library.

pub mod sf2;
pub mod vorbis;

pub use sf2::{Sf2Builder, Sf2Chunks, Sf2Instrument, Sf2Preset, Sf2Sample, Sf2Zone};

/// A tagged test tone (rule 2 of the fixture plan): `id` × 0.05 DC for 10 ms at `rate`, then
/// a sine at `hz`, amplitude 0.4.
pub fn tagged_tone(id: u32, hz: f64, frames: usize, rate: u32) -> Vec<f32> {
    let tag = (rate / 100) as usize;
    (0..frames)
        .map(|i| {
            if i < tag {
                0.05 * id as f32
            } else {
                (0.4 * (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin()) as f32
            }
        })
        .collect()
}
