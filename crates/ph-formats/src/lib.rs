//! Patternhaus file formats.
//!
//! - [`decode`]: audio file decoding to [`ph_node::SampleData`] (WAV, AIFF, FLAC, MP3,
//!   Ogg Vorbis) via `symphonia`, plus WAV `smpl` loop points and root key.

pub mod decode;
