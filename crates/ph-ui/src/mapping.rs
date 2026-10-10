//! Widget gestures -> command requests.
//!
//! Every UI action is one of these constructors. They are plain functions of ids and numbers,
//! so the mapping from gesture to command id and params is unit-tested without a window.
//! Panels never build JSON by hand and never touch a `Project`.

use ph_commands::Request;
use ph_core::{ChannelId, ClipId, EffectSpec, InstrumentSpec, NoteId, PatternId, Tick};
use serde_json::{Value, json};

fn req(command: &str, params: Value) -> Request {
    Request::new(command, params)
}

// ---- transport / project ----------------------------------------------------------------

pub fn transport(action: &str) -> Request {
    req(&format!("transport.{action}"), json!({}))
}
pub fn seek(position: Tick) -> Request {
    req("transport.seek", json!({ "position": position }))
}
pub fn set_mode_pattern(pattern: Option<PatternId>) -> Request {
    req(
        "transport.set_mode",
        json!({ "mode": "pattern", "pattern": pattern }),
    )
}
pub fn set_mode_song() -> Request {
    req("transport.set_mode", json!({ "mode": "song" }))
}
pub fn set_loop(range: Option<(Tick, Tick)>) -> Request {
    req("transport.set_loop", json!({ "range": range }))
}
pub fn set_metronome(on: bool) -> Request {
    req("transport.set_metronome", json!({ "on": on }))
}
pub fn set_tempo(bpm: f64) -> Request {
    req("project.set_tempo", json!({ "bpm": bpm }))
}
pub fn set_swing(swing: f32) -> Request {
    req("project.set_swing", json!({ "swing": swing }))
}

// ---- channels ----------------------------------------------------------------------------

pub fn channel_add(name: &str, instrument: &InstrumentSpec, insert: Option<u32>) -> Request {
    req(
        "channel.add",
        json!({ "name": name, "instrument": instrument, "insert": insert }),
    )
}
pub fn channel_remove(c: ChannelId) -> Request {
    req("channel.remove", json!({ "channel": c }))
}
pub fn channel_clone(c: ChannelId) -> Request {
    req("channel.clone", json!({ "channel": c }))
}
pub fn channel_rename(c: ChannelId, name: &str) -> Request {
    req("channel.rename", json!({ "channel": c, "name": name }))
}
pub fn channel_color(c: ChannelId, color: u32) -> Request {
    req("channel.set_color", json!({ "channel": c, "color": color }))
}
pub fn channel_volume(c: ChannelId, volume: f32) -> Request {
    req(
        "channel.set_volume",
        json!({ "channel": c, "volume": volume }),
    )
}
pub fn channel_pan(c: ChannelId, pan: f32) -> Request {
    req("channel.set_pan", json!({ "channel": c, "pan": pan }))
}
pub fn channel_mute(c: ChannelId, muted: bool) -> Request {
    req("channel.set_mute", json!({ "channel": c, "muted": muted }))
}
pub fn channel_solo(c: ChannelId, solo: bool) -> Request {
    req("channel.solo", json!({ "channel": c, "solo": solo }))
}
pub fn channel_insert(c: ChannelId, insert: u32) -> Request {
    req(
        "channel.set_insert",
        json!({ "channel": c, "insert": insert }),
    )
}
pub fn channel_move(c: ChannelId, index: usize) -> Request {
    req("channel.move", json!({ "channel": c, "index": index }))
}
pub fn channel_param(c: ChannelId, param: &str, value: f64) -> Request {
    req(
        "channel.set_param",
        json!({ "channel": c, "param": param, "value": value }),
    )
}
pub fn channel_set_instrument(c: ChannelId, instrument: &InstrumentSpec) -> Request {
    req(
        "channel.set_instrument",
        json!({ "channel": c, "instrument": instrument }),
    )
}

// ---- steps -------------------------------------------------------------------------------

pub fn step_toggle(p: PatternId, c: ChannelId, step: u32) -> Request {
    req(
        "step.toggle",
        json!({ "pattern": p, "channel": c, "step": step }),
    )
}
/// Paint a run of steps in one command (drag across the row).
pub fn step_set_row(p: PatternId, c: ChannelId, steps: &[bool]) -> Request {
    req(
        "step.set",
        json!({ "pattern": p, "channel": c, "steps": steps }),
    )
}
pub fn step_velocity(p: PatternId, c: ChannelId, step: u32, velocity: f32) -> Request {
    req(
        "step.set_velocity",
        json!({ "pattern": p, "channel": c, "step": step, "velocity": velocity }),
    )
}
pub fn pattern_length_steps(p: PatternId, steps: u32) -> Request {
    req(
        "pattern.set_length",
        json!({ "pattern": p, "steps": steps }),
    )
}

// ---- patterns ----------------------------------------------------------------------------

pub fn pattern_add() -> Request {
    req("pattern.add", json!({}))
}
pub fn pattern_clone(p: PatternId) -> Request {
    req("pattern.clone", json!({ "pattern": p }))
}
pub fn pattern_remove(p: PatternId) -> Request {
    req("pattern.remove", json!({ "pattern": p }))
}
pub fn pattern_rename(p: PatternId, name: &str) -> Request {
    req("pattern.rename", json!({ "pattern": p, "name": name }))
}

// ---- notes -------------------------------------------------------------------------------

pub fn note_add(p: PatternId, c: ChannelId, notes: &[NewNote]) -> Request {
    let notes: Vec<Value> = notes
        .iter()
        .map(|n| {
            json!({ "key": n.key, "start": n.start, "length": n.length, "velocity": n.velocity, "muted": n.muted })
        })
        .collect();
    req(
        "note.add",
        json!({ "pattern": p, "channel": c, "notes": notes }),
    )
}
/// Mutes or unmutes notes.
pub fn note_mute(p: PatternId, c: ChannelId, ids: &[NoteId], muted: bool) -> Request {
    let updates: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, "muted": muted }))
        .collect();
    req(
        "note.update",
        json!({ "pattern": p, "channel": c, "updates": updates }),
    )
}
/// Splits a note at `cut` (ticks): the first part keeps the id, the second is a new note with
/// the same key, velocity, pan, fine pitch and mute state. Send both as one batch.
pub fn note_slice(p: PatternId, c: ChannelId, n: &ph_core::Note, cut: u64) -> Vec<Request> {
    vec![
        req(
            "note.update",
            json!({ "pattern": p, "channel": c, "updates": [{ "id": n.id, "length": cut - n.start }] }),
        ),
        req(
            "note.add",
            json!({ "pattern": p, "channel": c, "notes": [{
                "key": n.key, "start": cut, "length": n.start + n.length - cut,
                "velocity": n.velocity, "pan": n.pan, "fine_pitch": n.fine_pitch, "muted": n.muted
            }] }),
        ),
    ]
}
pub fn note_remove(p: PatternId, c: ChannelId, ids: &[NoteId]) -> Request {
    req(
        "note.remove",
        json!({ "pattern": p, "channel": c, "ids": ids }),
    )
}
pub fn note_move(
    p: PatternId,
    c: ChannelId,
    ids: &[NoteId],
    delta_ticks: i64,
    delta_keys: i32,
) -> Request {
    req(
        "note.move",
        json!({ "pattern": p, "channel": c, "ids": ids,
                "delta_ticks": delta_ticks, "delta_keys": delta_keys }),
    )
}
pub fn note_resize_delta(p: PatternId, c: ChannelId, ids: &[NoteId], delta: i64) -> Request {
    req(
        "note.resize",
        json!({ "pattern": p, "channel": c, "ids": ids, "delta": delta }),
    )
}
pub fn note_velocity(p: PatternId, c: ChannelId, ids: &[NoteId], velocity: f32) -> Request {
    req(
        "note.set_velocity",
        json!({ "pattern": p, "channel": c, "ids": ids, "velocity": velocity }),
    )
}
pub fn note_transpose(p: PatternId, c: ChannelId, ids: &[NoteId], semitones: i32) -> Request {
    req(
        "note.transpose",
        json!({ "pattern": p, "channel": c, "ids": ids, "semitones": semitones }),
    )
}
pub fn note_quantize(p: PatternId, c: ChannelId, ids: &[NoteId], grid: Tick) -> Request {
    req(
        "note.quantize",
        json!({ "pattern": p, "channel": c, "ids": ids, "grid": grid }),
    )
}
/// Duplicate (Ctrl+B): copy the notes to just after the selection.
pub fn note_duplicate(p: PatternId, c: ChannelId, ids: &[NoteId], offset: i64) -> Request {
    req(
        "note.copy",
        json!({ "pattern": p, "channel": c, "ids": ids, "offset": offset }),
    )
}
/// Paste: copy a clipboard (given by source pattern/channel/ids) to a target and offset.
pub fn note_copy_to(
    from: (PatternId, ChannelId),
    ids: &[NoteId],
    to: (PatternId, ChannelId),
    offset: i64,
) -> Request {
    req(
        "note.copy",
        json!({ "pattern": from.0, "channel": from.1, "ids": ids,
                "to_pattern": to.0, "to_channel": to.1, "offset": offset }),
    )
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NewNote {
    pub key: u8,
    pub start: Tick,
    pub length: Tick,
    pub velocity: f32,
    /// Muted notes stay in the pattern but don't play (kept through copy/paste).
    pub muted: bool,
}

// ---- playlist ----------------------------------------------------------------------------

pub fn clip_add(track: u32, pattern: PatternId, start: Tick, length: Option<Tick>) -> Request {
    req(
        "playlist.add_clip",
        json!({ "track": track, "pattern": pattern, "start": start, "length": length }),
    )
}
pub fn clip_remove(clip: ClipId) -> Request {
    req("playlist.remove_clip", json!({ "clip": clip }))
}
pub fn clip_move(clip: ClipId, start: Tick, track: u32) -> Request {
    req(
        "playlist.move_clip",
        json!({ "clip": clip, "start": start, "track": track }),
    )
}
pub fn clip_resize(clip: ClipId, length: Tick) -> Request {
    req(
        "playlist.resize_clip",
        json!({ "clip": clip, "length": length }),
    )
}
pub fn track_mute(track: u32, muted: bool) -> Request {
    req(
        "playlist.set_track_mute",
        json!({ "track": track, "muted": muted }),
    )
}
pub fn track_rename(track: u32, name: &str) -> Request {
    req(
        "playlist.rename_track",
        json!({ "track": track, "name": name }),
    )
}

// ---- mixer -------------------------------------------------------------------------------

pub fn mixer_volume(insert: u32, volume: f32) -> Request {
    req(
        "mixer.set_volume",
        json!({ "insert": insert, "volume": volume }),
    )
}
pub fn mixer_pan(insert: u32, pan: f32) -> Request {
    req("mixer.set_pan", json!({ "insert": insert, "pan": pan }))
}
pub fn mixer_mute(insert: u32, muted: bool) -> Request {
    req(
        "mixer.set_mute",
        json!({ "insert": insert, "muted": muted }),
    )
}
pub fn mixer_solo(insert: u32, solo: bool) -> Request {
    req("mixer.solo", json!({ "insert": insert, "solo": solo }))
}
pub fn mixer_rename(insert: u32, name: &str) -> Request {
    req("mixer.rename", json!({ "insert": insert, "name": name }))
}
pub fn effect_add(insert: u32, name: &str) -> Request {
    let effect = EffectSpec::Builtin {
        name: name.into(),
        params: Default::default(),
    };
    req(
        "mixer.add_effect",
        json!({ "insert": insert, "effect": effect }),
    )
}
pub fn effect_remove(insert: u32, index: usize) -> Request {
    req(
        "mixer.remove_effect",
        json!({ "insert": insert, "index": index }),
    )
}
pub fn effect_move(insert: u32, from: usize, to: usize) -> Request {
    req(
        "mixer.move_effect",
        json!({ "insert": insert, "from": from, "to": to }),
    )
}
pub fn effect_param(insert: u32, index: usize, param: &str, value: f64) -> Request {
    req(
        "mixer.set_effect_param",
        json!({ "insert": insert, "index": index, "param": param, "value": value }),
    )
}
pub fn effect_bypass(insert: u32, index: usize, bypass: bool) -> Request {
    req(
        "mixer.set_effect_bypass",
        json!({ "insert": insert, "index": index, "bypass": bypass }),
    )
}
pub fn effect_mix(insert: u32, index: usize, mix: f32) -> Request {
    req(
        "mixer.set_effect_mix",
        json!({ "insert": insert, "index": index, "mix": mix }),
    )
}
pub fn send_add(from: u32, to: u32, level: f32) -> Request {
    req(
        "mixer.add_send",
        json!({ "from": from, "to": to, "level": level }),
    )
}
pub fn send_remove(from: u32, to: u32) -> Request {
    req("mixer.remove_send", json!({ "from": from, "to": to }))
}
pub fn send_level(from: u32, to: u32, level: f32) -> Request {
    req(
        "mixer.set_send_level",
        json!({ "from": from, "to": to, "level": level }),
    )
}
pub fn route_to_master(insert: u32, route: bool) -> Request {
    req(
        "mixer.set_route_to_master",
        json!({ "insert": insert, "route": route }),
    )
}
pub fn sidechain(insert: u32, source: Option<u32>) -> Request {
    req(
        "mixer.set_sidechain",
        json!({ "insert": insert, "source": source }),
    )
}

// ---- history / files ---------------------------------------------------------------------

pub fn undo_mine() -> Request {
    req("history.undo", json!({}))
}
pub fn redo_mine() -> Request {
    req("history.redo", json!({}))
}
/// Undo with a conflict policy: `stop`, `skip` or `force`.
pub fn undo_with(mode: &str) -> Request {
    req("history.undo", json!({ "mode": mode }))
}
pub fn undo_author(author: &str, mode: &str) -> Request {
    req(
        "history.undo_author",
        json!({ "author": author, "mode": mode }),
    )
}
pub fn undo_txn(txn: u64, mode: &str) -> Request {
    req("history.undo_txn", json!({ "txn": txn, "mode": mode }))
}
pub fn file_save() -> Request {
    req("file.save", json!({}))
}
pub fn file_save_as(path: &str) -> Request {
    req("file.save_as", json!({ "path": path }))
}
pub fn file_open(path: &str) -> Request {
    confirmed(req("file.open", json!({ "path": path })))
}
pub fn export_audio(path: &str, overwrite: bool) -> Request {
    req(
        "file.export_audio",
        json!({ "path": path, "overwrite": overwrite }),
    )
}
/// Destructive (discards unsaved work): the UI asks the person first, then sends `confirm`.
pub fn project_new(title: Option<&str>) -> Request {
    confirmed(req("project.new", json!({ "title": title })))
}
pub fn confirmed(mut r: Request) -> Request {
    r.confirm = true;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, Registry, Session};
    use ph_core::Project;
    use std::sync::Arc;

    /// Every request the UI can build must be accepted by the real registry (right command id
    /// and params shape). Runs them against a small project through a real `Session`.
    #[test]
    fn requests_are_accepted_by_the_registry() {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("t"));
        let mut host = ph_commands::NullHost::default();
        let who = Caller::human();
        let mut run = |r: Request| {
            s.execute(&who, r.clone(), &mut host)
                .unwrap_or_else(|e| panic!("{}: {e}", r.command))
        };
        run(project_new(Some("x")));
        let inst = InstrumentSpec::Synth {
            preset: Some("bass/sub".into()),
            params: Default::default(),
        };
        let ch = run(channel_add("Bass", &inst, Some(1))).result["channel"]
            .as_u64()
            .unwrap() as u32;
        let (c, p) = (ChannelId(ch), PatternId(0));
        let pat = run(pattern_add()).result["pattern"].as_u64().unwrap() as u32;
        let p = if pat == 0 { p } else { PatternId(pat) };
        run(step_toggle(p, c, 3));
        run(step_set_row(p, c, &[true, false, true]));
        run(step_velocity(p, c, 0, 0.5));
        run(pattern_length_steps(p, 32));
        let added = run(note_add(
            p,
            c,
            &[NewNote {
                key: 60,
                start: 0,
                length: 240,
                velocity: 0.8,
                muted: true,
            }],
        ));
        let id = NoteId(
            added.result["notes"]
                .as_array()
                .and_then(|a| a[0].as_u64())
                .unwrap_or(0) as u32,
        );
        run(note_move(p, c, &[id], 240, 2));
        run(note_resize_delta(p, c, &[id], 120));
        run(note_velocity(p, c, &[id], 0.4));
        run(note_transpose(p, c, &[id], -1));
        run(note_quantize(p, c, &[id], 240));
        run(note_duplicate(p, c, &[id], 480));
        run(note_copy_to((p, c), &[id], (p, c), 960));
        run(note_remove(p, c, &[id]));
        run(channel_volume(c, 0.5));
        run(channel_pan(c, -0.2));
        run(channel_mute(c, true));
        run(channel_solo(c, true));
        run(channel_insert(c, 2));
        run(channel_rename(c, "B"));
        run(channel_color(c, 0xff0000));
        run(channel_param(c, "cutoff", 1000.0));
        run(channel_move(c, 0));
        run(channel_clone(c));
        run(clip_add(0, p, 0, None));
        run(mixer_volume(1, 0.7));
        run(mixer_pan(1, 0.1));
        run(mixer_mute(1, false));
        run(mixer_solo(1, true));
        run(mixer_rename(1, "Bass"));
        run(effect_add(1, "eq"));
        run(effect_add(1, "reverb"));
        run(effect_bypass(1, 0, true));
        run(effect_mix(1, 1, 0.3));
        run(effect_move(1, 0, 1));
        run(effect_remove(1, 1));
        run(send_add(1, 2, 0.5));
        run(send_level(1, 2, 0.6));
        run(route_to_master(1, false));
        run(sidechain(2, Some(1)));
        run(send_remove(1, 2));
        run(track_mute(0, true));
        run(track_rename(0, "Drums"));
        run(set_tempo(140.0));
        run(set_swing(0.2));
        run(pattern_rename(p, "A"));
        run(undo_mine());
        run(redo_mine());
        run(undo_with("skip"));
        run(seek(0));
        run(set_loop(None));
        run(set_metronome(true));
        run(transport("toggle"));
        run(set_mode_song());
        run(set_mode_pattern(Some(p)));
    }
}
