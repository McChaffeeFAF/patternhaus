//! F-SF2-12 malformed SoundFonts, shared by `sf2_fixtures.rs` and `fuzz.rs` (the fuzz seeds):
//! files the reader must refuse, and files that break ignore-rules but still import.

#![allow(dead_code)]

use ph_sampler_formats::build::{Sf2Builder, Sf2Sample, Sf2Zone, tagged_tone};

fn base() -> Sf2Builder {
    let mut b = Sf2Builder::new();
    b.add_sample(Sf2Sample::from_f32(
        "a",
        &tagged_tone(1, 220.0, 4_000, 44_100),
        44_100,
        60,
    ));
    let i = b.add_instrument(
        "Two",
        vec![
            Sf2Zone::new().keys(0, 63).sample(0),
            Sf2Zone::new().keys(64, 127).sample(0),
        ],
    );
    b.add_preset("Two", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    b
}

/// Files that break ignore-rules (§4.4, §6.3, §4.1) but import: keyRange not first, a zone
/// without its terminal generator, unknown generator 70, a ROM sample (with `irom`), a
/// duplicate preset number.
pub fn accepted_oddities() -> Vec<u8> {
    let mut b = Sf2Builder::new();
    b.rom = Some("TESTROM".into());
    b.add_sample(Sf2Sample::from_f32(
        "a",
        &tagged_tone(1, 220.0, 12_000, 44_100),
        44_100,
        60,
    ));
    let mut rom = Sf2Sample::from_f32("rom", &[0.0; 100], 44_100, 60);
    rom.kind = 0x8001;
    b.add_sample(rom);
    let i = b.add_instrument(
        "Odd",
        vec![
            Sf2Zone::new().set(48, 10).set(70, 5).keys(60, 72).sample(0),
            Sf2Zone::new().set(17, 100),
            Sf2Zone::new().keys(0, 10).sample(1),
        ],
    );
    b.add_preset("Odd", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    b.add_preset("Odd copy", 0, 0, vec![Sf2Zone::new().instrument(i)]);
    b.build()
}

/// A ROM sample without `irom`: defective, the zone is skipped and the file still imports.
pub fn rom_without_irom() -> Vec<u8> {
    let mut b = base();
    b.samples[0].kind = 0x8001;
    let s = b.add_sample(Sf2Sample::from_f32(
        "b",
        &tagged_tone(2, 330.0, 4_000, 44_100),
        44_100,
        60,
    ));
    b.instruments[0]
        .zones
        .push(Sf2Zone::new().keys(64, 127).sample(s));
    b.build()
}

/// Files the reader must refuse (`ImportError::Malformed`).
pub fn rejected() -> Vec<(&'static str, Vec<u8>)> {
    let ok = base();
    let mut out = Vec::new();
    let mut c = ok.chunks();
    c.pdta_mut(b"pmod").pop();
    out.push(("pmod not a multiple of 10", c.bytes()));
    let mut c = ok.chunks();
    c.pdta_mut(b"ibag")[4..6].copy_from_slice(&9u16.to_le_bytes());
    out.push(("ibag not monotonic", c.bytes()));
    let mut c = ok.chunks();
    c.pdta.retain(|(id, _)| id != b"imod");
    out.push(("imod missing", c.bytes()));
    let mut c = ok.chunks();
    let phdr = c.pdta_mut(b"phdr");
    let at = phdr.len() - 38 + 24;
    phdr[at..at + 2].copy_from_slice(&7u16.to_le_bytes());
    out.push(("phdr terminal does not match pbag", c.bytes()));
    let mut c = ok.chunks();
    c.info[0].1.push(0);
    out.push(("ifil of 5 bytes", c.bytes()));
    let mut c = ok.chunks();
    c.info.remove(0);
    out.push(("no ifil", c.bytes()));
    let mut bad = ok.clone();
    bad.presets[0].zones[0] = Sf2Zone::new().instrument(5);
    out.push(("instrument index past the terminal", bad.build()));
    let mut bad = ok.clone();
    bad.instruments[0].zones[0] = Sf2Zone::new().sample(3);
    out.push(("sample index past the terminal", bad.build()));
    let mut bytes = ok.build();
    bytes.truncate(bytes.len() - 30);
    out.push(("truncated in shdr", bytes));
    out.push(("not RIFF", b"RIFX\x10\x00\x00\x00sfbk".to_vec()));
    out.push(("empty", Vec::new()));
    out
}

/// Every seed, for the fuzz target.
pub fn all() -> Vec<Vec<u8>> {
    let mut v = vec![base().build(), accepted_oddities(), rom_without_irom()];
    v.extend(rejected().into_iter().map(|(_, b)| b));
    v
}
