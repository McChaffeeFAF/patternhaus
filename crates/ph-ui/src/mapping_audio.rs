//! Gestures on audio clips, the audio editor and the slicer -> command requests (M2/T6).
//!
//! Same rule as [`crate::mapping`]: plain functions of ids and numbers, so the gesture to
//! command mapping is unit-tested without a window.

use ph_commands::Request;
use ph_core::{ClipId, Fade, FadeCurve, SampleId, StretchMode, Tick};
use serde_json::{Value, json};

fn req(command: &str, params: Value) -> Request {
    Request::new(command, params)
}

/// Place a file or registered sample on a track.
pub fn add_clip_path(track: u32, start: Tick, path: &str) -> Request {
    req(
        "audio.add_clip",
        json!({ "track": track, "start": start, "path": path }),
    )
}

pub fn add_clip_sample(track: u32, start: Tick, sample: SampleId) -> Request {
    req(
        "audio.add_clip",
        json!({ "track": track, "start": start, "sample": sample }),
    )
}

/// Trim or slip: any of the playlist start, the length and the source offset (frames).
pub fn set_range(
    clip: ClipId,
    start: Option<Tick>,
    length: Option<Tick>,
    source_offset: Option<u64>,
) -> Request {
    let mut p = json!({ "clip": clip });
    if let Some(s) = start {
        p["start"] = json!(s);
    }
    if let Some(l) = length {
        p["length"] = json!(l);
    }
    if let Some(o) = source_offset {
        p["source_offset"] = json!(o);
    }
    req("audio.set_range", p)
}

pub fn split(clip: ClipId, at: Tick) -> Request {
    req("audio.split", json!({ "clip": clip, "at": at }))
}

pub fn duplicate(clip: ClipId) -> Request {
    req("audio.duplicate", json!({ "clip": clip }))
}

pub fn set_fade_in(clip: ClipId, fade: Fade) -> Request {
    req("audio.set_fade", json!({ "clip": clip, "fade_in": fade }))
}

pub fn set_fade_out(clip: ClipId, fade: Fade) -> Request {
    req("audio.set_fade", json!({ "clip": clip, "fade_out": fade }))
}

pub fn set_gain(clip: ClipId, gain_db: f32) -> Request {
    req(
        "audio.set_gain",
        json!({ "clip": clip, "gain_db": gain_db }),
    )
}

pub fn set_pitch(clip: ClipId, semitones: Option<i32>, cents: Option<f32>) -> Request {
    req(
        "audio.set_pitch",
        json!({ "clip": clip, "semitones": semitones, "cents": cents }),
    )
}

pub fn set_reverse(clip: ClipId, reverse: bool) -> Request {
    req(
        "audio.set_reverse",
        json!({ "clip": clip, "reverse": reverse }),
    )
}

pub fn set_stretch(clip: ClipId, mode: Option<StretchMode>, tempo_sync: Option<bool>) -> Request {
    req(
        "audio.set_stretch",
        json!({ "clip": clip, "mode": mode, "tempo_sync": tempo_sync }),
    )
}

pub fn fit_to_tempo(clip: ClipId) -> Request {
    req("audio.fit_to_tempo", json!({ "clip": clip }))
}

pub fn set_bpm(sample: SampleId, bpm: Option<f64>) -> Request {
    req("sample.set_bpm", json!({ "sample": sample, "bpm": bpm }))
}

pub fn detect_bpm(sample: SampleId) -> Request {
    req("sample.detect_bpm", json!({ "sample": sample }))
}

pub fn editor_open(sample: SampleId) -> Request {
    req("editor.open", json!({ "sample": sample }))
}

/// One audio editor operation on a selection (frames, `end` exclusive).
pub fn editor_apply(
    sample: SampleId,
    operation: &str,
    selection: Option<(u64, u64)>,
    amount: Option<f64>,
    curve: Option<FadeCurve>,
) -> Request {
    let mut p = json!({ "sample": sample, "operation": operation });
    if let Some((s, e)) = selection {
        p["start"] = json!(s);
        p["end"] = json!(e);
    }
    if let Some(a) = amount {
        p["amount"] = json!(a);
    }
    if let Some(c) = curve {
        p["curve"] = json!(c);
    }
    req("editor.apply", p)
}

/// Paste `from_range` of `from` into `sample` at frame `at`.
pub fn editor_paste(sample: SampleId, from: SampleId, from_range: (u64, u64), at: u64) -> Request {
    req(
        "editor.apply",
        json!({
            "sample": sample, "operation": "paste", "from": from,
            "from_start": from_range.0, "from_end": from_range.1, "at": at,
        }),
    )
}

pub fn slicer_detect(sample: SampleId, sensitivity: f32) -> Request {
    req(
        "slicer.detect",
        json!({ "sample": sample, "sensitivity": sensitivity }),
    )
}

pub fn make_instrument(
    sample: SampleId,
    slices: &[u64],
    write_pattern: bool,
    root_key: Option<u8>,
) -> Request {
    req(
        "slicer.make_instrument",
        json!({
            "sample": sample, "slices": slices, "write_pattern": write_pattern,
            "root_key": root_key,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_name_their_commands_and_params() {
        let c = ClipId(3);
        let r = set_range(c, Some(960), None, Some(480));
        assert_eq!(r.command, "audio.set_range");
        assert_eq!(
            r.params,
            json!({ "clip": 3, "start": 960, "source_offset": 480 })
        );
        assert_eq!(split(c, 1920).params, json!({ "clip": 3, "at": 1920 }));
        let f = set_fade_in(
            c,
            Fade {
                length: 240,
                curve: FadeCurve::SCurve,
            },
        );
        assert_eq!(
            f.params,
            json!({ "clip": 3, "fade_in": { "length": 240, "curve": "s_curve" } })
        );
        let e = editor_apply(
            SampleId(1),
            "fade_in",
            Some((10, 20)),
            None,
            Some(FadeCurve::Exp),
        );
        assert_eq!(
            e.params,
            json!({ "sample": 1, "operation": "fade_in", "start": 10, "end": 20, "curve": "exp" })
        );
        assert_eq!(
            make_instrument(SampleId(2), &[0, 100], true, None).params["slices"],
            json!([0, 100])
        );
        assert_eq!(
            editor_paste(SampleId(1), SampleId(2), (0, 5), 9).params["from_end"],
            5
        );
    }
}
