//! Standard MIDI file import and export (`midly`).
//!
//! Import never trusts the file: every length and time is checked, and garbage returns an
//! error rather than panicking. Times are converted to Patternhaus ticks (960 per quarter).

use crate::{FormatError, FormatResult};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use ph_core::{
    AutomationPoint, CcPointSpec, ChannelId, Controller, CurveKind, Edit, InstrumentSpec, NoteSpec,
    PPQ, PatternId, Project, Tick, TimeSignature, value_at,
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
    /// Controller lane points written (CC 0-119 and pitch bend; M3/T10).
    pub controller_points: usize,
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

/// One controller message read from a file, in Patternhaus ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedController {
    pub controller: Controller,
    pub tick: Tick,
    /// CC 0..=1, pitch bend -1..=1 (the 7-bit or 14-bit value, normalised).
    pub value: f64,
}

/// Notes and controller messages of one (track, MIDI channel) pair.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedPart {
    pub name: String,
    pub notes: Vec<ParsedNote>,
    /// In file order (time, then arrival).
    pub controllers: Vec<ParsedController>,
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
        let mut ctl_by_channel: BTreeMap<u8, Vec<ParsedController>> = BTreeMap::new();
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
                        MidiMessage::Controller { controller, value }
                            if controller.as_int() <= 119 =>
                        {
                            ctl_by_channel
                                .entry(ch)
                                .or_default()
                                .push(ParsedController {
                                    controller: Controller::Cc(controller.as_int()),
                                    tick: conv(now),
                                    value: value.as_int() as f64 / 127.0,
                                });
                        }
                        MidiMessage::PitchBend { bend } => {
                            ctl_by_channel
                                .entry(ch)
                                .or_default()
                                .push(ParsedController {
                                    controller: Controller::PitchBend,
                                    tick: conv(now),
                                    value: (bend.as_int() as f64 / 8192.0).clamp(-1.0, 1.0),
                                });
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
            let controllers = ctl_by_channel.remove(&ch).unwrap_or_default();
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
            parts.push(ParsedPart {
                name,
                notes,
                controllers,
            });
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
    let mut controller_points = 0;
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
        // Controller messages are steps: each holds until the next one (a dense stream is
        // as smooth as the file made it). Controllers of channels without notes are dropped.
        for c in &part.controllers {
            ed.add_cc_point(
                pattern,
                ch,
                CcPointSpec {
                    controller: c.controller,
                    tick: c.tick,
                    value: c.value,
                    curve: CurveKind::Hold,
                    tension: 0.0,
                },
            )?;
            controller_points += 1;
        }
        channels.push(ch);
    }
    Ok(MidiImport {
        pattern,
        channels,
        notes,
        controller_points,
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

enum EvKind {
    NoteOff {
        key: u8,
    },
    /// Controller changes sit between a note's end and the next note's start.
    Cc {
        controller: u8,
        value: u8,
    },
    Bend {
        value: i16,
    },
    NoteOn {
        key: u8,
        vel: u8,
    },
}

struct Ev {
    time: Tick,
    kind: EvKind,
}

impl Ev {
    /// Sort key at one tick: offs, controllers (so a note starts under its new value), ons.
    fn order(&self) -> (Tick, u8, u8, i32) {
        match self.kind {
            EvKind::NoteOff { key } => (self.time, 0, key, 0),
            EvKind::Cc { controller, value } => (self.time, 1, controller, value as i32),
            EvKind::Bend { value } => (self.time, 1, 128, value as i32),
            EvKind::NoteOn { key, .. } => (self.time, 2, key, 0),
        }
    }
}

/// Ticks between the messages of an exported ramp (about 5 ms at 120 BPM).
const EXPORT_RAMP_STEP: Tick = 10;

/// A lane's messages for the pattern ticks `lo..hi`, placed with pattern tick `lo` at song
/// tick `origin`: the value at `lo`, every point inside, and ramps in between sampled every
/// [`EXPORT_RAMP_STEP`] ticks, each only where the 7-bit (14-bit) value changes.
fn lane_events(
    c: Controller,
    pts: &[AutomationPoint],
    lo: Tick,
    hi: Tick,
    origin: Tick,
    out: &mut Vec<Ev>,
) {
    let mut last: Option<i32> = None;
    let mut push = |tick: Tick, v: f64, out: &mut Vec<Ev>| {
        let q = c.quantize(v);
        if last == Some(q) {
            return;
        }
        last = Some(q);
        let time = origin + (tick - lo);
        out.push(Ev {
            time,
            kind: match c {
                Controller::Cc(n) => EvKind::Cc {
                    controller: n,
                    value: q as u8,
                },
                Controller::PitchBend => EvKind::Bend { value: q as i16 },
            },
        });
    };
    let at = |t: Tick| value_at(pts, t as f64).unwrap_or(0.0);
    push(lo, at(lo), out);
    let mut knots: Vec<Tick> = pts
        .iter()
        .map(|p| p.tick)
        .filter(|t| *t > lo && *t < hi)
        .collect();
    knots.dedup();
    knots.push(hi);
    let mut prev = lo;
    for k in knots {
        let mut t = prev + EXPORT_RAMP_STEP;
        while t < k {
            push(t, at(t), out);
            t += EXPORT_RAMP_STEP;
        }
        if k < hi {
            push(k, at(k), out);
        }
        prev = k;
    }
}

/// Writes a type-1 MIDI file: a conductor track (name, tempo, time signature) and one track
/// per channel that has notes.
pub fn export_midi(project: &Project, what: MidiExport) -> FormatResult<Vec<u8>> {
    let mut per_channel: BTreeMap<ChannelId, Vec<(Tick, Tick, u8, u8)>> = BTreeMap::new();
    // controller messages per channel (M3/T10), unsorted
    let mut per_channel_cc: BTreeMap<ChannelId, Vec<Ev>> = BTreeMap::new();
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
            let len = project.pattern_length(id)?.max(1);
            for ch in pat.cc.keys() {
                for lane in pat.lanes(*ch) {
                    lane_events(
                        lane.controller,
                        &lane.auto_points(),
                        0,
                        len,
                        0,
                        per_channel_cc.entry(*ch).or_default(),
                    );
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
                // lanes play the clip's window once, as the engine does
                let hi = (clip.offset + clip.length).min(plen);
                if clip.offset < hi {
                    for ch in pat.cc.keys() {
                        for lane in pat.lanes(*ch) {
                            lane_events(
                                lane.controller,
                                &lane.auto_points(),
                                clip.offset,
                                hi,
                                clip.start,
                                per_channel_cc.entry(*ch).or_default(),
                            );
                        }
                    }
                }
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
        if !per_channel.contains_key(ch) && !per_channel_cc.contains_key(ch) {
            continue;
        }
        let notes: &[(Tick, Tick, u8, u8)] = per_channel.get(ch).map_or(&[], |v| v.as_slice());
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
                kind: EvKind::NoteOn { key, vel },
            });
            evs.push(Ev {
                time: start + len,
                kind: EvKind::NoteOff { key },
            });
        }
        evs.extend(per_channel_cc.remove(ch).unwrap_or_default());
        // Offs before ons at the same tick so repeated notes retrigger cleanly; controllers
        // in between, so a note starts under its new value.
        evs.sort_by_key(|e| e.order());
        let mut track = vec![meta(0, MetaMessage::TrackName(channel.name.as_bytes()))];
        let mut last = 0;
        for e in evs {
            let delta = e.time - last;
            last = e.time;
            let delta = u32::try_from(delta)
                .ok()
                .filter(|d| *d <= 0x0FFF_FFFF)
                .ok_or_else(|| FormatError::Midi("song is too long for a MIDI file".into()))?;
            let message = match e.kind {
                EvKind::NoteOn { key, vel } => MidiMessage::NoteOn {
                    key: u7::new(key),
                    vel: u7::new(vel),
                },
                EvKind::NoteOff { key } => MidiMessage::NoteOff {
                    key: u7::new(key),
                    vel: u7::new(0),
                },
                EvKind::Cc { controller, value } => MidiMessage::Controller {
                    controller: u7::new(controller),
                    value: u7::new(value),
                },
                EvKind::Bend { value } => MidiMessage::PitchBend {
                    bend: midly::PitchBend::from_int(value),
                },
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

    /// Lanes of the test project: CC1 swell on Lead, a bend ramp and CC64 steps on Bass.
    fn with_lanes() -> (Project, PatternId) {
        let (p, pat) = beat();
        let mut ed = Edit::new(&p);
        let lead = p.channel_order[0];
        let bass = p.channel_order[1];
        let mut add = |ch, c: Controller, t: Tick, v: f64, k: CurveKind| {
            ed.add_cc_point(
                pat,
                ch,
                CcPointSpec {
                    controller: c,
                    tick: t,
                    value: v,
                    curve: k,
                    tension: 0.0,
                },
            )
            .unwrap();
        };
        add(lead, Controller::Cc(1), 0, 0.3, CurveKind::Smooth);
        add(lead, Controller::Cc(1), 960, 0.9, CurveKind::Smooth);
        add(lead, Controller::Cc(1), 1920, 0.4, CurveKind::Hold);
        add(bass, Controller::PitchBend, 0, -1.0, CurveKind::Linear);
        add(bass, Controller::PitchBend, 480, 0.5, CurveKind::Hold);
        add(bass, Controller::Cc(64), 0, 1.0, CurveKind::Hold);
        add(bass, Controller::Cc(64), 960, 0.0, CurveKind::Hold);
        (ed.finish().0, pat)
    }

    /// What a lane sounds like: its 7-bit (14-bit) value at `tick`, as the engine sends it.
    fn wire_at(p: &Project, pat: PatternId, ch: ChannelId, c: Controller, tick: Tick) -> i32 {
        let lane = p.pattern(pat).unwrap().lane(ch, c);
        c.quantize(lane.value_at(tick as f64).unwrap())
    }

    #[test]
    fn controller_lanes_round_trip_through_a_midi_file() {
        let (p, pat) = with_lanes();
        let bytes = export_midi(&p, MidiExport::Pattern(pat)).unwrap();
        let parsed = parse_midi(&bytes).unwrap();
        let lead = parsed.parts.iter().find(|x| x.name == "Lead").unwrap();
        // the swell is written as it sounds: the first value, every point, a ramp between
        assert_eq!(lead.controllers[0].tick, 0);
        assert_eq!(lead.controllers[0].controller, Controller::Cc(1));
        assert!((lead.controllers[0].value - 0.3).abs() < 1.0 / 254.0);
        assert!(
            lead.controllers
                .iter()
                .any(|c| Controller::Cc(1).quantize(c.value) == 114)
        );
        assert!(lead.controllers.len() > 20, "{}", lead.controllers.len());
        let last = lead.controllers.last().unwrap();
        // (the ramp reaches its final value a little before the last point; a repeat is not written)
        assert!(last.tick <= 1920 && last.tick > 1800);
        assert_eq!(last.value, 51.0 / 127.0);
        // importing brings the lanes back: same sound at every tick that was written
        let mut ed = Edit::new(&Project::empty("In"));
        let r = import_midi(&mut ed, &bytes, &MidiImportOptions::default()).unwrap();
        let (q, _) = ed.finish();
        assert!(r.controller_points > 20);
        let ch_of = |name: &str| {
            *q.channels
                .iter()
                .find(|(_, c)| c.name == name)
                .map(|(i, _)| i)
                .unwrap()
        };
        let (lead_in, bass_in) = (ch_of("Lead"), ch_of("Bass"));
        let (lead_out, bass_out) = (p.channel_order[0], p.channel_order[1]);
        for (cin, cout, c) in [
            (lead_in, lead_out, Controller::Cc(1)),
            (bass_in, bass_out, Controller::PitchBend),
            (bass_in, bass_out, Controller::Cc(64)),
        ] {
            let lane = q.pattern(r.pattern).unwrap().lane(cin, c);
            assert!(!lane.points.is_empty(), "{c:?}");
            for pt in &lane.points {
                assert_eq!(
                    c.quantize(pt.value),
                    wire_at(&p, pat, cout, c, pt.tick),
                    "{c:?} at tick {}",
                    pt.tick
                );
            }
        }
        // and a second trip changes nothing
        let again = export_midi(&q, MidiExport::Pattern(r.pattern)).unwrap();
        let mut ed = Edit::new(&Project::empty("In2"));
        let r2 = import_midi(&mut ed, &again, &MidiImportOptions::default()).unwrap();
        let (q2, _) = ed.finish();
        let lanes = |p: &Project, id: PatternId| {
            let pat = p.pattern(id).unwrap();
            let mut v: Vec<(String, String, Tick, i32)> = Vec::new();
            for ch in pat.cc.keys() {
                for l in pat.lanes(*ch) {
                    for x in &l.points {
                        v.push((
                            p.channel(*ch).unwrap().name.clone(),
                            l.controller.label(),
                            x.tick,
                            l.controller.quantize(x.value),
                        ));
                    }
                }
            }
            v.sort();
            v
        };
        assert_eq!(lanes(&q, r.pattern), lanes(&q2, r2.pattern));
    }

    #[test]
    fn imports_controller_messages_as_hold_points_and_skips_what_it_cannot_use() {
        use midly::num::u7;
        let ev = |delta: u32, kind: TrackEventKind<'static>| TrackEvent {
            delta: u28::new(delta),
            kind,
        };
        let midi = |ch: u8, message: MidiMessage| TrackEventKind::Midi {
            channel: u4::new(ch),
            message,
        };
        let track = vec![
            ev(
                0,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(90),
                    },
                ),
            ),
            ev(
                0,
                midi(
                    0,
                    MidiMessage::Controller {
                        controller: u7::new(11),
                        value: u7::new(127),
                    },
                ),
            ),
            ev(
                240,
                midi(
                    0,
                    MidiMessage::Controller {
                        controller: u7::new(11),
                        value: u7::new(0),
                    },
                ),
            ),
            // channel mode messages (CC 120+) are not controllers
            ev(
                0,
                midi(
                    0,
                    MidiMessage::Controller {
                        controller: u7::new(123),
                        value: u7::new(0),
                    },
                ),
            ),
            ev(
                0,
                midi(
                    0,
                    MidiMessage::PitchBend {
                        bend: midly::PitchBend::from_int(-8192),
                    },
                ),
            ),
            ev(
                0,
                midi(
                    0,
                    MidiMessage::PitchBend {
                        bend: midly::PitchBend::from_int(0),
                    },
                ),
            ),
            ev(
                480,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(0),
                    },
                ),
            ),
            // a channel with controllers but no notes gets no channel
            ev(
                0,
                midi(
                    3,
                    MidiMessage::Controller {
                        controller: u7::new(1),
                        value: u7::new(64),
                    },
                ),
            ),
            ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
        ];
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
            tracks: vec![track],
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let parsed = parse_midi(&bytes).unwrap();
        assert_eq!(parsed.parts[0].controllers.len(), 4);
        let mut ed = Edit::new(&Project::empty("cc"));
        let r = import_midi(&mut ed, &bytes, &MidiImportOptions::default()).unwrap();
        let (q, _) = ed.finish();
        assert_eq!((r.channels.len(), r.controller_points), (1, 4));
        let pat = q.pattern(r.pattern).unwrap();
        let ch = r.channels[0];
        let expr = pat.lane(ch, Controller::Cc(11));
        let got: Vec<_> = expr.points.iter().map(|p| (p.tick, p.value)).collect();
        // 240 file ticks at 480 ppq = 480 of ours
        assert_eq!(got, vec![(0, 1.0), (480, 0.0)]);
        assert!(expr.points.iter().all(|p| p.curve == CurveKind::Hold));
        let bend = pat.lane(ch, Controller::PitchBend);
        assert_eq!(
            bend.points.iter().map(|p| p.value).collect::<Vec<_>>(),
            vec![-1.0, 0.0]
        );
        assert!(pat.lane(ch, Controller::Cc(123)).points.is_empty());
    }

    #[test]
    fn song_export_writes_lanes_per_clip_window() {
        let (p, pat) = with_lanes();
        let mut ed = Edit::new(&p);
        // the pattern is 3 beats long here (3/4, one bar); place it at bar 2 trimmed
        let bar = p.settings.time_signature.ticks_per_bar();
        let c = ed.add_clip(0, pat, bar).unwrap();
        ed.update_clip(c, |c| {
            c.offset = 960;
            c.length = 960;
        })
        .unwrap();
        let p2 = ed.finish().0;
        let bytes = export_midi(&p2, MidiExport::Song).unwrap();
        let parsed = parse_midi(&bytes).unwrap();
        let lead = parsed.parts.iter().find(|x| x.name == "Lead").unwrap();
        // the window starts at the swell's peak, 0.9 (114/127), at the clip start
        let first = &lead.controllers[0];
        assert_eq!((first.tick, first.controller), (bar, Controller::Cc(1)));
        assert_eq!(Controller::Cc(1).quantize(first.value), 114);
        assert!(
            lead.controllers
                .iter()
                .all(|c| c.tick >= bar && c.tick < bar + 960)
        );
        // a channel with lanes but no notes still gets a track
        let mut ed = Edit::new(&p);
        let ch = ed
            .add_channel(
                "Mod only",
                InstrumentSpec::Synth {
                    preset: None,
                    params: Default::default(),
                },
            )
            .unwrap();
        ed.add_cc_point(
            pat,
            ch,
            CcPointSpec {
                controller: Controller::Cc(7),
                tick: 0,
                value: 0.5,
                curve: CurveKind::Hold,
                tension: 0.0,
            },
        )
        .unwrap();
        let p3 = ed.finish().0;
        let parsed = parse_midi(&export_midi(&p3, MidiExport::Pattern(pat)).unwrap()).unwrap();
        // (no notes: the importer drops it, but the file has the track)
        assert!(parsed.parts.iter().all(|x| x.name != "Mod only"));
        let smf = Smf::parse(&export_midi(&p3, MidiExport::Pattern(pat)).unwrap())
            .unwrap()
            .tracks
            .len();
        assert_eq!(smf, 4, "conductor + Lead + Bass + Mod only");
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
