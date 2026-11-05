//! `ExsBuilder`: writes EXS24 files from `docs/formats/exs.md` (§1–§6), in either byte
//! order. The layout is our spec's, unconfirmed on Logic's own files, so these files test
//! the reader's agreement with the spec (exs.md §10).

/// How a big-endian file stores the block signature (§1.1 [D?]).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BeSignature {
    /// `00 01 00 TT` (the per-byte reading).
    #[default]
    PerByte,
    /// `01 01 00 TT` (the reading of one community note).
    Tonv,
    /// `TT 00 01 01` (the little-endian u32 written big-endian).
    Reversed,
}

#[derive(Clone, Debug)]
pub struct ExsZone {
    pub name: String,
    pub options: u8,
    pub root: u8,
    pub fine: i8,
    pub pan: i8,
    pub volume: i8,
    pub keys: (u8, u8),
    pub vels: (u8, u8),
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
    /// Content size: 96, 104 (default) or more.
    pub size: usize,
    /// Header object id.
    pub id: u32,
}

impl ExsZone {
    pub fn new(sample: i32, group: i32, keys: (u8, u8), root: u8) -> Self {
        Self {
            name: format!("Zone {sample}"),
            options: 0,
            root,
            fine: 0,
            pan: 0,
            volume: 0,
            keys,
            vels: (0, 127),
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            loop_xfade_ms: 0,
            loop_tune: 0,
            loop_options: 0,
            loop_dir: 0,
            coarse: 0,
            output: 0,
            group,
            sample,
            size: 104,
            id: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExsGroup {
    pub name: String,
    pub volume: i8,
    pub pan: i8,
    pub polyphony: u8,
    pub options: u8,
    pub exclusive: u8,
    pub vels: (u8, u8),
    pub random_offset: u8,
    pub release_trigger: u8,
    pub output: u8,
    pub enable_note: u8,
    pub rr_position: i32,
    pub enable_by: u8,
    pub enable_cc: (u8, u8, u8),
    pub keys: (u8, u8),
    /// Content size: 76 or 92 (default).
    pub size: usize,
}

impl ExsGroup {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            volume: 0,
            pan: 0,
            polyphony: 0,
            options: 0,
            exclusive: 0,
            vels: (0, 127),
            random_offset: 0,
            release_trigger: 0,
            output: 0,
            enable_note: 0,
            rr_position: -1,
            enable_by: 0,
            enable_cc: (0, 0, 127),
            keys: (0, 127),
            size: 92,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExsSample {
    /// Header name (the display name; the file name in the 336-byte form).
    pub name: String,
    pub path: String,
    pub file_name: String,
    pub frames: u32,
    pub rate: u32,
    pub file_type: [u8; 4],
    pub compressed: bool,
    /// Content size: 336 or 592 (default).
    pub size: usize,
}

impl ExsSample {
    pub fn new(file_name: &str, path: &str) -> Self {
        Self {
            name: file_name.into(),
            path: path.into(),
            file_name: file_name.into(),
            frames: 0,
            rate: 48_000,
            file_type: *b"WAVE",
            compressed: false,
            size: 592,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExsBuilder {
    pub name: String,
    pub big_endian: bool,
    pub be_signature: BeSignature,
    /// Set the 0x80 flag and bit 15 of every size (big-endian variant, §1.3).
    pub size_flag: bool,
    /// Override the magic of every block (or of block `n` with `magic_at`).
    pub magic: Option<[u8; 4]>,
    pub magic_at: Option<(usize, [u8; 4])>,
    pub zones: Vec<ExsZone>,
    pub groups: Vec<ExsGroup>,
    pub samples: Vec<ExsSample>,
    /// Section 1 parameters (id < 256) and section 2 parameters (any id).
    pub params: Vec<(u8, i16)>,
    pub params2: Vec<(u16, u16)>,
    /// Extra blocks (type, content) appended after the parameters.
    pub extra: Vec<(u8, Vec<u8>)>,
    /// Instrument-header counts written as given instead of the real ones.
    pub counts: Option<[u32; 4]>,
}

struct W {
    big: bool,
    out: Vec<u8>,
}

impl W {
    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&if self.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&if self.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        });
    }
    fn at_u32(&mut self, at: usize, v: u32) {
        let b = if self.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        };
        self.out[at..at + 4].copy_from_slice(&b);
    }
}

fn put_str(buf: &mut [u8], s: &str) {
    for (i, c) in s.bytes().take(buf.len().saturating_sub(1)).enumerate() {
        buf[i] = c;
    }
}

impl ExsBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    fn block(&self, w: &mut W, n: usize, kind: u8, id: u32, name: &str, content: &[u8]) {
        let mut flags = 0u8;
        let mut size = content.len() as u32;
        if self.size_flag {
            flags |= 0x80;
            size |= 0x8000;
        }
        let t = kind | flags;
        let sig = if !self.big_endian {
            [1, 1, 0, t]
        } else {
            match self.be_signature {
                BeSignature::PerByte => [0, 1, 0, t],
                BeSignature::Tonv => [1, 1, 0, t],
                BeSignature::Reversed => [t, 0, 1, 1],
            }
        };
        w.out.extend_from_slice(&sig);
        w.u32(size);
        w.u32(id);
        w.u32(0);
        let mut magic = self
            .magic
            .unwrap_or(if self.big_endian { *b"SOBT" } else { *b"TBOS" });
        if let Some((at, m)) = self.magic_at
            && at == n
        {
            magic = m;
        }
        w.out.extend_from_slice(&magic);
        let mut nm = [0u8; 64];
        put_str(&mut nm, name);
        w.out.extend_from_slice(&nm);
        w.out.extend_from_slice(content);
    }

    pub fn build(&self) -> Vec<u8> {
        let big = self.big_endian;
        let mut w = W {
            big,
            out: Vec::new(),
        };
        let n_params = (!self.params.is_empty() || !self.params2.is_empty()) as u32;
        let counts = self.counts.unwrap_or([
            self.zones.len() as u32,
            self.groups.len() as u32,
            self.samples.len() as u32,
            n_params,
        ]);
        let mut c = W {
            big,
            out: Vec::new(),
        };
        c.u32(0);
        for v in counts {
            c.u32(v);
        }
        c.out.resize(40, 0);
        let mut n = 0;
        self.block(&mut w, n, 0, 0, &self.name, &c.out);
        n += 1;
        for (i, z) in self.zones.iter().enumerate() {
            let mut c = W {
                big,
                out: vec![0; z.size.max(96)],
            };
            c.out[0] = z.options;
            c.out[1] = z.root;
            c.out[2] = z.fine as u8;
            c.out[3] = z.pan as u8;
            c.out[4] = z.volume as u8;
            c.out[6] = z.keys.0;
            c.out[7] = z.keys.1;
            c.out[9] = z.vels.0;
            c.out[10] = z.vels.1;
            c.at_u32(12, z.start);
            c.at_u32(16, z.end);
            c.at_u32(20, z.loop_start);
            c.at_u32(24, z.loop_end);
            c.at_u32(28, z.loop_xfade_ms);
            c.out[32] = z.loop_tune;
            c.out[33] = z.loop_options;
            c.out[34] = z.loop_dir;
            c.out[80] = z.coarse as u8;
            c.out[82] = z.output;
            c.at_u32(88, z.group as u32);
            c.at_u32(92, z.sample as u32);
            let id = if z.id != 0 { z.id } else { i as u32 };
            self.block(&mut w, n, 1, id, &z.name, &c.out);
            n += 1;
        }
        for (i, g) in self.groups.iter().enumerate() {
            let mut c = W {
                big,
                out: vec![0; g.size.max(76)],
            };
            c.out[0] = g.volume as u8;
            c.out[1] = g.pan as u8;
            c.out[2] = g.polyphony;
            c.out[3] = g.options;
            c.out[4] = g.exclusive;
            c.out[5] = g.vels.0;
            c.out[6] = g.vels.1;
            c.out[7] = g.random_offset;
            c.out[32] = 128;
            c.out[35] = 128;
            c.out[38] = 80;
            c.out[39] = 140;
            c.out[73] = g.release_trigger;
            c.out[74] = g.output;
            c.out[75] = g.enable_note;
            if c.out.len() >= 92 {
                c.at_u32(80, g.rr_position as u32);
                c.out[84] = g.enable_by;
                c.out[85] = g.enable_cc.0;
                c.out[86] = g.enable_cc.1;
                c.out[87] = g.enable_cc.2;
                c.out[88] = g.keys.0;
                c.out[89] = g.keys.1;
            }
            self.block(&mut w, n, 2, i as u32, &g.name, &c.out);
            n += 1;
        }
        for (i, s) in self.samples.iter().enumerate() {
            let mut c = W {
                big,
                out: vec![0; s.size.max(336)],
            };
            c.at_u32(4, s.frames);
            c.at_u32(8, s.rate);
            c.at_u32(12, 16);
            c.at_u32(16, 1);
            c.at_u32(20, 1);
            c.out[28..32].copy_from_slice(&s.file_type);
            c.at_u32(36, s.compressed as u32);
            put_str(&mut c.out[80..336], &s.path);
            if c.out.len() >= 592 {
                put_str(&mut c.out[336..592], &s.file_name);
            }
            let name = if s.size < 592 { &s.file_name } else { &s.name };
            self.block(&mut w, n, 3, i as u32, name, &c.out);
            n += 1;
        }
        if n_params > 0 {
            let mut c = W {
                big,
                out: Vec::new(),
            };
            c.u32(100);
            let mut ids = [0u8; 100];
            let mut vals = [0i16; 100];
            for (i, (id, v)) in self.params.iter().take(100).enumerate() {
                ids[i] = *id;
                vals[i] = *v;
            }
            c.out.extend_from_slice(&ids);
            for v in vals {
                c.u16(v as u16);
            }
            if !self.params2.is_empty() {
                c.u32(200);
                for i in 0..200 {
                    let (id, v) = self.params2.get(i).copied().unwrap_or((0, 0));
                    c.u16(id);
                    c.u16(v);
                }
            }
            self.block(&mut w, n, 4, 0, "Parameters", &c.out);
            n += 1;
        }
        for (kind, content) in &self.extra {
            self.block(&mut w, n, *kind, 0, "Extra", content);
            n += 1;
        }
        w.out
    }
}
