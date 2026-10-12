//! Tests on tiny FLP byte sequences built here, never on real project files.

use flp_probe::{
    Payload, ProbeError, SizeClass, Stats, TextGuess, dump_line, guess_text, parse, read_varint,
    version, write_varint,
};

/// Builds a file: `FLhd` with a 6-byte body, then `FLdt` holding `events`.
fn file(channels: u16, ppq: u16, events: &[u8]) -> Vec<u8> {
    let mut b = b"FLhd".to_vec();
    b.extend(6u32.to_le_bytes());
    b.extend(0i16.to_le_bytes());
    b.extend(channels.to_le_bytes());
    b.extend(ppq.to_le_bytes());
    b.extend(b"FLdt");
    b.extend((events.len() as u32).to_le_bytes());
    b.extend(events);
    b
}

fn var(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut e = vec![id];
    write_varint(payload.len() as u32, &mut e);
    e.extend(payload);
    e
}

fn utf16z(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    v.extend([0, 0]);
    v
}

#[test]
fn size_class_boundaries() {
    assert_eq!(SizeClass::of(0), SizeClass::Byte);
    assert_eq!(SizeClass::of(63), SizeClass::Byte);
    assert_eq!(SizeClass::of(64), SizeClass::Word);
    assert_eq!(SizeClass::of(127), SizeClass::Word);
    assert_eq!(SizeClass::of(128), SizeClass::Dword);
    assert_eq!(SizeClass::of(191), SizeClass::Dword);
    assert_eq!(SizeClass::of(192), SizeClass::Var);
    assert_eq!(SizeClass::of(255), SizeClass::Var);
}

#[test]
fn header_fields() {
    let b = file(3, 96, &[]);
    let flp = parse(&b).unwrap();
    assert_eq!(flp.header.length, 6);
    assert_eq!(flp.header.format, 0);
    assert_eq!(flp.header.channels, 3);
    assert_eq!(flp.header.ppq, 96);
    assert!(flp.events.is_empty());
    assert_eq!(flp.trailing, 0);
}

#[test]
fn each_fixed_size_class_is_little_endian() {
    let mut ev = vec![9, 1]; // byte
    ev.extend([67, 0x34, 0x12]); // word
    ev.push(156);
    ev.extend(140_000u32.to_le_bytes()); // dword
    let b = file(0, 96, &ev);
    let flp = parse(&b).unwrap();
    let got: Vec<(u8, Payload)> = flp.events.iter().map(|e| (e.id, e.payload)).collect();
    assert_eq!(
        got,
        vec![
            (9, Payload::Fixed(1)),
            (67, Payload::Fixed(0x1234)),
            (156, Payload::Fixed(140_000))
        ]
    );
    assert_eq!(flp.events[1].offset, 22 + 2);
    assert_eq!(flp.events[2].offset, 22 + 5);
}

#[test]
fn varint_round_trip_and_multi_byte() {
    for n in [0u32, 1, 127, 128, 300, 16_383, 16_384, 2_755_425, u32::MAX] {
        let mut v = Vec::new();
        write_varint(n, &mut v);
        assert_eq!(read_varint(&v, 0).unwrap(), (n, v.len()));
    }
    let mut v = Vec::new();
    write_varint(300, &mut v);
    assert_eq!(v, vec![0xac, 0x02]);
    assert_eq!(
        read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 0),
        Err(ProbeError::BadVarint { offset: 0 })
    );
}

#[test]
fn variable_event_with_long_payload() {
    let payload = vec![0xabu8; 200];
    let ev = var(213, &payload);
    assert_eq!(&ev[1..3], &[0xc8, 0x01]);
    let b = file(0, 96, &ev);
    let flp = parse(&b).unwrap();
    assert_eq!(flp.events.len(), 1);
    assert_eq!(flp.events[0].payload, Payload::Bytes(&payload));
}

#[test]
fn text_guesses() {
    assert_eq!(guess_text(&utf16z("Kick")), TextGuess::Utf16("Kick".into()));
    assert_eq!(guess_text(&[0, 0]), TextGuess::Utf16(String::new()));
    assert_eq!(
        guess_text(b"21.2.2.3914\0"),
        TextGuess::Ascii("21.2.2.3914".into())
    );
    assert_eq!(guess_text(&[1, 2, 3, 4]), TextGuess::Binary);
    assert_eq!(guess_text(&[0u8; 8]), TextGuess::Binary);
}

#[test]
fn version_and_stats() {
    let mut ev = var(199, b"21.2.2.3914\0");
    ev.extend(var(194, &utf16z("t")));
    ev.extend([64, 0, 0, 21, 2, 64, 1, 0, 21, 4]);
    let b = file(2, 96, &ev);
    let flp = parse(&b).unwrap();
    assert_eq!(version(&flp).as_deref(), Some("21.2.2.3914"));
    let mut s = Stats::default();
    s.add(&flp);
    s.add(&flp);
    s.add_failure();
    assert_eq!(s.files, 2);
    assert_eq!(s.failed, 1);
    assert_eq!(s.versions.get("21"), Some(&2));
    assert_eq!(s.ppq.get(&96), Some(&2));
    let ch = &s.ids[&21];
    assert_eq!(
        (ch.count, ch.files, ch.min, ch.max, ch.distinct),
        (4, 2, Some(2), Some(4), 2)
    );
    let t = &s.ids[&194];
    assert_eq!((t.utf16, t.min_len), (2, Some(4)));
    assert_eq!(s.ids[&199].ascii, 2);
    let r = s.report();
    assert!(r.contains("files 2 (failed 1)"));
    assert!(!r.contains("3914"), "the report carries no file content");
}

#[test]
fn errors() {
    assert!(matches!(parse(b"RIFF"), Err(ProbeError::BadMagic { .. })));
    assert!(matches!(parse(b"FLhd"), Err(ProbeError::Truncated { .. })));
    let mut short = file(0, 96, &[]);
    short[4] = 2;
    assert_eq!(parse(&short).unwrap_err(), ProbeError::ShortHeader(2));
    // Data chunk claims more bytes than the file holds.
    let mut b = file(0, 96, &[9, 1]);
    let n = b.len();
    b[n - 6] = 9;
    assert!(matches!(parse(&b), Err(ProbeError::Truncated { .. })));
    // A word event cut off by the end of the data chunk.
    assert!(matches!(
        parse(&file(0, 96, &[67, 1])),
        Err(ProbeError::Truncated { .. })
    ));
    // A variable payload longer than the chunk.
    assert!(matches!(
        parse(&file(0, 96, &[213, 5, 1, 2])),
        Err(ProbeError::Truncated { .. })
    ));
}

#[test]
fn trailing_bytes_are_counted() {
    let mut b = file(0, 96, &[9, 1]);
    b.extend([0, 0, 0]);
    assert_eq!(parse(&b).unwrap().trailing, 3);
}

#[test]
fn dump_lines() {
    let ev = [var(203, &utf16z("Pad")), var(213, &[1, 2, 3])].concat();
    let b = file(0, 96, &[ev.as_slice(), &[22, 7]].concat());
    let flp = parse(&b).unwrap();
    let lines: Vec<String> = flp.events.iter().map(|e| dump_line(e, 2)).collect();
    assert!(lines[0].ends_with("203 utf16 \"Pad\""));
    assert!(lines[1].ends_with("213 [3] 01 02 .."));
    assert!(lines[2].ends_with("22 = 7 (0x7)"));
}
