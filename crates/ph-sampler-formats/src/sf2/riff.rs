//! The SF2 RIFF container and hydra records (`docs/formats/sf2.md` §1–§4, §6).
//!
//! [`read`] checks the structure (the rejection rules of §4) and returns the records as
//! they are; zone generators and modulators are sorted out by [`super::zones`].

use crate::notes::Notes;

/// A structural fault: the file is rejected with this reason.
pub type Fault = String;

pub(crate) fn u16_at(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .unwrap_or(0)
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .unwrap_or(0)
}

/// A NUL-terminated (or full-width) Latin-1 string (§2).
pub(crate) fn zstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end]
        .iter()
        .map(|&c| c as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub version: (u16, u16),
    pub engine: String,
    pub name: String,
    pub rom: Option<String>,
    /// INAM, ICOP, IENG, ICMT, ICRD, ISFT, IPRD as found (kept as metadata).
    pub meta: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct PresetHdr {
    pub name: String,
    pub program: u16,
    pub bank: u16,
    /// Zones `[bag, bag_end)` in `pbag`.
    pub bag: usize,
    pub bag_end: usize,
}

#[derive(Clone, Debug)]
pub struct InstHdr {
    pub name: String,
    pub bag: usize,
    pub bag_end: usize,
}

/// A zone's generator and modulator index ranges.
#[derive(Clone, Copy, Debug)]
pub struct Bag {
    pub gen_start: usize,
    pub gen_end: usize,
    pub mod_start: usize,
    pub mod_end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gen {
    pub id: u16,
    pub amount: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModRec {
    pub src: u16,
    pub dest: u16,
    pub amount: i16,
    pub amt_src: u16,
    pub transform: u16,
}

#[derive(Clone, Debug)]
pub struct SampleHdr {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub rate: u32,
    pub pitch: u8,
    pub correction: i8,
    pub link: u16,
    pub kind: u16,
}

/// A parsed SoundFont, borrowing the sample data.
#[derive(Debug)]
pub struct Sf2<'a> {
    pub info: Info,
    /// The `smpl` bytes (16-bit PCM, plus SF3 compressed streams).
    pub smpl: &'a [u8],
    /// The `sm24` bytes, when they apply (§3).
    pub sm24: Option<&'a [u8]>,
    pub presets: Vec<PresetHdr>,
    pub pbag: Vec<Bag>,
    pub pmod: Vec<ModRec>,
    pub pgen: Vec<Gen>,
    pub insts: Vec<InstHdr>,
    pub ibag: Vec<Bag>,
    pub imod: Vec<ModRec>,
    pub igen: Vec<Gen>,
    pub samples: Vec<SampleHdr>,
}

impl Sf2<'_> {
    pub fn is_sf3(&self) -> bool {
        self.info.version.0 == 3
    }
}

/// A chunk: its id and data.
type Chunk<'a> = ([u8; 4], &'a [u8]);

/// Sub-chunks of a chunk body: (id, data). A size past the end is cut to the bytes left
/// when `tolerant` (SF3 may omit the final pad, §9), else a fault.
fn chunks<'a>(body: &'a [u8], tolerant: bool, notes: &mut Notes) -> Result<Vec<Chunk<'a>>, Fault> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= body.len() {
        let id = [body[pos], body[pos + 1], body[pos + 2], body[pos + 3]];
        let size = u32_at(body, pos + 4) as usize;
        let start = pos + 8;
        let end = match start.checked_add(size) {
            Some(e) if e <= body.len() => e,
            _ if tolerant => {
                notes.approx(
                    format!("chunk:{}", String::from_utf8_lossy(&id)),
                    "the chunk runs past the end of the file; read up to the end",
                );
                body.len()
            }
            _ => {
                return Err(format!(
                    "chunk {} runs past the end of its list",
                    String::from_utf8_lossy(&id)
                ));
            }
        };
        out.push((id, &body[start..end]));
        pos = end + (size & 1);
        // SF3 writers may leave out the pad byte after an odd `smpl`/`sdta` (§9): when the
        // padded position does not start a chunk but the unpadded one does, use it.
        if size & 1 == 1 && !plausible_id(body, pos) && plausible_id(body, end) {
            notes.approx(
                format!("chunk:{}", String::from_utf8_lossy(&id)),
                "an odd-sized chunk without its pad byte (SF3) was accepted",
            );
            pos = end;
        }
    }
    Ok(out)
}

/// Four printable ASCII bytes followed by a size: what a chunk header looks like.
fn plausible_id(body: &[u8], at: usize) -> bool {
    body.get(at..at + 8).is_some_and(|b| {
        b[..4]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b' ')
    })
}

fn records<T>(
    data: &[u8],
    size: usize,
    what: &str,
    f: impl Fn(&[u8]) -> T,
) -> Result<Vec<T>, Fault> {
    if !data.len().is_multiple_of(size) {
        return Err(format!(
            "{what} is {} bytes, not a multiple of its {size}-byte record",
            data.len()
        ));
    }
    Ok(data.chunks_exact(size).map(f).collect())
}

fn bags(data: &[u8], what: &str, gens: usize, mods: usize) -> Result<Vec<Bag>, Fault> {
    let raw = records(data, 4, what, |r| {
        (u16_at(r, 0) as usize, u16_at(r, 2) as usize)
    })?;
    if raw.is_empty() {
        return Err(format!("{what} has no terminal record"));
    }
    for w in raw.windows(2) {
        if w[1].0 < w[0].0 || w[1].1 < w[0].1 {
            return Err(format!("{what} indices are not monotonic"));
        }
    }
    let last = raw[raw.len() - 1];
    if last.0 != gens || last.1 != mods {
        return Err(format!(
            "{what}'s terminal record does not match its generator and modulator lists"
        ));
    }
    Ok(raw
        .windows(2)
        .map(|w| Bag {
            gen_start: w[0].0,
            gen_end: w[1].0,
            mod_start: w[0].1,
            mod_end: w[1].1,
        })
        .collect())
}

fn mods(data: &[u8], what: &str) -> Result<Vec<ModRec>, Fault> {
    records(data, 10, what, |r| ModRec {
        src: u16_at(r, 0),
        dest: u16_at(r, 2),
        amount: u16_at(r, 4) as i16,
        amt_src: u16_at(r, 6),
        transform: u16_at(r, 8),
    })
}

fn gens(data: &[u8], what: &str) -> Result<Vec<Gen>, Fault> {
    records(data, 4, what, |r| Gen {
        id: u16_at(r, 0),
        amount: u16_at(r, 2),
    })
}

/// Reads the RIFF structure. Errors are structural faults (§1, §4): the file is rejected.
pub fn read<'a>(bytes: &'a [u8], notes: &mut Notes) -> Result<Sf2<'a>, Fault> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
        return Err("not a RIFF sfbk file".into());
    }
    let size = u32_at(bytes, 4) as usize;
    let body_end = 8usize.saturating_add(size).min(bytes.len());
    if 8 + size > bytes.len() {
        notes.approx(
            "chunk:RIFF",
            "the RIFF size is larger than the file; read up to the end",
        );
    }
    let top = chunks(&bytes[12..body_end], true, notes)?;
    let mut info_l = None;
    let mut sdta_l = None;
    let mut pdta_l = None;
    for (id, data) in &top {
        if id == b"LIST" && data.len() >= 4 {
            match &data[0..4] {
                b"INFO" => info_l = info_l.or(Some(&data[4..])),
                b"sdta" => sdta_l = sdta_l.or(Some(&data[4..])),
                b"pdta" => pdta_l = pdta_l.or(Some(&data[4..])),
                other => notes.unknown(format!("list:{}", String::from_utf8_lossy(other))),
            }
        } else {
            notes.unknown(format!("chunk:{}", String::from_utf8_lossy(id)));
        }
    }
    let info_l = info_l.ok_or("no INFO list")?;
    let pdta_l = pdta_l.ok_or("no pdta list")?;

    // INFO (§2)
    let mut info = Info::default();
    let mut have_ifil = false;
    for (id, data) in chunks(info_l, true, notes)? {
        match &id {
            b"ifil" => {
                if data.len() != 4 {
                    return Err("ifil is not 4 bytes".into());
                }
                info.version = (u16_at(data, 0), u16_at(data, 2));
                have_ifil = true;
            }
            b"isng" => info.engine = zstr(data),
            b"INAM" => {
                info.name = zstr(data);
                info.meta.push(("INAM".into(), info.name.clone()));
            }
            b"irom" => info.rom = Some(zstr(data)).filter(|s| !s.is_empty()),
            b"iver" => {}
            b"ICRD" | b"IENG" | b"IPRD" | b"ICOP" | b"ICMT" | b"ISFT" => {
                info.meta
                    .push((String::from_utf8_lossy(&id).into_owned(), zstr(data)));
            }
            other => notes.unknown(format!("chunk:{}", String::from_utf8_lossy(other))),
        }
    }
    if !have_ifil {
        return Err("no ifil version chunk".into());
    }
    if info.engine.is_empty() {
        info.engine = "EMU8000".into();
    }
    if info.version.0 > 3 {
        notes.approx(
            "chunk:ifil",
            format!(
                "version {}.{} is newer than the reader knows; read as SF2",
                info.version.0, info.version.1
            ),
        );
    }

    // sdta (§3)
    let mut smpl: &[u8] = &[];
    let mut sm24 = None;
    if let Some(s) = sdta_l {
        for (id, data) in chunks(s, true, notes)? {
            match &id {
                b"smpl" => smpl = data,
                b"sm24" => sm24 = Some(data),
                other => notes.unknown(format!("chunk:{}", String::from_utf8_lossy(other))),
            }
        }
    }
    if let Some(s24) = sm24 {
        let (maj, min) = info.version;
        let half = smpl.len() / 2;
        let size_ok = s24.len() == half || (half % 2 == 1 && s24.len() == half + 1);
        // SPEC-GAP: sf2.md §3: the size rule is read as "half of smpl, or half + 1 when the
        // half is odd" (a pad byte may or may not be counted).
        let why = if maj < 2 || (maj == 2 && min < 4) {
            Some("the file is older than SoundFont 2.04")
        } else if smpl.is_empty() {
            Some("there is no 16-bit sample data")
        } else if !size_ok {
            Some("its size does not match the 16-bit sample data")
        } else {
            None
        };
        match why {
            Some(w) => {
                notes.skip(
                    "chunk:sm24",
                    format!("24-bit sample data ignored: {w}; samples play at 16 bits"),
                );
                sm24 = None;
            }
            None => sm24 = Some(&s24[..half]),
        }
    }

    // pdta (§4)
    let mut sub: Vec<([u8; 4], &[u8])> = Vec::new();
    for (id, data) in chunks(pdta_l, false, notes)? {
        sub.push((id, data));
    }
    let get = |name: &[u8; 4]| -> Result<&[u8], Fault> {
        sub.iter()
            .find(|(id, _)| id == name)
            .map(|(_, d)| *d)
            .ok_or_else(|| format!("pdta has no {} chunk", String::from_utf8_lossy(name)))
    };
    for (id, _) in &sub {
        if ![
            b"phdr", b"pbag", b"pmod", b"pgen", b"inst", b"ibag", b"imod", b"igen", b"shdr",
        ]
        .contains(&id)
        {
            notes.unknown(format!("chunk:{}", String::from_utf8_lossy(id)));
        }
    }
    let pmod = mods(get(b"pmod")?, "pmod")?;
    let pgen = gens(get(b"pgen")?, "pgen")?;
    let imod = mods(get(b"imod")?, "imod")?;
    let igen = gens(get(b"igen")?, "igen")?;
    // the terminal generator/modulator records are not zone data
    let pgen_n = pgen.len().saturating_sub(1);
    let pmod_n = pmod.len().saturating_sub(1);
    let igen_n = igen.len().saturating_sub(1);
    let imod_n = imod.len().saturating_sub(1);
    let pbag = bags(get(b"pbag")?, "pbag", pgen_n, pmod_n)?;
    let ibag = bags(get(b"ibag")?, "ibag", igen_n, imod_n)?;

    let phdr = records(get(b"phdr")?, 38, "phdr", |r| {
        (
            zstr(&r[0..20]),
            u16_at(r, 20),
            u16_at(r, 22),
            u16_at(r, 24) as usize,
        )
    })?;
    if phdr.len() < 2 {
        return Err("phdr needs a preset and its terminal record".into());
    }
    for w in phdr.windows(2) {
        if w[1].3 < w[0].3 {
            return Err("phdr bag indices are not monotonic".into());
        }
    }
    if phdr[phdr.len() - 1].3 != pbag.len() {
        return Err("phdr's terminal record does not match pbag".into());
    }
    let presets = phdr
        .windows(2)
        .map(|w| PresetHdr {
            name: w[0].0.clone(),
            program: w[0].1,
            bank: w[0].2,
            bag: w[0].3,
            bag_end: w[1].3,
        })
        .collect();

    let inst = records(get(b"inst")?, 22, "inst", |r| {
        (zstr(&r[0..20]), u16_at(r, 20) as usize)
    })?;
    if inst.len() < 2 {
        return Err("inst needs an instrument and its terminal record".into());
    }
    for w in inst.windows(2) {
        if w[1].1 < w[0].1 {
            return Err("inst bag indices are not monotonic".into());
        }
    }
    if inst[inst.len() - 1].1 != ibag.len() {
        return Err("inst's terminal record does not match ibag".into());
    }
    let insts = inst
        .windows(2)
        .map(|w| InstHdr {
            name: w[0].0.clone(),
            bag: w[0].1,
            bag_end: w[1].1,
        })
        .collect();

    let mut samples = records(get(b"shdr")?, 46, "shdr", |r| SampleHdr {
        name: zstr(&r[0..20]),
        start: u32_at(r, 20),
        end: u32_at(r, 24),
        loop_start: u32_at(r, 28),
        loop_end: u32_at(r, 32),
        rate: u32_at(r, 36),
        pitch: r[40],
        correction: r[41] as i8,
        link: u16_at(r, 42),
        kind: u16_at(r, 44),
    })?;
    if samples.is_empty() {
        return Err("shdr has no terminal record".into());
    }
    samples.pop();

    Ok(Sf2 {
        info,
        smpl,
        sm24,
        presets,
        pbag,
        pmod: pmod[..pmod_n].to_vec(),
        pgen: pgen[..pgen_n].to_vec(),
        insts,
        ibag,
        imod: imod[..imod_n].to_vec(),
        igen: igen[..igen_n].to_vec(),
        samples,
    })
}
