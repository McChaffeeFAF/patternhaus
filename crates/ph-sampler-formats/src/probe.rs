//! What an importer needs to know about a sample file (length, rate, loop and root key from
//! the file), read through `ph-formats`' decoders. WAV files are read by their header and
//! `smpl` chunk only, so a big library imports without decoding its audio.

use ph_formats::decode::{decode_file, read_wav_smpl};
use ph_formats::wav_stream::WavStream;
use ph_node::SampleStream;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleInfo {
    pub frames: u64,
    pub sample_rate: u32,
    /// The file's first loop, `[start, end)` frames.
    pub loop_range: Option<(u64, u64)>,
    pub root_key: Option<u8>,
}

/// The `smpl` chunk of a WAV file, found by seeking from chunk to chunk.
fn wav_smpl(path: &Path) -> Option<ph_formats::decode::SamplerChunk> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h).ok()?;
        let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
        if &h[0..4] == b"smpl" {
            let size = size.min(4096);
            let mut body = vec![0u8; size as usize];
            f.read_exact(&mut body).ok()?;
            // a minimal RIFF holding only this chunk, for the existing parser
            let mut riff = Vec::with_capacity(20 + body.len());
            riff.extend_from_slice(b"RIFF");
            riff.extend_from_slice(&((12 + body.len()) as u32).to_le_bytes());
            riff.extend_from_slice(b"WAVEsmpl");
            riff.extend_from_slice(&(body.len() as u32).to_le_bytes());
            riff.extend_from_slice(&body);
            return read_wav_smpl(&riff);
        }
        pos += 8 + size + (size & 1);
    }
    None
}

/// Reads a sample file's facts. Errors are plain words for the import report.
pub fn probe(path: &Path) -> Result<SampleInfo, String> {
    let is_wav = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("wave"));
    if is_wav && let Ok(s) = WavStream::open(path) {
        let frames = s.frames();
        let smpl = wav_smpl(path);
        return Ok(SampleInfo {
            frames,
            sample_rate: s.sample_rate().round() as u32,
            loop_range: smpl
                .and_then(|c| c.loop_range)
                .map(|(a, b)| (a as u64, b as u64))
                .filter(|(a, b)| a < b && *b <= frames),
            root_key: smpl.and_then(|c| c.root_key),
        });
    }
    let d = decode_file(path).map_err(|e| e.to_string())?;
    if d.frames() == 0 {
        return Err("the file contains no audio".into());
    }
    Ok(SampleInfo {
        frames: d.frames() as u64,
        sample_rate: d.sample_rate.round() as u32,
        loop_range: d.loop_range.map(|(a, b)| (a as u64, b as u64)),
        root_key: d.root_key,
    })
}
