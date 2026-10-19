//! `editor.*`: the audio editor (M2/T6). `editor.open` shows a sample in the editor window;
//! `editor.apply` edits it **without ever changing a file**: the result is written as a new
//! file in the project's `samples/` folder (`Host::write_audio`), registered as a new sample
//! (`Origin::Edited`), and the audio clips and Sampler channels that played the old one are
//! pointed at the new one, all in one undo step. The original file and its registry entry
//! stay, so undo only has to put the pointers back.

use super::util::*;
use crate::audio_edit as ae;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "editor.open",
        "Open a sample in the audio editor.",
        Scope::Compose,
        READ,
        open,
    ));
    r.register(typed(
        "editor.apply",
        "Apply an edit to a sample; the result is a new file and registry entry, the original is never overwritten.",
        Scope::Compose,
        MUTATES,
        apply,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditorOpen {
    /// Sample registry id.
    sample: SampleId,
}

fn open(ctx: &mut CommandCtx<'_>, p: EditorOpen) -> CmdResult<CommandOutput> {
    let e = ctx.project.sample(p.sample)?.clone();
    ctx.host.ui(UiRequest::Open {
        panel: format!("audio_editor:{}", p.sample.0),
    })?;
    Ok(CommandOutput::value(json!({
        "sample": p.sample, "name": e.name, "path": e.path, "frames": e.frames,
        "sample_rate": e.sample_rate, "channels": e.channels, "seconds": e.seconds(),
    })))
}

/// An audio editor operation.
#[derive(Copy, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EditorOp {
    /// Keep only the selection.
    Trim,
    /// Remove the selection.
    Cut,
    /// Replace the selection by silence.
    Silence,
    /// Scale the selection so its peak reaches `amount` dBFS (default 0).
    Normalize,
    /// Fade the selection from silence up (`curve`: linear, exp, log, s_curve).
    FadeIn,
    /// Fade the selection down to silence.
    FadeOut,
    /// Play the selection backwards.
    Reverse,
    /// Change the selection's level by `amount` dB.
    Gain,
    /// Remove clicks (short spikes) inside the selection.
    Declick,
    /// Insert audio copied from `from` (`from_start..from_end`, the whole file by default)
    /// at frame `at` (default: the selection start).
    Paste,
}

impl EditorOp {
    fn name(self) -> &'static str {
        match self {
            EditorOp::Trim => "trim",
            EditorOp::Cut => "cut",
            EditorOp::Silence => "silence",
            EditorOp::Normalize => "normalize",
            EditorOp::FadeIn => "fade_in",
            EditorOp::FadeOut => "fade_out",
            EditorOp::Reverse => "reverse",
            EditorOp::Gain => "gain",
            EditorOp::Declick => "declick",
            EditorOp::Paste => "paste",
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditorApply {
    /// Sample registry id.
    sample: SampleId,
    /// trim, cut, silence, normalize, fade_in, fade_out, reverse, gain, declick or paste.
    operation: EditorOp,
    /// Selection start in frames; omit for the whole file.
    #[serde(default)]
    start: Option<u64>,
    /// Selection end in frames (exclusive); omit for the whole file.
    #[serde(default)]
    end: Option<u64>,
    /// Gain in dB for `gain`, target peak in dBFS for `normalize`.
    #[serde(default)]
    amount: Option<f64>,
    /// Fade shape for fade_in and fade_out: linear (default), exp, log or s_curve.
    #[serde(default)]
    curve: Option<FadeCurve>,
    /// paste: the sample to copy from (default: the same sample).
    #[serde(default)]
    from: Option<SampleId>,
    /// paste: first frame to copy (default 0).
    #[serde(default)]
    from_start: Option<u64>,
    /// paste: end frame to copy, exclusive (default: the end of the source).
    #[serde(default)]
    from_end: Option<u64>,
    /// paste: frame to insert at (default: the selection start, else the end of the file).
    #[serde(default)]
    at: Option<u64>,
    /// Point the audio clips and Sampler channels that play this sample at the new file
    /// (default true). With false the new sample is only registered.
    #[serde(default)]
    repoint: Option<bool>,
}

/// How source positions move when an operation changes the length.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Remap {
    Same,
    /// Only frames `.0..`.1 are kept.
    Trim(u64, u64),
    /// Frames `.0..`.1 are removed.
    Cut(u64, u64),
    /// `.1` frames are inserted at `.0`.
    Insert(u64, u64),
}

impl Remap {
    fn map(self, f: u64) -> u64 {
        match self {
            Remap::Same => f,
            Remap::Trim(s, e) => f.clamp(s, e) - s,
            Remap::Cut(s, e) => {
                if f >= e {
                    f - (e - s)
                } else {
                    f.min(s)
                }
            }
            Remap::Insert(at, n) => {
                if f >= at {
                    f + n
                } else {
                    f
                }
            }
        }
    }
}

fn apply(ctx: &mut CommandCtx<'_>, p: EditorApply) -> CmdResult<CommandOutput> {
    let entry = ctx.project.sample(p.sample)?.clone();
    let audio = ctx.host.read_audio(&entry.path)?;
    let len = audio.frames();
    if len == 0 {
        return invalid(format!("{} is empty", entry.name));
    }
    let range = ae::clamp_range(len, p.start, p.end);
    let whole = p.start.is_none() && p.end.is_none();
    let (s, e) = range;
    let empty = || invalid::<()>(format!("the selection {s}..{e} is empty"));
    let curve = p.curve.unwrap_or_default();
    let mut remap = Remap::Same;
    let mut note = String::new();
    let channels = match p.operation {
        EditorOp::Trim => {
            if s == e {
                empty()?;
            }
            remap = Remap::Trim(s as u64, e as u64);
            ae::trim(&audio.channels, range)
        }
        EditorOp::Cut => {
            if s == e {
                empty()?;
            }
            if e - s >= len {
                return invalid("cutting everything would leave an empty file");
            }
            remap = Remap::Cut(s as u64, e as u64);
            ae::cut(&audio.channels, range)
        }
        EditorOp::Silence => {
            if s == e {
                empty()?;
            }
            ae::silence(&audio.channels, range)
        }
        EditorOp::Normalize => {
            if s == e {
                empty()?;
            }
            let target = p.amount.unwrap_or(0.0);
            if !target.is_finite() || !(-60.0..=0.0).contains(&target) {
                return invalid(format!("normalize target {target} dBFS is outside -60..0"));
            }
            match ae::normalize(&audio.channels, range, target) {
                Some(c) => c,
                None => return invalid("the selection is silent, there is nothing to normalize"),
            }
        }
        EditorOp::FadeIn | EditorOp::FadeOut => {
            if s == e {
                empty()?;
            }
            ae::fade(
                &audio.channels,
                range,
                curve,
                matches!(p.operation, EditorOp::FadeIn),
            )
        }
        EditorOp::Reverse => {
            if s == e {
                empty()?;
            }
            ae::reverse(&audio.channels, range)
        }
        EditorOp::Gain => {
            if s == e {
                empty()?;
            }
            let db = p.amount.unwrap_or(0.0);
            if !db.is_finite() || !(-60.0..=24.0).contains(&db) {
                return invalid(format!("gain {db} dB is outside -60..24"));
            }
            ae::gain(&audio.channels, range, 10f64.powf(db / 20.0))
        }
        EditorOp::Declick => {
            if s == e {
                empty()?;
            }
            let (c, n) = ae::declick(&audio.channels, range);
            if n == 0 {
                return invalid("no clicks found in the selection");
            }
            note = format!("{n} clicks removed");
            c
        }
        EditorOp::Paste => {
            let src_id = p.from.unwrap_or(p.sample);
            let src_entry = ctx.project.sample(src_id)?.clone();
            let src = if src_id == p.sample {
                audio.clone()
            } else {
                ctx.host.read_audio(&src_entry.path)?
            };
            if src.sample_rate != audio.sample_rate {
                return invalid(format!(
                    "{} is {} Hz and {} is {} Hz: paste needs the same sample rate",
                    src_entry.name, src.sample_rate, entry.name, audio.sample_rate
                ));
            }
            let (fs, fe) = ae::clamp_range(src.frames(), p.from_start, p.from_end);
            if fs == fe {
                return invalid("nothing to paste: the copied range is empty");
            }
            let piece =
                ae::match_channels(&ae::trim(&src.channels, (fs, fe)), audio.channels.len());
            let at =
                p.at.map_or(if whole { len } else { s }, |a| a as usize)
                    .min(len);
            remap = Remap::Insert(at as u64, (fe - fs) as u64);
            ae::insert(&audio.channels, &piece, at)
        }
    };

    // the new file: a suggestion for its name; the host never replaces a file
    let stem = std::path::Path::new(&entry.name)
        .file_stem()
        .map_or_else(|| entry.name.clone(), |s| s.to_string_lossy().into_owned());
    let new_audio = AudioBuffer {
        channels,
        sample_rate: audio.sample_rate,
        root_key: audio.root_key,
    };
    let reference = ctx
        .host
        .write_audio(&format!("{stem} ({})", p.operation.name()), &new_audio)?;
    let mut ed = Edit::new(ctx.project);
    let name = std::path::Path::new(&reference)
        .file_name()
        .map_or_else(|| reference.clone(), |n| n.to_string_lossy().into_owned());
    let new_id = ed.add_sample(SampleEntry {
        id: SampleId(0),
        path: reference.clone(),
        name,
        frames: new_audio.frames() as u64,
        sample_rate: new_audio.sample_rate,
        channels: new_audio.channels.len() as u16,
        // a length-preserving edit keeps the material's tempo, a different length does not
        bpm: if remap == Remap::Same {
            entry.bpm
        } else {
            None
        },
        root_key: entry.root_key,
        analysis: SampleAnalysis::default(),
        provenance: Provenance {
            origin: Origin::Edited {
                from: p.sample,
                operation: p.operation.name().into(),
            },
            at: now_unix(),
        },
    })?;

    let (mut clips, mut channels_repointed) = (0usize, 0usize);
    if p.repoint.unwrap_or(true) {
        let users: Vec<ClipId> = ed
            .project()
            .clips
            .values()
            .filter(|c| c.audio().is_some_and(|a| a.sample == p.sample))
            .map(|c| c.id)
            .collect();
        let new_frames = new_audio.frames() as u64;
        for id in users {
            ed.update_clip(id, |c| {
                if let ClipKind::Audio(a) = &mut c.kind {
                    a.sample = new_id;
                    a.source_offset = remap.map(a.source_offset).min(new_frames.saturating_sub(1));
                }
            })?;
            clips += 1;
        }
        let chans: Vec<ChannelId> = ed
            .project()
            .channels
            .values()
            .filter(|c| {
                matches!(&c.instrument,
                    InstrumentSpec::Sampler { sample: Some(s), .. } if *s == entry.path)
            })
            .map(|c| c.id)
            .collect();
        for id in chans {
            ed.update_channel(id, |c| {
                if let InstrumentSpec::Sampler { sample, slices, .. } = &mut c.instrument {
                    *sample = Some(reference.clone());
                    for f in slices.iter_mut() {
                        *f = remap.map(*f).min(new_frames.saturating_sub(1));
                    }
                    slices.dedup();
                }
            })?;
            channels_repointed += 1;
        }
    }
    let mut result = json!({
        "sample": new_id, "from": p.sample, "path": reference, "frames": new_audio.frames(),
        "operation": p.operation.name(), "clips_repointed": clips,
        "channels_repointed": channels_repointed,
    });
    if !note.is_empty() {
        result["note"] = json!(note);
    }
    Ok(CommandOutput::from_edit(
        ed,
        result,
        format!(
            "{} {}{}",
            p.operation.name().replace('_', " "),
            entry.name,
            if note.is_empty() {
                String::new()
            } else {
                format!(" ({note})")
            }
        ),
    ))
}
