//! Standard MIDI file import and export (`midly`).
//!
//! Import never trusts the file: every length and time is checked, and garbage returns an
//! error rather than panicking. Times are converted to Patternhaus ticks (960 per quarter).

use crate::{FormatError, FormatResult};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use ph_core::{
    ChannelId, Edit, InstrumentSpec, NoteSpec, PPQ, PatternId, Project, Tick, TimeSignature,
};
use std::collections::BTreeMap;

/// What to do with a parsed file.
#[derive(Clone, Debug)]
pub struct MidiImportOptions {
    /// Name of the pattern that receives the notes.
    pub pattern_name: String,
}

impl Default for MidiImportOptions {
    fn default() -> Self {
        Self {
            pattern_name: "MIDI import".into(),
        }
    }
}

/// What an import created.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiImport {
    pub pattern: PatternId,
    /// One Synth placeholder channel per (track, MIDI channel) that has notes.
    pub channels: Vec<ChannelId>,
    pub notes: usize,
    /// First tempo in the file, if it is in the supported range.
    pub tempo_bpm: Option<f64>,
    pub time_signature: Option<TimeSignature>,
}

/// One note read from a file, in Patternhaus ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedNote {
    pub key: u8,
    pub start: Tick,
    pub length: Tick,
    pub velocity: f32,
}

/// Notes of one (track, MIDI channel) pair.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedPart {
    pub name: String,
    pub notes: Vec<ParsedNote>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedMidi {
    pub parts: Vec<ParsedPart>,
    pub tempo_bpm: Option<f64>,
    pub time_signature: Option<TimeSignature>,
}

/// Parses a MIDI file into per-track, per-channel note lists.
pub fn parse_midi(bytes: &[u8]) -> FormatResult<ParsedMidi> {
    let smf = Smf::parse(bytes).map_err(|e| FormatError::Midi(e.to_string()))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) if t.as_int() > 0 => t.as_int() as u64,
        Timing::Metrical(_) => return Err(FormatError::Midi("zero ticks per beat".into())),
        Timing::Timecode(..) => {
            return Err(FormatError::Midi(
                "SMPTE timecode files are not supported".into(),
            ));
        }
    };
    let conv = |t: u64| t.saturating_mul(PPQ) / ppq;
    let mut tempo_bpm = None;
    let mut time_signature = None;
    let mut parts = Vec::new();
    for (ti, track) in smf.tracks.iter().enumerate() {
        let mut now: u64 = 0;
        let mut track_name = None;
        // (midi channel, key) -> start ticks and velocity of notes still sounding (FIFO).
        let mut open: BTreeMap<(u8, u8), Vec<(u64, f32)>> = BTreeMap::new();
        let mut by_channel: BTreeMap<u8, Vec<ParsedNote>> = BTreeMap::new();
        let close = |by_channel: &mut BTreeMap<u8, Vec<ParsedNote>>,
                     ch: u8,
                     key: u8,
                     start: u64,
                     vel: f32,
                     end: u64| {
            let (s, e) = (conv(start), conv(end));
            by_channel.entry(ch).or_default().push(ParsedNote {
                key,
                start: s,
                length: e.saturating_sub(s).max(1),
                velocity: vel,
            });
        };
        for ev in track {
            now = now.saturating_add(ev.delta.as_int() as u64);
            match ev.kind {
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => open
                            .entry((ch, key.as_int()))
                            .or_default()
                            .push((now, vel.as_int() as f32 / 127.0)),
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            let slot = open.entry((ch, key.as_int())).or_default();
                            if !slot.is_empty() {
                                let (start, vel) = slot.remove(0);
                                close(&mut by_channel, ch, key.as_int(), start, vel, now);
                            }
                        }
                        _ => {}
                    }
                }
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if track_name.is_none() => {
                    track_name = Some(String::from_utf8_lossy(n).trim().to_string());
                }
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if tempo_bpm.is_none() => {
                    let us = us.as_int();
                    if us > 0 {
                        let bpm = 60_000_000.0 / us as f64;
                        if (10.0..=999.0).contains(&bpm) {
                            tempo_bpm = Some(bpm);
                        }
                    }
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, _, _))
                    if time_signature.is_none() && n > 0 && d <= 6 =>
                {
                    time_signature = Some(TimeSignature {
                        numerator: n,
                        denominator: 1 << d,
                    });
                }
                _ => {}
            }
        }
        // Notes never switched off end where the track ends.
        for ((ch, key), starts) in std::mem::take(&mut open) {
            for (start, vel) in starts {
                close(&mut by_channel, ch, key, start, vel, now);
            }
        }
        let many = by_channel.len() > 1;
        for (ch, mut notes) in by_channel {
            notes.sort_by_key(|n| (n.start, n.key));
            let base = track_name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Track {}", ti + 1));
            let name = if many {
                format!("{base} ch{}", ch + 1)
            } else {
                base
            };
            parts.push(ParsedPart { name, notes });
        }
    }
    Ok(ParsedMidi {
        parts,
        tempo_bpm,
        time_signature,
    })
}

/// Adds a pattern and one Synth placeholder channel per part to `ed`, with the file's notes.
pub fn import_midi(
    ed: &mut Edit,
    bytes: &[u8],
    opts: &MidiImportOptions,
) -> FormatResult<MidiImport> {
    let parsed = parse_midi(bytes)?;
    let pattern = ed.add_pattern(&opts.pattern_name)?;
    let mut channels = Vec::new();
    let mut notes = 0;
    for part in &parsed.parts {
        let ch = ed.add_channel(
            &part.name,
            InstrumentSpec::Synth {
                preset: None,
                params: Default::default(),
            },
        )?;
        for n in &part.notes {
            let mut spec = NoteSpec::new(n.key, n.start, n.length);
            spec.velocity = n.velocity;
            ed.add_note(pattern, ch, spec)?;
            notes += 1;
        }
        channels.push(ch);
    }
    Ok(MidiImport {
        pattern,
        channels,
        notes,
        tempo_bpm: parsed.tempo_bpm,
        time_signature: parsed.time_signature,
    })
}

/// What to write out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MidiExport {
    /// One pattern, from its start.
    Pattern(PatternId),
    /// The playlist: every clip on an unmuted track, placed at its song position.
    Song,
}

struct Ev {
    time: Tick,
    on: bool,
    key: u8,
    vel: u8,
}

/// Writes a type-1 MIDI file: a conductor track (name, tempo, time signature) and one track
/// per channel that has notes.
pub fn export_midi(project: &Project, what: MidiExport) -> FormatResult<Vec<u8>> {
    let mut per_channel: BTreeMap<ChannelId, Vec<(Tick, Tick, u8, u8)>> = BTreeMap::new();
    let vel_of = |v: f32| ((v * 127.0).round() as i64).clamp(1, 127) as u8;
    match what {
        MidiExport::Pattern(id) => {
            let pat = project.pattern(id)?;
            for (ch, notes) in &pat.notes {
                for n in notes.values().filter(|n| !n.muted) {
                    per_channel.entry(*ch).or_default().push((
                        n.start,
                        n.length,
                        n.key,
                        vel_of(n.velocity),
                    ));
                }
            }
        }
        MidiExport::Song => {
            for clip in project.clips.values() {
                if project
                    .tracks
                    .get(clip.track as usize)
                    .is_none_or(|t| t.muted)
                {
                    continue;
                }
                // only pattern clips hold notes
                let Some(pid) = clip.pattern() else {
                    continue;
                };
                let plen = project.pattern_length(pid)?.max(1);
                let pat = project.pattern(pid)?;
                for (ch, notes) in &pat.notes {
                    for n in notes.values() {
                        if n.muted || n.start < clip.offset {
                            continue;
                        }
                        // The clip plays the pattern from `offset`, wrapping at its length.
                        let mut x = (n.start - clip.offset) % plen;
                        while x < clip.length {
                            let len = n.length.min(clip.length - x);
                            per_channel.entry(*ch).or_default().push((
                                clip.start + x,
                                len,
                                n.key,
                                vel_of(n.velocity),
                            ));
                            x += plen;
                        }
                    }
                }
            }
        }
    }

    let ts = project.settings.time_signature;
    let us_per_beat = (60_000_000.0 / project.settings.bpm).round() as u32;
    let mut tracks: Vec<Vec<TrackEvent<'_>>> = Vec::new();
    let mut conductor = vec![
        meta(0, MetaMessage::TrackName(project.settings.title.as_bytes())),
        meta(
            0,
            MetaMessage::Tempo(u24::new(us_per_beat.min(0x00FF_FFFF))),
        ),
        meta(
            0,
            MetaMessage::TimeSignature(ts.numerator, ts.denominator.trailing_zeros() as u8, 24, 8),
        ),
    ];
    conductor.push(meta(0, MetaMessage::EndOfTrack));
    tracks.push(conductor);

    let mut index = 0usize;
    for ch in project.channel_order.iter() {
        let Some(notes) = per_channel.get(ch) else {
            continue;
        };
        let channel = project.channel(*ch)?;
        // MIDI channel 10 (index 9) is the drum channel; keep synth tracks off it.
        let mut midi_ch = (index % 15) as u8;
        if midi_ch >= 9 {
            midi_ch += 1;
        }
        index += 1;
        let mut evs: Vec<Ev> = Vec::with_capacity(notes.len() * 2);
        for &(start, len, key, vel) in notes {
            evs.push(Ev {
                time: start,
                on: true,
                key,
                vel,
            });
            evs.push(Ev {
                time: start + len,
                on: false,
                key,
                vel: 0,
            });
        }
        // Offs before ons at the same tick so repeated notes retrigger cleanly.
        evs.sort_by_key(|e| (e.time, e.on, e.key));
        let mut track = vec![meta(0, MetaMessage::TrackName(channel.name.as_bytes()))];
        let mut last = 0;
        for e in evs {
            let delta = e.time - last;
            last = e.time;
            let delta = u32::try_from(delta)
                .ok()
                .filter(|d| *d <= 0x0FFF_FFFF)
                .ok_or_else(|| FormatError::Midi("song is too long for a MIDI file".into()))?;
            let message = if e.on {
                MidiMessage::NoteOn {
                    key: u7::new(e.key),
                    vel: u7::new(e.vel),
                }
            } else {
                MidiMessage::NoteOff {
                    key: u7::new(e.key),
                    vel: u7::new(0),
                }
            };
            track.push(TrackEvent {
                delta: u28::new(delta),
                kind: TrackEventKind::Midi {
                    channel: u4::new(midi_ch),
                    message,
                },
            });
        }
        track.push(meta(0, MetaMessage::EndOfTrack));
        tracks.push(track);
    }

    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ as u16))),
        tracks,
    };
    let mut out = Vec::new();
    smf.write_std(&mut out)
        .map_err(|e| FormatError::Midi(e.to_string()))?;
    Ok(out)
}

fn meta(delta: u32, m: MetaMessage<'_>) -> TrackEvent<'_> {
    TrackEvent {
        delta: u28::new(delta),
        kind: TrackEventKind::Meta(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::{STEP, validate_note};
    use proptest::prelude::*;

    fn beat() -> (Project, PatternId) {
        let mut ed = Edit::new(&Project::empty("Beat"));
        ed.update_settings(|s| {
            s.bpm = 96.0;
            s.time_signature = TimeSignature {
                numerator: 3,
                denominator: 4,
            };
        })
        .unwrap();
        let pat = ed.add_pattern("P").unwrap();
        let a = ed
            .add_channel(
                "Lead",
                InstrumentSpec::Synth {
                    preset: None,
                    params: Default::default(),
                },
            )
            .unwrap();
        let b = ed
            .add_channel(
                "Bass",
                InstrumentSpec::Synth {
                    preset: None,
                    params: Default::default(),
                },
            )
            .unwrap();
        for i in 0..4u64 {
            let mut n = NoteSpec::new(60 + i as u8, i * STEP * 2, STEP);
            n.velocity = 0.5 + 0.1 * i as f32;
            ed.add_note(pat, a, n).unwrap();
        }
        ed.add_note(pat, b, NoteSpec::new(36, 0, PPQ)).unwrap();
        (ed.finish().0, pat)
    }

    fn notes_of(p: &Project, id: PatternId) -> Vec<(String, u8, Tick, Tick, u8)> {
        let pat = p.pattern(id).unwrap();
        let mut v = Vec::new();
        for (ch, m) in &pat.notes {
            for n in m.values() {
                v.push((
                    p.channel(*ch).unwrap().name.clone(),
                    n.key,
                    n.start,
                    n.length,
                    (n.velocity * 127.0).round() as u8,
                ));
            }
        }
        v.sort();
        v
    }

    #[test]
    fn pattern_export_import_round_trip() {
        let (p, pat) = beat();
        let bytes = export_midi(&p, MidiExport::Pattern(pat)).unwrap();
        let mut ed = Edit::new(&Project::empty("In"));
        let r = import_midi(&mut ed, &bytes, &MidiImportOptions::default()).unwrap();
        let (q, _) = ed.finish();
        assert_eq!(r.notes, 5);
        assert_eq!(r.channels.len(), 2);
        assert_eq!(r.tempo_bpm.map(|b| b.round()), Some(96.0));
        assert_eq!(
            r.time_signature,
            Some(TimeSignature {
                numerator: 3,
                denominator: 4
            })
        );
        assert_eq!(notes_of(&q, r.pattern), notes_of(&p, pat));
        for c in q.channels.values() {
            assert!(matches!(c.instrument, InstrumentSpec::Synth { .. }));
        }
    }

    #[test]
    fn song_export_places_and_loops_clips() {
        let (p, pat) = beat();
        let mut ed = Edit::new(&p);
        let plen = p.pattern_length(pat).unwrap();
        // 2.5 pattern lengths on track 1, plus a muted track that must be skipped.
        let c = ed.add_clip(1, pat, PPQ * 4).unwrap();
        ed.update_clip(c, |c| c.length = plen * 5 / 2).unwrap();
        ed.add_clip(2, pat, 0).unwrap();
        ed.update_track(2, |t| t.muted = true).unwrap();
        let p2 = ed.finish().0;
        let bytes = export_midi(&p2, MidiExport::Song).unwrap();
        let parsed = parse_midi(&bytes).unwrap();
        let bass = parsed.parts.iter().find(|x| x.name == "Bass").unwrap();
        let starts: Vec<_> = bass.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, vec![PPQ * 4, PPQ * 4 + plen, PPQ * 4 + 2 * plen]);
    }

    #[test]
    fn imports_note_off_as_velocity_zero_and_unterminated_notes() {
        use midly::num::u7;
        let track = vec![
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(100),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(480),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(0),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(9),
                    message: MidiMessage::NoteOn {
                        key: u7::new(36),
                        vel: u7::new(90),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(480),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ];
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
            tracks: vec![track],
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let parsed = parse_midi(&bytes).unwrap();
        assert_eq!(
            parsed.parts.len(),
            2,
            "one part per MIDI channel in the track"
        );
        // 480 file ticks at 480 ppq = 960 ticks at ours.
        assert_eq!(parsed.parts[0].notes[0].length, 960);
        assert_eq!(
            parsed.parts[1].notes[0].length, 960,
            "ends at the track end"
        );
    }

    #[test]
    fn rejects_garbage_and_timecode() {
        assert!(parse_midi(b"not midi").is_err());
        assert!(parse_midi(&[]).is_err());
    }

    proptest! {
        // Importer fuzz: arbitrary bytes never panic, and anything accepted is valid.
        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            let mut data = b"MThd\0\0\0\x06\0\x01\0\x01\x01\xe0MTrk".to_vec();
            data.extend_from_slice(&bytes);
            for input in [&bytes[..], &data[..]] {
                let mut ed = Edit::new(&Project::empty("fuzz"));
                if import_midi(&mut ed, input, &MidiImportOptions::default()).is_ok() {
                    let (p, _) = ed.finish();
                    for pat in p.patterns.values() {
                        for n in pat.notes.values().flat_map(|m| m.values()) {
                            prop_assert!(validate_note(n).is_ok());
                        }
                    }
                }
            }
        }

        #[test]
        fn export_import_preserves_notes(
            notes in proptest::collection::vec((0u8..128, 0u64..20_000, 1u64..4_000, 1u8..128), 1..40)
        ) {
            let mut ed = Edit::new(&Project::empty("rt"));
            let pat = ed.add_pattern("P").unwrap();
            let ch = ed.add_channel("C", InstrumentSpec::Synth { preset: None, params: Default::default() }).unwrap();
            for (k, s, l, v) in &notes {
                let mut n = NoteSpec::new(*k, *s, *l);
                n.velocity = *v as f32 / 127.0;
                ed.add_note(pat, ch, n).unwrap();
            }
            let (p, _) = ed.finish();
            let bytes = export_midi(&p, MidiExport::Pattern(pat)).unwrap();
            let mut ed2 = Edit::new(&Project::empty("rt2"));
            let r = import_midi(&mut ed2, &bytes, &MidiImportOptions::default()).unwrap();
            let (q, _) = ed2.finish();
            let mut a: Vec<_> = notes.iter().map(|(k, s, l, v)| (*k, *s, *l, *v)).collect();
            let mut b: Vec<_> = q.pattern(r.pattern).unwrap().notes.values().flat_map(|m| m.values())
                .map(|n| (n.key, n.start, n.length, (n.velocity * 127.0).round() as u8)).collect();
            // Same-key overlapping notes may pair differently; compare as multisets of
            // (key, start) and total length per key instead of exact pairing.
            a.sort();
            b.sort();
            prop_assert_eq!(a.len(), b.len());
            let ks = |v: &Vec<(u8, u64, u64, u8)>| {
                let mut m: Vec<(u8, u64)> = v.iter().map(|x| (x.0, x.1)).collect();
                m.sort();
                m
            };
            prop_assert_eq!(ks(&a), ks(&b));
        }
    }
}
