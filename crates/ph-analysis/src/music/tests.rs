//! Known-answer tests: generated material with a known key and known chords.

use super::*;
use ph_core::{ChannelId, TimeSignature};
use std::f64::consts::PI;

fn timing() -> Timing {
    Timing::new(120.0, TimeSignature::default())
}

fn n(key: u8, start: Tick, length: Tick) -> SongNote {
    SongNote {
        channel: ChannelId(0),
        key,
        start,
        length,
        velocity: 0.8,
    }
}

/// Chord quality intervals (for building test material independently of the templates).
fn ivs(q: &str) -> Vec<u8> {
    match q {
        "" => vec![0, 4, 7],
        "m" => vec![0, 3, 7],
        "dim" => vec![0, 3, 6],
        "aug" => vec![0, 4, 8],
        "sus2" => vec![0, 2, 7],
        "sus4" => vec![0, 5, 7],
        "7" => vec![0, 4, 7, 10],
        "maj7" => vec![0, 4, 7, 11],
        "m7" => vec![0, 3, 7, 10],
        "m7b5" => vec![0, 3, 6, 10],
        "dim7" => vec![0, 3, 6, 9],
        _ => panic!("{q}"),
    }
}

/// One bar of a chord: bass root at octave 2, upper voices at octave 4.
fn chord_bar(bar: u64, root: u8, q: &str) -> Vec<SongNote> {
    let t = timing();
    let start = bar * t.ticks_per_bar();
    let len = t.ticks_per_bar();
    let mut v = vec![n(36 + root, start, len)];
    for i in ivs(q) {
        v.push(n(60 + root + i, start, len));
    }
    v
}

#[test]
fn note_names() {
    assert_eq!(note_name(60), "C4");
    assert_eq!(note_name(69), "A4");
    assert_eq!(note_name(61), "C#4");
    assert_eq!(note_name(0), "C-1");
    assert_eq!(parse_note_name("C4"), Some(60));
    assert_eq!(parse_note_name("f#3"), Some(54));
    assert_eq!(parse_note_name("Bb2"), Some(46));
    assert_eq!(parse_note_name("H4"), None);
    for k in 0..=127u8 {
        assert_eq!(parse_note_name(&note_name(k)), Some(k));
    }
}

#[test]
fn every_chord_quality_on_every_root_is_named_exactly() {
    let t = timing();
    for q in [
        "", "m", "dim", "aug", "sus2", "sus4", "7", "maj7", "m7", "m7b5", "dim7",
    ] {
        for root in 0..12u8 {
            let notes = chord_bar(0, root, q);
            let c = chord_in_window(&notes, 0, t.ticks_per_bar()).unwrap();
            assert_eq!(
                c.symbol,
                format!("{}{q}", pc_name(root as usize)),
                "root {root} quality {q:?}"
            );
        }
    }
}

#[test]
fn inversions_get_a_slash_bass() {
    let t = timing();
    let bar = t.ticks_per_bar();
    // C/E: E in the bass under C and G.
    let c_e = vec![n(40, 0, bar), n(60, 0, bar), n(67, 0, bar)];
    assert_eq!(chord_in_window(&c_e, 0, bar).unwrap().symbol, "C/E");
    // C/G
    let c_g = vec![n(43, 0, bar), n(60, 0, bar), n(64, 0, bar)];
    assert_eq!(chord_in_window(&c_g, 0, bar).unwrap().symbol, "C/G");
    // Am/C
    let a_c = vec![n(48, 0, bar), n(57, 0, bar), n(64, 0, bar)];
    assert_eq!(chord_in_window(&a_c, 0, bar).unwrap().symbol, "Am/C");
    // G7/B
    let g7_b = vec![n(47, 0, bar), n(55, 0, bar), n(62, 0, bar), n(65, 0, bar)];
    assert_eq!(chord_in_window(&g7_b, 0, bar).unwrap().symbol, "G7/B");
}

#[test]
fn passing_notes_do_not_change_the_chord() {
    let t = timing();
    let bar = t.ticks_per_bar();
    let mut v = chord_bar(0, 9, "m");
    v.push(n(62, 0, 120)); // a short D passing through Am
    assert_eq!(chord_in_window(&v, 0, bar).unwrap().symbol, "Am");
}

#[test]
fn power_chords_and_silence() {
    let bar = timing().ticks_per_bar();
    let p = vec![n(40, 0, bar), n(47, 0, bar)];
    assert_eq!(chord_in_window(&p, 0, bar).unwrap().symbol, "E5");
    assert!(chord_in_window(&[], 0, bar).is_none());
}

fn progression_bars(root: u8, minor: bool, which: usize) -> Vec<(u8, &'static str)> {
    // (semitones above the tonic, quality)
    let degrees: &[(u8, &str)] = match (minor, which) {
        (false, 0) => &[(0, ""), (7, ""), (9, "m"), (5, "")],
        (false, 1) => &[(0, ""), (9, "m"), (5, ""), (7, "")],
        (false, _) => &[(0, "maj7"), (5, "maj7"), (2, "m7"), (7, "7")],
        (true, 0) => &[(0, "m"), (8, ""), (3, ""), (10, "")],
        (true, 1) => &[(0, "m"), (5, "m"), (7, "m"), (0, "m")],
        (true, _) => &[(0, "m"), (5, "m"), (7, "7"), (0, "m")],
    };
    degrees.iter().map(|(d, q)| ((root + d) % 12, *q)).collect()
}

fn progression_notes(chords: &[(u8, &str)]) -> Vec<SongNote> {
    chords
        .iter()
        .enumerate()
        .flat_map(|(i, (r, q))| chord_bar(i as u64, *r, q))
        .collect()
}

#[test]
fn key_and_chords_from_notes_are_exact() {
    let t = timing();
    let (mut keys_ok, mut keys_total, mut chords_ok, mut chords_total) = (0, 0, 0, 0);
    for minor in [false, true] {
        for which in 0..3 {
            for tonic in 0..12u8 {
                let prog = progression_bars(tonic, minor, which);
                let notes = progression_notes(&prog);
                let key = key_from_notes(&notes, &t).unwrap();
                keys_total += 1;
                let want = format!(
                    "{} {}",
                    pc_name(tonic as usize),
                    if minor { "minor" } else { "major" }
                );
                if key.key == want {
                    keys_ok += 1;
                } else {
                    eprintln!(
                        "key miss: wanted {want}, got {} ({:?})",
                        key.key, key.alternatives
                    );
                }
                let spans = chords_from_notes(&notes, &t, 1, 4, ChordGrain::Auto);
                for (i, (r, q)) in prog.iter().enumerate() {
                    chords_total += 1;
                    if spans.get(i).map(|s| s.symbol.clone())
                        == Some(format!("{}{q}", pc_name(*r as usize)))
                    {
                        chords_ok += 1;
                    }
                }
            }
        }
    }
    eprintln!("notes: key {keys_ok}/{keys_total}, chords {chords_ok}/{chords_total}");
    assert_eq!(chords_ok, chords_total);
    assert_eq!(keys_ok, keys_total);
}

#[test]
fn progression_string_and_split_bars() {
    let t = timing();
    let bar = t.ticks_per_bar();
    // Bar 1: Am for two beats, F for two beats; bar 2: C all bar.
    let mut notes = vec![];
    for (i, (root, q)) in [(9u8, "m"), (5, "")].iter().enumerate() {
        let start = i as u64 * bar / 2;
        notes.push(n(36 + root, start, bar / 2));
        for iv in ivs(q) {
            notes.push(n(60 + root + iv, start, bar / 2));
        }
    }
    notes.extend(chord_bar(1, 0, ""));
    let spans = chords_from_notes(&notes, &t, 1, 2, ChordGrain::Auto);
    assert_eq!(progression(&spans), "Am - F - C");
    assert_eq!((spans[1].bar, spans[1].beat), (1, 3));
    let bars = chords_from_notes(&notes, &t, 1, 2, ChordGrain::Bar);
    assert_eq!(bars.len(), 2);
    let beats = chords_from_notes(&notes, &t, 1, 1, ChordGrain::Beat);
    assert_eq!(beats.len(), 4);
}

// -- audio ------------------------------------------------------------------------------

/// Harmonic tones (a saw-ish spectrum, 1/h amplitudes up to the 6th) for each chord bar.
fn render(notes: &[SongNote], sr: u32, seconds: f64) -> Vec<Vec<f32>> {
    let t = timing();
    let n_samples = (seconds * sr as f64) as usize;
    let mut out = vec![0.0f32; n_samples];
    for note in notes {
        let f0 = 440.0 * 2f64.powf((note.key as f64 - 69.0) / 12.0);
        let s0 = (t.ticks_to_seconds(note.start) * sr as f64) as usize;
        let s1 = ((t.ticks_to_seconds(note.start + note.length)) * sr as f64) as usize;
        for (i, o) in out.iter_mut().enumerate().take(s1.min(n_samples)).skip(s0) {
            let time = i as f64 / sr as f64;
            let mut v = 0.0;
            for h in 1..=6 {
                let f = f0 * h as f64;
                if f < sr as f64 / 2.0 {
                    v += (2.0 * PI * f * time).sin() / h as f64;
                }
            }
            *o += (0.1 * v) as f32;
        }
    }
    vec![out.clone(), out]
}

#[test]
fn key_and_chords_from_audio_accuracy() {
    let t = timing();
    let sr = 44_100;
    let (mut keys_ok, mut keys_total, mut chords_ok, mut chords_total) = (0, 0, 0, 0);
    for minor in [false, true] {
        for which in 0..2 {
            for tonic in (0..12u8).step_by(1) {
                let prog = progression_bars(tonic, minor, which);
                let audio = render(&progression_notes(&prog), sr, 8.0);
                let key = key_from_audio(&audio, sr, &t).unwrap();
                keys_total += 1;
                let want = format!(
                    "{} {}",
                    pc_name(tonic as usize),
                    if minor { "minor" } else { "major" }
                );
                keys_ok += (key.key == want) as i32;
                if key.key != want {
                    eprintln!(
                        "audio key miss: wanted {want}, got {} (which {which})",
                        key.key
                    );
                }
                let spans = chords_from_audio(&audio, sr, &t, 1, ChordGrain::Bar);
                for (i, (r, q)) in prog.iter().enumerate() {
                    chords_total += 1;
                    chords_ok += (spans.get(i).map(|s| s.symbol.as_str())
                        == Some(format!("{}{q}", pc_name(*r as usize)).as_str()))
                        as i32;
                }
            }
        }
    }
    eprintln!("audio: key {keys_ok}/{keys_total}, chords {chords_ok}/{chords_total}");
    // Audio is approximate (relative major/minor ambiguity, harmonics): demand a high bar.
    assert!(
        keys_ok as f64 / keys_total as f64 >= 0.85,
        "{keys_ok}/{keys_total}"
    );
    assert!(
        chords_ok as f64 / chords_total as f64 >= 0.85,
        "{chords_ok}/{chords_total}"
    );
}

#[test]
fn muted_notes_are_not_analysed() {
    use ph_core::{Edit, InstrumentSpec, NoteSpec, Project};
    let mut ed = Edit::new(&Project::empty("t"));
    let pat = ed.add_pattern("p").unwrap();
    let ch = ed
        .add_channel(
            "c",
            InstrumentSpec::Synth {
                preset: None,
                params: Default::default(),
            },
        )
        .unwrap();
    let a = ed.add_note(pat, ch, NoteSpec::new(60, 0, 240)).unwrap();
    ed.add_note(pat, ch, NoteSpec::new(64, 240, 240)).unwrap();
    assert_eq!(pattern_notes(ed.project(), pat).len(), 2);
    ed.update_note(pat, ch, a, |n| n.muted = true).unwrap();
    let p = ed.project().clone();
    let keys: Vec<u8> = pattern_notes(&p, pat).iter().map(|n| n.key).collect();
    assert_eq!(keys, vec![64], "the muted C is not heard, so not analysed");
    assert_eq!(song_notes(&p).len(), 1);
}
