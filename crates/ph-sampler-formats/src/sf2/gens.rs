//! Generators and modulators (`docs/formats/sf2.md` §4.3–§4.4, §5, §7, §8): zone parsing
//! with the ordering rules, the summing of preset and instrument levels, and the modulator
//! hierarchy.

use super::riff::{Bag, Fault, Gen, ModRec, Sf2};
use crate::notes::Notes;

pub const START: usize = 0;
pub const END: usize = 1;
pub const LOOP_START: usize = 2;
pub const LOOP_END: usize = 3;
pub const START_COARSE: usize = 4;
pub const MOD_LFO_TO_PITCH: usize = 5;
pub const VIB_LFO_TO_PITCH: usize = 6;
pub const MOD_ENV_TO_PITCH: usize = 7;
pub const FC: usize = 8;
pub const Q: usize = 9;
pub const MOD_LFO_TO_FC: usize = 10;
pub const MOD_ENV_TO_FC: usize = 11;
pub const END_COARSE: usize = 12;
pub const MOD_LFO_TO_VOL: usize = 13;
pub const CHORUS: usize = 15;
pub const REVERB: usize = 16;
pub const PAN: usize = 17;
pub const DELAY_MOD_LFO: usize = 21;
pub const FREQ_MOD_LFO: usize = 22;
pub const DELAY_VIB_LFO: usize = 23;
pub const FREQ_VIB_LFO: usize = 24;
pub const DELAY_MOD_ENV: usize = 25;
pub const KEY_TO_MOD_HOLD: usize = 31;
pub const KEY_TO_MOD_DECAY: usize = 32;
pub const DELAY_VOL_ENV: usize = 33;
pub const SUSTAIN_VOL: usize = 37;
pub const KEY_TO_VOL_HOLD: usize = 39;
pub const KEY_TO_VOL_DECAY: usize = 40;
pub const INSTRUMENT: u16 = 41;
pub const KEY_RANGE: u16 = 43;
pub const VEL_RANGE: u16 = 44;
pub const LOOP_START_COARSE: usize = 45;
pub const KEYNUM: usize = 46;
pub const VELOCITY: usize = 47;
pub const ATTEN: usize = 48;
pub const LOOP_END_COARSE: usize = 50;
pub const COARSE: usize = 51;
pub const FINE: usize = 52;
pub const SAMPLE_ID: u16 = 53;
pub const SAMPLE_MODES: usize = 54;
pub const SCALE: usize = 56;
pub const EXCLUSIVE: usize = 57;
pub const ROOT_KEY: usize = 58;

/// Number of generator ids the spec defines (0..=60).
pub const N_GENS: usize = 61;

/// Reserved and unused ids (§5): ignored.
const UNUSED: [u16; 9] = [14, 18, 19, 20, 42, 49, 55, 59, 60];
/// Instrument-only generators, ignored in preset zones (§5, §8.5).
const INST_ONLY: [u16; 13] = [0, 1, 2, 3, 4, 12, 45, 46, 47, 50, 54, 57, 58];

#[cfg(test)]
pub fn is_time(g: usize) -> bool {
    matches!(g, 21 | 23 | 25..=28 | 30 | 33..=36 | 38)
}

/// The default value of a generator (§5).
pub fn default(g: usize) -> i32 {
    match g {
        FC => 13500,
        21 | 23 | 25..=28 | 30 | 33..=36 | 38 => -12000,
        SCALE => 100,
        KEYNUM | VELOCITY | ROOT_KEY => -1,
        _ => 0,
    }
}

/// The legal range of a summed value (§5, §7 step 4). Time generators keep −32768 (= 0 s).
pub fn range(g: usize) -> (i32, i32) {
    match g {
        0 | 4 => (0, i32::MAX),
        1 | 12 => (i32::MIN, 0),
        5..=7 | 10 | 11 => (-12000, 12000),
        FC => (1500, 13500),
        Q => (0, 960),
        MOD_LFO_TO_VOL => (-960, 960),
        15 | 16 | 29 => (0, 1000),
        PAN => (-500, 500),
        21 | 23 | 25 | 27 | 33 | 35 => (-32768, 5000),
        22 | 24 => (-16000, 4500),
        26 | 28 | 30 | 34 | 36 | 38 => (-32768, 8000),
        31 | 32 | 39 | 40 => (-1200, 1200),
        SUSTAIN_VOL | ATTEN => (0, 1440),
        COARSE => (-120, 120),
        FINE => (-99, 99),
        SAMPLE_MODES => (0, 3),
        SCALE => (0, 1200),
        EXCLUSIVE => (0, 127),
        KEYNUM | ROOT_KEY => (-1, 127),
        VELOCITY => (-1, 127),
        _ => (i32::MIN, i32::MAX),
    }
}

/// One zone after the ordering rules (§4.4).
#[derive(Clone, Debug, Default)]
pub struct ZoneGens {
    pub vals: Vec<Option<i32>>,
    pub key: Option<(u8, u8)>,
    pub vel: Option<(u8, u8)>,
    /// The instrument (preset zone) or sample (instrument zone) index.
    pub target: Option<usize>,
    pub mods: Vec<ModRec>,
}

impl ZoneGens {
    fn empty() -> Self {
        Self {
            vals: vec![None; N_GENS],
            ..Default::default()
        }
    }
}

/// A preset's or instrument's zones: the global zone (if any) and the local ones.
#[derive(Clone, Debug, Default)]
pub struct Zones {
    pub global: Option<ZoneGens>,
    pub local: Vec<ZoneGens>,
}

/// Checks every zone's terminal index (§4.4: an index at or past the terminal record
/// rejects the file).
pub fn check_indices(sf: &Sf2<'_>) -> Result<(), Fault> {
    for b in &sf.pbag {
        for g in sf.pgen.get(b.gen_start..b.gen_end).unwrap_or(&[]) {
            if g.id == INSTRUMENT && g.amount as usize >= sf.insts.len() {
                return Err(format!(
                    "a preset zone names instrument {} of {}",
                    g.amount,
                    sf.insts.len()
                ));
            }
        }
    }
    for b in &sf.ibag {
        for g in sf.igen.get(b.gen_start..b.gen_end).unwrap_or(&[]) {
            if g.id == SAMPLE_ID && g.amount as usize >= sf.samples.len() {
                return Err(format!(
                    "an instrument zone names sample {} of {}",
                    g.amount,
                    sf.samples.len()
                ));
            }
        }
    }
    Ok(())
}

/// The zones of bags `[from, to)` (§4.4). `preset` selects the preset-level rules.
pub fn zones(
    lists: (&[Bag], &[Gen], &[ModRec]),
    range: (usize, usize),
    preset: bool,
    owner: &str,
    notes: &mut Notes,
) -> Zones {
    let terminal = if preset { INSTRUMENT } else { SAMPLE_ID };
    let level = if preset { "preset" } else { "instrument" };
    let mut out = Zones::default();
    let (bags, gens, mods) = lists;
    let (from, to) = range;
    let bags = bags.get(from..to.min(bags.len())).unwrap_or(&[]);
    for (zi, b) in bags.iter().enumerate() {
        let mut z = ZoneGens::empty();
        let list = gens.get(b.gen_start..b.gen_end).unwrap_or(&[]);
        let mut first_key = false;
        for (i, g) in list.iter().enumerate() {
            match g.id {
                KEY_RANGE => {
                    if i == 0 {
                        z.key = Some(range_of(g.amount));
                        first_key = true;
                    } else {
                        notes.skip(
                            "generator:keyRange",
                            "a keyRange that is not a zone's first generator was ignored",
                        );
                    }
                }
                VEL_RANGE => {
                    if i == 0 || (i == 1 && first_key) {
                        z.vel = Some(range_of(g.amount));
                    } else {
                        notes.skip(
                            "generator:velRange",
                            "a velRange after other generators was ignored",
                        );
                    }
                }
                id if id == terminal => {
                    z.target = Some(g.amount as usize);
                    if i + 1 < list.len() {
                        notes.skip(
                            format!(
                                "generator:after-{}",
                                if preset { "instrument" } else { "sampleID" }
                            ),
                            "generators after a zone's terminal generator were ignored",
                        );
                    }
                    break;
                }
                SAMPLE_ID | INSTRUMENT => notes.skip(
                    format!("generator:{}", g.id),
                    format!("an index generator in a {level} zone of the wrong level was ignored"),
                ),
                id if UNUSED.contains(&id) => {}
                id if (id as usize) >= N_GENS => notes.unknown(format!("generator:{id}")),
                id if preset && INST_ONLY.contains(&id) => notes.skip(
                    format!("generator:{id}"),
                    "an instrument-only generator in a preset zone was ignored",
                ),
                id => z.vals[id as usize] = Some(g.amount as i16 as i32),
            }
        }
        let zm = mods.get(b.mod_start..b.mod_end).unwrap_or(&[]);
        for m in zm {
            // §4.3: same source, destination and amount source: the later one wins
            z.mods
                .retain(|o| !(o.src == m.src && o.dest == m.dest && o.amt_src == m.amt_src));
            z.mods.push(*m);
        }
        if z.target.is_some() {
            out.local.push(z);
        } else if zi == 0 {
            let has = z.vals.iter().any(Option::is_some)
                || z.key.is_some()
                || z.vel.is_some()
                || !z.mods.is_empty();
            if has {
                out.global = Some(z);
            }
        } else {
            notes.skip(
                format!("zone:{level}"),
                format!("a zone without its terminal generator was ignored ({owner})"),
            );
        }
    }
    out
}

fn range_of(amount: u16) -> (u8, u8) {
    let lo = (amount & 0xFF) as u8;
    let hi = (amount >> 8) as u8;
    (lo.min(127), hi.min(127))
}

/// Intersects two inclusive ranges; `None` when empty.
pub fn intersect(a: (u8, u8), b: (u8, u8)) -> Option<(u8, u8)> {
    let lo = a.0.max(b.0);
    let hi = a.1.min(b.1);
    (lo <= hi).then_some((lo, hi))
}

/// The summed generator values of one (preset zone × instrument zone) pair (§7 step 3–4).
pub fn sum(
    pglobal: Option<&ZoneGens>,
    plocal: &ZoneGens,
    iglobal: Option<&ZoneGens>,
    ilocal: &ZoneGens,
) -> Vec<i32> {
    (0..N_GENS)
        .map(|g| {
            let inst = ilocal.vals[g]
                .or_else(|| iglobal.and_then(|z| z.vals[g]))
                .unwrap_or_else(|| default(g));
            let relative = !(INST_ONLY.contains(&(g as u16)));
            let pre = if relative {
                plocal.vals[g]
                    .or_else(|| pglobal.and_then(|z| z.vals[g]))
                    .unwrap_or(0)
            } else {
                0
            };
            let (lo, hi) = range(g);
            let v = inst as i64 + pre as i64;
            v.clamp(lo as i64, hi as i64) as i32
        })
        .collect()
}

// ------------------------------------------------------------------------- modulators

// SPEC-GAP: sf2.md §8.4 #10: "initial pitch" has no generator number, so a file cannot
// override default modulator 10 by naming it; it always applies (its amount sets the bend).
/// Destination of the default pitch-wheel modulator: the voice's pitch summing node, which
/// has no generator number (§8.4 #10). Files cannot name it (bit 15 means a link).
pub const PITCH_DEST: u16 = 0xFFFF;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modulator {
    pub src: u16,
    pub dest: u16,
    pub amount: f64,
    pub amt_src: u16,
    pub transform: u16,
    /// Which default (1-based, §8.4) this modulator is identical to, if any.
    pub default: Option<u8>,
}

impl Modulator {
    fn same(&self, o: &Modulator) -> bool {
        self.src == o.src
            && self.dest == o.dest
            && self.amt_src == o.amt_src
            && self.transform == o.transform
    }
}

/// The ten default modulators (§8.4).
pub fn defaults() -> Vec<Modulator> {
    const D: [(u16, u16, f64, u16); 10] = [
        (0x0502, ATTEN as u16, 960.0, 0),
        (0x0102, FC as u16, -2400.0, 0),
        (0x000D, VIB_LFO_TO_PITCH as u16, 50.0, 0),
        (0x0081, VIB_LFO_TO_PITCH as u16, 50.0, 0),
        (0x0582, ATTEN as u16, 960.0, 0),
        (0x028A, PAN as u16, 1000.0, 0),
        (0x058B, ATTEN as u16, 960.0, 0),
        (0x00DB, REVERB as u16, 200.0, 0),
        (0x00DD, CHORUS as u16, 200.0, 0),
        (0x020E, PITCH_DEST, 12700.0, 0x0010),
    ];
    D.iter()
        .enumerate()
        .map(|(i, &(src, dest, amount, amt_src))| Modulator {
            src,
            dest,
            amount,
            amt_src,
            transform: 0,
            default: Some(i as u8 + 1),
        })
        .collect()
}

/// Why a modulator is not valid (§4.3, §8.1), if it is not.
pub fn invalid(m: &ModRec) -> Option<&'static str> {
    if m.dest & 0x8000 != 0 {
        return Some("linked modulators are not supported");
    }
    if m.dest as usize >= N_GENS || UNUSED.contains(&m.dest) {
        return Some("unknown destination");
    }
    if m.transform != 0 && m.transform != 2 {
        return Some("unknown transform");
    }
    for (s, is_amt) in [(m.src, false), (m.amt_src, true)] {
        // SPEC-GAP: sf2.md §8.1 says "any of bits 11–15 set → ignore", which would exclude
        // the convex (2) and switch (3) types it lists; read as "a type above 3".
        if s >> 10 > 3 {
            return Some("unknown source curve type");
        }
        let idx = s & 0x7F;
        if s & 0x80 != 0 {
            if matches!(idx, 0 | 6 | 32 | 38 | 98..=101 | 120..=127) || (33..=63).contains(&idx) {
                return Some("illegal MIDI controller source");
            }
        } else {
            match idx {
                0 | 2 | 3 | 10 | 13 | 14 | 16 => {}
                127 if !is_amt => return Some("linked modulators are not supported"),
                _ => return Some("reserved source"),
            }
        }
    }
    None
}

fn apply_level(list: &mut Vec<Modulator>, zone: Option<&ZoneGens>, notes: &mut Notes) {
    let Some(z) = zone else { return };
    for m in &z.mods {
        if let Some(why) = invalid(m) {
            notes.skip(
                format!("modulator:{:#06x}->{}", m.src, m.dest),
                format!("ignored: {why}"),
            );
            continue;
        }
        let new = Modulator {
            src: m.src,
            dest: m.dest,
            amount: m.amount as f64,
            amt_src: m.amt_src,
            transform: m.transform,
            default: None,
        };
        match list.iter_mut().find(|o| o.same(&new)) {
            Some(o) => o.amount = new.amount,
            None => list.push(new),
        }
    }
}

/// The modulators of one voice (§8.3): defaults, replaced or added to by the instrument
/// global and local zones; the preset level (local replacing global) added on top.
pub fn voice_mods(
    pglobal: Option<&ZoneGens>,
    plocal: &ZoneGens,
    iglobal: Option<&ZoneGens>,
    ilocal: &ZoneGens,
    notes: &mut Notes,
) -> Vec<Modulator> {
    let mut list = defaults();
    apply_level(&mut list, iglobal, notes);
    apply_level(&mut list, Some(ilocal), notes);
    let mut pre = Vec::new();
    apply_level(&mut pre, pglobal, notes);
    apply_level(&mut pre, Some(plocal), notes);
    for p in pre {
        match list.iter_mut().find(|o| o.same(&p)) {
            Some(o) => o.amount += p.amount,
            None => list.push(p),
        }
    }
    list
}

/// Generators that may appear in a preset zone (for tests).
#[cfg(test)]
pub fn preset_level(id: u16) -> bool {
    !INST_ONLY.contains(&id) && id != SAMPLE_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_ranges() {
        assert_eq!(default(FC), 13500);
        assert_eq!(default(34), -12000);
        assert_eq!(default(SCALE), 100);
        assert_eq!(default(ROOT_KEY), -1);
        assert!(is_time(38) && !is_time(37));
        assert!(preset_level(ATTEN as u16) && !preset_level(SAMPLE_MODES as u16));
    }

    #[test]
    fn instrument_absolute_preset_relative() {
        let mut il = ZoneGens::empty();
        il.vals[34] = Some(1200);
        let mut pl = ZoneGens::empty();
        pl.vals[34] = Some(2400);
        pl.vals[SAMPLE_MODES] = Some(1); // instrument-only: never added
        let mut pg = ZoneGens::empty();
        pg.vals[ATTEN] = Some(100);
        let mut ig = ZoneGens::empty();
        ig.vals[ATTEN] = Some(50);
        let v = sum(Some(&pg), &pl, Some(&ig), &il);
        assert_eq!(v[34], 3600);
        assert_eq!(v[ATTEN], 150);
        assert_eq!(v[SAMPLE_MODES], 0);
        assert_eq!(v[FC], 13500);
        // clamped to the range
        il.vals[PAN] = Some(400);
        pl.vals[PAN] = Some(400);
        assert_eq!(sum(None, &pl, None, &il)[PAN], 500);
    }

    #[test]
    fn modulator_hierarchy() {
        let mut ig = ZoneGens::empty();
        // replaces default #1 (velocity → attenuation) with 0: disabled
        ig.mods.push(ModRec {
            src: 0x0502,
            dest: ATTEN as u16,
            amount: 0,
            amt_src: 0,
            transform: 0,
        });
        let mut il = ZoneGens::empty();
        let cc74 = ModRec {
            src: 0x00CA,
            dest: FC as u16,
            amount: 1200,
            amt_src: 0,
            transform: 0,
        };
        il.mods.push(cc74);
        let mut pl = ZoneGens::empty();
        pl.mods.push(ModRec {
            amount: 600,
            ..cc74
        });
        let mut notes = Notes::default();
        let m = voice_mods(None, &pl, Some(&ig), &il, &mut notes);
        assert_eq!(m[0].amount, 0.0);
        let c = m.iter().find(|x| x.src == 0x00CA).unwrap();
        assert_eq!(c.amount, 1800.0);
        assert_eq!(m.len(), 11);
    }

    #[test]
    fn invalid_modulators() {
        let ok = ModRec {
            src: 0x0081,
            dest: 6,
            amount: 1,
            amt_src: 0,
            transform: 0,
        };
        assert_eq!(invalid(&ok), None);
        assert!(invalid(&ModRec { dest: 0x8001, ..ok }).is_some());
        assert!(
            invalid(&ModRec {
                src: 0x0080 | 6,
                ..ok
            })
            .is_some()
        );
        assert!(
            invalid(&ModRec {
                src: 0x0080 | 40,
                ..ok
            })
            .is_some()
        );
        assert!(invalid(&ModRec { src: 0x1002, ..ok }).is_some());
        assert_eq!(invalid(&ModRec { src: 0x0C02, ..ok }), None); // switch
        assert!(invalid(&ModRec { transform: 1, ..ok }).is_some());
        assert!(invalid(&ModRec { src: 0x007F, ..ok }).is_some());
    }
}
