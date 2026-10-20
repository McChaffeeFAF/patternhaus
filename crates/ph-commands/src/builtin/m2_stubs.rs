//! M2 command families whose implementation needs devices or DSP: `slicer.make_instrument`
//! and `editor.*` (T6). (`record.*` and `midi.*`, T8, are implemented in `record.rs`.) (`plugin.*`, T3, is implemented in
//! `plugin.rs`; `slicer.detect`, T1, in `slicer.rs`.)
//!
//! Their names, scopes and parameter schemas are fixed here so agents, the coverage guard
//! and the parallel tracks agree on them; each returns `CmdError::Unavailable` naming its
//! track until that track replaces the stub with the real handler (same id, same params).

#![allow(dead_code)] // the parameters are read by the tracks that replace these stubs

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;

macro_rules! stub {
    ($r:ident, $id:literal, $summary:literal, $scope:expr, $flags:expr, $params:ty, $feature:literal, $track:literal) => {{
        fn handler(_: &mut CommandCtx<'_>, _: $params) -> CmdResult<CommandOutput> {
            unavailable($id, $feature, $track)
        }
        $r.register(typed($id, $summary, $scope, $flags, handler));
    }};
}

pub fn register(r: &mut Registry) {
    // ---- slicer (`slicer.detect` is in `slicer.rs`, M2/T1)
    stub!(
        r,
        "slicer.make_instrument",
        "Build a sliced Sampler channel from a sample, one slice per key, optionally with a pattern that replays the loop.",
        Scope::Compose,
        MUTATES,
        MakeInstrument,
        "the slicer",
        "T6"
    );
    // ---- audio editor (M2/T6)
    stub!(
        r,
        "editor.open",
        "Open a sample in the audio editor.",
        Scope::Compose,
        READ,
        EditorOpen,
        "the audio editor",
        "T6"
    );
    stub!(
        r,
        "editor.apply",
        "Apply an edit to a sample; the result is a new file and registry entry, the original is never overwritten.",
        Scope::Compose,
        MUTATES,
        EditorApply,
        "the audio editor",
        "T6"
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MakeInstrument {
    /// Sample registry id.
    sample: SampleId,
    /// Slice start points in frames of the file; omit to detect them.
    #[serde(default)]
    slices: Option<Vec<u64>>,
    /// Also write a pattern that replays the original loop.
    #[serde(default)]
    write_pattern: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditorOpen {
    /// Sample registry id.
    sample: SampleId,
}

/// An audio editor operation.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EditorOp {
    Trim,
    Cut,
    Silence,
    Normalize,
    FadeIn,
    FadeOut,
    Reverse,
    Gain,
    Declick,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditorApply {
    /// Sample registry id.
    sample: SampleId,
    /// trim, cut, silence, normalize, fade_in, fade_out, reverse, gain or declick.
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
}
