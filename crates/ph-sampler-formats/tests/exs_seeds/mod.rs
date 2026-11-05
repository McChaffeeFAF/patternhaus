//! F-EXS-10 malformed EXS files, shared by `exs_fixtures.rs` and `fuzz.rs` (the fuzz seeds).

#![allow(dead_code)]

use ph_sampler_formats::build::{ExsBuilder, ExsGroup, ExsSample, ExsZone};

/// One zone, one group, one sample (`a.wav` beside the file), parameters.
pub fn minimal(big_endian: bool) -> ExsBuilder {
    let mut b = ExsBuilder::new("Minimal");
    b.big_endian = big_endian;
    b.groups.push(ExsGroup::new("Main"));
    b.samples
        .push(ExsSample::new("a.wav", "/Users/someone/Music/Samples"));
    b.zones.push(ExsZone::new(0, 0, (48, 72), 60));
    b.params = vec![(0x07, 0), (0x05, 16)];
    b
}

/// Files the reader must refuse (`ImportError::Malformed`).
pub fn rejected() -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();
    let mut b = minimal(false);
    b.magic = Some(*b"ABCD");
    out.push(("wrong magic", b.build()));
    let mut b = minimal(false);
    b.magic_at = Some((2, *b"SOBT"));
    out.push(("mixed magics", b.build()));
    let mut bytes = minimal(false).build();
    bytes.truncate(bytes.len() - 10);
    out.push(("size past the end", bytes));
    let mut b = minimal(false);
    b.zones.clear();
    let mut bytes = b.build();
    bytes[3] = 0x02; // the first block is not the instrument header
    out.push(("no instrument header first", bytes));
    out.push(("empty", Vec::new()));
    out.push(("short", b"\x01\x01\x00\x00TBOS".to_vec()));
    out
}

/// Files that import with notes: unknown block types, a zone whose sample index is out of
/// range, every index −1 (the object-id fallback).
pub fn accepted_oddities() -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();
    let mut b = minimal(false);
    b.extra = vec![
        (5, b"GAMETSPP\0\0\0\0".to_vec()),
        (7, vec![0; 4]),
        (8, vec![0; 4]),
        (0x0A, b"bplist00".to_vec()),
        (0x0B, b"<plist/>".to_vec()),
        (0x0C, vec![1, 2, 3]),
    ];
    let mut z = ExsZone::new(5, 0, (80, 90), 84);
    z.name = "Lost".into();
    b.zones.push(z);
    out.push(("unknown blocks and a bad sample index", b.build()));
    let mut b = minimal(false);
    b.zones[0].sample = -1;
    b.zones[0].group = -1;
    b.zones[0].id = 0;
    out.push(("every index -1: object-id fallback", b.build()));
    out
}

/// Every seed, for the fuzz target.
pub fn all() -> Vec<Vec<u8>> {
    let mut v = vec![minimal(false).build(), minimal(true).build()];
    let mut flag = minimal(true);
    flag.size_flag = true;
    v.push(flag.build());
    v.extend(rejected().into_iter().map(|(_, b)| b));
    v.extend(accepted_oddities().into_iter().map(|(_, b)| b));
    v
}
