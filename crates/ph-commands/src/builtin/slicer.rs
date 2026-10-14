//! `slicer.*`: transients of a sample (M2/T1). Building a sliced instrument from them is
//! `slicer.make_instrument` (M2/T6).

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
