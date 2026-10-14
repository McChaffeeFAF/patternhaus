//! M2 command families whose implementation needs devices or DSP: `record.*` and `midi.*`
//! (T8), `slicer.make_instrument` and `editor.*` (T6). (`plugin.*`, T3, is implemented in
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
    // ---- recording (M2/T8)
    stub!(
        r,
        "record.list_inputs",
        "List the audio inputs that can be recorded, with their monitoring latency.",
        Scope::Read,
        READ,
        NoParams,
        "audio input recording",
        "T8"
    );
    stub!(
        r,
        "record.arm",
        "Arm an audio input to record onto a playlist track or into a Sampler channel.",
        Scope::Compose,
        READ,
        Arm,
        "audio input recording",
        "T8"
    );
    stub!(
        r,
        "record.disarm",
        "Disarm an audio input.",
        Scope::Compose,
        READ,
        InputOnly,
        "audio input recording",
        "T8"
    );
    stub!(
        r,
        "record.set_monitor",
        "Hear an armed input while recording, or stop hearing it.",
        Scope::Compose,
        READ,
        Monitor,
        "audio input recording",
        "T8"
    );
    stub!(
        r,
        "record.start",
        "Start recording every armed input (with count-in and punch range if given).",
        Scope::Compose,
        READ,
        Start,
        "audio input recording",
        "T8"
    );
    stub!(
        r,
        "record.stop",
        "Stop recording and add each take to the project as an audio clip.",
        Scope::Compose,
        MUTATES,
        NoParams,
        "audio input recording",
        "T8"
    );
    // ---- MIDI controllers (M2/T8)
    stub!(
        r,
        "midi.list_inputs",
        "List MIDI input devices.",
        Scope::Read,
        READ,
        NoParams,
        "MIDI input",
        "T8"
    );
    stub!(
        r,
        "midi.learn",
        "Map the next controller moved on a MIDI device to a parameter.",
        Scope::Full,
        READ,
        Learn,
        "MIDI learn",
        "T8"
    );
    stub!(
        r,
        "midi.map",
        "Map a MIDI controller (CC) to a parameter (saved per user).",
        Scope::Full,
        READ,
        Map,
        "MIDI learn",
        "T8"
    );
    stub!(
        r,
        "midi.unmap",
        "Remove a MIDI controller mapping.",
        Scope::Full,
        READ,
        Unmap,
        "MIDI learn",
        "T8"
    );
    stub!(
        r,
        "midi.list_mappings",
        "List the MIDI controller mappings.",
        Scope::Read,
        READ,
        NoParams,
        "MIDI learn",
        "T8"
    );
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
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Learn {
    /// The parameter to map (see automation.add_clip for the form).
    target: ParamPath,
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Unmap {
    /// MIDI CC number 0..=127.
    controller: u8,
    /// Only this MIDI input device's mapping; omit for any.
    #[serde(default)]
    device: Option<String>,
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
