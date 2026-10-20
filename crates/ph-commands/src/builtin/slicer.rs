//! `slicer.*`: transients of a sample (M2/T1) and the sliced Sampler instrument built from
//! them (`slicer.make_instrument`, M2/T6).

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "slicer.detect",
        "Find the transients of a sample and return slice points.",
        Scope::Compose,
        READ,
        detect,
    ));
    r.register(typed(
        "slicer.make_instrument",
        "Build a sliced Sampler channel from a sample, one slice per key, optionally with a pattern that replays the loop.",
        Scope::Compose,
        MUTATES,
        make_instrument,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Detect {
    /// Sample registry id.
    sample: SampleId,
    /// 0..=1, higher finds more transients (default 0.5).
    #[serde(default)]
    sensitivity: Option<f32>,
}

fn detect(ctx: &mut CommandCtx<'_>, p: Detect) -> CmdResult<CommandOutput> {
    let entry = ctx.project.sample(p.sample)?.clone();
    let sensitivity = p.sensitivity.unwrap_or(0.5);
    if !sensitivity.is_finite() || !(0.0..=1.0).contains(&sensitivity) {
        return invalid(format!("sensitivity {sensitivity} outside 0..1"));
    }
    let a = ctx.host.analyze_audio(&entry.path, sensitivity)?;
    // a slice starts at the file start and at every transient after it
    let mut slices: Vec<u64> = vec![0];
    slices.extend(a.onsets.iter().copied().filter(|o| *o > 0));
    let rate = a.sample_rate.max(1) as f64;
    let seconds: Vec<f64> = slices
        .iter()
        .map(|f| (*f as f64 / rate * 1e4).round() / 1e4)
        .collect();
    Ok(CommandOutput::value(json!({
        "sample": p.sample,
        "sensitivity": sensitivity,
        "slices": slices,
        "seconds": seconds,
        "bpm": a.bpm,
    })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MakeInstrument {
    /// Sample registry id.
    sample: SampleId,
    /// Slice start points in frames of the file, ascending; omit to detect them
    /// (see slicer.detect). Slice `i` plays on key `root_key + i`.
    #[serde(default)]
    slices: Option<Vec<u64>>,
    /// Also write a pattern that replays the original loop.
    #[serde(default)]
    write_pattern: bool,
    /// Detection sensitivity 0..=1 when `slices` is omitted (default 0.5).
    #[serde(default)]
    sensitivity: Option<f32>,
    /// MIDI key of the first slice (default 36, C2).
    #[serde(default)]
    root_key: Option<u8>,
    /// Name of the new channel (default: the sample's name).
    #[serde(default)]
    name: Option<String>,
}

fn make_instrument(ctx: &mut CommandCtx<'_>, p: MakeInstrument) -> CmdResult<CommandOutput> {
    let entry = ctx.project.sample(p.sample)?.clone();
    let root_key = p.root_key.unwrap_or(36);
    if root_key > 127 {
        return invalid(format!("root_key {root_key} is above 127"));
    }
    let slices: Vec<u64> = match p.slices {
        Some(s) => s,
        None => {
            let sens = p.sensitivity.unwrap_or(0.5);
            if !sens.is_finite() || !(0.0..=1.0).contains(&sens) {
                return invalid(format!("sensitivity {sens} outside 0..1"));
            }
            let a = ctx.host.analyze_audio(&entry.path, sens)?;
            let mut v = vec![0];
            v.extend(a.onsets.iter().copied().filter(|o| *o > 0));
            v
        }
    };
    if slices.is_empty() {
        return invalid("a sliced instrument needs at least one slice point");
    }
    if slices.windows(2).any(|w| w[1] <= w[0]) {
        return invalid("slice points must be strictly ascending");
    }
    if let Some(last) = slices.last()
        && *last >= entry.frames
    {
        return invalid(format!(
            "slice point {last} is past the end of {} ({} frames)",
            entry.name, entry.frames
        ));
    }
    let keys = 128 - root_key as usize;
    if slices.len() > keys {
        return invalid(format!(
            "{} slices do not fit on the keys above {root_key}: at most {keys}",
            slices.len()
        ));
    }
    let mut ed = Edit::new(ctx.project);
    let stem = std::path::Path::new(&entry.name)
        .file_stem()
        .map_or_else(|| entry.name.clone(), |s| s.to_string_lossy().into_owned());
    let name = p.name.clone().unwrap_or_else(|| stem.clone());
    let channel = ed.add_channel(
        &name,
        InstrumentSpec::Sampler {
            sample: Some(entry.path.clone()),
            root_key,
            params: Default::default(),
            slices: slices.clone(),
        },
    )?;
    let mut result = json!({
        "channel": channel, "slices": slices.len(),
        "first_key": root_key, "last_key": root_key as usize + slices.len() - 1,
    });
    let mut summary = format!("sliced {} into {} keys", entry.name, slices.len());
    if p.write_pattern {
        // where each slice starts in the loop: at the sample's own tempo if it has one (so the
        // pattern lays out the original timing), else at the project tempo
        let bpm = entry.bpm().unwrap_or(ctx.project.settings.bpm);
        let rate = entry.sample_rate.max(1) as f64;
        let tick = |frame: u64| seconds_to_ticks(frame as f64 / rate, bpm).round() as Tick;
        let pattern = ed.add_pattern(&format!("{stem} loop"))?;
        let total = tick(entry.frames).max(1);
        for (i, f) in slices.iter().enumerate() {
            let start = tick(*f);
            let end = slices.get(i + 1).map_or(total, |n| tick(*n));
            let mut spec =
                NoteSpec::new(root_key + i as u8, start, end.saturating_sub(start).max(1));
            spec.velocity = 0.8;
            ed.add_note(pattern, channel, spec)?;
        }
        let bar = ctx.project.settings.time_signature.ticks_per_bar().max(1);
        ed.update_pattern(pattern, |h| {
            h.length = Some(total.div_ceil(bar).max(1) * bar)
        })?;
        result["pattern"] = json!(pattern);
        summary.push_str(" and wrote a pattern that replays the loop");
    }
    Ok(CommandOutput::from_edit(ed, result, summary))
}
