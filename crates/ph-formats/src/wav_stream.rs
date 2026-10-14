//! Seekable WAV reading for disk streaming (M2/T1): [`WavStream`] parses the RIFF header
//! once and then reads any frame range straight from the file, decoding into the caller's
//! buffers without allocating. PCM 8/16/24/32-bit integer and 32/64-bit float, plain or
//! `WAVE_FORMAT_EXTENSIBLE`. Other formats (and odd WAV variants) are decoded whole through
//! [`crate::decode`] instead.
//!
//! Format facts from the public Microsoft RIFF/WAVE documentation ("Multimedia Programming
//! Interface and Data Specifications 1.0" and the `WAVEFORMATEXTENSIBLE` reference).

use crate::decode::DecodeError;
use ph_node::{SampleStream, StreamError};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Frames decoded per file read (the read buffer is this many frames long).
const CHUNK_FRAMES: usize = 4096;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Encoding {
    /// Unsigned 8-bit, signed 16/24/32-bit little-endian.
    Int(u16),
    /// IEEE float, 32 or 64 bits.
    Float(u16),
}

/// An open WAV file, ready for random-access reads.
pub struct WavStream {
    file: File,
    data_offset: u64,
    frames: u64,
    channels: usize,
    sample_rate: u32,
    encoding: Encoding,
    block_align: usize,
    buf: Vec<u8>,
}

impl std::fmt::Debug for WavStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WavStream")
            .field("frames", &self.frames)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("encoding", &self.encoding)
            .finish()
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl WavStream {
    /// Opens and validates `path`. Fails with [`DecodeError::Unsupported`] for anything that
    /// is not a WAV this reader handles.
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let io = |e: std::io::Error| DecodeError::Io(format!("{}: {e}", path.display()));
        let unsupported = |m: &str| DecodeError::Unsupported(format!("{}: {m}", path.display()));
        let mut file = File::open(path).map_err(io)?;
        let mut head = [0u8; 12];
        file.read_exact(&mut head).map_err(io)?;
        if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
            return Err(unsupported("not a RIFF/WAVE file"));
        }
        let mut fmt: Option<(u16, usize, u32, u16, usize)> = None;
        let mut data: Option<(u64, u64)> = None;
        let mut pos = 12u64;
        let len = file.metadata().map_err(io)?.len();
        while pos + 8 <= len && data.is_none() {
            let mut ch = [0u8; 8];
            file.seek(SeekFrom::Start(pos)).map_err(io)?;
            file.read_exact(&mut ch).map_err(io)?;
            let size = u32_at(&ch, 4) as u64;
            let body = pos + 8;
            match &ch[0..4] {
                b"fmt " => {
                    if size < 16 {
                        return Err(DecodeError::Malformed("fmt chunk too short".into()));
                    }
                    let mut f = vec![0u8; size.min(64) as usize];
                    file.read_exact(&mut f).map_err(io)?;
                    let mut tag = u16_at(&f, 0);
                    let channels = u16_at(&f, 2) as usize;
                    let rate = u32_at(&f, 4);
                    let align = u16_at(&f, 12) as usize;
                    let bits = u16_at(&f, 14);
                    if tag == 0xFFFE && f.len() >= 26 {
                        // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts with the tag
                        tag = u16_at(&f, 24);
                    }
                    fmt = Some((tag, channels, rate, bits, align));
                }
                b"data" => data = Some((body, size.min(len.saturating_sub(body)))),
                _ => {}
            }
            // chunks are word-aligned
            pos = body + size + (size & 1);
        }
        let (tag, channels, rate, bits, align) =
            fmt.ok_or_else(|| DecodeError::Malformed("no fmt chunk".into()))?;
        let (data_offset, data_len) = data.ok_or(DecodeError::Empty)?;
        let encoding = match (tag, bits) {
            (1, 8 | 16 | 24 | 32) => Encoding::Int(bits),
            (3, 32 | 64) => Encoding::Float(bits),
            _ => return Err(unsupported(&format!("format {tag} with {bits} bits"))),
        };
        let bytes = bits as usize / 8;
        if channels == 0 || rate == 0 || align != channels * bytes {
            return Err(DecodeError::Malformed(format!(
                "{} channels, {rate} Hz, block align {align}",
                channels
            )));
        }
        let frames = data_len / align as u64;
        if frames == 0 {
            return Err(DecodeError::Empty);
        }
        Ok(Self {
            file,
            data_offset,
            frames,
            channels,
            sample_rate: rate,
            encoding,
            block_align: align,
            buf: vec![0u8; CHUNK_FRAMES * align],
        })
    }

    #[inline]
    fn sample(&self, frame: &[u8], ch: usize) -> f32 {
        match self.encoding {
            Encoding::Int(8) => (frame[ch] as f32 - 128.0) / 128.0,
            Encoding::Int(16) => {
                i16::from_le_bytes([frame[2 * ch], frame[2 * ch + 1]]) as f32 / 32_768.0
            }
            Encoding::Int(24) => {
                let i = 3 * ch;
                let v = i32::from_le_bytes([0, frame[i], frame[i + 1], frame[i + 2]]) >> 8;
                v as f32 / 8_388_608.0
            }
            Encoding::Int(_) => {
                let i = 4 * ch;
                i32::from_le_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]) as f32
                    / 2_147_483_648.0
            }
            Encoding::Float(32) => {
                let i = 4 * ch;
                f32::from_le_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]])
            }
            Encoding::Float(_) => {
                let i = 8 * ch;
                let mut b = [0u8; 8];
                b.copy_from_slice(&frame[i..i + 8]);
                f64::from_le_bytes(b) as f32
            }
        }
    }
}

impl SampleStream for WavStream {
    fn frames(&self) -> u64 {
        self.frames
    }

    fn sample_rate(&self) -> f64 {
        self.sample_rate as f64
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn read(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, StreamError> {
        let n = out.iter().map(|c| c.len()).min().unwrap_or(0);
        let chans = out.len().min(self.channels);
        for c in out.iter_mut() {
            c[..n].fill(0.0);
        }
        let avail = self.frames.saturating_sub(start).min(n as u64) as usize;
        if avail == 0 || chans == 0 {
            return Ok(0);
        }
        let offset = self.data_offset + start * self.block_align as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| StreamError::Io)?;
        let mut done = 0;
        while done < avail {
            let k = (avail - done).min(CHUNK_FRAMES);
            let bytes = k * self.block_align;
            let mut buf = std::mem::take(&mut self.buf);
            let r = self.file.read_exact(&mut buf[..bytes]);
            if r.is_err() {
                self.buf = buf;
                return Err(StreamError::Io);
            }
            for i in 0..k {
                let frame = &buf[i * self.block_align..(i + 1) * self.block_align];
                for (c, dst) in out.iter_mut().enumerate().take(chans) {
                    dst[done + i] = self.sample(frame, c);
                }
            }
            self.buf = buf;
            done += k;
        }
        Ok(avail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{WavFormat, write_wav};

    #[test]
    fn reads_any_range_and_matches_the_whole_file_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let n = 10_000usize;
        let l: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.013).sin() * 0.7).collect();
        let r: Vec<f32> = l.iter().map(|v| -v * 0.5).collect();
        for (name, fmt) in [
            ("a16.wav", WavFormat::Pcm16),
            ("a24.wav", WavFormat::Pcm24),
            ("af.wav", WavFormat::Float32),
        ] {
            let path = dir.path().join(name);
            write_wav(&path, &[l.clone(), r.clone()], 44_100, fmt).unwrap();
            let whole = crate::decode::decode_file(&path).unwrap();
            let mut s = WavStream::open(&path).unwrap();
            assert_eq!(
                (s.frames(), s.channels(), s.sample_rate()),
                (n as u64, 2, 44_100.0)
            );
            for (start, len) in [(0u64, 100usize), (4321, 5000), (9_990, 64), (20_000, 8)] {
                let (mut a, mut b) = (vec![9.0f32; len], vec![9.0f32; len]);
                let got = s.read(start, &mut [&mut a, &mut b]).unwrap();
                let want = (n as u64).saturating_sub(start).min(len as u64) as usize;
                assert_eq!(got, want, "{name} {start}");
                for i in 0..len {
                    let k = start as usize + i;
                    let (el, er) = if k < n {
                        (whole.channels[0][k], whole.channels[1][k])
                    } else {
                        (0.0, 0.0)
                    };
                    assert_eq!((a[i], b[i]), (el, er), "{name} frame {k}");
                }
            }
        }
        let bad = dir.path().join("x.wav");
        std::fs::write(&bad, b"RIFF\0\0\0\0WAVEjunk").unwrap();
        assert!(WavStream::open(&bad).is_err());
    }
}
