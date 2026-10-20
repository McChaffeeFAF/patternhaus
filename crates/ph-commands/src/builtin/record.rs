//! `record.*` and `midi.*` (M2/T8): audio input recording and MIDI controllers.
//!
//! The devices live behind the [`Host`] (the engine and the MIDI service); these commands
//! only choose what happens and, at the end of a recording, put the result into the project
//! as **one undo step**: a registry entry and an audio clip per take (placed at the record
//! position, playing from the latency compensation), and the MIDI notes played in pattern
//! mode. Takes are files in `samples/` before they are anything else, so a failed placement
//! never loses one: the sample is registered regardless and the command says what it could
//! not place.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// A result with the one-line summary a musician reads.
fn said(result: serde_json::Value, summary: impl Into<String>) -> CommandOutput {
    let mut o = CommandOutput::value(result);
    o.summary = Some(summary.into());
    o
}

pub fn register(r: &mut Registry) {
    r.register(typed(
        "record.list_inputs",
        "List the audio inputs that can be recorded, with their monitoring latency.",
        Scope::Read,
        READ,
        |ctx, _: NoParams| {
            let v = ctx.host.record_inputs()?;
            let n = v.inputs.len();
            Ok(said(
                json!({ "inputs": v.inputs, "latency": v.latency, "recording": v.recording }),
                format!(
                    "{}, monitoring latency {:.1} ms",
                    plural(n, "audio input", "audio inputs"),
                    v.latency.monitor_ms
                ),
            ))
        },
    ));
    r.register(typed(
        "record.arm",
        "Arm an audio input to record onto a playlist track or into a Sampler channel.",
        Scope::Compose,
        READ,
        arm,
    ));
    r.register(typed(
        "record.disarm",
        "Disarm an audio input.",
        Scope::Compose,
        READ,
        |ctx, p: InputOnly| {
            ctx.host.record_disarm(p.input)?;
            Ok(CommandOutput::value(json!({ "input": p.input })))
        },
    ));
    r.register(typed(
        "record.set_monitor",
        "Hear an armed input while recording, or stop hearing it.",
        Scope::Compose,
        READ,
        |ctx, p: Monitor| {
            ctx.host.record_set_monitor(p.input, p.on)?;
            Ok(CommandOutput::value(
                json!({ "input": p.input, "monitor": p.on }),
            ))
        },
    ));
    r.register(typed(
        "record.start",
        "Start recording every armed input (with count-in and punch range if given), and the MIDI notes played in pattern mode.",
        Scope::Compose,
        READ,
        start,
    ));
    r.register(typed(
        "record.stop",
        "Stop recording and add each take to the project as an audio clip, and the played MIDI notes to the pattern, as one undo step.",
        Scope::Compose,
        MUTATES,
        stop,
    ));
    // ---- MIDI controllers
    r.register(typed(
        "midi.list_inputs",
        "List MIDI input devices.",
        Scope::Read,
        READ,
        |ctx, _: NoParams| {
            let v = ctx.host.midi_inputs()?;
            Ok(CommandOutput::value(json!({ "inputs": v })))
        },
    ));
    r.register(typed(
        "midi.learn",
        "Map the next controller moved on a MIDI device to a parameter.",
        Scope::Full,
        READ,
        learn,
    ));
    r.register(typed(
        "midi.map",
        "Map a MIDI controller (CC) to a parameter (saved per user).",
        Scope::Full,
        READ,
        map,
    ));
    r.register(typed(
        "midi.unmap",
        "Remove a MIDI controller mapping.",
        Scope::Full,
        READ,
        unmap,
    ));
    r.register(typed(
        "midi.list_mappings",
        "List the MIDI controller mappings.",
        Scope::Read,
        READ,
        |ctx, _: NoParams| {
            let v = ctx.host.midi_mappings()?;
            Ok(CommandOutput::value(json!({ "mappings": v })))
        },
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InputOnly {
    /// Audio input id from record.list_inputs.
    input: u32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Arm {
    /// Audio input id from record.list_inputs.
    input: u32,
    /// Record onto this playlist track as a new audio clip; or give `channel`.
    #[serde(default)]
    track: Option<u32>,
    /// Record into this Sampler channel (replaces its sample).
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Hear the input while armed (default true).
    #[serde(default)]
    monitor: Option<bool>,
}

fn arm(ctx: &mut CommandCtx<'_>, p: Arm) -> CmdResult<CommandOutput> {
    match (p.track, p.channel) {
        (Some(t), None) if (t as usize) >= ctx.project.tracks.len() => {
            return Err(ModelError::NoTrack(t as usize).into());
        }
        (Some(_), None) => {}
        (None, Some(c)) => {
            if !matches!(
                ctx.project.channel(c)?.instrument,
                InstrumentSpec::Sampler { .. }
            ) {
                return invalid(format!(
                    "channel {c} is not a Sampler channel: a take replaces a Sampler's sample"
                ));
            }
        }
        _ => return invalid("give exactly one of track and channel"),
    }
    let monitor = p.monitor.unwrap_or(true);
    ctx.host.record_arm(ArmRequest {
        input: p.input,
        track: p.track,
        channel: p.channel,
        monitor,
    })?;
    Ok(CommandOutput::value(json!({
        "input": p.input, "track": p.track, "channel": p.channel, "monitor": monitor,
    })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Monitor {
    /// Audio input id from record.list_inputs.
    input: u32,
    /// True to hear the input.
    on: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Start {
    /// Bars of count-in before recording (default 0).
    #[serde(default)]
    count_in_bars: u32,
    /// Punch range in ticks: record only inside it.
    #[serde(default)]
    punch: Option<(Tick, Tick)>,
    /// Use this latency compensation in milliseconds instead of the measured one.
    #[serde(default)]
    latency_ms: Option<f64>,
    /// In pattern mode, also collect the MIDI notes played into this channel's notes of the
    /// pattern (default: the channel MIDI notes go to, the selected one).
    #[serde(default)]
    channel: Option<ChannelId>,
    /// Collect MIDI notes in pattern mode (default true).
    #[serde(default = "yes")]
    midi_notes: bool,
}

fn yes() -> bool {
    true
}

fn start(ctx: &mut CommandCtx<'_>, p: Start) -> CmdResult<CommandOutput> {
    if let Some((a, b)) = p.punch
        && b <= a
    {
        return invalid("the punch range must end after it starts");
    }
    if let Some(ms) = p.latency_ms
        && !(0.0..=2000.0).contains(&ms)
    {
        return invalid("latency_ms is 0..2000");
    }
    if let Some(c) = p.channel {
        ctx.project.channel(c)?;
    }
    let tr = ctx.host.transport_state();
    let notes = match (p.midi_notes, tr.mode, tr.pattern) {
        (true, TransportMode::Pattern, Some(pattern)) => p
            .channel
            .or_else(|| ctx.host.midi_target_channel())
            .map(|channel| NoteCapture { pattern, channel }),
        _ => None,
    };
    if let Some(n) = &notes {
        ctx.project.channel(n.channel)?;
    }
    let started = ctx.host.record_start(RecordStart {
        count_in_bars: p.count_in_bars,
        punch: p.punch,
        latency_ms: p.latency_ms,
        notes,
    })?;
    let what = match (started.audio_inputs, started.notes) {
        (0, true) => "recording MIDI notes".to_string(),
        (n, true) => format!("recording {} and MIDI notes", plural(n as usize, "input", "inputs")),
        (n, false) => format!("recording {}", plural(n as usize, "input", "inputs")),
    };
    Ok(said(
        json!({
            "audio_inputs": started.audio_inputs,
            "notes": started.notes,
            "count_in_bars": p.count_in_bars,
            "punch": p.punch,
        }),
        what,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Stop {
    /// Snap the start of recorded MIDI notes to this grid in ticks (240 = a 16th step);
    /// omit to keep them as played.
    #[serde(default)]
    quantize: Option<Tick>,
}

fn stop(ctx: &mut CommandCtx<'_>, p: Stop) -> CmdResult<CommandOutput> {
    if p.quantize == Some(0) {
        return invalid("quantize must be greater than zero");
    }
    let out = ctx.host.record_stop()?;
    let mut ed = Edit::new(ctx.project);
    let bpm = ctx.project.settings.bpm;
    let mut placed = Vec::new();
    let mut warnings = Vec::new();
    for t in &out.takes {
        // the sample is registered whatever happens next: the file exists
        let id = ed.add_sample(SampleEntry {
            id: SampleId(0),
            path: t.reference.clone(),
            name: t
                .reference
                .rsplit('/')
                .next()
                .unwrap_or(&t.reference)
                .to_string(),
            frames: t.frames,
            sample_rate: t.sample_rate,
            channels: t.channels,
            bpm: None,
            root_key: None,
            analysis: SampleAnalysis::default(),
            provenance: Provenance {
                origin: Origin::Recorded(Take {
                    input: t.input.clone(),
                    start: t.start,
                    length: t.length,
                }),
                at: now_unix(),
            },
        })?;
        if let Some(e) = &t.error {
            warnings.push(format!("{}: the disk reported {e}", t.reference));
        }
        if t.overflow > 0 {
            warnings.push(format!(
                "{}: {} frames were dropped (the disk or the machine could not keep up)",
                t.reference, t.overflow
            ));
        }
        match (t.track, t.channel) {
            (Some(track), _) => {
                // the sound that belongs at `start` is `latency_frames` into the file
                let offset = t.latency_frames.min(t.frames.saturating_sub(1));
                let seconds = (t.frames - offset) as f64 / t.sample_rate.max(1) as f64;
                let length = (seconds_to_ticks(seconds, bpm).round() as Tick).max(1);
                let track = track.min(ctx.project.tracks.len().saturating_sub(1) as u32);
                let mut a = AudioClip::new(id);
                a.source_offset = offset;
                let clip = ed.add_audio_clip(track, t.start, length, a)?;
                placed.push(json!({
                    "clip": clip, "sample": id, "track": track, "start": t.start, "length": length,
                    "file": t.reference, "latency_frames": t.latency_frames,
                }));
            }
            (None, Some(channel)) => {
                let ch = ctx.project.channel(channel)?.clone();
                let (root_key, params) = match &ch.instrument {
                    InstrumentSpec::Sampler {
                        root_key, params, ..
                    } => (*root_key, params.clone()),
                    _ => (60, Default::default()),
                };
                ed.update_channel(channel, |c| {
                    c.instrument = InstrumentSpec::Sampler {
                        sample: Some(t.reference.clone()),
                        root_key,
                        params: params.clone(),
                    }
                })?;
                placed.push(json!({
                    "channel": channel, "sample": id, "file": t.reference,
                }));
            }
            (None, None) => warnings.push(format!("{}: no target, only registered", t.reference)),
        }
    }
    let mut note_ids = Vec::new();
    if let Some(target) = out.note_target {
        let len = ctx.project.pattern_length(target.pattern)?.max(1);
        ctx.project.channel(target.channel)?;
        for n in &out.notes {
            let mut start = n.start;
            if let Some(q) = p.quantize {
                start = ((start + q / 2) / q) * q;
            }
            let start = start.min(len - 1);
            let length = n.length.max(1).min(len - start);
            let mut spec = NoteSpec::new(n.key, start, length);
            spec.velocity = n.velocity.clamp(0.01, 1.0);
            note_ids.push(ed.add_note(target.pattern, target.channel, spec)?);
        }
    }
    let mut parts = Vec::new();
    if !out.takes.is_empty() {
        parts.push(plural(out.takes.len(), "take", "takes"));
    }
    if !note_ids.is_empty() {
        parts.push(plural(note_ids.len(), "note", "notes"));
    }
    let summary = if parts.is_empty() {
        "recording stopped, nothing was recorded".to_string()
    } else {
        format!("recorded {}", parts.join(" and "))
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({
            "takes": placed,
            "notes": note_ids,
            "pattern": out.note_target.map(|t| t.pattern),
            "channel": out.note_target.map(|t| t.channel),
            "warnings": warnings,
        }),
        summary,
    ))
}

// ---- MIDI learn

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Learn {
    /// The parameter to map (see automation.add_clip for the form); omit to cancel a
    /// learn in progress.
    #[serde(default)]
    target: Option<ParamPath>,
}

fn learn(ctx: &mut CommandCtx<'_>, p: Learn) -> CmdResult<CommandOutput> {
    if let Some(t) = &p.target {
        check_target(ctx.project, t)?;
    }
    ctx.host.midi_learn(p.target.clone())?;
    let summary = match &p.target {
        Some(t) => format!("move a controller to map it to {t}"),
        None => "MIDI learn cancelled".to_string(),
    };
    Ok(said(json!({ "learning": p.target }), summary))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Map {
    /// MIDI CC number 0..=127.
    controller: u8,
    /// The parameter it drives.
    target: ParamPath,
    /// Only this MIDI input device; omit for any.
    #[serde(default)]
    device: Option<String>,
}

/// The target must exist in this project (a mapping to nothing would be dead weight).
fn check_target(p: &Project, t: &ParamPath) -> CmdResult<()> {
    let ok = match t {
        ParamPath::Tempo => true,
        ParamPath::ChannelVolume(c)
        | ParamPath::ChannelPan(c)
        | ParamPath::ChannelParam(c, _) => p.channels.contains_key(c),
        ParamPath::InsertVolume(i)
        | ParamPath::InsertPan(i)
        | ParamPath::InsertMute(i)
        | ParamPath::InsertSend { insert: i, .. }
        | ParamPath::EffectParam { insert: i, .. } => p.insert(*i).is_ok(),
    };
    if ok {
        Ok(())
    } else {
        invalid(format!("{t}: that does not exist in this project"))
    }
}

fn map(ctx: &mut CommandCtx<'_>, p: Map) -> CmdResult<CommandOutput> {
    if p.controller > 127 {
        return invalid("a MIDI controller number is 0..127");
    }
    check_target(ctx.project, &p.target)?;
    let replaced = ctx.host.midi_map(MidiMappingEntry {
        controller: p.controller,
        device: p.device.clone(),
        target: p.target.clone(),
    })?;
    Ok(said(
        json!({
            "controller": p.controller,
            "device": p.device,
            "target": p.target,
            "replaced": replaced,
        }),
        format!("CC {} now moves {}", p.controller, p.target),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Unmap {
    /// MIDI CC number 0..=127.
    controller: u8,
    /// Only this MIDI input device's mapping; omit for any.
    #[serde(default)]
    device: Option<String>,
}

fn unmap(ctx: &mut CommandCtx<'_>, p: Unmap) -> CmdResult<CommandOutput> {
    match ctx.host.midi_unmap(p.controller, p.device.clone())? {
        Some(m) => Ok(said(
            json!({ "removed": m }),
            format!("CC {} no longer moves {}", m.controller, m.target),
        )),
        None => invalid(format!(
            "CC {} has no mapping{}",
            p.controller,
            p.device
                .as_deref()
                .map_or(String::new(), |d| format!(" for {d}"))
        )),
    }
}
