//! Headers and inheritance (`docs/formats/sfz.md` §3): the token stream to regions whose
//! opcodes are the merge global ∪ master ∪ group ∪ region, plus control settings, curves and
//! effect blocks.

use super::lex::{Tok, is_digits};
use super::opcodes::{canonical, is_key_opcode, parse_key};
use crate::notes::Notes;
use std::collections::BTreeMap;

pub type Ops = BTreeMap<String, String>;

/// One emitted region.
#[derive(Clone, Debug, Default)]
pub struct Region {
    /// Merged opcodes (canonical names; key values already numbers).
    pub ops: Ops,
    /// Ordinal of the `<group>` header it sits under (0 = before any group header).
    pub group_ix: usize,
    /// `default_path` in force when the region was read.
    pub default_path: String,
}

/// A `<curve>` (§3.5): 128 points.
#[derive(Clone, Debug)]
pub struct Curve(pub [f32; 128]);

#[derive(Debug, Default)]
pub struct Parsed {
    pub regions: Vec<Region>,
    /// Control opcodes other than the offsets and `default_path` (set_ccN, labels, ...),
    /// last value wins.
    pub control: Ops,
    pub curves: BTreeMap<u32, Curve>,
    /// `<effect>` blocks in order.
    pub effects: Vec<Ops>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Level {
    Control,
    Global,
    Master,
    Group,
    Region,
    Curve,
    Effect,
    /// `<midi>`, `<sample>`, unknown headers: opcodes are skipped.
    Ignored,
}

#[derive(Default)]
struct Offsets {
    note: i64,
    octave: i64,
    sw_note: i64,
    sw_octave: i64,
}

struct Builder<'n> {
    notes: &'n mut Notes,
    level: Level,
    global: Ops,
    master: Ops,
    group: Ops,
    region: Option<Ops>,
    curve: Option<Ops>,
    effect: Option<Ops>,
    group_ix: usize,
    default_path: String,
    offsets: Offsets,
    out: Parsed,
}

fn parse_int(v: &str) -> Option<i64> {
    v.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .map(|x| x as i64)
}

impl Builder<'_> {
    fn end_block(&mut self) {
        if let Some(r) = self.region.take() {
            let mut ops = self.global.clone();
            ops.extend(self.master.clone());
            ops.extend(self.group.clone());
            ops.extend(r);
            self.out.regions.push(Region {
                ops,
                group_ix: self.group_ix,
                default_path: self.default_path.clone(),
            });
        }
        if let Some(c) = self.curve.take() {
            self.finish_curve(c);
        }
        if let Some(e) = self.effect.take() {
            self.out.effects.push(e);
        }
    }

    fn finish_curve(&mut self, c: Ops) {
        let Some(index) = c.get("curve_index").and_then(|v| parse_int(v)) else {
            self.notes.skip(
                "header:<curve>",
                "a <curve> without curve_index was ignored",
            );
            return;
        };
        if !(0..=255).contains(&index) {
            self.notes.skip(
                "header:<curve>",
                "a curve_index outside 0..=255 was ignored",
            );
            return;
        }
        if index <= 6 {
            self.notes.skip(
                format!("curve:{index}"),
                "built-in curves 0-6 cannot be redefined",
            );
            return;
        }
        let mut set: BTreeMap<usize, f32> = BTreeMap::new();
        for (k, v) in &c {
            if let Some(n) = k.strip_prefix('v').filter(|n| is_digits(n)) {
                match (n.parse::<usize>(), v.parse::<f32>()) {
                    (Ok(i), Ok(x)) if i < 128 && x.is_finite() => {
                        set.insert(i, x);
                    }
                    _ => self
                        .notes
                        .skip(format!("opcode:{k}"), "a curve point that does not parse"),
                }
            } else if k != "curve_index" {
                self.notes.unknown(format!("opcode:{k}"));
            }
        }
        set.entry(0).or_insert(0.0);
        set.entry(127).or_insert(1.0);
        let pts: Vec<(usize, f32)> = set.into_iter().collect();
        let mut out = [0f32; 128];
        for w in pts.windows(2) {
            let ((a, va), (b, vb)) = (w[0], w[1]);
            for (i, o) in out.iter_mut().enumerate().take(b + 1).skip(a) {
                let t = (i - a) as f32 / (b - a).max(1) as f32;
                *o = va + (vb - va) * t;
            }
        }
        self.out.curves.insert(index as u32, Curve(out));
    }

    fn header(&mut self, name: &str) {
        self.end_block();
        self.level = match name {
            "control" => {
                // a later <control> replaces the earlier settings
                self.default_path.clear();
                self.offsets = Offsets::default();
                Level::Control
            }
            "global" => {
                self.global.clear();
                self.master.clear();
                self.group.clear();
                Level::Global
            }
            "master" => {
                self.master.clear();
                self.group.clear();
                Level::Master
            }
            "group" => {
                self.group.clear();
                self.group_ix += 1;
                Level::Group
            }
            "region" => {
                self.region = Some(Ops::new());
                Level::Region
            }
            "curve" => {
                self.curve = Some(Ops::new());
                Level::Curve
            }
            "effect" => {
                self.effect = Some(Ops::new());
                Level::Effect
            }
            "midi" => {
                self.notes.skip(
                    "header:<midi>",
                    "MIDI-processing settings are not used by the sampler",
                );
                Level::Ignored
            }
            "sample" => {
                self.notes.skip(
                    "header:<sample>",
                    "embedded samples are not read; regions playing them are reported as missing",
                );
                Level::Ignored
            }
            other => {
                self.notes.unknown(format!("header:<{other}>"));
                Level::Ignored
            }
        };
    }

    fn opcode(&mut self, raw: &str, value: &str) {
        let name = canonical(raw);
        match self.level {
            Level::Ignored => {}
            Level::Curve => {
                if let Some(c) = &mut self.curve {
                    c.insert(name, value.to_string());
                }
            }
            Level::Effect => {
                if let Some(e) = &mut self.effect {
                    e.insert(name, value.to_string());
                }
            }
            Level::Control => self.control(&name, value),
            _ => {
                if self.is_control_only(&name) {
                    // control opcodes are accepted wherever they appear
                    self.control(&name, value);
                    return;
                }
                let mut ops = Vec::new();
                if name == "key" {
                    match self.key(&name, value) {
                        Some(k) => {
                            for n in ["lokey", "hikey", "pitch_keycenter"] {
                                ops.push((n.to_string(), k.to_string()));
                            }
                        }
                        None => return,
                    }
                } else if is_key_opcode(&name) {
                    if (name == "lokey" || name == "hikey") && value.trim() == "-1" {
                        ops.push((name, "-1".into()));
                    } else if name == "pitch_keycenter"
                        && value.trim().eq_ignore_ascii_case("sample")
                    {
                        ops.push((name, "sample".into()));
                    } else {
                        match self.key(&name, value) {
                            Some(k) => ops.push((name, k.to_string())),
                            None => return,
                        }
                    }
                } else {
                    ops.push((name, value.to_string()));
                }
                let map = match self.level {
                    Level::Global => &mut self.global,
                    Level::Master => &mut self.master,
                    Level::Group => &mut self.group,
                    _ => self.region.get_or_insert_with(Ops::new),
                };
                map.extend(ops);
            }
        }
    }

    fn is_control_only(&self, name: &str) -> bool {
        matches!(
            name,
            "default_path"
                | "note_offset"
                | "octave_offset"
                | "sw_note_offset"
                | "sw_octave_offset"
        ) || ["set_cc", "set_hdcc"]
            .iter()
            .any(|p| name.strip_prefix(p).is_some_and(is_digits))
    }

    fn control(&mut self, name: &str, value: &str) {
        let clamp = |v: Option<i64>, lo: i64, hi: i64| v.map(|x| x.clamp(lo, hi));
        match name {
            "default_path" => self.default_path = value.replace('\\', "/"),
            "note_offset" => self.offsets.note = clamp(parse_int(value), -127, 127).unwrap_or(0),
            "octave_offset" => self.offsets.octave = clamp(parse_int(value), -10, 10).unwrap_or(0),
            "sw_note_offset" => {
                self.offsets.sw_note = clamp(parse_int(value), -127, 127).unwrap_or(0)
            }
            "sw_octave_offset" => {
                self.offsets.sw_octave = clamp(parse_int(value), -10, 10).unwrap_or(0)
            }
            _ => {
                self.out.control.insert(name.to_string(), value.to_string());
            }
        }
    }

    /// A key value with the offsets in force, clamped to 0..=127 (§3.3).
    fn key(&mut self, name: &str, value: &str) -> Option<i64> {
        let Some(k) = parse_key(value) else {
            self.notes.skip(
                format!("opcode:{name}"),
                format!("the key value {value:?} does not parse; left unset"),
            );
            return None;
        };
        let mut k = k + self.offsets.note + 12 * self.offsets.octave;
        if name.starts_with("sw_") {
            k += self.offsets.sw_note + 12 * self.offsets.sw_octave;
        }
        if !(0..=127).contains(&k) {
            self.notes.approx(
                format!("opcode:{name}"),
                "a key outside 0..=127 was clamped",
            );
        }
        Some(k.clamp(0, 127))
    }
}

/// Builds the regions from the token stream.
pub fn build(tokens: Vec<Tok>, notes: &mut Notes) -> Parsed {
    let mut b = Builder {
        notes,
        level: Level::Global,
        global: Ops::new(),
        master: Ops::new(),
        group: Ops::new(),
        region: None,
        curve: None,
        effect: None,
        group_ix: 0,
        default_path: String::new(),
        offsets: Offsets::default(),
        out: Parsed::default(),
    };
    for t in tokens {
        match t {
            Tok::Header(h) => b.header(&h),
            Tok::Opcode { name, value } => b.opcode(&name, &value),
        }
    }
    b.end_block();
    b.out
}
