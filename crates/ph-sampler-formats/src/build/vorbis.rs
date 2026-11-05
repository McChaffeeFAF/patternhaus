//! A minimal Ogg Vorbis **writer** for the SF3 fixtures (`docs/formats/sf2.md` §12,
//! F-SF3-01/02). `ph-formats` decodes Vorbis but has no encoder, so the fixtures build their
//! streams here, from the public Vorbis I and Ogg framing specifications (no encoder source
//! was read): short blocks only, a flat floor 1 curve, and a type 1 residue that puts one
//! spectral line in every block, so the decoded audio is a steady tone. It is not a general
//! encoder: it writes exactly the streams the fixtures need.

/// LSB-first bit packer (Vorbis I §2.1).
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    bit: u32,
}

impl Bits {
    fn put(&mut self, value: u64, n: u32) {
        for i in 0..n {
            if self.bit == 0 {
                self.out.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.out.last_mut().expect("byte") |= 1 << self.bit;
            }
            self.bit = (self.bit + 1) % 8;
        }
    }
}

const BLOCK: usize = 256;
const HALF: usize = BLOCK / 2;
const PART: usize = 32;

/// Vorbis `float32_unpack` encoding of a small positive integer.
fn float32(v: u32) -> u64 {
    if v == 0 { 0 } else { (788u64 << 21) | v as u64 }
}

fn identification(channels: u8, rate: u32) -> Vec<u8> {
    let mut v = vec![1];
    v.extend_from_slice(b"vorbis");
    v.extend_from_slice(&0u32.to_le_bytes());
    v.push(channels);
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&[0; 12]); // bitrates
    let exp = BLOCK.trailing_zeros() as u8;
    v.push(exp | exp << 4); // blocksize 0 and 1 both 256
    v.push(1); // framing
    v
}

fn comment() -> Vec<u8> {
    let vendor = b"Patternhaus fixture";
    let mut v = vec![3];
    v.extend_from_slice(b"vorbis");
    v.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    v.extend_from_slice(vendor);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.push(1);
    v
}

fn setup(level: u32) -> Vec<u8> {
    let mut b = Bits::default();
    b.out.push(5);
    b.out.extend_from_slice(b"vorbis");
    b.put(1, 8); // two codebooks
    // codebook 0: residue classes, 1 dimension, 2 entries of length 1, no lookup
    // codebook 1: values {0, level}, 1 dimension, 2 entries of length 1, lookup type 1
    for book in 0..2 {
        b.put(0x56_4342, 24);
        b.put(1, 16);
        b.put(2, 24);
        b.put(0, 1); // not ordered
        b.put(0, 1); // not sparse
        b.put(0, 5);
        b.put(0, 5);
        if book == 0 {
            b.put(0, 4);
        } else {
            b.put(1, 4);
            b.put(float32(0), 32);
            b.put(float32(level), 32);
            b.put(0, 4); // value bits − 1
            b.put(0, 1); // sequence_p
            b.put(0, 1);
            b.put(1, 1);
        }
    }
    b.put(0, 6); // one time-domain transform
    b.put(0, 16);
    b.put(0, 6); // one floor, type 1
    b.put(1, 16);
    b.put(1, 5); // one partition
    b.put(0, 4); // of class 0
    b.put(0, 3); // class 0: dimension 1
    b.put(0, 2); // no subclasses
    b.put(0, 8); // subclass book unused
    b.put(0, 2); // multiplier 1
    b.put(7, 4); // range bits: X list 0, 128
    b.put(64, 7);
    b.put(0, 6); // one residue, type 1
    b.put(1, 16);
    b.put(0, 24);
    b.put(HALF as u64, 24);
    b.put(PART as u64 - 1, 24);
    b.put(1, 6); // two classifications
    b.put(0, 8); // class book 0
    b.put(0, 3); // class 0: no passes
    b.put(0, 1);
    b.put(1, 3); // class 1: pass 0
    b.put(0, 1);
    b.put(1, 8); // pass 0 book 1
    b.put(0, 6); // one mapping, type 0
    b.put(0, 16);
    b.put(0, 1); // one submap
    b.put(0, 1); // no coupling
    b.put(0, 2);
    b.put(0, 8);
    b.put(0, 8); // floor 0
    b.put(0, 8); // residue 0
    b.put(0, 6); // one mode
    b.put(0, 1); // short blocks
    b.put(0, 16);
    b.put(0, 16);
    b.put(0, 8);
    b.put(1, 1); // framing
    b.out
}

fn audio_packet(channels: u8, bin: usize) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(0, 1); // audio packet; one mode: no mode bits
    for _ in 0..channels {
        b.put(1, 1); // floor in use
        b.put(255, 8);
        b.put(255, 8);
    }
    // residue type 1: per partition a class word, then its values
    for p in 0..HALF / PART {
        for _ in 0..channels {
            let has = (p * PART..(p + 1) * PART).contains(&bin);
            b.put(has as u64, 1);
            if has {
                for i in 0..PART {
                    b.put((p * PART + i == bin) as u64, 1);
                }
            }
        }
    }
    b.out
}

fn crc(data: &[u8]) -> u32 {
    let mut c = 0u32;
    for &byte in data {
        c ^= (byte as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04c1_1db7
            } else {
                c << 1
            };
        }
    }
    c
}

fn page(out: &mut Vec<u8>, packets: &[Vec<u8>], flags: u8, granule: u64, seq: u32) {
    let mut lacing = Vec::new();
    for p in packets {
        let mut n = p.len();
        while n >= 255 {
            lacing.push(255);
            n -= 255;
        }
        lacing.push(n as u8);
    }
    let mut pg = b"OggS".to_vec();
    pg.push(0);
    pg.push(flags);
    pg.extend_from_slice(&granule.to_le_bytes());
    pg.extend_from_slice(&0x5048_u32.to_le_bytes());
    pg.extend_from_slice(&seq.to_le_bytes());
    pg.extend_from_slice(&[0; 4]);
    pg.push(lacing.len() as u8);
    pg.extend_from_slice(&lacing);
    for p in packets {
        pg.extend_from_slice(p);
    }
    let c = crc(&pg);
    pg[22..26].copy_from_slice(&c.to_le_bytes());
    out.extend(pg);
}

/// An Ogg Vorbis stream of `blocks` short blocks (128 frames each after the first) holding
/// one spectral line at `bin` (0..128) in every block: a steady tone of about
/// `(bin + 0.5) × rate / 256` Hz. `channels` 1 or 2.
pub fn tone_stream(rate: u32, channels: u8, blocks: usize, bin: usize) -> Vec<u8> {
    let mut out = Vec::new();
    page(&mut out, &[identification(channels, rate)], 0x02, 0, 0);
    page(&mut out, &[comment(), setup(1)], 0, 0, 1);
    let blocks = blocks.max(2);
    let audio: Vec<Vec<u8>> = (0..blocks).map(|_| audio_packet(channels, bin)).collect();
    let mut seq = 2;
    for (i, chunk) in audio.chunks(32).enumerate() {
        let last = (i + 1) * 32 >= blocks;
        let done = (i * 32 + chunk.len()) as u64;
        page(
            &mut out,
            chunk,
            if last { 0x04 } else { 0 },
            (done - 1) * HALF as u64,
            seq,
        );
        seq += 1;
    }
    out
}

/// An Ogg page holding an Opus identification header (no audio): for the "Opus stream"
/// rejection path.
pub fn opus_header_stream() -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(1);
    head.extend_from_slice(&0u16.to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes());
    head.push(0);
    let mut out = Vec::new();
    page(&mut out, &[head], 0x06, 0, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_decodes_to_a_tone() {
        let s = tone_stream(44_100, 1, 40, 6);
        let d = ph_formats::decode::decode_bytes(&s, Some("ogg")).expect("decodes");
        assert_eq!(d.channels.len(), 1);
        assert_eq!(d.sample_rate, 44_100.0);
        let frames = d.frames();
        assert!(
            (39 * HALF - HALF..=40 * HALF).contains(&frames),
            "{frames} frames"
        );
        let peak = d.channels[0].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.05, "peak {peak}");
        let st = tone_stream(44_100, 2, 8, 6);
        let d2 = ph_formats::decode::decode_bytes(&st, Some("ogg")).expect("stereo decodes");
        assert_eq!(d2.channels.len(), 2);
    }
}
