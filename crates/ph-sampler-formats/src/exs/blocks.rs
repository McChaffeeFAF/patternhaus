//! The EXS block chain (`docs/formats/exs.md` §1–§6): byte order, block headers, and the
//! instrument, zone, group, sample and parameter records as stored.

use crate::notes::Notes;

/// A structural fault: the file is refused with this reason.
pub type Fault = String;

pub const HEADER: usize = 84;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Little,
    Big,
}

/// Reads integers in the file's byte order; out-of-range reads give 0.
#[derive(Clone, Copy)]
pub struct Rd<'a> {
    pub b: &'a [u8],
    pub order: Order,
}

impl Rd<'_> {
    pub fn u8(&self, at: usize) -> u8 {
        self.b.get(at).copied().unwrap_or(0)
    }
    pub fn i8(&self, at: usize) -> i8 {
        self.u8(at) as i8
    }
    pub fn u16(&self, at: usize) -> u16 {
        match self.b.get(at..at + 2) {
            Some(s) => match self.order {
                Order::Little => u16::from_le_bytes([s[0], s[1]]),
                Order::Big => u16::from_be_bytes([s[0], s[1]]),
            },
            None => 0,
        }
    }
    pub fn i16(&self, at: usize) -> i16 {
        self.u16(at) as i16
    }
    pub fn u32(&self, at: usize) -> u32 {
        match self.b.get(at..at + 4) {
            Some(s) => {
                let a = [s[0], s[1], s[2], s[3]];
                match self.order {
                    Order::Little => u32::from_le_bytes(a),
                    Order::Big => u32::from_be_bytes(a),
                }
            }
            None => 0,
        }
    }
    pub fn i32(&self, at: usize) -> i32 {
        self.u32(at) as i32
    }
    /// A NUL-terminated string in `[at, at + len)`, UTF-8 with Latin-1 fallback.
    pub fn str(&self, at: usize, len: usize) -> String {
        let s = self.b.get(at..(at + len).min(self.b.len())).unwrap_or(&[]);
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        let s = &s[..end];
        match std::str::from_utf8(s) {
            Ok(t) => t.trim_end().to_string(),
            Err(_) => s
                .iter()
                .map(|&c| c as char)
                .collect::<String>()
                .trim_end()
                .to_string(),
        }
    }
}

/// One block: its type and flags, object id, name and content.
#[derive(Clone, Debug)]
pub struct Block<'a> {
    pub kind: u8,
    pub flags: u8,
    pub id: u32,
    pub name: String,
    pub content: &'a [u8],
}

fn valid_type(t: u8) -> bool {
    matches!(t, 0..=5 | 7 | 8 | 0x0A | 0x0B)
}

/// Splits the file into blocks (§1). The first block's magic sets the byte order.
pub fn blocks<'a>(bytes: &'a [u8], notes: &mut Notes) -> Result<(Order, Vec<Block<'a>>), Fault> {
    if bytes.len() < HEADER {
        return Err("too short for an EXS block header".into());
    }
    let order = match &bytes[16..20] {
        b"TBOS" | b"JBOS" => Order::Little,
        b"SOBT" | b"SOBJ" => Order::Big,
        _ => return Err("no EXS magic (TBOS/SOBT) in the first block".into()),
    };
    if matches!(&bytes[16..20], b"JBOS" | b"SOBJ") {
        notes.kept(
            "exs:magic",
            "the J-variant magic (meaning unknown) was read as standard",
        );
    }
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        if bytes.len() - pos < HEADER {
            notes.approx(
                "exs:trailing",
                "bytes after the last block that are too few for a block were ignored",
            );
            break;
        }
        let h = &bytes[pos..pos + HEADER];
        let magic_order = match &h[16..20] {
            b"TBOS" | b"JBOS" => Order::Little,
            b"SOBT" | b"SOBJ" => Order::Big,
            _ => return Err(format!("block at byte {pos} has no EXS magic")),
        };
        if magic_order != order {
            return Err(format!("block at byte {pos} switches byte order"));
        }
        let rd = Rd { b: h, order };
        // type and flags (§1.1 [PH]): little-endian files keep them in byte 3; big-endian
        // files are seen with the signature in byte 3 (`xx 01 00 TT`) or reversed
        // (`TT 00 01 01`)
        let (sig, reversed) = if order == Order::Little || (h[1] == 1 && h[2] == 0) {
            (h[3], false)
        } else if h[1] == 0 && h[2] == 1 {
            (h[0], true)
        } else if valid_type(h[3] & 0x0F) {
            (h[3], false)
        } else {
            (h[0], true)
        };
        if reversed {
            notes.approx(
                "exs:signature",
                "a big-endian signature stored in reverse byte order was read (layout unconfirmed)",
            );
        }
        let kind = sig & 0x0F;
        let flags = sig & 0xF0;
        let left = bytes.len() - pos - HEADER;
        let mut size = rd.u32(4) as usize;
        if flags & 0x80 != 0 || size > left {
            let cleared = size & !0x8000;
            if cleared != size {
                notes.approx(
                    "exs:size-flag",
                    "block sizes with the big-endian size flag (bit 15) were read with the flag cleared",
                );
            }
            size = cleared;
        }
        if size > left {
            return Err(format!(
                "block at byte {pos} claims {size} bytes, only {left} remain"
            ));
        }
        let content = &bytes[pos + HEADER..pos + HEADER + size];
        out.push(Block {
            kind,
            flags,
            id: rd.u32(8),
            name: rd.str(20, 64),
            content,
        });
        pos += HEADER + size;
    }
    if out.first().map(|b| b.kind) != Some(0) {
        return Err("the first block is not the instrument header".into());
    }
    Ok((order, out))
}

#[derive(Clone, Debug, Default)]
pub struct ZoneRec {
    pub id: u32,
    pub options: u8,
    pub root: u8,
    pub fine: i8,
    pub pan: i8,
    pub volume: i8,
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub loop_xfade_ms: u32,
    pub loop_tune: u8,
    pub loop_options: u8,
    pub loop_dir: u8,
    pub coarse: i8,
    pub output: u8,
    pub group: i32,
    pub sample: i32,
    pub fade_out: u32,
    pub sample_offset: u32,
}

pub fn zone(b: &Block<'_>, order: Order) -> Option<ZoneRec> {
    if b.content.len() < 96 {
        return None;
    }
    let r = Rd {
        b: b.content,
        order,
    };
    Some(ZoneRec {
        id: b.id,
        options: r.u8(0),
        root: r.u8(1),
        fine: r.i8(2),
        pan: r.i8(3),
        volume: r.i8(4),
        key_lo: r.u8(6),
        key_hi: r.u8(7),
        vel_lo: r.u8(9),
        vel_hi: r.u8(10),
        start: r.u32(12),
        end: r.u32(16),
        loop_start: r.u32(20),
        loop_end: r.u32(24),
        loop_xfade_ms: r.u32(28),
        loop_tune: r.u8(32),
        loop_options: r.u8(33),
        loop_dir: r.u8(34),
        coarse: r.i8(80),
        output: r.u8(82),
        group: r.i32(88),
        sample: r.i32(92),
        fade_out: if b.content.len() > 104 { r.u32(104) } else { 0 },
        sample_offset: if b.content.len() > 108 { r.u32(108) } else { 0 },
    })
}

#[derive(Clone, Debug, Default)]
pub struct GroupRec {
    pub name: String,
    pub volume: i8,
    pub pan: i8,
    pub polyphony: u8,
    pub options: u8,
    pub exclusive: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
    pub random_offset: u8,
    pub release_time: u16,
    pub vel_xfade: i32,
    pub vel_xfade_type: u8,
    pub key_xfade_type: u8,
    pub key_xfade: i32,
    pub tempo_lo: u8,
    pub tempo_hi: u8,
    pub cutoff: u8,
    pub resonance: u8,
    pub env: [u32; 4],
    pub release_trigger: u8,
    pub output: u8,
    pub enable_note: u8,
    /// The extended part (offsets 76–91) was present.
    pub extended: bool,
    pub rr_position: i32,
    pub enable_by: u8,
    pub enable_cc: u8,
    pub enable_cc_lo: u8,
    pub enable_cc_hi: u8,
    pub key_lo: u8,
    pub key_hi: u8,
    pub enable_channel: u8,
    pub enable_articulation: u8,
}

pub fn group(b: &Block<'_>, order: Order) -> Option<GroupRec> {
    if b.content.len() < 76 {
        return None;
    }
    let r = Rd {
        b: b.content,
        order,
    };
    let extended = b.content.len() >= 92;
    Some(GroupRec {
        name: b.name.clone(),
        volume: r.i8(0),
        pan: r.i8(1),
        polyphony: r.u8(2),
        options: r.u8(3),
        exclusive: r.u8(4),
        vel_lo: r.u8(5),
        vel_hi: r.u8(6),
        random_offset: r.u8(7),
        release_time: r.u16(16),
        vel_xfade: r.u8(32) as i32 - 128,
        vel_xfade_type: r.u8(33),
        key_xfade_type: r.u8(34),
        key_xfade: r.u8(35) as i32 - 128,
        tempo_lo: r.u8(38),
        tempo_hi: r.u8(39),
        cutoff: r.u8(41),
        resonance: r.u8(43),
        env: [r.u32(56), r.u32(60), r.u32(64), r.u32(68)],
        release_trigger: r.u8(73),
        output: r.u8(74),
        enable_note: r.u8(75),
        extended,
        rr_position: if extended { r.i32(80) } else { -1 },
        enable_by: if extended { r.u8(84) } else { 0 },
        enable_cc: r.u8(85),
        enable_cc_lo: r.u8(86),
        enable_cc_hi: if extended { r.u8(87) } else { 127 },
        key_lo: r.u8(88),
        key_hi: if extended { r.u8(89) } else { 127 },
        enable_channel: r.u8(90),
        enable_articulation: r.u8(91),
    })
}

#[derive(Clone, Debug, Default)]
pub struct SampleRec {
    pub name: String,
    pub frames: u32,
    pub file_type: [u8; 4],
    pub compressed: bool,
    pub path: String,
    pub file_name: String,
}

pub fn sample(b: &Block<'_>, order: Order) -> Option<SampleRec> {
    if b.content.len() < 336 {
        return None;
    }
    let r = Rd {
        b: b.content,
        order,
    };
    let file_name = if b.content.len() >= 592 {
        r.str(336, 256)
    } else {
        String::new()
    };
    Some(SampleRec {
        name: b.name.clone(),
        frames: r.u32(4),
        file_type: [b.content[28], b.content[29], b.content[30], b.content[31]],
        compressed: r.u32(36) != 0,
        path: r.str(80, 256),
        file_name: if file_name.is_empty() {
            b.name.clone()
        } else {
            file_name
        },
    })
}

/// The parameters block (§6): (id, value) pairs, section 1 then section 2.
pub fn params(b: &Block<'_>, order: Order, notes: &mut Notes) -> Vec<(u16, i32)> {
    let r = Rd {
        b: b.content,
        order,
    };
    let mut out = Vec::new();
    let n = r.u32(0) as usize;
    let s1 = 4 + n * 3;
    if n > 0 && s1 <= b.content.len() {
        for i in 0..n {
            let id = r.u8(4 + i);
            if id != 0 {
                out.push((id as u16, r.i16(4 + n + 2 * i) as i32));
            }
        }
    } else if n > 0 {
        notes.skip(
            "exs:params",
            "the parameter block's first section does not fit; ignored",
        );
        return out;
    }
    let s1 = if n > 0 { s1 } else { 4 };
    if b.content.len() >= s1 + 4 {
        let m = r.u32(s1) as usize;
        if m.checked_mul(4)
            .and_then(|x| x.checked_add(s1 + 4))
            .is_some_and(|e| e <= b.content.len())
        {
            for i in 0..m {
                let at = s1 + 4 + 4 * i;
                let id = r.u16(at);
                if id != 0 {
                    out.push((id, r.u16(at + 2) as i32));
                }
            }
        } else {
            notes.skip(
                "exs:params",
                "the parameter block's second section does not fit; ignored",
            );
        }
    }
    out
}
