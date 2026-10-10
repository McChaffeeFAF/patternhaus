//! Audio decoding: WAV, AIFF, FLAC, MP3 and Ogg Vorbis through `symphonia`, to
//! de-interleaved `f32` [`SampleData`] at the file's own sample rate (the sampler
//! resamples at play time). WAV files also yield their `smpl` chunk's first loop and MIDI
//! unity note.
//!
//! Decoding never runs on the audio thread. A panic inside the decoder (malformed input)
//! is caught and reported as [`DecodeError::Malformed`].

use ph_node::SampleData;
use std::io::Cursor;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DecodeError {
    #[error("cannot read {0}")]
    Io(String),
    #[error("unsupported or unrecognised audio format: {0}")]
    Unsupported(String),
    #[error("malformed audio data: {0}")]
    Malformed(String),
    #[error("the file contains no audio")]
    Empty,
}

/// WAV `smpl` chunk contents we use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerChunk {
    /// MIDI unity note (root key).
    pub root_key: Option<u8>,
    /// First loop, `[start, end)` in frames (the chunk stores an inclusive end).
    pub loop_range: Option<(usize, usize)>,
}

/// Decode an in-memory file. `extension` (e.g. `"wav"`) helps format detection.
pub fn decode_bytes(bytes: &[u8], extension: Option<&str>) -> Result<SampleData, DecodeError> {
    let smpl = read_wav_smpl(bytes);
    let source = Box::new(Cursor::new(bytes.to_vec()));
    let mut data = decode_source(source, extension)?;
    apply_smpl(&mut data, smpl);
    Ok(data)
}

/// Decode a file from disk.
pub fn decode_file(path: &Path) -> Result<SampleData, DecodeError> {
    let bytes =
        std::fs::read(path).map_err(|e| DecodeError::Io(format!("{}: {e}", path.display())))?;
    let ext = path.extension().and_then(|e| e.to_str());
    decode_bytes(&bytes, ext)
}

fn apply_smpl(data: &mut SampleData, smpl: Option<SamplerChunk>) {
    if let Some(s) = smpl {
        data.root_key = s.root_key;
        data.loop_range = s.loop_range.filter(|(a, b)| a < b && *b <= data.frames());
    }
}

fn decode_source(
    source: Box<dyn MediaSource>,
    extension: Option<&str>,
) -> Result<SampleData, DecodeError> {
    let ext = extension.map(|e| e.to_ascii_lowercase());
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_inner(source, ext.as_deref())
    }))
    .unwrap_or_else(|_| Err(DecodeError::Malformed("decoder panicked".into())))
}

fn decode_inner(
    source: Box<dyn MediaSource>,
    ext: Option<&str>,
) -> Result<SampleData, DecodeError> {
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(e) = ext {
        hint.with_extension(e);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::Empty)?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0) as f64;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if channels.first().is_some_and(|c| !c.is_empty()) {
                    // keep what decoded cleanly before a damaged tail
                    break;
                }
                return Err(DecodeError::Malformed(e.to_string()));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(DecodeError::Malformed(e.to_string())),
        };
        let spec = *decoded.spec();
        let n_ch = spec.channels.count().max(1);
        if sample_rate == 0.0 {
            sample_rate = spec.rate as f64;
        }
        if channels.is_empty() {
            channels = vec![Vec::new(); n_ch];
        }
        let needs_new = buf
            .as_ref()
            .is_none_or(|b| b.capacity() < decoded.capacity() * n_ch);
        if needs_new {
            buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        }
        let b = buf.as_mut().expect("buffer");
        b.copy_interleaved_ref(decoded);
        for (i, v) in b.samples().iter().enumerate() {
            if let Some(c) = channels.get_mut(i % n_ch) {
                c.push(v.clamp(-1.0, 1.0));
            }
        }
    }
    if channels.first().is_none_or(|c| c.is_empty()) || sample_rate <= 0.0 {
        return Err(DecodeError::Empty);
    }
    Ok(SampleData {
        sample_rate,
        channels,
        loop_range: None,
        root_key: None,
    })
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parse the `smpl` chunk of a RIFF/WAVE file (MIDI unity note and first loop), if any.
pub fn read_wav_smpl(bytes: &[u8]) -> Option<SamplerChunk> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_le(bytes, at + 4)? as usize;
        let body = at + 8;
        if id == b"smpl" {
            let unity = u32_le(bytes, body + 12)?;
            let loops = u32_le(bytes, body + 28)?;
            let mut loop_range = None;
            if loops > 0 {
                let l = body + 36;
                let start = u32_le(bytes, l + 8)? as usize;
                let end = u32_le(bytes, l + 12)? as usize;
                if end >= start {
                    loop_range = Some((start, end + 1));
                }
            }
            return Some(SamplerChunk {
                root_key: (unity <= 127).then_some(unity as u8),
                loop_range,
            });
        }
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tiny encoders for test fixtures ----------------------------------------------

    fn wav(channels: &[Vec<f32>], sr: u32, format: &str, smpl: Option<(u32, u32, u32)>) -> Vec<u8> {
        let n_ch = channels.len() as u16;
        let frames = channels[0].len();
        let (fmt_tag, bits): (u16, u16) = match format {
            "pcm16" => (1, 16),
            "pcm24" => (1, 24),
            _ => (3, 32),
        };
        let mut data = Vec::new();
        for i in 0..frames {
            for c in channels {
                let v = c[i];
                match format {
                    "pcm16" => {
                        data.extend_from_slice(&((v * 32767.0).round() as i16).to_le_bytes())
                    }
                    "pcm24" => {
                        let x = (v * 8_388_607.0).round() as i32;
                        data.extend_from_slice(&x.to_le_bytes()[..3]);
                    }
                    _ => data.extend_from_slice(&v.to_le_bytes()),
                }
            }
        }
        let block = n_ch * bits / 8;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&fmt_tag.to_le_bytes());
        fmt.extend_from_slice(&n_ch.to_le_bytes());
        fmt.extend_from_slice(&sr.to_le_bytes());
        fmt.extend_from_slice(&(sr * block as u32).to_le_bytes());
        fmt.extend_from_slice(&block.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        let mut chunk = |id: &[u8], c: &[u8]| {
            body.extend_from_slice(id);
            body.extend_from_slice(&(c.len() as u32).to_le_bytes());
            body.extend_from_slice(c);
            if c.len() % 2 == 1 {
                body.push(0);
            }
        };
        chunk(b"fmt ", &fmt);
        if let Some((unity, ls, le)) = smpl {
            let mut s = Vec::new();
            for v in [
                0u32,
                0,
                1_000_000_000 / sr,
                unity,
                0,
                0,
                0,
                1,
                0,
                0,
                0,
                ls,
                le,
                0,
                0,
            ] {
                s.extend_from_slice(&v.to_le_bytes());
            }
            chunk(b"smpl", &s);
        }
        chunk(b"data", &data);
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// 80-bit IEEE extended, as AIFF stores its sample rate.
    fn extended(v: f64) -> [u8; 10] {
        let mut out = [0u8; 10];
        let exp = v.log2().floor() as i32;
        let mant = (v / 2f64.powi(exp) * (1u64 << 63) as f64) as u64;
        out[..2].copy_from_slice(&((exp + 16383) as u16).to_be_bytes());
        out[2..].copy_from_slice(&mant.to_be_bytes());
        out
    }

    fn aiff(channels: &[Vec<f32>], sr: f64) -> Vec<u8> {
        let frames = channels[0].len() as u32;
        let mut comm = Vec::new();
        comm.extend_from_slice(&(channels.len() as u16).to_be_bytes());
        comm.extend_from_slice(&frames.to_be_bytes());
        comm.extend_from_slice(&16u16.to_be_bytes());
        comm.extend_from_slice(&extended(sr));
        let mut ssnd = vec![0u8; 8];
        for i in 0..frames as usize {
            for c in channels {
                ssnd.extend_from_slice(&((c[i] * 32767.0).round() as i16).to_be_bytes());
            }
        }
        let mut body = b"AIFF".to_vec();
        for (id, c) in [(b"COMM", &comm), (b"SSND", &ssnd)] {
            body.extend_from_slice(id);
            body.extend_from_slice(&(c.len() as u32).to_be_bytes());
            body.extend_from_slice(c);
        }
        let mut out = b"FORM".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    struct Bits {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }

    impl Bits {
        fn put(&mut self, v: u64, bits: u32) {
            for i in (0..bits).rev() {
                self.acc = (self.acc << 1) | ((v >> i) & 1);
                self.n += 1;
                if self.n == 8 {
                    self.out.push(self.acc as u8);
                    self.acc = 0;
                    self.n = 0;
                }
            }
        }
    }

    fn crc8(d: &[u8]) -> u8 {
        let mut c = 0u8;
        for b in d {
            c ^= b;
            for _ in 0..8 {
                c = if c & 0x80 != 0 {
                    (c << 1) ^ 0x07
                } else {
                    c << 1
                };
            }
        }
        c
    }

    fn crc16(d: &[u8]) -> u16 {
        let mut c = 0u16;
        for b in d {
            c ^= (*b as u16) << 8;
            for _ in 0..8 {
                c = if c & 0x8000 != 0 {
                    (c << 1) ^ 0x8005
                } else {
                    c << 1
                };
            }
        }
        c
    }

    /// Minimal FLAC: STREAMINFO + one frame of VERBATIM 16-bit subframes.
    fn flac(channels: &[Vec<f32>], sr: u32) -> Vec<u8> {
        let n = channels[0].len();
        assert!(n <= 4096);
        let n_ch = channels.len() as u64;
        let mut out = b"fLaC".to_vec();
        // STREAMINFO (last metadata block)
        out.push(0x80);
        out.extend_from_slice(&[0, 0, 34]);
        let mut si = Bits {
            out: Vec::new(),
            acc: 0,
            n: 0,
        };
        si.put(n as u64, 16);
        si.put(n as u64, 16);
        si.put(0, 24);
        si.put(0, 24);
        si.put(sr as u64, 20);
        si.put(n_ch - 1, 3);
        si.put(15, 5);
        si.put(n as u64, 36);
        si.put(0, 64);
        si.put(0, 64);
        out.extend_from_slice(&si.out);
        // frame
        let mut f = Bits {
            out: Vec::new(),
            acc: 0,
            n: 0,
        };
        f.put(0b11111111111110, 14);
        f.put(0, 1);
        f.put(0, 1); // fixed blocksize
        f.put(0b0111, 4); // blocksize: 16-bit (n-1) at end of header
        f.put(0, 4); // sample rate from STREAMINFO
        f.put(n_ch - 1, 4); // independent channels
        f.put(0b100, 3); // 16 bits per sample
        f.put(0, 1);
        f.put(0, 8); // frame number 0 (UTF-8)
        f.put(n as u64 - 1, 16);
        let crc = crc8(&f.out);
        f.put(crc as u64, 8);
        for c in channels {
            f.put(0, 1);
            f.put(1, 6); // VERBATIM
            f.put(0, 1);
            for v in c {
                f.put(((v * 32767.0).round() as i16) as u16 as u64, 16);
            }
        }
        while f.n != 0 {
            f.put(0, 1);
        }
        let crc = crc16(&f.out);
        f.out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&f.out);
        out
    }

    fn tone(n: usize, k: f32) -> Vec<Vec<f32>> {
        vec![
            (0..n).map(|i| 0.5 * (i as f32 * 0.05 * k).sin()).collect(),
            (0..n)
                .map(|i| -0.25 * (i as f32 * 0.031 * k).cos())
                .collect(),
        ]
    }

    fn close(a: &[Vec<f32>], b: &[Vec<f32>], tol: f32) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x.len(), y.len());
            for (u, v) in x.iter().zip(y) {
                assert!((u - v).abs() <= tol, "{u} vs {v}");
            }
        }
    }

    #[test]
    fn wav_formats_and_smpl() {
        let src = tone(1000, 1.0);
        for (fmt, tol) in [("pcm16", 1.0 / 32000.0), ("pcm24", 1e-6), ("f32", 0.0)] {
            let bytes = wav(&src, 44100, fmt, Some((62, 100, 899)));
            let d = decode_bytes(&bytes, Some("wav")).unwrap();
            assert_eq!(d.sample_rate, 44100.0);
            close(&d.channels, &src, tol);
            assert_eq!(d.root_key, Some(62));
            assert_eq!(d.loop_range, Some((100, 900)));
        }
        let plain = wav(&src, 48000, "pcm16", None);
        let d = decode_bytes(&plain, None).unwrap();
        assert_eq!(d.sample_rate, 48000.0);
        assert_eq!((d.root_key, d.loop_range), (None, None));
    }

    #[test]
    fn aiff_decodes() {
        let src = tone(777, 0.7);
        let d = decode_bytes(&aiff(&src, 22050.0), Some("aiff")).unwrap();
        assert_eq!(d.sample_rate, 22050.0);
        close(&d.channels, &src, 1.0 / 32000.0);
    }

    #[test]
    fn flac_decodes() {
        let src = tone(1024, 1.3);
        let d = decode_bytes(&flac(&src, 48000), Some("flac")).unwrap();
        assert_eq!(d.sample_rate, 48000.0);
        close(&d.channels, &src, 1.0 / 32000.0);
    }

    #[test]
    fn file_roundtrip_and_io_error() {
        let dir = std::env::temp_dir().join(format!("ph-formats-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        std::fs::write(&path, wav(&tone(64, 1.0), 44100, "pcm16", None)).unwrap();
        assert_eq!(decode_file(&path).unwrap().frames(), 64);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(
            decode_file(Path::new("no/such/file.wav")),
            Err(DecodeError::Io(_))
        ));
    }

    /// Robustness (a light fuzz): garbage, truncations and bit flips of every fixture
    /// return errors or audio, never panic or hang.
    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        for ext in ["wav", "aiff", "flac", "mp3", "ogg"] {
            assert!(decode_bytes(b"definitely not audio", Some(ext)).is_err());
            assert!(decode_bytes(&[], Some(ext)).is_err());
        }
        let src = tone(500, 1.0);
        let fixtures = [
            (wav(&src, 44100, "pcm16", Some((60, 1, 400))), "wav"),
            (aiff(&src, 44100.0), "aiff"),
            (flac(&src, 44100), "flac"),
        ];
        let mut r = 0x1234_5678u64;
        let mut next = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            r
        };
        for (bytes, ext) in &fixtures {
            for cut in (0..bytes.len()).step_by(37) {
                let _ = decode_bytes(&bytes[..cut], Some(ext));
            }
            for _ in 0..200 {
                let mut b = bytes.clone();
                for _ in 0..4 {
                    let i = (next() as usize) % b.len();
                    b[i] ^= 1 << (next() % 8);
                }
                if let Ok(d) = decode_bytes(&b, Some(ext)) {
                    assert!(d.channels.iter().all(|c| c.iter().all(|v| v.is_finite())));
                }
            }
        }
    }
}
