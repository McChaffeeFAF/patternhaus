//! The events, interpreted: which channel, pattern, arrangement or mixer insert each one
//! belongs to (spec section 2, the context rules), as plain typed data. No Patternhaus types
//! here; [`crate::flp::import`] maps this to a project.

use super::parse::*;
use std::collections::BTreeMap;

/// Channel types (spec 4.1) as the file stores them.
pub const CH_SAMPLER: u8 = 0;
pub const CH_GENERATOR: u8 = 2;
pub const CH_LAYER: u8 = 3;
pub const CH_AUDIO: u8 = 4;
pub const CH_AUTOMATION: u8 = 5;

/// A channel-rack block (event 64 and what follows it).
#[derive(Debug, Clone, Default)]
pub struct FlpChannel {
    pub iid: u16,
    pub kind: Option<u8>,
    /// Event 201: `Fruity Wrapper`, a native plugin's name, or empty.
    pub plugin_name: String,
    pub name: Option<String>,
    pub color: Option<u32>,
    /// Event 213, verbatim.
    pub state: Option<Vec<u8>>,
    /// Event 0 (absent = on).
    pub enabled: Option<bool>,
    pub insert: Option<u8>,
    /// Event 219: pan, volume, pitch (raw).
    pub levels: Option<[i32; 3]>,
    pub sample_path: Option<String>,
    /// Event 234, verbatim.
    pub points: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlpNote {
    pub position: u32,
    pub flags: u16,
    pub channel: u16,
    pub length: u32,
    pub key: u16,
    pub group: u16,
    pub fine_pitch: u8,
    pub release: u8,
    pub midi_channel: u8,
    pub pan: u8,
    pub velocity: u8,
    pub mod_x: u8,
    pub mod_y: u8,
}

#[derive(Debug, Clone, Default)]
pub struct FlpPattern {
    pub id: u16,
    pub name: Option<String>,
    pub color: Option<u32>,
    pub length: u32,
    pub notes: Vec<FlpNote>,
    /// Event 223 payload bytes (not interpreted).
    pub controller_bytes: usize,
}

/// A playlist item (event 233 record), offsets kept raw because their meaning depends on
/// the item kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlpItem {
    pub position: u32,
    pub index: u16,
    pub length: u32,
    pub track_raw: u32,
    pub flags: u16,
    pub start_raw: [u8; 4],
    pub end_raw: [u8; 4],
}

pub const PATTERN_BASE: u16 = 0x5000;

impl FlpItem {
    /// `Some(pattern id)` for a pattern clip.
    pub fn pattern(&self) -> Option<u16> {
        (self.index > PATTERN_BASE).then(|| self.index - PATTERN_BASE)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FlpTrack {
    pub iid: u32,
    pub color: u32,
    pub enabled: bool,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FlpArrangement {
    pub index: u16,
    pub name: Option<String>,
    pub items: Vec<FlpItem>,
    pub tracks: Vec<FlpTrack>,
    /// Bytes of a 233 payload that did not fill a whole record.
    pub stray_item_bytes: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FlpSlot {
    /// Event 98; `None` when the file has no slot indices (FL 12).
    pub index: Option<u16>,
    pub internal_name: String,
    pub user_name: Option<String>,
    pub state: Option<Vec<u8>>,
}

impl FlpSlot {
    fn is_empty(&self) -> bool {
        self.internal_name.is_empty() && self.user_name.is_none() && self.state.is_none()
    }
}

#[derive(Debug, Clone, Default)]
pub struct FlpInsert {
    pub color: Option<u32>,
    pub name: Option<String>,
    /// Second u32 of event 236.
    pub flags: u32,
    pub slots: Vec<FlpSlot>,
    /// Event 235: one byte per target insert.
    pub routing: Vec<u8>,
}

/// One record of event 225 (spec 6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixerParam {
    pub param: u8,
    pub insert: u8,
    pub slot: u8,
    pub value: i32,
}

/// The whole file as typed data.
#[derive(Debug, Clone)]
pub struct FlpFile {
    pub header: Header,
    pub version: Option<String>,
    pub fl_major: u32,
    /// Event 156 (BPM x 1000).
    pub tempo_milli: Option<u32>,
    /// Event 66, old coarse tempo (BPM).
    pub tempo_coarse: Option<u32>,
    pub ts_numerator: Option<u8>,
    pub ts_denominator: Option<u8>,
    pub title: String,
    pub comments: String,
    pub genre: String,
    pub author: String,
    pub url: String,
    pub filter_groups: u64,
    pub channels: Vec<FlpChannel>,
    pub patterns: BTreeMap<u16, FlpPattern>,
    pub arrangements: Vec<FlpArrangement>,
    pub inserts: Vec<FlpInsert>,
    pub mixer_params: Vec<MixerParam>,
    /// Event 227: automation channel iid -> target id.
    pub auto_targets: BTreeMap<u16, u32>,
    /// Events no rule above reads, by id (count of events).
    pub unread: BTreeMap<u8, u64>,
}

/// Decodes a text event (spec 1.3): UTF-16LE with a two-byte NUL for FL 12 and later,
/// 8-bit before that.
pub fn decode_text(b: &[u8], utf16: bool) -> String {
    if utf16 {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|u| *u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        let end = b.iter().position(|x| *x == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).into_owned()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    None,
    Channel,
    Pattern(u16),
    Arrangement,
    Insert,
}

struct Reader {
    f: FlpFile,
    ctx: Ctx,
    utf16: bool,
    pre_color: Option<u32>,
    pre_name: Option<String>,
    slot: FlpSlot,
}

/// Interprets a parsed container.
pub fn read(raw: &RawFlp<'_>) -> FlpFile {
    let mut r = Reader {
        f: FlpFile {
            header: raw.header,
            version: None,
            fl_major: 0,
            tempo_milli: None,
            tempo_coarse: None,
            ts_numerator: None,
            ts_denominator: None,
            title: String::new(),
            comments: String::new(),
            genre: String::new(),
            author: String::new(),
            url: String::new(),
            filter_groups: 0,
            channels: Vec::new(),
            patterns: BTreeMap::new(),
            arrangements: Vec::new(),
            inserts: Vec::new(),
            mixer_params: Vec::new(),
            auto_targets: BTreeMap::new(),
            unread: BTreeMap::new(),
        },
        ctx: Ctx::None,
        utf16: true,
        pre_color: None,
        pre_name: None,
        slot: FlpSlot::default(),
    };
    for ev in &raw.events {
        if !r.handle(ev) {
            *r.f.unread.entry(ev.id).or_insert(0) += 1;
        }
    }
    r.finish_slot();
    r.f
}

impl Reader {
    fn text(&self, ev: &Event<'_>) -> Option<String> {
        ev.bytes().map(|b| decode_text(b, self.utf16))
    }

    fn arrangement(&mut self) -> &mut FlpArrangement {
        if self.f.arrangements.is_empty() {
            // FL 12 has no arrangement events: one implicit arrangement (spec 2).
            self.f.arrangements.push(FlpArrangement::default());
        }
        self.f.arrangements.last_mut().expect("just ensured")
    }

    fn finish_slot(&mut self) {
        let slot = std::mem::take(&mut self.slot);
        if !slot.is_empty()
            && let Some(ins) = self.f.inserts.last_mut()
        {
            ins.slots.push(slot);
        }
    }

    fn handle(&mut self, ev: &Event<'_>) -> bool {
        let fixed = ev.fixed();
        let bytes = ev.bytes();
        match ev.id {
            199 => {
                if let Some(b) = bytes {
                    let v = decode_text(b, false);
                    self.f.fl_major = v
                        .split('.')
                        .next()
                        .and_then(|m| m.trim().parse().ok())
                        .unwrap_or(0);
                    // SPEC-GAP: 1.3: 8-bit text before FL 12 is inferred (no such file in the corpus).
                    // Spec 1.3: major >= 12 writes UTF-16; older files are 8-bit [I]. An
                    // unreadable version keeps the UTF-16 default (everything seen).
                    self.utf16 = self.f.fl_major == 0 || self.f.fl_major >= 12;
                    self.f.version = Some(v);
                }
                true
            }
            156 => {
                self.f.tempo_milli = fixed;
                true
            }
            66 => {
                self.f.tempo_coarse = fixed;
                true
            }
            17 => {
                self.f.ts_numerator = fixed.map(|v| v as u8);
                true
            }
            18 => {
                self.f.ts_denominator = fixed.map(|v| v as u8);
                true
            }
            194 => self.set_text(ev, |f| &mut f.title),
            195 => self.set_text(ev, |f| &mut f.comments),
            197 => self.set_text(ev, |f| &mut f.url),
            206 => self.set_text(ev, |f| &mut f.genre),
            207 => self.set_text(ev, |f| &mut f.author),
            231 => {
                self.f.filter_groups += 1;
                true
            }
            64 => {
                self.finish_slot();
                self.f.channels.push(FlpChannel {
                    iid: fixed.unwrap_or(0) as u16,
                    ..Default::default()
                });
                self.ctx = Ctx::Channel;
                true
            }
            65 => {
                let id = fixed.unwrap_or(0) as u16;
                self.finish_slot();
                self.f.patterns.entry(id).or_insert_with(|| FlpPattern {
                    id,
                    ..Default::default()
                });
                self.ctx = Ctx::Pattern(id);
                true
            }
            99 => {
                self.finish_slot();
                self.f.arrangements.push(FlpArrangement {
                    index: fixed.unwrap_or(0) as u16,
                    ..Default::default()
                });
                self.ctx = Ctx::Arrangement;
                true
            }
            241 => {
                let t = self.text(ev);
                self.arrangement().name = t;
                true
            }
            233 => {
                if let Some(b) = bytes {
                    let major = self.f.fl_major;
                    let (items, stray) = decode_items(b, major);
                    let a = self.arrangement();
                    a.items.extend(items);
                    a.stray_item_bytes += stray;
                }
                true
            }
            238 => {
                if let Some(b) = bytes {
                    let t = FlpTrack {
                        iid: u32_at(b, 0).unwrap_or(0),
                        color: u32_at(b, 4).unwrap_or(0x0056_5148),
                        enabled: b.get(12).is_none_or(|v| *v != 0),
                        name: None,
                    };
                    self.arrangement().tracks.push(t);
                }
                true
            }
            239 => {
                let name = self.text(ev);
                match self
                    .f
                    .arrangements
                    .last_mut()
                    .and_then(|a| a.tracks.last_mut())
                {
                    Some(t) => {
                        t.name = name;
                        true
                    }
                    None => false,
                }
            }
            236 => {
                self.finish_slot();
                let flags = bytes.and_then(|b| u32_at(b, 4)).unwrap_or(0);
                self.f.inserts.push(FlpInsert {
                    color: self.pre_color.take(),
                    name: self.pre_name.take(),
                    flags,
                    ..Default::default()
                });
                self.ctx = Ctx::Insert;
                true
            }
            // The insert's colour and name come before its 236 (spec 6.1).
            149 => {
                self.pre_color = fixed;
                true
            }
            204 => {
                self.pre_name = self.text(ev).filter(|t| !t.is_empty());
                true
            }
            227 => {
                if let Some(b) = bytes
                    && let (Some(ch), Some(target)) = (u16_at(b, 2), u32_at(b, 8))
                {
                    self.f.auto_targets.insert(ch, target);
                }
                true
            }
            225 => {
                if let Some(b) = bytes {
                    for rec in b.as_chunks::<12>().0 {
                        if let Some(p) = mixer_param(rec) {
                            self.f.mixer_params.push(p);
                        }
                    }
                }
                true
            }
            _ => match self.ctx {
                Ctx::Channel => self.channel_event(ev),
                Ctx::Pattern(id) => self.pattern_event(ev, id),
                Ctx::Insert => self.insert_event(ev),
                Ctx::None | Ctx::Arrangement => false,
            },
        }
    }

    fn set_text(&mut self, ev: &Event<'_>, field: fn(&mut FlpFile) -> &mut String) -> bool {
        if let Some(t) = self.text(ev) {
            *field(&mut self.f) = t;
        }
        true
    }

    fn channel_event(&mut self, ev: &Event<'_>) -> bool {
        let text = self.text(ev);
        let Some(ch) = self.f.channels.last_mut() else {
            return false;
        };
        let fixed = ev.fixed();
        let bytes = ev.bytes();
        match ev.id {
            21 => ch.kind = fixed.map(|v| v as u8),
            201 => ch.plugin_name = text.unwrap_or_default(),
            203 => ch.name = text.filter(|t| !t.is_empty()),
            128 => ch.color = fixed,
            213 => ch.state = bytes.map(<[u8]>::to_vec),
            0 => ch.enabled = fixed.map(|v| v != 0),
            22 => ch.insert = fixed.map(|v| v as u8),
            219 => {
                ch.levels = bytes.and_then(|b| Some([i32_at(b, 0)?, i32_at(b, 4)?, i32_at(b, 8)?]))
            }
            196 => ch.sample_path = text.filter(|t| !t.is_empty()),
            234 => ch.points = bytes.map(<[u8]>::to_vec),
            _ => return false,
        }
        true
    }

    fn pattern_event(&mut self, ev: &Event<'_>, id: u16) -> bool {
        let text = self.text(ev);
        let Some(p) = self.f.patterns.get_mut(&id) else {
            return false;
        };
        match ev.id {
            193 => p.name = text.filter(|t| !t.is_empty()),
            150 => p.color = ev.fixed(),
            164 => p.length = ev.fixed().unwrap_or(0),
            224 => {
                if let Some(b) = ev.bytes() {
                    p.notes
                        .extend(b.as_chunks::<24>().0.iter().map(|r| note(r)));
                }
            }
            223 => p.controller_bytes += ev.bytes().map_or(0, <[u8]>::len),
            _ => return false,
        }
        true
    }

    fn insert_event(&mut self, ev: &Event<'_>) -> bool {
        match ev.id {
            201 => {
                // FL 12 writes slots back to back without indices: a new plugin name
                // while one is pending starts the next slot.
                if !self.slot.internal_name.is_empty() {
                    self.finish_slot();
                }
                self.slot.internal_name = self.text(ev).unwrap_or_default();
            }
            203 => self.slot.user_name = self.text(ev).filter(|t| !t.is_empty()),
            213 => self.slot.state = ev.bytes().map(<[u8]>::to_vec),
            98 => {
                self.slot.index = ev.fixed().map(|v| v as u16);
                self.finish_slot();
            }
            235 => {
                if let Some(ins) = self.f.inserts.last_mut() {
                    ins.routing = ev.bytes().map(<[u8]>::to_vec).unwrap_or_default();
                }
            }
            _ => return false,
        }
        true
    }
}

fn note(r: &[u8]) -> FlpNote {
    // `r` is a whole 24-byte record (chunks_exact); the accessors cannot fail.
    let u16v = |at| u16_at(r, at).unwrap_or(0);
    FlpNote {
        position: u32_at(r, 0).unwrap_or(0),
        flags: u16v(4),
        channel: u16v(6),
        length: u32_at(r, 8).unwrap_or(0),
        key: u16v(12),
        group: u16v(14),
        fine_pitch: r[16],
        release: r[18],
        midi_channel: r[19],
        pan: r[20],
        velocity: r[21],
        mod_x: r[22],
        mod_y: r[23],
    }
}

/// Splits a 32-bit id of the mixer form `0x2xxx1Fpp` into (param, insert, slot)
/// (spec 6.5, 9.2).
pub fn mixer_target(target: u32) -> Option<(u8, u8, u8)> {
    let loc = (target >> 16) as u16;
    if (target >> 8) & 0xff != 0x1f || loc & 0xe000 != 0x2000 {
        return None;
    }
    Some((
        (target & 0xff) as u8,
        ((loc >> 6) & 0x7f) as u8,
        (loc & 0x3f) as u8,
    ))
}

fn mixer_param(rec: &[u8]) -> Option<MixerParam> {
    let target = u32_at(rec, 4)?;
    let (param, insert, slot) = mixer_target(target)?;
    Some(MixerParam {
        param,
        insert,
        slot,
        value: i32_at(rec, 8)?,
    })
}

/// Splits a 233 payload into playlist items. The record size depends on the FL version (60
/// bytes in FL 21, 32 before, spec 7.2); the candidate whose every record has the 0x5000
/// pattern base at offset 4 wins, so a wrong version guess cannot misread the stream.
fn decode_items(b: &[u8], major: u32) -> (Vec<FlpItem>, usize) {
    let order: [usize; 2] = if major >= 21 { [60, 32] } else { [32, 60] };
    let based = |size: usize| {
        b.chunks_exact(size)
            .all(|r| u16_at(r, 4) == Some(PATTERN_BASE))
    };
    let size = order
        .iter()
        .copied()
        .find(|s| b.len().is_multiple_of(*s) && based(*s))
        .or_else(|| order.iter().copied().find(|s| b.len().is_multiple_of(*s)))
        .unwrap_or(order[0]);
    let items = b
        .chunks_exact(size)
        .map(|r| {
            let four = |at: usize| {
                let mut a = [0u8; 4];
                if let Some(s) = r.get(at..at + 4) {
                    a.copy_from_slice(s);
                }
                a
            };
            FlpItem {
                position: u32_at(r, 0).unwrap_or(0),
                index: u16_at(r, 6).unwrap_or(0),
                length: u32_at(r, 8).unwrap_or(0),
                track_raw: u32_at(r, 12).unwrap_or(0),
                flags: u16_at(r, 18).unwrap_or(0),
                start_raw: four(24),
                end_raw: four(28),
            }
        })
        .collect();
    (items, b.len() % size)
}

/// What a VST wrapper state (event 213 on a `Fruity Wrapper`) identifies (spec 8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WrapperId {
    /// Record 52: VST3 class id, bytes in file order.
    Vst3([u8; 16]),
    /// Record 51: VST2 unique id.
    Vst2([u8; 4]),
    /// Only options records: no plugin loaded, or it was missing.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperInfo {
    pub id: WrapperId,
    /// Record 54, the plugin's own name.
    pub name: Option<String>,
}

/// Walks the wrapper records (`u32 version`, then `u32 id, u64 length, bytes`). Stops quietly
/// at the first malformed record.
pub fn parse_wrapper(state: &[u8]) -> WrapperInfo {
    let mut at = 4usize;
    let (mut v2, mut v3, mut name) = (None, None, None);
    while let (Some(id), Some(len)) = (u32_at(state, at), u64_at(state, at + 4)) {
        let start = at + 12;
        let Some(end) = usize::try_from(len)
            .ok()
            .and_then(|l| start.checked_add(l))
            .filter(|e| *e <= state.len())
        else {
            break;
        };
        let body = &state[start..end];
        match id {
            51 if body.len() >= 4 => v2 = Some([body[0], body[1], body[2], body[3]]),
            52 if body.len() >= 16 => {
                let mut a = [0u8; 16];
                a.copy_from_slice(&body[..16]);
                v3 = Some(a);
            }
            54 => {
                let t = decode_text(body, false);
                if !t.is_empty() {
                    name = Some(t);
                }
            }
            _ => {}
        }
        at = end;
    }
    let id = match (v3, v2) {
        (Some(c), _) => WrapperId::Vst3(c),
        (None, Some(f)) => WrapperId::Vst2(f),
        _ => WrapperId::Unknown,
    };
    WrapperInfo { id, name }
}
