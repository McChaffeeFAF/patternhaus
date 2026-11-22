//! Audio export: WAV (16-bit, 24-bit, 32-bit float, via `hound`), FLAC (16/24-bit, via
//! `flacenc`) and Ogg Vorbis (lossy, via `vorbis_rs`: bindings to libvorbis with the aoTuV
//! patches, BSD-3-Clause; the C source is compiled by the crate, nothing is installed).
//!
//! Input is planar float audio (`[channel][frame]`, nominally -1.0..=1.0). Integer formats
//! clip at full scale. Reducing to 16 bits applies TPDF dither (two uniform random values
//! summed, ±1 LSB peak) from a seeded generator, so an export is reproducible bit for bit.
//! 24-bit and float exports are not dithered.

use std::io::Cursor;
use std::path::Path;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EncodeError {
    #[error("nothing to encode: {0}")]
    Empty(String),
    #[error("channels have different lengths")]
    Ragged,
    #[error("unsupported export setting: {0}")]
    Unsupported(String),
    #[error("encoder failed: {0}")]
    Encoder(String),
    #[error("cannot write {path}: {message}")]
    Io { path: String, message: String },
}

pub type EncodeResult<T> = Result<T, EncodeError>;

/// Sample format of a WAV export.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WavFormat {
    /// 16-bit integer PCM, TPDF-dithered.
    Pcm16,
    /// 24-bit integer PCM.
    Pcm24,
    /// 32-bit IEEE float.
    Float32,
}

/// Bit depth of a FLAC export (FLAC is integer-only).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlacDepth {
    /// 16-bit, TPDF-dithered.
    Bits16,
    Bits24,
}

/// Default Ogg Vorbis quality: about 160 kbit/s for stereo music, so a five-minute stereo
/// render is roughly 6 MB.
pub const VORBIS_DEFAULT_QUALITY: f32 = 0.5;
/// Lowest and highest Ogg Vorbis quality (libvorbis VBR scale).
pub const VORBIS_QUALITY_RANGE: (f32, f32) = (-0.1, 1.0);
/// Fixed Ogg stream serial, so an export is reproducible bit for bit.
const VORBIS_SERIAL: i32 = 0x5048_4155;

/// Encodes planar audio as an Ogg Vorbis file in memory. `quality` is the libvorbis VBR
/// scale, -0.1 (about 45 kbit/s) to 1.0 (about 500 kbit/s); 0.5 is the default.
pub fn encode_vorbis<C: AsRef<[f32]>>(
    channels: &[C],
    sample_rate: u32,
    quality: f32,
) -> EncodeResult<Vec<u8>> {
    use std::num::{NonZeroU8, NonZeroU32};
    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};
    let frames = check(channels, sample_rate)?;
    if channels.len() > 8 {
        return Err(EncodeError::Unsupported(format!(
            "Ogg Vorbis supports at most 8 channels, got {}",
            channels.len()
        )));
    }
    if !quality.is_finite() || quality < VORBIS_QUALITY_RANGE.0 || quality > VORBIS_QUALITY_RANGE.1
    {
        return Err(EncodeError::Unsupported(format!(
            "Ogg Vorbis quality must be between {} and {}, got {quality}",
            VORBIS_QUALITY_RANGE.0, VORBIS_QUALITY_RANGE.1
        )));
    }
    let enc = |e: vorbis_rs::VorbisError| EncodeError::Encoder(e.to_string());
    let mut builder = VorbisEncoderBuilder::new_with_serial(
        NonZeroU32::new(sample_rate).expect("checked above"),
        NonZeroU8::new(channels.len() as u8).expect("checked above"),
        Vec::with_capacity(frames / 4 + 4096),
        VORBIS_SERIAL,
    );
    builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr {
        target_quality: quality,
    });
    let mut encoder = builder.build().map_err(enc)?;
    // blocks of about a second: the encoder copies each block into its own buffers
    const BLOCK: usize = 4096;
    let mut start = 0;
    while start < frames {
        let end = (start + BLOCK).min(frames);
        let block: Vec<Vec<f32>> = channels
            .iter()
            .map(|c| {
                c.as_ref()[start..end]
                    .iter()
                    .map(|v| {
                        if v.is_finite() {
                            v.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        }
                    })
                    .collect()
            })
            .collect();
        encoder.encode_audio_block(&block).map_err(enc)?;
        start = end;
    }
    encoder.finish().map_err(enc)
}

/// Writes an Ogg Vorbis file.
pub fn write_vorbis<C: AsRef<[f32]>>(
    path: &Path,
    channels: &[C],
    sample_rate: u32,
    quality: f32,
) -> EncodeResult<()> {
    let bytes = encode_vorbis(channels, sample_rate, quality)?;
    write_file(path, &bytes)
}

/// Default dither seed; any fixed value gives reproducible exports.
pub const DITHER_SEED: u64 = 0x5eed_d174_e500_0001;

/// Encodes planar audio as a WAV file in memory.
pub fn encode_wav<C: AsRef<[f32]>>(
    channels: &[C],
    sample_rate: u32,
    format: WavFormat,
    dither_seed: u64,
) -> EncodeResult<Vec<u8>> {
    let frames = check(channels, sample_rate)?;
    let (bits, sample_format) = match format {
        WavFormat::Pcm16 => (16, hound::SampleFormat::Int),
        WavFormat::Pcm24 => (24, hound::SampleFormat::Int),
        WavFormat::Float32 => (32, hound::SampleFormat::Float),
    };
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate,
        bits_per_sample: bits,
        sample_format,
    };
    let enc = |e: hound::Error| EncodeError::Encoder(e.to_string());
    let mut cursor = Cursor::new(Vec::with_capacity(
        frames * channels.len() * bits as usize / 8 + 64,
    ));
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec).map_err(enc)?;
        match format {
            WavFormat::Float32 => {
                for i in 0..frames {
                    for c in channels {
                        w.write_sample(c.as_ref()[i]).map_err(enc)?;
                    }
                }
            }
            WavFormat::Pcm16 => {
                let mut q = Quantizer::new(16, true, dither_seed);
                for i in 0..frames {
                    for c in channels {
                        w.write_sample(q.quantize(c.as_ref()[i]) as i16)
                            .map_err(enc)?;
                    }
                }
            }
            WavFormat::Pcm24 => {
                let mut q = Quantizer::new(24, false, dither_seed);
                for i in 0..frames {
                    for c in channels {
                        w.write_sample(q.quantize(c.as_ref()[i])).map_err(enc)?;
                    }
                }
            }
        }
        w.finalize().map_err(enc)?;
    }
    Ok(cursor.into_inner())
}

/// Encodes planar audio as a FLAC file in memory.
pub fn encode_flac<C: AsRef<[f32]>>(
    channels: &[C],
    sample_rate: u32,
    depth: FlacDepth,
    dither_seed: u64,
) -> EncodeResult<Vec<u8>> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let frames = check(channels, sample_rate)?;
    if channels.len() > 8 {
        return Err(EncodeError::Unsupported(format!(
            "FLAC supports at most 8 channels, got {}",
            channels.len()
        )));
    }
    let (bits, dither) = match depth {
        FlacDepth::Bits16 => (16, true),
        FlacDepth::Bits24 => (24, false),
    };
    let mut q = Quantizer::new(bits, dither, dither_seed);
    let mut interleaved = Vec::with_capacity(frames * channels.len());
    for i in 0..frames {
        for c in channels {
            interleaved.push(q.quantize(c.as_ref()[i]));
        }
    }
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| EncodeError::Encoder(format!("{e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(
        &interleaved,
        channels.len(),
        bits as usize,
        sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| EncodeError::Encoder(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| EncodeError::Encoder(format!("{e:?}")))?;
    let mut bytes = sink.as_slice().to_vec();
    fix_streaminfo_min_block(&mut bytes);
    Ok(bytes)
}

/// FLAC's STREAMINFO minimum block size excludes the last block, but `flacenc` records the
/// short final block there. Decoders (symphonia among them) then take the stream for a
/// variable-block-size one and fail. With a fixed block size, min = max is correct.
fn fix_streaminfo_min_block(bytes: &mut [u8]) {
    // "fLaC", then the STREAMINFO header (type 0) and its 16-bit min and max block sizes
    if bytes.len() >= 12 && &bytes[..4] == b"fLaC" && bytes[4] & 0x7f == 0 {
        bytes[8] = bytes[10];
        bytes[9] = bytes[11];
    }
}

/// Writes a WAV file.
pub fn write_wav<C: AsRef<[f32]>>(
    path: &Path,
    channels: &[C],
    sample_rate: u32,
    format: WavFormat,
) -> EncodeResult<()> {
    let bytes = encode_wav(channels, sample_rate, format, DITHER_SEED)?;
    write_file(path, &bytes)
}

/// Writes a FLAC file.
pub fn write_flac<C: AsRef<[f32]>>(
    path: &Path,
    channels: &[C],
    sample_rate: u32,
    depth: FlacDepth,
) -> EncodeResult<()> {
    let bytes = encode_flac(channels, sample_rate, depth, DITHER_SEED)?;
    write_file(path, &bytes)
}

fn write_file(path: &Path, bytes: &[u8]) -> EncodeResult<()> {
    std::fs::write(path, bytes).map_err(|e| EncodeError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}

fn check<C: AsRef<[f32]>>(channels: &[C], sample_rate: u32) -> EncodeResult<usize> {
    if channels.is_empty() {
        return Err(EncodeError::Empty("no channels".into()));
    }
    if sample_rate == 0 {
        return Err(EncodeError::Unsupported("sample rate 0".into()));
    }
    if channels.len() > u16::MAX as usize {
        return Err(EncodeError::Unsupported("too many channels".into()));
    }
    let frames = channels[0].as_ref().len();
    if channels.iter().any(|c| c.as_ref().len() != frames) {
        return Err(EncodeError::Ragged);
    }
    Ok(frames)
}

/// Float → integer conversion with optional TPDF dither. Full scale is `2^(bits-1)`; the
/// result is clamped to the integer range, so +1.0 maps to the largest positive code.
struct Quantizer {
    scale: f64,
    min: f64,
    max: f64,
    dither: bool,
    state: u64,
}

impl Quantizer {
    fn new(bits: u32, dither: bool, seed: u64) -> Self {
        let scale = (1u64 << (bits - 1)) as f64;
        Self {
            scale,
            min: -scale,
            max: scale - 1.0,
            dither,
            // never zero: xorshift would stick
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// Uniform in `[0, 1)`.
    fn uniform(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn quantize(&mut self, x: f32) -> i32 {
        let x = if x.is_finite() { x as f64 } else { 0.0 };
        let mut v = x * self.scale;
        if self.dither {
            // TPDF: sum of two uniforms, -1..1 LSB
            v += self.uniform() - self.uniform();
        }
        v.round().clamp(self.min, self.max) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_bytes;

    fn sine(frames: usize, amp: f32) -> Vec<Vec<f32>> {
        let l: Vec<f32> = (0..frames).map(|i| amp * (i as f32 * 0.05).sin()).collect();
        let r: Vec<f32> = l.iter().map(|v| -v * 0.5).collect();
        vec![l, r]
    }

    #[test]
    fn wav_float_round_trips_exactly() {
        let a = sine(1000, 0.9);
        let bytes = encode_wav(&a, 48000, WavFormat::Float32, 1).unwrap();
        let d = decode_bytes(&bytes, Some("wav")).unwrap();
        assert_eq!(d.sample_rate, 48000.0);
        assert_eq!(d.channels, a);
    }

    #[test]
    fn wav_24_and_16_round_trip_within_one_lsb_plus_dither() {
        let a = sine(4000, 0.8);
        for (fmt, tol) in [
            (WavFormat::Pcm24, 1.0 / 8_388_608.0),
            (WavFormat::Pcm16, 2.0 / 32768.0),
        ] {
            let bytes = encode_wav(&a, 44100, fmt, 7).unwrap();
            let d = decode_bytes(&bytes, Some("wav")).unwrap();
            assert_eq!(d.channels.len(), 2);
            for (src, dec) in a.iter().zip(&d.channels) {
                for (x, y) in src.iter().zip(dec) {
                    assert!((x - y).abs() <= tol + 1e-7, "{fmt:?}: {x} vs {y}");
                }
            }
        }
    }

    #[test]
    fn sixteen_bit_is_dithered_and_reproducible() {
        // a constant quarter-LSB signal: undithered it rounds to 0 everywhere
        let a = vec![vec![0.25f32 / 32768.0; 20000]];
        let b1 = encode_wav(&a, 48000, WavFormat::Pcm16, 3).unwrap();
        let b2 = encode_wav(&a, 48000, WavFormat::Pcm16, 3).unwrap();
        assert_eq!(b1, b2, "same seed, same bytes");
        let d = decode_bytes(&b1, Some("wav")).unwrap();
        let mean: f64 = d.channels[0].iter().map(|v| *v as f64).sum::<f64>() / 20000.0;
        let nonzero = d.channels[0].iter().filter(|v| **v != 0.0).count();
        assert!(nonzero > 1000, "dither should toggle codes");
        // TPDF dither is unbiased: the mean keeps the sub-LSB level
        assert!(
            (mean * 32768.0 - 0.25).abs() < 0.05,
            "mean {}",
            mean * 32768.0
        );
        let peak = d.channels[0].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 2.0 / 32768.0);
    }

    #[test]
    fn clipping_is_clamped() {
        let a = vec![vec![2.0f32, -2.0, 1.0, -1.0, f32::NAN]];
        let bytes = encode_wav(&a, 48000, WavFormat::Pcm24, 0).unwrap();
        let d = decode_bytes(&bytes, Some("wav")).unwrap();
        assert!(d.channels[0][0] > 0.9999 && d.channels[0][0] <= 1.0);
        assert_eq!(d.channels[0][1], -1.0);
        assert_eq!(d.channels[0][4], 0.0);
    }

    /// Two seconds of a chord-like mix with a decaying noise burst each beat.
    fn music(seconds: f32, sr: u32) -> Vec<Vec<f32>> {
        let n = (seconds * sr as f32) as usize;
        let mut seed = 12345u32;
        let l: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
                let env = (-((t * 2.0).fract()) * 12.0).exp();
                0.25 * (t * 220.0 * std::f32::consts::TAU).sin()
                    + 0.2 * (t * 277.2 * std::f32::consts::TAU).sin()
                    + 0.2 * (t * 329.6 * std::f32::consts::TAU).sin()
                    + 0.2 * noise * env
            })
            .collect();
        let r: Vec<f32> = l.iter().rev().map(|v| v * 0.8).collect();
        vec![l, r]
    }

    fn rms(c: &[f32]) -> f64 {
        (c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64).sqrt()
    }

    #[test]
    fn vorbis_round_trips_with_matching_level_and_duration() {
        let a = music(4.0, 48000);
        let bytes = encode_vorbis(&a, 48000, VORBIS_DEFAULT_QUALITY).unwrap();
        assert_eq!(&bytes[..4], b"OggS");
        let d = decode_bytes(&bytes, Some("ogg")).unwrap();
        assert_eq!(d.sample_rate, 48000.0);
        assert_eq!(d.channels.len(), 2);
        // duration within one Vorbis block of the source
        let diff = (d.frames() as i64 - a[0].len() as i64).abs();
        assert!(diff <= 2048, "frames {} vs {}", d.frames(), a[0].len());
        for (src, dec) in a.iter().zip(&d.channels) {
            let (rs, rd) = (rms(src), rms(dec));
            let db = 20.0 * (rd / rs).log10();
            assert!(db.abs() < 0.5, "level moved by {db} dB");
        }
    }

    #[test]
    fn vorbis_is_reproducible_and_quality_sets_the_size() {
        let a = music(4.0, 44100);
        let q5 = encode_vorbis(&a, 44100, 0.5).unwrap();
        assert_eq!(q5, encode_vorbis(&a, 44100, 0.5).unwrap(), "same bytes");
        let q0 = encode_vorbis(&a, 44100, 0.0).unwrap();
        let q9 = encode_vorbis(&a, 44100, 0.9).unwrap();
        assert!(
            q0.len() < q5.len() && q5.len() < q9.len(),
            "{} {} {}",
            q0.len(),
            q5.len(),
            q9.len()
        );
        // far smaller than the 16-bit WAV (4 s x 44.1 kHz x 2 ch x 2 B = 705 kB)
        assert!(q5.len() < 705_600 / 4, "{} bytes", q5.len());
    }

    #[test]
    fn vorbis_default_quality_stays_within_a_few_megabytes_per_five_minutes() {
        // bytes per second of the default quality, extrapolated to 300 s
        let a = music(8.0, 48000);
        let per_second = encode_vorbis(&a, 48000, VORBIS_DEFAULT_QUALITY)
            .unwrap()
            .len() as f64
            / 8.0;
        let five_minutes = per_second * 300.0 / 1e6;
        assert!(
            (1.0..=12.0).contains(&five_minutes),
            "five minutes would be {five_minutes} MB"
        );
    }

    #[test]
    fn vorbis_rejects_bad_settings() {
        let a = music(0.5, 48000);
        assert!(matches!(
            encode_vorbis(&a, 48000, 1.5),
            Err(EncodeError::Unsupported(_))
        ));
        assert!(matches!(
            encode_vorbis(&a, 48000, f32::NAN),
            Err(EncodeError::Unsupported(_))
        ));
        let none: Vec<Vec<f32>> = vec![];
        assert!(matches!(
            encode_vorbis(&none, 48000, 0.5),
            Err(EncodeError::Empty(_))
        ));
    }

    #[test]
    fn flac_round_trips_losslessly_at_24_bit() {
        let a = sine(10000, 0.7);
        let bytes = encode_flac(&a, 48000, FlacDepth::Bits24, 0).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        let d = decode_bytes(&bytes, Some("flac")).unwrap();
        assert_eq!(d.frames(), 10000);
        let wav = encode_wav(&a, 48000, WavFormat::Pcm24, 0).unwrap();
        let w = decode_bytes(&wav, Some("wav")).unwrap();
        assert_eq!(d.channels, w.channels, "FLAC 24 = WAV 24 sample for sample");
        let bytes16 = encode_flac(&a, 48000, FlacDepth::Bits16, 0).unwrap();
        let d16 = decode_bytes(&bytes16, Some("flac")).unwrap();
        let wav16 = encode_wav(&a, 48000, WavFormat::Pcm16, 0).unwrap();
        let w16 = decode_bytes(&wav16, Some("wav")).unwrap();
        assert_eq!(d16.channels, w16.channels, "same dither in both containers");
    }

    #[test]
    fn errors() {
        let none: Vec<Vec<f32>> = vec![];
        assert!(matches!(
            encode_wav(&none, 48000, WavFormat::Pcm16, 0),
            Err(EncodeError::Empty(_))
        ));
        let ragged = vec![vec![0.0f32; 3], vec![0.0f32; 4]];
        assert_eq!(
            encode_flac(&ragged, 48000, FlacDepth::Bits16, 0),
            Err(EncodeError::Ragged)
        );
        assert!(
            write_wav(
                Path::new("/nonexistent-dir/x/y.wav"),
                &[vec![0.0f32; 4]],
                48000,
                WavFormat::Float32
            )
            .is_err()
        );
    }
}
