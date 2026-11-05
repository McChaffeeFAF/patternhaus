//! `Sf2Builder`: writes SoundFont 2 / SF3 files from `docs/formats/sf2.md` (§1–§4, §9).
//!
//! Zones are written exactly as given (generator order included), so tests can build files
//! that break the ordering rules on purpose. [`Sf2Builder::chunks`] gives the chunks before
//! assembly for tests that damage the structure.

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8], pad: bool) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if pad && data.len() % 2 == 1 {
        out.push(0);
    }
}

fn list(id: &[u8; 4], kids: &[([u8; 4], Vec<u8>)], pad_last: bool) -> Vec<u8> {
    let mut body = id.to_vec();
    for (i, (cid, data)) in kids.iter().enumerate() {
        chunk(&mut body, cid, data, pad_last || i + 1 < kids.len());
    }
    body
}

fn name20(s: &str) -> [u8; 20] {
    let mut b = [0u8; 20];
    for (i, c) in s.bytes().take(19).enumerate() {
        b[i] = c;
    }
    b
}

fn zstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    if v.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// A sample. `pcm` holds 24-bit values (a 16-bit sample `s` is `s << 8`); the low byte is
/// written to `sm24` only when the builder writes `sm24`.
#[derive(Clone, Debug)]
pub struct Sf2Sample {
    pub name: String,
    pub pcm: Vec<i32>,
    pub rate: u32,
    pub pitch: u8,
    pub correction: i8,
    /// Loop `[start, end)` relative to the sample's first frame.
    pub loop_points: (u32, u32),
    /// Sample type (1 mono, 2 right, 4 left, 0x8001 ROM, ... ; 0x10 is added for streams).
    pub kind: u16,
    pub link: u16,
    /// SF3: a compressed stream written after the PCM samples instead of `pcm`.
    pub stream: Option<Vec<u8>>,
}

impl Sf2Sample {
    /// A 16-bit mono sample from float audio (-1..1).
    pub fn from_f32(name: &str, data: &[f32], rate: u32, pitch: u8) -> Self {
        Self {
            name: name.into(),
            pcm: data
                .iter()
                .map(|v| ((v.clamp(-1.0, 1.0) * 32767.0).round() as i32) << 8)
                .collect(),
            rate,
            pitch,
            correction: 0,
            loop_points: (0, 0),
            kind: 1,
            link: 0,
            stream: None,
        }
    }

    /// A compressed (SF3) sample: `stream` is written as is; `kind` gets bit 4.
    pub fn compressed(name: &str, stream: Vec<u8>, rate: u32, pitch: u8) -> Self {
        Self {
            name: name.into(),
            pcm: Vec::new(),
            rate,
            pitch,
            correction: 0,
            loop_points: (0, 0),
            kind: 1 | 0x10,
            link: 0,
            stream: Some(stream),
        }
    }
}

/// A preset or instrument zone: generators `(id, amount)` and modulators
/// `[src, dest, amount, amount source, transform]`, written in the order given.
#[derive(Clone, Debug, Default)]
pub struct Sf2Zone {
    pub gens: Vec<(u16, u16)>,
    pub mods: Vec<[u16; 5]>,
}

impl Sf2Zone {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn keys(mut self, lo: u8, hi: u8) -> Self {
        self.gens.push((43, lo as u16 | (hi as u16) << 8));
        self
    }
    pub fn vels(mut self, lo: u8, hi: u8) -> Self {
        self.gens.push((44, lo as u16 | (hi as u16) << 8));
        self
    }
    /// A value generator.
    pub fn set(mut self, id: u16, value: i16) -> Self {
        self.gens.push((id, value as u16));
        self
    }
    /// The terminal generator of an instrument zone.
    pub fn sample(mut self, index: u16) -> Self {
        self.gens.push((53, index));
        self
    }
    /// The terminal generator of a preset zone.
    pub fn instrument(mut self, index: u16) -> Self {
        self.gens.push((41, index));
        self
    }
    pub fn modulator(
        mut self,
        src: u16,
        dest: u16,
        amount: i16,
        amt_src: u16,
        transform: u16,
    ) -> Self {
        self.mods
            .push([src, dest, amount as u16, amt_src, transform]);
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sf2Instrument {
    pub name: String,
    pub zones: Vec<Sf2Zone>,
}

#[derive(Clone, Debug, Default)]
pub struct Sf2Preset {
    pub name: String,
    pub bank: u16,
    pub program: u16,
    pub zones: Vec<Sf2Zone>,
}

/// The chunks of a file before assembly.
#[derive(Clone, Debug)]
pub struct Sf2Chunks {
    pub info: Vec<([u8; 4], Vec<u8>)>,
    pub smpl: Vec<u8>,
    pub sm24: Option<Vec<u8>>,
    /// Pad an odd-length `smpl` (SF3 files may leave the pad out).
    pub pad_smpl: bool,
    pub pdta: Vec<([u8; 4], Vec<u8>)>,
}

impl Sf2Chunks {
    pub fn pdta_mut(&mut self, id: &[u8; 4]) -> &mut Vec<u8> {
        &mut self
            .pdta
            .iter_mut()
            .find(|(i, _)| i == id)
            .expect("pdta chunk")
            .1
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut sdta_kids = vec![(*b"smpl", self.smpl.clone())];
        if let Some(s) = &self.sm24 {
            sdta_kids.push((*b"sm24", s.clone()));
        }
        let mut body = b"sfbk".to_vec();
        chunk(&mut body, b"LIST", &list(b"INFO", &self.info, true), true);
        let pad = self.pad_smpl || self.sm24.is_some();
        let sdta = list(b"sdta", &sdta_kids, pad);
        chunk(&mut body, b"LIST", &sdta, pad);
        chunk(&mut body, b"LIST", &list(b"pdta", &self.pdta, true), true);
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }
}

/// Builds a SoundFont.
#[derive(Clone, Debug)]
pub struct Sf2Builder {
    pub version: (u16, u16),
    pub name: String,
    /// `irom`, for ROM-sample fixtures.
    pub rom: Option<String>,
    /// Write the low bytes of `pcm` as `sm24`.
    pub sm24: bool,
    pub pad_smpl: bool,
    pub samples: Vec<Sf2Sample>,
    pub instruments: Vec<Sf2Instrument>,
    pub presets: Vec<Sf2Preset>,
}

impl Default for Sf2Builder {
    fn default() -> Self {
        Self {
            version: (2, 1),
            name: "Test bank".into(),
            rom: None,
            sm24: false,
            pad_smpl: true,
            samples: Vec::new(),
            instruments: Vec::new(),
            presets: Vec::new(),
        }
    }
}

fn zone_lists(zones_of: &[&[Sf2Zone]]) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u16>) {
    // returns (bag, mod, gen, first bag index of each owner + terminal)
    let mut bag = Vec::new();
    let mut gens = Vec::new();
    let mut mods = Vec::new();
    let mut firsts = Vec::new();
    let (mut ng, mut nm, mut nb) = (0u16, 0u16, 0u16);
    for zones in zones_of {
        firsts.push(nb);
        for z in *zones {
            bag.extend_from_slice(&ng.to_le_bytes());
            bag.extend_from_slice(&nm.to_le_bytes());
            nb += 1;
            for (id, a) in &z.gens {
                gens.extend_from_slice(&id.to_le_bytes());
                gens.extend_from_slice(&a.to_le_bytes());
                ng += 1;
            }
            for m in &z.mods {
                for v in m {
                    mods.extend_from_slice(&v.to_le_bytes());
                }
                nm += 1;
            }
        }
    }
    firsts.push(nb);
    bag.extend_from_slice(&ng.to_le_bytes());
    bag.extend_from_slice(&nm.to_le_bytes());
    gens.extend_from_slice(&[0; 4]);
    mods.extend_from_slice(&[0; 10]);
    (bag, mods, gens, firsts)
}

impl Sf2Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a sample; returns its index.
    pub fn add_sample(&mut self, s: Sf2Sample) -> u16 {
        self.samples.push(s);
        self.samples.len() as u16 - 1
    }

    pub fn add_instrument(&mut self, name: &str, zones: Vec<Sf2Zone>) -> u16 {
        self.instruments.push(Sf2Instrument {
            name: name.into(),
            zones,
        });
        self.instruments.len() as u16 - 1
    }

    pub fn add_preset(&mut self, name: &str, bank: u16, program: u16, zones: Vec<Sf2Zone>) {
        self.presets.push(Sf2Preset {
            name: name.into(),
            bank,
            program,
            zones,
        });
    }

    pub fn chunks(&self) -> Sf2Chunks {
        let mut info = vec![(*b"ifil", {
            let mut v = self.version.0.to_le_bytes().to_vec();
            v.extend_from_slice(&self.version.1.to_le_bytes());
            v
        })];
        info.push((*b"isng", zstr("EMU8000")));
        info.push((*b"INAM", zstr(&self.name)));
        if let Some(r) = &self.rom {
            info.push((*b"irom", zstr(r)));
        }
        info.push((*b"ISFT", zstr("Patternhaus Sf2Builder")));

        // sample data: PCM first (each + 46 zero frames), then streams
        let mut smpl = Vec::new();
        let mut sm24 = Vec::new();
        let mut shdr = Vec::new();
        let mut placed: Vec<(u32, u32)> = vec![(0, 0); self.samples.len()];
        for (i, s) in self.samples.iter().enumerate() {
            if s.stream.is_some() {
                continue;
            }
            let start = (smpl.len() / 2) as u32;
            for v in s.pcm.iter().copied().chain(std::iter::repeat_n(0, 46)) {
                smpl.extend_from_slice(&((v >> 8) as i16).to_le_bytes());
                sm24.push((v & 0xFF) as u8);
            }
            placed[i] = (start, start + s.pcm.len() as u32);
        }
        for (i, s) in self.samples.iter().enumerate() {
            if let Some(st) = &s.stream {
                let start = smpl.len() as u32;
                smpl.extend_from_slice(st);
                placed[i] = (start, start + st.len().max(1) as u32 - 1);
            }
        }
        for (i, s) in self.samples.iter().enumerate() {
            let (start, end) = placed[i];
            let (ls, le) = if s.stream.is_some() {
                s.loop_points
            } else {
                (start + s.loop_points.0, start + s.loop_points.1)
            };
            shdr.extend_from_slice(&name20(&s.name));
            for v in [start, end, ls, le, s.rate] {
                shdr.extend_from_slice(&v.to_le_bytes());
            }
            shdr.push(s.pitch);
            shdr.push(s.correction as u8);
            shdr.extend_from_slice(&s.link.to_le_bytes());
            shdr.extend_from_slice(&s.kind.to_le_bytes());
        }
        shdr.extend_from_slice(&name20("EOS"));
        shdr.extend_from_slice(&[0; 26]);

        let izones: Vec<&[Sf2Zone]> = self.instruments.iter().map(|i| &i.zones[..]).collect();
        let (ibag, imod, igen, ifirst) = zone_lists(&izones);
        let mut inst = Vec::new();
        for (i, ins) in self.instruments.iter().enumerate() {
            inst.extend_from_slice(&name20(&ins.name));
            inst.extend_from_slice(&ifirst[i].to_le_bytes());
        }
        inst.extend_from_slice(&name20("EOI"));
        inst.extend_from_slice(&ifirst[self.instruments.len()].to_le_bytes());

        let pzones: Vec<&[Sf2Zone]> = self.presets.iter().map(|p| &p.zones[..]).collect();
        let (pbag, pmod, pgen, pfirst) = zone_lists(&pzones);
        let mut phdr = Vec::new();
        for (i, p) in self.presets.iter().enumerate() {
            phdr.extend_from_slice(&name20(&p.name));
            phdr.extend_from_slice(&p.program.to_le_bytes());
            phdr.extend_from_slice(&p.bank.to_le_bytes());
            phdr.extend_from_slice(&pfirst[i].to_le_bytes());
            phdr.extend_from_slice(&[0; 12]);
        }
        phdr.extend_from_slice(&name20("EOP"));
        phdr.extend_from_slice(&[0; 4]);
        phdr.extend_from_slice(&pfirst[self.presets.len()].to_le_bytes());
        phdr.extend_from_slice(&[0; 12]);

        Sf2Chunks {
            info,
            smpl,
            sm24: self.sm24.then_some(sm24),
            pad_smpl: self.pad_smpl,
            pdta: vec![
                (*b"phdr", phdr),
                (*b"pbag", pbag),
                (*b"pmod", pmod),
                (*b"pgen", pgen),
                (*b"inst", inst),
                (*b"ibag", ibag),
                (*b"imod", imod),
                (*b"igen", igen),
                (*b"shdr", shdr),
            ],
        }
    }

    pub fn build(&self) -> Vec<u8> {
        self.chunks().bytes()
    }
}
