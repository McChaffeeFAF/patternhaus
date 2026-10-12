//! `flp-probe`: an analysis instrument for FL Studio `.flp` project files.
//!
//! Written from the clean-room specification in `docs/formats/flp.md` (and the byte-level
//! observations behind it). It understands only the container: the two chunks and the
//! event framing. It never interprets the meaning of an event beyond guessing whether a
//! variable-length payload looks like text, which is what the statistics need.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Magic of the header chunk.
pub const HEADER_MAGIC: [u8; 4] = *b"FLhd";
/// Magic of the data chunk.
pub const DATA_MAGIC: [u8; 4] = *b"FLdt";
/// The FL version string event (ASCII, NUL-terminated).
pub const EV_VERSION: u8 = 199;

/// Why a byte sequence could not be read as an FLP file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The file ended before `what` could be read.
    Truncated { what: &'static str, offset: usize },
    /// A chunk did not start with the expected four bytes.
    BadMagic { expected: [u8; 4], offset: usize },
    /// The header chunk is shorter than the six bytes of fields it must hold.
    ShortHeader(u32),
    /// A variable-length size prefix ran past 5 bytes (more than 32 bits).
    BadVarint { offset: usize },
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what, offset } => write!(f, "truncated {what} at byte {offset}"),
            Self::BadMagic { expected, offset } => write!(
                f,
                "expected {:?} at byte {offset}",
                String::from_utf8_lossy(expected)
            ),
            Self::ShortHeader(n) => write!(f, "header chunk is {n} bytes, need at least 6"),
            Self::BadVarint { offset } => write!(f, "size prefix too long at byte {offset}"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// The fields of the `FLhd` chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Declared length of the header chunk body (6 in every file seen).
    pub length: u32,
    /// File format (0 = full project).
    pub format: i16,
    /// Number of channel-rack channels.
    pub channels: u16,
    /// Pulses (ticks) per quarter note.
    pub ppq: u16,
}

/// Payload size class, fixed by the event id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SizeClass {
    /// Ids 0..=63: one byte.
    Byte,
    /// Ids 64..=127: two bytes, little-endian.
    Word,
    /// Ids 128..=191: four bytes, little-endian.
    Dword,
    /// Ids 192..=255: a variable-length size prefix, then that many bytes.
    Var,
}

impl SizeClass {
    pub fn of(id: u8) -> Self {
        match id {
            0..=63 => Self::Byte,
            64..=127 => Self::Word,
            128..=191 => Self::Dword,
            _ => Self::Var,
        }
    }
}

/// One event: its id, where it starts in the file, and its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event<'a> {
    pub id: u8,
    /// Offset of the id byte from the start of the file.
    pub offset: usize,
    pub payload: Payload<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    /// A fixed-size value (byte, word or dword), zero-extended.
    Fixed(u32),
    /// A variable-length payload.
    Bytes(&'a [u8]),
}

/// A parsed file: the header and the events in file order.
#[derive(Debug, Clone)]
pub struct Flp<'a> {
    pub header: Header,
    pub events: Vec<Event<'a>>,
    /// Bytes after the end of the data chunk (0 in every file seen).
    pub trailing: usize,
}

fn take<'a>(b: &'a [u8], at: usize, n: usize, what: &'static str) -> Result<&'a [u8], ProbeError> {
    b.get(
        at..at
            .checked_add(n)
            .ok_or(ProbeError::Truncated { what, offset: at })?,
    )
    .ok_or(ProbeError::Truncated { what, offset: at })
}

fn u16_at(b: &[u8], at: usize, what: &'static str) -> Result<u16, ProbeError> {
    let s = take(b, at, 2, what)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize, what: &'static str) -> Result<u32, ProbeError> {
    let s = take(b, at, 4, what)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Reads a variable-length size: 7 bits per byte, least significant group first, the
/// high bit set on every byte but the last. Returns the value and the bytes consumed.
pub fn read_varint(b: &[u8], at: usize) -> Result<(u32, usize), ProbeError> {
    let mut value: u64 = 0;
    for i in 0..5 {
        let byte = *b.get(at + i).ok_or(ProbeError::Truncated {
            what: "size prefix",
            offset: at + i,
        })?;
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return u32::try_from(value)
                .map(|v| (v, i + 1))
                .map_err(|_| ProbeError::BadVarint { offset: at });
        }
    }
    Err(ProbeError::BadVarint { offset: at })
}

/// Encodes a size the way [`read_varint`] reads it (used to build test files).
pub fn write_varint(mut n: u32, out: &mut Vec<u8>) {
    loop {
        let low = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

/// Parses the header chunk and the event stream of the data chunk.
pub fn parse(b: &[u8]) -> Result<Flp<'_>, ProbeError> {
    if take(b, 0, 4, "header magic")? != HEADER_MAGIC {
        return Err(ProbeError::BadMagic {
            expected: HEADER_MAGIC,
            offset: 0,
        });
    }
    let length = u32_at(b, 4, "header length")?;
    if length < 6 {
        return Err(ProbeError::ShortHeader(length));
    }
    let header = Header {
        length,
        format: u16_at(b, 8, "header format")? as i16,
        channels: u16_at(b, 10, "header channels")?,
        ppq: u16_at(b, 12, "header ppq")?,
    };
    let data = 8 + length as usize;
    if take(b, data, 4, "data magic")? != DATA_MAGIC {
        return Err(ProbeError::BadMagic {
            expected: DATA_MAGIC,
            offset: data,
        });
    }
    let data_len = u32_at(b, data + 4, "data length")? as usize;
    let start = data + 8;
    let end = start + data_len;
    if end > b.len() {
        return Err(ProbeError::Truncated {
            what: "data chunk",
            offset: b.len(),
        });
    }
    let mut events = Vec::new();
    let mut at = start;
    while at < end {
        let offset = at;
        let id = b[at];
        at += 1;
        let payload = match SizeClass::of(id) {
            SizeClass::Byte => {
                let v = take(&b[..end], at, 1, "byte event")?[0];
                at += 1;
                Payload::Fixed(u32::from(v))
            }
            SizeClass::Word => {
                let v = u16_at(&b[..end], at, "word event")?;
                at += 2;
                Payload::Fixed(u32::from(v))
            }
            SizeClass::Dword => {
                let v = u32_at(&b[..end], at, "dword event")?;
                at += 4;
                Payload::Fixed(v)
            }
            SizeClass::Var => {
                let (n, used) = read_varint(&b[..end], at)?;
                at += used;
                let p = take(&b[..end], at, n as usize, "event payload")?;
                at += n as usize;
                Payload::Bytes(p)
            }
        };
        events.push(Event {
            id,
            offset,
            payload,
        });
    }
    Ok(Flp {
        header,
        events,
        trailing: b.len() - end,
    })
}

/// How a variable-length payload looks, judged from its bytes alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextGuess {
    /// UTF-16LE, ending in a two-byte NUL (an empty string is just the NUL).
    Utf16(String),
    /// Printable ASCII ending in a single NUL.
    Ascii(String),
    /// Anything else.
    Binary,
}

/// Guesses whether a payload is text. UTF-16 is accepted only when it decodes cleanly,
/// ends in `00 00`, and contains no control characters.
pub fn guess_text(p: &[u8]) -> TextGuess {
    if p.len() >= 2 && p.len().is_multiple_of(2) && p[p.len() - 2..] == [0, 0] {
        let units: Vec<u16> = p[..p.len() - 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        if let Ok(s) = String::from_utf16(&units)
            && !s.chars().any(char::is_control)
            && (units.is_empty() || units.iter().any(|u| *u >= 0x20 && *u < 0x7f))
        {
            return TextGuess::Utf16(s);
        }
    }
    if let Some((0, body)) = p.split_last()
        && !body.is_empty()
        && body.iter().all(|c| (0x20..0x7f).contains(c))
    {
        return TextGuess::Ascii(String::from_utf8_lossy(body).into_owned());
    }
    TextGuess::Binary
}

/// The FL version string of a parsed file (event 199), if present.
pub fn version(flp: &Flp<'_>) -> Option<String> {
    flp.events.iter().find_map(|e| match (e.id, e.payload) {
        (EV_VERSION, Payload::Bytes(p)) => match guess_text(p) {
            TextGuess::Ascii(s) | TextGuess::Utf16(s) => Some(s),
            TextGuess::Binary => None,
        },
        _ => None,
    })
}

/// Aggregate facts about one event id across many files. Holds numbers only, never content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdStats {
    /// Number of events with this id.
    pub count: u64,
    /// Number of files containing it at least once.
    pub files: u64,
    /// Fixed-size ids: smallest and largest value seen.
    pub min: Option<u32>,
    pub max: Option<u32>,
    /// Fixed-size ids: number of distinct values (capped at [`DISTINCT_CAP`]).
    pub distinct: usize,
    /// Variable-length ids: payload length range and how many looked like text.
    pub min_len: Option<usize>,
    pub max_len: Option<usize>,
    pub utf16: u64,
    pub ascii: u64,
    values: BTreeSet<u32>,
}

/// Distinct values are tracked up to this many per id.
pub const DISTINCT_CAP: usize = 4096;

/// Aggregate statistics over a set of files.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub files: u64,
    pub failed: u64,
    pub events: u64,
    /// Files per major FL version ("21", "20", ...; "?" when the version event is missing).
    pub versions: BTreeMap<String, u64>,
    /// Files per header PPQ.
    pub ppq: BTreeMap<u16, u64>,
    /// Files per header format value.
    pub formats: BTreeMap<i16, u64>,
    pub ids: BTreeMap<u8, IdStats>,
}

impl Stats {
    /// Adds one parsed file.
    pub fn add(&mut self, flp: &Flp<'_>) {
        self.files += 1;
        self.events += flp.events.len() as u64;
        let major = version(flp)
            .and_then(|v| v.split('.').next().map(str::to_owned))
            .unwrap_or_else(|| "?".into());
        *self.versions.entry(major).or_default() += 1;
        *self.ppq.entry(flp.header.ppq).or_default() += 1;
        *self.formats.entry(flp.header.format).or_default() += 1;
        let mut seen = [false; 256];
        for e in &flp.events {
            let s = self.ids.entry(e.id).or_default();
            s.count += 1;
            if !seen[e.id as usize] {
                seen[e.id as usize] = true;
                s.files += 1;
            }
            match e.payload {
                Payload::Fixed(v) => {
                    s.min = Some(s.min.map_or(v, |m| m.min(v)));
                    s.max = Some(s.max.map_or(v, |m| m.max(v)));
                    if s.values.len() < DISTINCT_CAP {
                        s.values.insert(v);
                    }
                    s.distinct = s.values.len();
                }
                Payload::Bytes(p) => {
                    s.min_len = Some(s.min_len.map_or(p.len(), |m| m.min(p.len())));
                    s.max_len = Some(s.max_len.map_or(p.len(), |m| m.max(p.len())));
                    match guess_text(p) {
                        TextGuess::Utf16(_) => s.utf16 += 1,
                        TextGuess::Ascii(_) => s.ascii += 1,
                        TextGuess::Binary => {}
                    }
                }
            }
        }
    }

    /// Records a file that could not be parsed.
    pub fn add_failure(&mut self) {
        self.failed += 1;
    }

    /// A plain-text report of the aggregate numbers (no content from any file).
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut o = String::new();
        let _ = writeln!(
            o,
            "files {} (failed {}), events {}",
            self.files, self.failed, self.events
        );
        let _ = writeln!(o, "versions {:?}", self.versions);
        let _ = writeln!(o, "ppq {:?}", self.ppq);
        let _ = writeln!(o, "formats {:?}", self.formats);
        let _ = writeln!(o, "id  class  files  count  range / lengths  text");
        for (id, s) in &self.ids {
            let class = match SizeClass::of(*id) {
                SizeClass::Byte => "byte",
                SizeClass::Word => "word",
                SizeClass::Dword => "dword",
                SizeClass::Var => "var",
            };
            let detail = match (s.min, s.min_len) {
                (Some(min), _) => {
                    format!("{min}..={} ({} distinct)", s.max.unwrap_or(min), s.distinct)
                }
                (None, Some(l)) => format!("len {l}..={}", s.max_len.unwrap_or(l)),
                (None, None) => String::new(),
            };
            let text = if s.utf16 + s.ascii > 0 {
                format!("utf16 {} ascii {}", s.utf16, s.ascii)
            } else {
                String::new()
            };
            let _ = writeln!(
                o,
                "{id:<3} {class:<6} {:<6} {:<6} {detail}  {text}",
                s.files, s.count
            );
        }
        o
    }
}

/// One line per event, for `flp-probe dump`. Text payloads are shown decoded; binary
/// payloads as a length and at most `preview` leading bytes in hex.
pub fn dump_line(e: &Event<'_>, preview: usize) -> String {
    match e.payload {
        Payload::Fixed(v) => format!("{:>8} {:>3} = {v} (0x{v:x})", e.offset, e.id),
        Payload::Bytes(p) => match guess_text(p) {
            TextGuess::Utf16(s) => format!("{:>8} {:>3} utf16 {s:?}", e.offset, e.id),
            TextGuess::Ascii(s) => format!("{:>8} {:>3} ascii {s:?}", e.offset, e.id),
            TextGuess::Binary => {
                let hex: Vec<String> = p.iter().take(preview).map(|b| format!("{b:02x}")).collect();
                let more = if p.len() > preview { " .." } else { "" };
                format!(
                    "{:>8} {:>3} [{}] {}{more}",
                    e.offset,
                    e.id,
                    p.len(),
                    hex.join(" ")
                )
            }
        },
    }
}
