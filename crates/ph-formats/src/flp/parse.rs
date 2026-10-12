//! The container: the two chunks and the event framing (spec section 1). Never panics: every
//! read is bounds-checked and every failure is a [`FlpError`].

use std::fmt;

/// Why a byte sequence could not be read as an FLP file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlpError {
    /// Fewer bytes than the chunk headers need.
    TooShort,
    /// A chunk did not start with the expected four ASCII bytes.
    BadMagic {
        expected: &'static str,
        offset: usize,
    },
    /// The header chunk is shorter than its six bytes of fields.
    BadHeader(u32),
    /// The header says PPQ 0.
    BadPpq,
    /// The file is not a full project (spec 1.1: format 0), e.g. a score or state file.
    UnsupportedFormat(i16),
    /// The data ended in the middle of an event.
    Truncated { offset: usize },
    /// A size prefix ran past five bytes.
    BadVarint { offset: usize },
    /// The project model refused something the importer built (a bug, reported not panicked).
    Model(String),
}

impl From<ph_core::ModelError> for FlpError {
    fn from(e: ph_core::ModelError) -> Self {
        Self::Model(e.to_string())
    }
}

impl fmt::Display for FlpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "too short to be an FL Studio project"),
            Self::BadMagic { expected, offset } => {
                write!(
                    f,
                    "expected {expected:?} at byte {offset}: not an FL Studio project"
                )
            }
            Self::BadHeader(n) => write!(f, "header chunk is {n} bytes, need at least 6"),
            Self::BadPpq => write!(f, "the file declares 0 ticks per quarter note"),
            Self::UnsupportedFormat(n) => {
                write!(
                    f,
                    "file format {n} is not a full project (only 0 is supported)"
                )
            }
            Self::Truncated { offset } => {
                write!(f, "the file ends inside an event at byte {offset}")
            }
            Self::BadVarint { offset } => write!(f, "bad event size at byte {offset}"),
            Self::Model(m) => write!(f, "could not build the project: {m}"),
        }
    }
}

impl std::error::Error for FlpError {}

/// The `FLhd` chunk fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// File format; 0 = full project.
    pub format: i16,
    /// Declared channel count (the number of event-64 records).
    pub channels: u16,
    /// Ticks per quarter note.
    pub ppq: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    /// Ids 0..=191: a byte, word or dword, zero-extended.
    Fixed(u32),
    /// Ids 192..=255: the payload bytes.
    Bytes(&'a [u8]),
}

/// One event: id, where it starts, payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event<'a> {
    pub id: u8,
    pub offset: usize,
    pub payload: Payload<'a>,
}

impl<'a> Event<'a> {
    pub fn fixed(&self) -> Option<u32> {
        match self.payload {
            Payload::Fixed(v) => Some(v),
            Payload::Bytes(_) => None,
        }
    }

    pub fn bytes(&self) -> Option<&'a [u8]> {
        match self.payload {
            Payload::Bytes(b) => Some(b),
            Payload::Fixed(_) => None,
        }
    }
}

/// The container, parsed: header and events in file order.
#[derive(Debug, Clone)]
pub struct RawFlp<'a> {
    pub header: Header,
    pub events: Vec<Event<'a>>,
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    u32_at(b, at).map(|v| v as i32)
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

pub(crate) fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    u32_at(b, at).map(f32::from_bits)
}

pub(crate) fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    u64_at(b, at).map(f64::from_bits)
}

/// Reads a size: 7 bits per byte, least significant group first, high bit set on every byte
/// but the last (spec 1.2). Returns the value and the bytes used.
fn varint(b: &[u8], at: usize) -> Result<(usize, usize), FlpError> {
    let mut value: u64 = 0;
    for i in 0..5 {
        let byte = *b
            .get(at + i)
            .ok_or(FlpError::Truncated { offset: at + i })?;
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value as usize, i + 1));
        }
    }
    Err(FlpError::BadVarint { offset: at })
}

/// Parses the container. The data chunk is located at `8 + header length` (spec 1.1).
pub fn parse(bytes: &[u8]) -> Result<RawFlp<'_>, FlpError> {
    if bytes.len() < 8 {
        return Err(FlpError::TooShort);
    }
    if &bytes[0..4] != b"FLhd" {
        return Err(FlpError::BadMagic {
            expected: "FLhd",
            offset: 0,
        });
    }
    let hlen = u32_at(bytes, 4).ok_or(FlpError::TooShort)?;
    if hlen < 6 {
        return Err(FlpError::BadHeader(hlen));
    }
    let header = Header {
        format: u16_at(bytes, 8).ok_or(FlpError::TooShort)? as i16,
        channels: u16_at(bytes, 10).ok_or(FlpError::TooShort)?,
        ppq: u16_at(bytes, 12).ok_or(FlpError::TooShort)?,
    };
    if header.ppq == 0 {
        return Err(FlpError::BadPpq);
    }
    if header.format != 0 {
        return Err(FlpError::UnsupportedFormat(header.format));
    }
    let data_at = 8usize
        .checked_add(hlen as usize)
        .ok_or(FlpError::BadHeader(hlen))?;
    if bytes.get(data_at..data_at.saturating_add(4)) != Some(b"FLdt") {
        return Err(FlpError::BadMagic {
            expected: "FLdt",
            offset: data_at,
        });
    }
    let dlen = u32_at(bytes, data_at + 4).ok_or(FlpError::TooShort)? as usize;
    let start = data_at + 8;
    let end = start
        .checked_add(dlen)
        .filter(|e| *e <= bytes.len())
        .ok_or(FlpError::Truncated {
            offset: bytes.len(),
        })?;
    let data = &bytes[..end];

    let mut events = Vec::new();
    let mut at = start;
    while at < end {
        let id = data[at];
        let offset = at;
        at += 1;
        let payload = match id {
            0..=63 => fixed(data, &mut at, 1, offset)?,
            64..=127 => fixed(data, &mut at, 2, offset)?,
            128..=191 => fixed(data, &mut at, 4, offset)?,
            192..=255 => {
                let (len, used) = varint(data, at)?;
                at += used;
                let b = at
                    .checked_add(len)
                    .and_then(|e| data.get(at..e))
                    .ok_or(FlpError::Truncated { offset })?;
                at += len;
                Payload::Bytes(b)
            }
        };
        events.push(Event {
            id,
            offset,
            payload,
        });
    }
    Ok(RawFlp { header, events })
}

fn fixed<'a>(
    data: &'a [u8],
    at: &mut usize,
    n: usize,
    offset: usize,
) -> Result<Payload<'a>, FlpError> {
    let s = data
        .get(*at..*at + n)
        .ok_or(FlpError::Truncated { offset })?;
    *at += n;
    let mut v = 0u32;
    for (i, b) in s.iter().enumerate() {
        v |= u32::from(*b) << (8 * i);
    }
    Ok(Payload::Fixed(v))
}
