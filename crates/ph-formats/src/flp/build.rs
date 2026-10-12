//! A writer for synthetic FLP files: the events of spec section 1, assembled from calls. Used
//! by the tests, the fuzz seeds and the other crates' tests (no real project is ever
//! committed). Not a faithful FL writer: it emits only what a test asks for.

/// Builds a file event by event.
pub struct FlpBuilder {
    ppq: u16,
    data: Vec<u8>,
    channels: u16,
}

fn varint(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// UTF-16LE with a two-byte NUL (spec 1.3).
pub fn utf16(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    v.extend([0, 0]);
    v
}

impl FlpBuilder {
    pub fn new(ppq: u16) -> Self {
        Self {
            ppq,
            data: Vec::new(),
            channels: 0,
        }
    }

    pub fn u8(&mut self, id: u8, v: u8) -> &mut Self {
        assert!(id < 64);
        self.data.extend([id, v]);
        self
    }

    pub fn u16(&mut self, id: u8, v: u16) -> &mut Self {
        assert!((64..128).contains(&id));
        self.data.push(id);
        self.data.extend(v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, id: u8, v: u32) -> &mut Self {
        assert!((128..192).contains(&id));
        self.data.push(id);
        self.data.extend(v.to_le_bytes());
        self
    }

    pub fn bytes(&mut self, id: u8, b: &[u8]) -> &mut Self {
        assert!(id >= 192);
        self.data.push(id);
        varint(b.len(), &mut self.data);
        self.data.extend(b);
        self
    }

    /// A UTF-16 text event.
    pub fn text(&mut self, id: u8, s: &str) -> &mut Self {
        self.bytes(id, &utf16(s))
    }

    /// The FL version event (ASCII, NUL-terminated).
    pub fn version(&mut self, v: &str) -> &mut Self {
        let mut b = v.as_bytes().to_vec();
        b.push(0);
        self.bytes(199, &b)
    }

    /// Opens a channel block (`64 iid`) with its type.
    pub fn channel(&mut self, iid: u16, kind: u8) -> &mut Self {
        self.channels += 1;
        self.u16(64, iid).u8(21, kind)
    }

    /// One 24-byte note record (spec 5.2) with the default fields.
    pub fn note_record(
        position: u32,
        channel: u16,
        length: u32,
        key: u16,
        velocity: u8,
    ) -> Vec<u8> {
        let mut r = Vec::with_capacity(24);
        r.extend(position.to_le_bytes());
        r.extend(0x4000u16.to_le_bytes());
        r.extend(channel.to_le_bytes());
        r.extend(length.to_le_bytes());
        r.extend(key.to_le_bytes());
        r.extend(0u16.to_le_bytes());
        r.extend([120, 0, 64, 0, 64, velocity, 128, 128]);
        r
    }

    /// One 60-byte (FL 21) playlist item (spec 7.2). `index` is the raw item index
    /// (`0x5000 + pattern id` for a pattern clip); `track` the 0-based track number.
    pub fn item_record(position: u32, index: u16, length: u32, track: u32) -> Vec<u8> {
        let mut r = Vec::with_capacity(60);
        r.extend(position.to_le_bytes());
        r.extend(0x5000u16.to_le_bytes());
        r.extend(index.to_le_bytes());
        r.extend(length.to_le_bytes());
        r.extend((499 - track).to_le_bytes());
        r.extend(120u16.to_le_bytes());
        r.extend(0x0040u16.to_le_bytes());
        r.extend(0x6440u16.to_le_bytes());
        r.extend(0x8080u16.to_le_bytes());
        r.extend(u32::MAX.to_le_bytes()); // start offset: not trimmed
        r.extend(u32::MAX.to_le_bytes());
        r.resize(60, 0);
        r
    }

    /// A 12-byte mixer parameter record (spec 6.5).
    pub fn mixer_record(insert: u8, slot: u8, param: u8, value: i32) -> Vec<u8> {
        let loc: u16 = 0x2000 | (u16::from(insert) << 6) | u16::from(slot);
        let mut r = vec![0, 0, 0, 0, param, 0x1f];
        r.extend(loc.to_le_bytes());
        r.extend(value.to_le_bytes());
        r
    }

    /// An event-234 payload with `points` as (delta quarter notes, value, tension).
    pub fn automation_payload(points: &[(f64, f64, f32)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(1u32.to_le_bytes());
        b.extend(64u32.to_le_bytes());
        b.push(0);
        b.extend(4u32.to_le_bytes());
        b.extend(3u32.to_le_bytes());
        b.extend((points.len() as u32).to_le_bytes());
        for (d, v, t) in points {
            b.extend(d.to_le_bytes());
            b.extend(v.to_le_bytes());
            b.extend(t.to_le_bytes());
            b.extend(0u32.to_le_bytes());
        }
        b.extend([0u8; 112]);
        b
    }

    /// A VST wrapper state (spec 8.2) with the given records.
    pub fn wrapper_state(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = 10u32.to_le_bytes().to_vec();
        for (id, body) in records {
            b.extend(id.to_le_bytes());
            b.extend((body.len() as u64).to_le_bytes());
            b.extend(body);
        }
        b
    }

    /// The finished file: header chunk then data chunk.
    pub fn finish(&self) -> Vec<u8> {
        let mut out = b"FLhd".to_vec();
        out.extend(6u32.to_le_bytes());
        out.extend(0i16.to_le_bytes());
        out.extend(self.channels.to_le_bytes());
        out.extend(self.ppq.to_le_bytes());
        out.extend(b"FLdt");
        out.extend((self.data.len() as u32).to_le_bytes());
        out.extend(&self.data);
        out
    }
}

/// A small synthetic project (FL 21 layout, PPQ 96) that exercises every mapped feature:
/// tempo 123.456, a sampler, a VST3 and a native generator, an audio-clip channel, an
/// automation clip of insert 2's volume, two patterns, three playlist tracks, a send from
/// insert 1 to insert 2 and a native effect on insert 1. Nothing in it comes from a real
/// FL project.
pub fn sample_project() -> Vec<u8> {
    let mut b = FlpBuilder::new(96);
    b.version("21.2.2.3914")
        .u32(156, 123_456)
        .u8(17, 4)
        .u8(18, 4)
        .text(194, "Synthetic song")
        .text(207, "Test Author");
    // automation target for channel 3: insert 2 volume (param 192)
    let mut t = vec![0u8; 20];
    t[2..4].copy_from_slice(&3u16.to_le_bytes());
    let target: u32 = ((0x2000u32 | (2 << 6)) << 16) | 0x1f00 | 192;
    t[8..12].copy_from_slice(&target.to_le_bytes());
    b.bytes(227, &t);
    // pattern 1 notes: two on channel 0, one on channel 1
    let mut notes = FlpBuilder::note_record(0, 0, 96, 60, 100);
    notes.extend(FlpBuilder::note_record(96, 0, 48, 64, 64));
    notes.extend(FlpBuilder::note_record(192, 1, 0, 72, 128));
    b.u16(65, 1).bytes(224, &notes);
    // channels
    let mut lv = Vec::new();
    for v in [3200i32, 8000, 0, 0, 256, 0] {
        lv.extend(v.to_le_bytes());
    }
    b.channel(0, 0)
        .text(201, "")
        .text(203, "Kick")
        .u32(128, 0x00ff_8040)
        .u8(0, 1)
        .u8(22, 1)
        .bytes(219, &lv)
        .text(196, r"%FLStudioFactoryData%\Packs\Kick.wav");
    let vst3 = FlpBuilder::wrapper_state(&[
        (52, (1..=16).collect()),
        (53, vec![1, 0, 0, 0, 9, 9, 9]),
        (54, b"Super Synth".to_vec()),
    ]);
    b.channel(1, 2)
        .text(201, "Fruity Wrapper")
        .bytes(213, &vst3)
        .u8(0, 0)
        .u8(22, 2)
        .bytes(219, &lv);
    b.channel(2, 4)
        .text(201, "")
        .text(196, r"C:\Audio\loop.wav");
    b.channel(3, 5).text(201, "").bytes(
        234,
        &FlpBuilder::automation_payload(&[(0.0, 1.0, 0.0), (4.0, 0.5, 0.25)]),
    );
    b.channel(4, 2)
        .text(201, "3x Osc")
        .bytes(213, &[7, 7, 7, 7])
        .u8(22, 1);
    // pattern metadata
    b.u16(65, 1).text(193, "Beat").u32(150, 0x0000_00ff);
    b.u16(65, 2).text(193, "Empty pattern");
    // arrangement
    b.u16(99, 0).text(241, "Arrangement");
    let mut items = FlpBuilder::item_record(0, 0x5001, 384, 0);
    items.extend(FlpBuilder::item_record(384, 0x5001, 384, 2));
    items.extend(FlpBuilder::item_record(768, 2, 1920, 1)); // audio clip of channel 2
    items.extend(FlpBuilder::item_record(0, 3, 768, 3)); // automation clip of channel 3
    b.bytes(233, &items);
    for i in 1..=3u32 {
        let mut tr = vec![0u8; 66];
        tr[0..4].copy_from_slice(&i.to_le_bytes());
        tr[4..8].copy_from_slice(&0x0056_5148u32.to_le_bytes());
        tr[12] = 1;
        b.bytes(238, &tr).u8(43, 0);
        if i == 1 {
            b.text(239, "Drums");
        }
    }
    // mixer: master, insert 1 (named, routed to 2 and master, one native effect), insert 2
    let mut routing = vec![0u8; 127];
    routing[0] = 1;
    routing[2] = 1;
    let mut master_route = vec![0u8; 127];
    master_route[0] = 0;
    let mut two_route = vec![0u8; 127];
    two_route[0] = 1;
    let flags = [0u8, 0, 0, 0, 0x4c, 0, 0, 0, 0, 0, 0, 0];
    b.bytes(236, &flags).u16(98, 0).bytes(235, &master_route);
    b.u32(149, 0x0000_00ff)
        .text(204, "Drum bus")
        .bytes(236, &flags)
        .text(201, "Fruity Parametric EQ 2")
        .bytes(213, &[1, 2, 3])
        .u16(98, 0)
        .u16(98, 1)
        .bytes(235, &routing);
    b.bytes(236, &flags).bytes(235, &two_route);
    let mut params = FlpBuilder::mixer_record(1, 0, 192, 6400);
    params.extend(FlpBuilder::mixer_record(1, 0, 193, -3200));
    params.extend(FlpBuilder::mixer_record(0, 0, 192, 12800));
    params.extend(FlpBuilder::mixer_record(1, 0, 0, 0)); // slot 0 disabled
    params.extend(FlpBuilder::mixer_record(1, 0, 1, 6400)); // slot 0 mix 50%
    b.bytes(225, &params);
    b.finish()
}
