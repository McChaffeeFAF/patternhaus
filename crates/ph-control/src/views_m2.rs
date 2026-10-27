//! Readable views of the M2 model for agents (M2/T9): audio clips and samples, automation
//! clips, plugin instances; plus the words the musical diff uses for them. Pure functions of
//! `Project` snapshots.

use crate::views::{bar_span, timing};
use ph_analysis::Timing;
use ph_core::{
    AudioClip, AutomationClip, Clip, EffectSpec, InstrumentSpec, ParamPath, PluginFormat, Project,
    SampleEntry, StretchMode, ticks_to_seconds,
};
use serde_json::{Value, json};

fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

pub fn stretch_name(m: StretchMode) -> &'static str {
    match m {
        StretchMode::Resample => "resample",
        StretchMode::Stretch => "stretch",
        StretchMode::Slice => "slice",
    }
}

pub fn format_name(f: PluginFormat) -> &'static str {
    match f {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
    }
}

/// An automation target in words: "Bass volume", "insert 3 (Drums) pan", "tempo".
pub fn target_words(p: &Project, t: &ParamPath) -> String {
    let ch = |c: &ph_core::ChannelId| {
        p.channels
            .get(c)
            .map_or(format!("channel {c}"), |x| x.name.clone())
    };
    let ins = |i: &ph_core::InsertId| match p.mixer.get(i.0 as usize) {
        Some(m) if i.0 == 0 => format!("Master ({})", m.name),
        Some(m) => format!("insert {} ({})", i.0, m.name),
        None => format!("insert {}", i.0),
    };
    match t {
        ParamPath::Tempo => "tempo".into(),
        ParamPath::ChannelVolume(c) => format!("{} volume", ch(c)),
        ParamPath::ChannelPan(c) => format!("{} pan", ch(c)),
        ParamPath::ChannelParam(c, name) => format!("{} {name}", ch(c)),
        ParamPath::InsertVolume(i) => format!("{} volume", ins(i)),
        ParamPath::InsertPan(i) => format!("{} pan", ins(i)),
        ParamPath::InsertMute(i) => format!("{} mute", ins(i)),
        ParamPath::InsertSend { insert, to } => {
            format!("{} send to insert {}", ins(insert), to.0)
        }
        ParamPath::EffectParam {
            insert,
            slot,
            param,
        } => {
            let fx = p
                .mixer
                .get(insert.0 as usize)
                .and_then(|m| m.effects.get(*slot as usize))
                .map(|e| match &e.effect {
                    EffectSpec::Builtin { name, .. } => name.clone(),
                    EffectSpec::Plugin { plugin_id, .. } => plugin_id.clone(),
                })
                .unwrap_or_else(|| format!("slot {slot}"));
            format!("{} {fx} {param}", ins(insert))
        }
    }
}

fn sample_label(p: &Project, a: &AudioClip) -> String {
    p.sample(a.sample)
        .map_or_else(|_| format!("sample {}", a.sample), |e| e.name.clone())
}

fn track_name(p: &Project, track: u32) -> String {
    match p.tracks.get(track as usize) {
        Some(t) if !t.name.is_empty() && t.name != format!("Track {}", track + 1) => {
            format!("track {} ({})", track + 1, t.name)
        }
        _ => format!("track {}", track + 1),
    }
}

/// One audio clip as a line: `loop.wav (clip 4) bars 1-4 on track 2: stretch, follows tempo
/// (120 -> 128 bpm), gain -3 dB`.
pub fn audio_clip_line(p: &Project, c: &Clip, a: &AudioClip, t: &Timing) -> String {
    let mut s = format!(
        "{} (clip {}) {} on {}: {}",
        sample_label(p, a),
        c.id,
        bar_span(t, c.start, c.start + c.length),
        track_name(p, c.track),
        stretch_name(a.stretch)
    );
    if a.tempo_sync {
        match p.sample(a.sample).ok().and_then(|e| e.bpm()) {
            Some(src) => s.push_str(&format!(
                ", follows tempo ({} -> {} bpm)",
                round(src, 1),
                round(p.settings.bpm, 1)
            )),
            None => s.push_str(", tempo sync on but the sample has no bpm (plays unsynced)"),
        }
    }
    if a.gain_db != 0.0 {
        s.push_str(&format!(", gain {:+.1} dB", a.gain_db));
    }
    if a.pitch() != 0.0 {
        s.push_str(&format!(", pitch {:+.2} st", a.pitch()));
    }
    if a.reverse {
        s.push_str(", reversed");
    }
    if a.fade_in.length > 0 {
        s.push_str(&format!(", fade in {}t", a.fade_in.length));
    }
    if a.fade_out.length > 0 {
        s.push_str(&format!(", fade out {}t", a.fade_out.length));
    }
    s
}

fn sample_json(e: &SampleEntry) -> Value {
    json!({
        "id": e.id,
        "file": e.path,
        "name": e.name,
        "seconds": round(e.seconds(), 3),
        "sample_rate": e.sample_rate,
        "channels": e.channels,
        "bpm": e.bpm().map(|b| round(b, 2)),
        "bpm_source": e.bpm.map(|b| format!("{:?}", b.source).to_lowercase()),
        "root_key": e.root_key,
    })
}

/// `audio` view: the sample registry and every audio clip, in playlist order.
pub fn audio_view(p: &Project) -> Value {
    let t = timing(p);
    let mut clips: Vec<&Clip> = p.clips.values().filter(|c| c.audio().is_some()).collect();
    clips.sort_by_key(|c| (c.start, c.id));
    let rows: Vec<Value> = clips
        .iter()
        .map(|c| {
            let a = c.audio().expect("filtered");
            let entry = p.sample(a.sample).ok();
            let ratio = entry.map_or(1.0, |e| a.time_ratio(e, p.settings.bpm));
            let sr = entry.map_or(48_000.0, |e| e.sample_rate.max(1) as f64);
            let from = a.source_offset as f64 / sr;
            let played = ticks_to_seconds(c.length, p.settings.bpm) * ratio;
            json!({
                "clip": c.id,
                "sample": a.sample,
                "file": entry.map(|e| e.path.clone()),
                "name": sample_label(p, a),
                "track": c.track + 1,
                "from": t.position(c.start),
                "to": t.position(c.start + c.length),
                "bars": bar_span(&t, c.start, c.start + c.length),
                "start_tick": c.start,
                "length_ticks": c.length,
                "source_range_s": [round(from, 3), round(from + played, 3)],
                "mode": stretch_name(a.stretch),
                "tempo_sync": a.tempo_sync,
                "bpm": entry.and_then(|e| e.bpm()).map(|b| round(b, 2)),
                "project_bpm": p.settings.bpm,
                "gain_db": a.gain_db,
                "pitch_semitones": round(a.pitch(), 2),
                "reverse": a.reverse,
                "fade_in_ticks": a.fade_in.length,
                "fade_out_ticks": a.fade_out.length,
                "text": audio_clip_line(p, c, a, &t),
            })
        })
        .collect();
    let used: std::collections::BTreeSet<_> = clips
        .iter()
        .filter_map(|c| c.audio().map(|a| a.sample))
        .collect();
    json!({
        "audio_clips": rows,
        "samples": p.samples.values().map(|e| {
            let mut v = sample_json(e);
            v["used_by_clips"] = json!(used.contains(&e.id));
            v
        }).collect::<Vec<_>>(),
        "text": if rows.is_empty() {
            "no audio clips (add one with add_audio_clip)".to_string()
        } else {
            rows.iter().map(|r| r["text"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("; ")
        },
    })
}

/// The value range of an automation clip's points, `(lo, hi)`.
pub fn auto_range(a: &AutomationClip) -> Option<(f64, f64)> {
    let lo = a.points.values().map(|x| x.value).reduce(f64::min)?;
    let hi = a.points.values().map(|x| x.value).reduce(f64::max)?;
    Some((lo, hi))
}

pub fn automation_clip_line(p: &Project, c: &Clip, a: &AutomationClip, t: &Timing) -> String {
    let range = match auto_range(a) {
        Some((lo, hi)) if lo == hi => format!("constant {}", round(lo, 3)),
        Some((lo, hi)) => format!("{} to {}", round(lo, 3), round(hi, 3)),
        None => "no points".into(),
    };
    format!(
        "{} (clip {}) {} on {}: {} point(s), {range}",
        target_words(p, &a.target),
        c.id,
        bar_span(t, c.start, c.start + c.length),
        track_name(p, c.track),
        a.points.len()
    )
}

/// `automation` view: every automation clip with its target, points and range.
pub fn automation_view(p: &Project) -> Value {
    let t = timing(p);
    let mut clips: Vec<&Clip> = p
        .clips
        .values()
        .filter(|c| c.automation().is_some())
        .collect();
    clips.sort_by_key(|c| (c.start, c.id));
    let rows: Vec<Value> = clips
        .iter()
        .map(|c| {
            let a = c.automation().expect("filtered");
            let r = auto_range(a);
            json!({
                "clip": c.id,
                "target": a.target.to_string(),
                "target_words": target_words(p, &a.target),
                "track": c.track + 1,
                "from": t.position(c.start),
                "to": t.position(c.start + c.length),
                "bars": bar_span(&t, c.start, c.start + c.length),
                "start_tick": c.start,
                "length_ticks": c.length,
                "points": a.points.len(),
                "min": r.map(|r| round(r.0, 4)),
                "max": r.map(|r| round(r.1, 4)),
                "point_list": a.points_in_order().iter().take(64).map(|x| json!({
                    "id": x.id, "tick": x.tick, "value": round(x.value, 4),
                    "curve": format!("{:?}", x.curve).to_lowercase(),
                })).collect::<Vec<_>>(),
                "text": automation_clip_line(p, c, a, &t),
            })
        })
        .collect();
    json!({
        "automation_clips": rows,
        "text": if rows.is_empty() {
            "no automation clips (add one with add_automation)".to_string()
        } else {
            rows.iter().map(|r| r["text"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("; ")
        },
    })
}

/// `plugins` view: every plugin instrument and effect instance in the project.
pub fn plugins_view(p: &Project) -> Value {
    let mut out = Vec::new();
    for c in p.channels_in_order() {
        if let InstrumentSpec::Plugin {
            format,
            plugin_id,
            state,
            params,
        } = &c.instrument
        {
            out.push(json!({
                "role": "instrument",
                "where": format!("channel {} (id {})", c.name, c.id),
                "channel": c.id,
                "format": format_name(*format),
                "plugin": plugin_id,
                "has_saved_state": state.is_some(),
                "params_set": params.len(),
            }));
        }
    }
    for (i, ins) in p.mixer.iter().enumerate() {
        for (slot, e) in ins.effects.iter().enumerate() {
            if let EffectSpec::Plugin {
                format,
                plugin_id,
                state,
                params,
            } = &e.effect
            {
                out.push(json!({
                    "role": "effect",
                    "where": format!("insert {i} ({}) slot {slot}", ins.name),
                    "insert": i,
                    "slot": slot,
                    "format": format_name(*format),
                    "plugin": plugin_id,
                    "bypassed": e.bypass,
                    "mix": e.mix,
                    "has_saved_state": state.is_some(),
                    "params_set": params.len(),
                }));
            }
        }
    }
    json!({
        "plugins": out,
        "text": if out.is_empty() {
            "no plugins in the project (plugin.scan, then add_plugin_instrument / add_plugin_effect)".to_string()
        } else {
            out.iter().map(|r| format!("{} {} {}", r["role"].as_str().unwrap_or(""), r["plugin"].as_str().unwrap_or(""), r["where"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ")
        },
    })
}

/// Counts for the overview.
pub fn overview_extra(p: &Project) -> Value {
    let audio = p.clips.values().filter(|c| c.audio().is_some()).count();
    let auto = p
        .clips
        .values()
        .filter(|c| c.automation().is_some())
        .count();
    let plugins = plugins_view(p)["plugins"].as_array().map_or(0, |a| a.len());
    json!({
        "audio_clips": audio,
        "automation_clips": auto,
        "samples": p.samples.len(),
        "plugins": plugins,
    })
}
