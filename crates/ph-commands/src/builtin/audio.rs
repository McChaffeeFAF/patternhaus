//! `audio.*`: audio clips in the playlist (M2). Model-only: playback is the engine's
//! (M2/T1). Moving, removing and resizing any clip kind stays with `playlist.*`.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "audio.add_clip",
        "Place an audio clip on a playlist track, from a registered sample or a file to import.",
        Scope::Compose,
        MUTATES,
        add_clip,
    ));
    r.register(typed(
        "audio.set_fade",
        "Set an audio clip's fade-in and/or fade-out length and curve.",
        Scope::Compose,
        MUTATES,
        set_fade,
    ));
    r.register(typed(
        "audio.set_gain",
        "Set an audio clip's gain in dB.",
        Scope::Compose,
        MUTATES,
        set_gain,
    ));
    r.register(typed(
        "audio.set_pitch",
        "Transpose an audio clip in semitones and/or fine-tune it in cents.",
        Scope::Compose,
        MUTATES,
        set_pitch,
    ));
    r.register(typed(
        "audio.set_reverse",
        "Play an audio clip backwards, or forwards again.",
        Scope::Compose,
        MUTATES,
        set_reverse,
    ));
    r.register(typed(
        "audio.set_stretch",
        "Choose how an audio clip fits time (resample, stretch or slice) and whether it follows the project tempo.",
        Scope::Compose,
        MUTATES,
        set_stretch,
    ));
    r.register(typed(
        "audio.set_range",
        "Trim or slip an audio clip: its playlist start and length, and where in the file it starts.",
        Scope::Compose,
        MUTATES,
        set_range,
    ));
    r.register(typed(
        "audio.split",
        "Split an audio clip in two at a song position.",
        Scope::Compose,
        MUTATES,
        split,
    ));
    r.register(typed(
        "audio.fit_to_tempo",
        "Make an audio clip follow the project tempo using its sample's BPM, sized to the whole loop.",
        Scope::Compose,
        MUTATES,
        fit_to_tempo,
    ));
}

/// The audio settings of clip `id`, which must be an audio clip.
fn audio_of(p: &Project, id: ClipId) -> CmdResult<(Clip, AudioClip)> {
    let c = p.clip(id)?;
    match c.audio() {
        Some(a) => Ok((c.clone(), a.clone())),
        None => invalid(format!(
            "clip {id} is a {} clip, not an audio clip",
            c.kind_name()
        )),
    }
}

fn update_audio(
    ed: &mut Edit,
    id: ClipId,
    f: impl FnOnce(&mut Clip, &mut AudioClip),
) -> CmdResult<()> {
    ed.update_clip(id, |c| {
        let mut kind = std::mem::replace(
            &mut c.kind,
            ClipKind::Pattern {
                pattern: PatternId(0),
            },
        );
        if let ClipKind::Audio(a) = &mut kind {
            f(c, a);
        }
        c.kind = kind;
    })?;
    Ok(())
}

/// Source frames played over `ticks` of the playlist at the project tempo.
fn source_frames(p: &Project, entry: &SampleEntry, a: &AudioClip, ticks: Tick) -> u64 {
    let bpm = p.settings.bpm;
    let seconds = ticks_to_seconds(ticks, bpm) * a.time_ratio(entry, bpm);
    (seconds * entry.sample_rate as f64).round() as u64
}

/// Playlist ticks that play source frames `from..` to the end of the file.
fn natural_length(p: &Project, entry: &SampleEntry, a: &AudioClip) -> Tick {
    let bpm = p.settings.bpm;
    let frames = entry.frames.saturating_sub(a.source_offset);
    let seconds = frames as f64 / entry.sample_rate.max(1) as f64 / a.time_ratio(entry, bpm);
    (seconds_to_ticks(seconds, bpm).ceil() as Tick).max(1)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddClip {
    /// Playlist track index, 0 = first track.
    track: u32,
    /// Start in ticks (960 per quarter note).
    start: Tick,
    /// A registered sample (see sample.list). Give this or `path`.
    #[serde(default)]
    sample: Option<SampleId>,
    /// An audio file to copy into the project's samples folder first (agents: inside the
    /// project or library folders), or an asset reference such as `samples/loop.wav` or
    /// `factory:drums/haus808/kick`. Give this or `sample`.
    #[serde(default)]
    path: Option<String>,
    /// Length in ticks; defaults to the rest of the sample (at its own tempo when synced).
    #[serde(default)]
    length: Option<Tick>,
    /// Follow the project tempo (needs the sample's BPM, see sample.set_bpm).
    #[serde(default)]
    tempo_sync: bool,
    /// How the clip fits time: `resample` (default; speed changes pitch), `stretch` (keeps
    /// pitch) or `slice` (re-times at transients).
    #[serde(default)]
    stretch: StretchMode,
    /// Where in the file to start, in frames of the file (default 0).
    #[serde(default)]
    source_offset: u64,
}

fn add_clip(ctx: &mut CommandCtx<'_>, p: AddClip) -> CmdResult<CommandOutput> {
    if (p.track as usize) >= ctx.project.tracks.len() {
        return Err(ModelError::NoTrack(p.track as usize).into());
    }
    let mut ed = Edit::new(ctx.project);
    let sample = match (p.sample, &p.path) {
        (Some(s), None) => {
            ctx.project.sample(s)?;
            s
        }
        (None, Some(path)) => {
            let registered = ctx.project.sample_by_path(path).map(|e| e.id);
            match registered {
                Some(id) => id,
                None if path.starts_with("factory:") => {
                    register_sample(&mut ed, &*ctx.host, path, None)?
                }
                None => {
                    let reference =
                        ctx.host
                            .import_file(Path::new(path), ImportKind::Sample, ctx.author)?;
                    register_sample(&mut ed, &*ctx.host, &reference, Some(path))?
                }
            }
        }
        _ => return invalid("give exactly one of sample and path"),
    };
    let entry = ed.project().sample(sample)?.clone();
    if p.source_offset >= entry.frames {
        return invalid(format!(
            "source_offset {} is past the end of {} ({} frames)",
            p.source_offset, entry.name, entry.frames
        ));
    }
    let mut a = AudioClip::new(sample);
    a.tempo_sync = p.tempo_sync;
    a.stretch = p.stretch;
    a.source_offset = p.source_offset;
    let length = match p.length {
        Some(0) => return invalid("length must be greater than zero"),
        Some(l) => l,
        None => natural_length(ed.project(), &entry, &a),
    };
    let id = ed.add_audio_clip(p.track, p.start, length, a)?;
    let mut warnings = Vec::new();
    if p.tempo_sync && entry.bpm.is_none() {
        warnings.push(format!(
            "{} has no tempo, so the clip plays unsynced until one is set (sample.set_bpm)",
            entry.name
        ));
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": id, "sample": sample, "length": length, "warnings": warnings }),
        format!(
            "placed {} on track {} at tick {}",
            entry.name,
            p.track + 1,
            p.start
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetFade {
    /// Audio clip id.
    clip: ClipId,
    /// Fade-in: `{ length (ticks), curve: linear | exp | log | s_curve }`; omit to keep.
    #[serde(default)]
    fade_in: Option<Fade>,
    /// Fade-out, as `fade_in`; omit to keep.
    #[serde(default)]
    fade_out: Option<Fade>,
}

fn set_fade(ctx: &mut CommandCtx<'_>, p: SetFade) -> CmdResult<CommandOutput> {
    audio_of(ctx.project, p.clip)?;
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |_, a| {
        if let Some(f) = p.fade_in {
            a.fade_in = f;
        }
        if let Some(f) = p.fade_out {
            a.fade_out = f;
        }
    })?;
    let (_, a) = audio_of(ed.project(), p.clip)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "fade_in": a.fade_in, "fade_out": a.fade_out }),
        format!(
            "set fades of clip {} to {} in, {} out",
            p.clip, a.fade_in.length, a.fade_out.length
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetGain {
    /// Audio clip id.
    clip: ClipId,
    /// Gain in dB, -60..=24 (0 = unchanged).
    gain_db: f32,
}

fn set_gain(ctx: &mut CommandCtx<'_>, p: SetGain) -> CmdResult<CommandOutput> {
    audio_of(ctx.project, p.clip)?;
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |_, a| a.gain_db = p.gain_db)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "gain_db": p.gain_db }),
        format!("set clip {} to {} dB", p.clip, p.gain_db),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetPitch {
    /// Audio clip id.
    clip: ClipId,
    /// Transposition in semitones, -48..=48; omit to keep.
    #[serde(default)]
    semitones: Option<i32>,
    /// Fine tuning in cents, -100..=100; omit to keep.
    #[serde(default)]
    cents: Option<f32>,
}

fn set_pitch(ctx: &mut CommandCtx<'_>, p: SetPitch) -> CmdResult<CommandOutput> {
    audio_of(ctx.project, p.clip)?;
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |_, a| {
        if let Some(s) = p.semitones {
            a.pitch_semitones = s;
        }
        if let Some(c) = p.cents {
            a.fine_cents = c;
        }
    })?;
    let (_, a) = audio_of(ed.project(), p.clip)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "semitones": a.pitch_semitones, "cents": a.fine_cents }),
        format!("pitched clip {} to {:+} semitones", p.clip, a.pitch()),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetReverse {
    /// Audio clip id.
    clip: ClipId,
    /// True plays the clip backwards.
    reverse: bool,
}

fn set_reverse(ctx: &mut CommandCtx<'_>, p: SetReverse) -> CmdResult<CommandOutput> {
    audio_of(ctx.project, p.clip)?;
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |_, a| a.reverse = p.reverse)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "reverse": p.reverse }),
        format!(
            "clip {} plays {}",
            p.clip,
            if p.reverse { "backwards" } else { "forwards" }
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetStretch {
    /// Audio clip id.
    clip: ClipId,
    /// `resample`, `stretch` or `slice`; omit to keep.
    #[serde(default)]
    mode: Option<StretchMode>,
    /// Follow the project tempo; omit to keep.
    #[serde(default)]
    tempo_sync: Option<bool>,
}

fn set_stretch(ctx: &mut CommandCtx<'_>, p: SetStretch) -> CmdResult<CommandOutput> {
    audio_of(ctx.project, p.clip)?;
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |_, a| {
        if let Some(m) = p.mode {
            a.stretch = m;
        }
        if let Some(t) = p.tempo_sync {
            a.tempo_sync = t;
        }
    })?;
    let (_, a) = audio_of(ed.project(), p.clip)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "mode": a.stretch, "tempo_sync": a.tempo_sync }),
        format!("set clip {} to {:?}", p.clip, a.stretch).to_lowercase(),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRange {
    /// Audio clip id.
    clip: ClipId,
    /// New playlist start in ticks; omit to keep.
    #[serde(default)]
    start: Option<Tick>,
    /// New length in ticks; omit to keep.
    #[serde(default)]
    length: Option<Tick>,
    /// Where in the file the clip starts, in frames of the file; omit to keep (slip edit).
    #[serde(default)]
    source_offset: Option<u64>,
}

fn set_range(ctx: &mut CommandCtx<'_>, p: SetRange) -> CmdResult<CommandOutput> {
    let (_, a) = audio_of(ctx.project, p.clip)?;
    let frames = ctx.project.sample(a.sample)?.frames;
    if p.source_offset.is_some_and(|o| o >= frames) {
        return invalid(format!(
            "source_offset is past the end of the file ({frames} frames)"
        ));
    }
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |c, a| {
        if let Some(s) = p.start {
            c.start = s;
        }
        if let Some(l) = p.length {
            c.length = l;
        }
        if let Some(o) = p.source_offset {
            a.source_offset = o;
        }
    })?;
    let c = ed.project().clip(p.clip)?.clone();
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clip": p.clip, "start": c.start, "length": c.length }),
        format!(
            "trimmed clip {} to ticks {}..{}",
            p.clip,
            c.start,
            c.start + c.length
        ),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Split {
    /// Audio clip id.
    clip: ClipId,
    /// Song position in ticks, strictly inside the clip.
    at: Tick,
}

fn split(ctx: &mut CommandCtx<'_>, p: Split) -> CmdResult<CommandOutput> {
    let (c, a) = audio_of(ctx.project, p.clip)?;
    if p.at <= c.start || p.at >= c.start + c.length {
        return invalid(format!(
            "tick {} is not inside the clip ({}..{})",
            p.at,
            c.start,
            c.start + c.length
        ));
    }
    let entry = ctx.project.sample(a.sample)?.clone();
    let left_len = p.at - c.start;
    let right_len = c.length - left_len;
    let fd = source_frames(ctx.project, &entry, &a, left_len);
    let span = source_frames(ctx.project, &entry, &a, c.length);
    let mut right = a.clone();
    right.fade_in = Fade::default();
    right.fade_out.length = right.fade_out.length.min(right_len);
    let mut left_offset = a.source_offset;
    if a.reverse {
        // played backwards: the left part plays the end of the range
        left_offset = a.source_offset + span.saturating_sub(fd);
    } else {
        right.source_offset = a.source_offset + fd;
    }
    let mut ed = Edit::new(ctx.project);
    update_audio(&mut ed, p.clip, |c, a| {
        c.length = left_len;
        a.source_offset = left_offset;
        a.fade_out = Fade::default();
        a.fade_in.length = a.fade_in.length.min(left_len);
    })?;
    let new = ed.add_audio_clip(c.track, p.at, right_len, right)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "clips": [p.clip, new] }),
        format!("split {} at tick {}", entry.name, p.at),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClipOnly {
    /// Audio clip id.
    clip: ClipId,
}

fn fit_to_tempo(ctx: &mut CommandCtx<'_>, p: ClipOnly) -> CmdResult<CommandOutput> {
    let (_, a) = audio_of(ctx.project, p.clip)?;
    let mut entry = ctx.project.sample(a.sample)?.clone();
    let mut ed = Edit::new(ctx.project);
    let mut detected = None;
    if entry.bpm.is_none() {
        // detect it first (stored like sample.detect_bpm would)
        let bpm = ctx
            .host
            .analyze_audio(&entry.path, 0.5)
            .ok()
            .and_then(|r| r.bpm.map(|b| (b, r.key)));
        let Some((bpm, key)) = bpm else {
            return invalid(format!(
                "{} has no tempo and none could be detected: set it with sample.set_bpm",
                entry.name
            ));
        };
        ed.update_sample(a.sample, |e| {
            e.bpm = Some(SampleBpm {
                value: bpm,
                source: BpmSource::Detected,
            });
            e.analysis.onsets = Some(key);
        })?;
        entry = ed.project().sample(a.sample)?.clone();
        detected = Some(bpm);
    }
    let mut fitted = a.clone();
    fitted.tempo_sync = true;
    if fitted.stretch == StretchMode::Resample {
        fitted.stretch = StretchMode::Stretch;
    }
    // the loop's length in ticks, to the nearest tick (a loop is a whole number of beats
    // give or take a rounded frame; `ceil` would add a tick)
    let length = {
        let bpm = ctx.project.settings.bpm;
        let frames = entry.frames.saturating_sub(fitted.source_offset);
        let seconds =
            frames as f64 / entry.sample_rate.max(1) as f64 / fitted.time_ratio(&entry, bpm);
        (seconds_to_ticks(seconds, bpm).round() as Tick).max(1)
    };
    update_audio(&mut ed, p.clip, |c, a| {
        *a = fitted.clone();
        c.length = length;
    })?;
    let mut result = json!({ "clip": p.clip, "length": length, "mode": fitted.stretch });
    if let Some(bpm) = detected {
        result["detected_bpm"] = json!(bpm);
    }
    Ok(CommandOutput::from_edit(
        ed,
        result,
        format!("fitted {} to the project tempo", entry.name),
    ))
}
