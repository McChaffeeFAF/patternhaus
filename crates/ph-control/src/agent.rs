//! The agent interface core (DESIGN §5): the fixed toolset every route shares.
//!
//! [`Agent`] turns a tool name plus JSON arguments into a [`ToolOutput`] (text and PNG blocks).
//! The MCP server (`mcp.rs`), the JSON-RPC endpoint (`jsonrpc.rs`) and the CLI all call
//! [`Agent::call`], so the three routes cannot drift apart. Everything here is blocking; async
//! front ends call it from a blocking task.

use crate::StudioHandle;
use crate::diff::{Diff, diff};
use crate::events::Event;
use crate::grants::{Grants, grant_text};
use crate::views;
use ph_analysis::image::{self, PianoRollSpec, PlaylistClipBox, PlaylistSpec, PlaylistTrackRow};
use ph_analysis::music::{
    ChordGrain, SongNote, chords_from_audio, chords_from_notes, key_from_audio, key_from_notes,
    progression, song_notes,
};
use ph_analysis::{health, loudness, masking, onsets, spectrum};
use ph_commands::{Author, Caller, CmdError, CommandSpec, Executed, Request, UndoMode};
use ph_core::{ChannelId, PatternId, Project, Tick};
use ph_engine::{AudioBuffer, PlayMode, RenderSettings};
use ph_formats::encode::{FlacDepth, WavFormat, write_flac, write_wav};
use ph_node::Quality;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// One piece of a tool result.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(String),
    /// PNG bytes.
    Image(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub blocks: Vec<Block>,
    pub is_error: bool,
}

impl ToolOutput {
    /// MCP-shaped JSON: `{ content: [text | image blocks], isError, value }` where `value` is the
    /// first text block parsed as JSON. Images are base64 PNG.
    pub fn to_json(self) -> Value {
        use base64::Engine as _;
        let value = self.value();
        let content: Vec<Value> = self
            .blocks
            .into_iter()
            .map(|b| match b {
                Block::Text(t) => json!({ "type": "text", "text": t }),
                Block::Image(png) => json!({
                    "type": "image",
                    "mimeType": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                }),
            })
            .collect();
        json!({ "content": content, "isError": self.is_error, "value": value })
    }

    fn json(v: Value) -> Self {
        ToolOutput {
            blocks: vec![Block::Text(v.to_string())],
            is_error: false,
        }
    }

    fn error(msg: impl Into<String>) -> Self {
        ToolOutput {
            blocks: vec![Block::Text(
                json!({ "ok": false, "error": msg.into() }).to_string(),
            )],
            is_error: true,
        }
    }

    /// The first text block, parsed as JSON (tests and the CLI).
    pub fn value(&self) -> Option<Value> {
        self.blocks.iter().find_map(|b| match b {
            Block::Text(t) => serde_json::from_str(t).ok(),
            _ => None,
        })
    }
}

/// A tool as listed to clients.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

/// Direct tools: the high-traffic commands, as thin aliases with the command's own schema.
const DIRECT: &[(&str, &str)] = &[
    ("add_pattern", "pattern.add"),
    ("add_channel", "channel.add"),
    ("add_notes", "note.add"),
    ("toggle_steps", "step.toggle"),
    ("add_clip", "playlist.add_clip"),
    ("set_tempo", "project.set_tempo"),
    ("set_channel_volume", "channel.set_volume"),
    ("route_channel", "channel.set_insert"),
    ("set_mixer_volume", "mixer.set_volume"),
    ("add_effect", "mixer.add_effect"),
    ("set_effect_param", "mixer.set_effect_param"),
    ("add_send", "mixer.add_send"),
    ("play", "transport.play"),
    ("stop", "transport.stop"),
    ("export_audio", "file.export_audio"),
];

struct Rendered {
    audio: AudioBuffer,
    /// 1-based bar of the first sample.
    first_bar: u32,
}

type RenderKey = (usize, Option<(Tick, Tick)>, bool, u32);

pub struct Agent {
    studio: StudioHandle,
    grants: Arc<Grants>,
    seen: Mutex<HashMap<String, u64>>,
    cache: Mutex<Option<(RenderKey, Arc<Rendered>)>>,
}

fn err_text(e: &CmdError) -> String {
    match e {
        CmdError::BadParams { command, message } => format!(
            "bad parameters for {command}: {message}. Call describe_command {{ id: \"{command}\" }} for the schema."
        ),
        CmdError::Scope { command, needed } => format!(
            "{command} needs the {needed:?} scope, which this client was not granted. Ask the human to raise this client's grant (read < compose/mix < full)."
        ),
        CmdError::NeedsConfirm(c) => format!(
            "{c} is destructive (it overwrites or discards something). Repeat the call with confirm: true if that is intended."
        ),
        other => other.to_string(),
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() { "render".into() } else { s }
}

impl Agent {
    pub fn new(studio: StudioHandle, grants: Arc<Grants>) -> Arc<Agent> {
        Arc::new(Agent {
            studio,
            grants,
            seen: Mutex::default(),
            cache: Mutex::default(),
        })
    }

    pub fn studio(&self) -> &StudioHandle {
        &self.studio
    }

    pub fn grants(&self) -> &Arc<Grants> {
        &self.grants
    }

    /// The caller identity of a client name, with its grant right now.
    pub fn caller(&self, client: &str) -> Caller {
        Caller::agent(client, self.grants.grant_for(client))
    }

    // ---- tool list --------------------------------------------------------------------

    pub fn tools(&self) -> Vec<ToolSpec> {
        let mut v = vec![
            tool(
                "list_commands",
                "Discover commands. Every operation in Patternhaus is a command (pattern.add_note, mixer.add_effect, ...). `filter` matches ids and summaries; lists everything when empty. Then use describe_command and run.",
                json!({ "type": "object", "properties": { "filter": { "type": "string", "description": "substring of the command id or summary, e.g. \"mixer\" or \"note\"" } } }),
                true,
            ),
            tool(
                "describe_command",
                "Parameter JSON-Schema, scope and flags of one command. Times are ticks (960 per quarter note, 240 per 16th step).",
                json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }),
                true,
            ),
            tool(
                "run",
                "Run one command by id with a params object. Returns the result, a one-line summary and a musical diff of what changed. Destructive commands need confirm: true.",
                json!({ "type": "object", "properties": {
                    "id": { "type": "string", "description": "command id, e.g. note.add" },
                    "params": { "type": "object" },
                    "confirm": { "type": "boolean", "description": "required for destructive commands" } }, "required": ["id"] }),
                false,
            ),
            tool(
                "run_batch",
                "Run several commands in order. With as_one_undo (default true) they are one transaction, all-or-nothing, undone with a single undo_mine. Returns every result and the combined musical diff.",
                json!({ "type": "object", "properties": {
                    "commands": { "type": "array", "items": { "type": "object", "properties": {
                        "id": { "type": "string" }, "params": { "type": "object" }, "confirm": { "type": "boolean" } }, "required": ["id"] } },
                    "as_one_undo": { "type": "boolean", "description": "default true" } }, "required": ["commands"] }),
                false,
            ),
            tool(
                "get_state",
                "Readable views of the project. view: overview (default), notes (names and bar.beat.tick; args pattern, channel, from_bar, to_bar), pattern (channels, ranges, chords per bar; args pattern), sections (arrangement map), mixer, channels, transport, session (who you are and what you may do), events (args since), warnings, raw (args query: a query.* command id, params).",
                json!({ "type": "object", "properties": {
                    "view": { "type": "string" }, "args": { "type": "object" } } }),
                true,
            ),
            tool(
                "render",
                "Render the song (or the first pattern when the playlist is empty) offline to a WAV or FLAC under the project's renders folder. Returns the file path and a quick analysis (loudness, true peak, clipping, key). range: { from_bar, to_bar } (1-based, inclusive).",
                json!({ "type": "object", "properties": {
                    "range": range_schema(), "format": { "type": "string", "enum": ["wav", "flac"] },
                    "name": { "type": "string", "description": "file name without extension" },
                    "bit_depth": { "type": "integer", "enum": [16, 24, 32] },
                    "sample_rate": { "type": "integer" } } }),
                true,
            ),
            tool(
                "analyze",
                "Hear the result as numbers. what: any of loudness (LUFS integrated/short-term, true peak, RMS, crest), spectrum (7 bands + centroid), health (clipping, DC, phase, mono loss, energy per bar), onsets, key, chords, clashes (semitone note clashes), masking (per-channel spectral overlap), or \"all\". Default: loudness, spectrum, health, key, chords, clashes.",
                json!({ "type": "object", "properties": {
                    "range": range_schema(),
                    "what": { "type": "array", "items": { "type": "string" } } } }),
                true,
            ),
            tool(
                "snapshot",
                "See the result: returns a PNG image. kind: waveform, spectrogram, piano_roll (args pattern, channels[], range; all channels coloured), playlist (arrangement overview). Optional save: true also writes the PNG under renders/.",
                json!({ "type": "object", "properties": {
                    "kind": { "type": "string", "enum": ["waveform", "spectrogram", "piano_roll", "playlist"] },
                    "pattern": { "description": "pattern id or name (piano_roll)" },
                    "channels": { "type": "array", "items": {}, "description": "channel ids or names to show (piano_roll)" },
                    "range": range_schema(),
                    "width": { "type": "integer", "description": "image width in pixels, 480-4000, default 1400" },
                    "save": { "type": "boolean" } }, "required": ["kind"] }),
                true,
            ),
            tool(
                "node_params",
                "Parameters (stable ids, ranges, defaults, units, choices) of an instrument kind or built-in effect. With neither argument, lists instruments, effects and synth presets.",
                json!({ "type": "object", "properties": {
                    "instrument": { "type": "string", "enum": ["sampler", "synth", "multi_sampler"] },
                    "effect": { "type": "string", "description": "eq, compressor, limiter, reverb, delay, filter, saturator" } } }),
                true,
            ),
            tool(
                "undo_mine",
                "Undo your own most recent transaction (not anyone else's). Stops and names the conflict if someone else touched the same objects since; mode skip/force overrides.",
                json!({ "type": "object", "properties": { "mode": { "type": "string", "enum": ["stop", "skip", "force"] } } }),
                false,
            ),
            tool(
                "redo_mine",
                "Redo what undo_mine undid.",
                json!({ "type": "object", "properties": { "mode": { "type": "string", "enum": ["stop", "skip", "force"] } } }),
                false,
            ),
            tool(
                "history",
                "Recent transactions with author, command ids and summaries (newest last).",
                json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "default 15" } } }),
                true,
            ),
        ];
        for (name, cmd) in DIRECT {
            if let Ok(c) = self.studio.registry.get(cmd) {
                let s = c.spec();
                v.push(ToolSpec {
                    name: (*name).into(),
                    description: format!("{} (shortcut for run {{ id: \"{cmd}\" }})", s.summary),
                    input_schema: s.params.clone(),
                    read_only: !s.mutating,
                });
            }
        }
        v
    }

    // ---- dispatch ---------------------------------------------------------------------

    pub fn call(&self, client: &str, tool: &str, args: Value) -> ToolOutput {
        let args = if args.is_null() { json!({}) } else { args };
        // Make sure events are being collected, and remember where this client started.
        if let Ok(hub) = self.studio.events() {
            self.seen
                .lock()
                .unwrap()
                .entry(client.to_string())
                .or_insert_with(|| hub.last_seq());
        }
        let caller = self.caller(client);
        let out = match tool {
            "list_commands" => self.list_commands(&caller, &args),
            "describe_command" => self.describe_command(&caller, &args),
            "run" => self.run(&caller, &args),
            "run_batch" => self.run_batch(&caller, &args),
            "get_state" => self.get_state(&caller, &args),
            "render" => self.render_tool(&args),
            "analyze" => self.analyze_tool(&args),
            "snapshot" => self.snapshot_tool(&args),
            "node_params" => self.run_one(&caller, Request::new("query.node_params", args.clone())),
            "undo_mine" => self.undo_redo(&caller, &args, true),
            "redo_mine" => self.undo_redo(&caller, &args, false),
            "history" => self.history_tool(&args),
            other => match DIRECT.iter().find(|(n, _)| *n == other) {
                Some((_, cmd)) => self.run_one(&caller, Request::new(cmd, args.clone())),
                None => ToolOutput::error(format!("unknown tool {other:?}")),
            },
        };
        self.with_events(client, out)
    }

    /// Appends what other authors did since this client last looked.
    fn with_events(&self, client: &str, mut out: ToolOutput) -> ToolOutput {
        let Ok(hub) = self.studio.events() else {
            return out;
        };
        let after = *self.seen.lock().unwrap().get(client).unwrap_or(&0);
        let events = hub.since(after);
        let Some(last) = events.last().map(|e| e.seq) else {
            return out;
        };
        self.seen.lock().unwrap().insert(client.to_string(), last);
        let me = Author::Agent(client.to_string()).to_string();
        let others: Vec<String> = events
            .iter()
            .filter(|e| e.author.as_deref() != Some(me.as_str()))
            .map(event_line)
            .collect();
        if others.is_empty() {
            return out;
        }
        let shown: Vec<&String> = others.iter().rev().take(8).rev().collect();
        out.blocks.push(Block::Text(
            json!({ "while_you_worked": shown, "total": others.len() }).to_string(),
        ));
        out
    }

    // ---- discovery --------------------------------------------------------------------

    fn list_commands(&self, caller: &Caller, args: &Value) -> ToolOutput {
        let filter = args["filter"].as_str().unwrap_or("");
        let specs = self.studio.registry.list(filter);
        let lines: Vec<String> = specs
            .iter()
            .map(|s| {
                let mut flags = format!("{:?}", s.scope).to_lowercase();
                if s.destructive {
                    flags.push_str(", destructive");
                } else if s.mutating {
                    flags.push_str(", mutates");
                }
                if !caller.grant.allows(s.scope) {
                    flags.push_str(", NOT GRANTED");
                }
                format!("{} [{flags}] {}", s.id, s.summary)
            })
            .collect();
        ToolOutput::json(json!({ "count": lines.len(), "commands": lines }))
    }

    fn describe_command(&self, caller: &Caller, args: &Value) -> ToolOutput {
        let Some(id) = args["id"].as_str() else {
            return ToolOutput::error("id is required");
        };
        match self.studio.registry.get(id) {
            Ok(c) => {
                let s: &CommandSpec = c.spec();
                ToolOutput::json(json!({
                    "id": s.id,
                    "summary": s.summary,
                    "scope": s.scope,
                    "mutates": s.mutating,
                    "destructive": s.destructive,
                    "granted": caller.grant.allows(s.scope),
                    "params": s.params,
                }))
            }
            Err(e) => {
                let near = self
                    .studio
                    .registry
                    .list(id.split('.').next_back().unwrap_or(id));
                ToolOutput::error(format!(
                    "{e}. Similar: {}",
                    near.iter()
                        .take(6)
                        .map(|s| s.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }

    // ---- running ----------------------------------------------------------------------

    /// Runs requests as one transaction and diffs the project across it, atomically on the
    /// control thread so nobody else's edit is attributed to this one.
    fn exec(&self, caller: &Caller, reqs: Vec<Request>) -> Result<(Vec<Executed>, Diff), CmdError> {
        let c = caller.clone();
        let (done, before, after) = self.studio.session.call(move |s, h| {
            let before = s.project().clone();
            let r = s.batch(&c, reqs, h);
            let after = s.project().clone();
            r.map(|d| (d, before, after))
        })??;
        self.studio.after_save(&done);
        let d = if Arc::ptr_eq(&before, &after) {
            Diff::default()
        } else {
            diff(&before, &after)
        };
        Ok((done, d))
    }

    fn run_one(&self, caller: &Caller, req: Request) -> ToolOutput {
        match self.exec(caller, vec![req]) {
            Ok((done, d)) => ToolOutput::json(self.result_json(&done, &d)),
            Err(e) => ToolOutput::error(err_text(&e)),
        }
    }

    fn result_json(&self, done: &[Executed], d: &Diff) -> Value {
        let mut v = json!({
            "ok": true,
            "results": done.iter().map(|e| {
                let mut r = json!({ "command": e.command, "result": e.result });
                if let Some(s) = &e.summary { r["summary"] = json!(s); }
                r
            }).collect::<Vec<_>>(),
        });
        if let Some(t) = done.iter().filter_map(|e| e.txn).max() {
            v["txn"] = json!(t);
        }
        if !d.is_empty() {
            v["changes"] = d.to_json();
        }
        let w = self.studio.engine.stats().warnings;
        if !w.is_empty() {
            v["engine_warnings"] = json!(w);
        }
        v
    }

    fn run(&self, caller: &Caller, args: &Value) -> ToolOutput {
        let Some(id) = args["id"].as_str() else {
            return ToolOutput::error(
                "id is required, e.g. { \"id\": \"note.add\", \"params\": {...} }",
            );
        };
        let mut req = Request::new(id, args.get("params").cloned().unwrap_or(json!({})));
        req.confirm = args["confirm"].as_bool().unwrap_or(false);
        self.run_one(caller, req)
    }

    fn run_batch(&self, caller: &Caller, args: &Value) -> ToolOutput {
        let Some(cmds) = args["commands"].as_array() else {
            return ToolOutput::error("commands must be an array of { id, params }");
        };
        let mut reqs = Vec::new();
        for (i, c) in cmds.iter().enumerate() {
            let Some(id) = c["id"].as_str() else {
                return ToolOutput::error(format!("commands[{i}] has no id"));
            };
            let mut r = Request::new(id, c.get("params").cloned().unwrap_or(json!({})));
            r.confirm = c["confirm"].as_bool().unwrap_or(false);
            reqs.push(r);
        }
        if args["as_one_undo"].as_bool().unwrap_or(true) {
            return match self.exec(caller, reqs) {
                Ok((done, d)) => ToolOutput::json(self.result_json(&done, &d)),
                Err(e) => ToolOutput::error(format!(
                    "{} (nothing was applied: the batch is all-or-nothing)",
                    err_text(&e)
                )),
            };
        }
        // Separate transactions, stopping at the first failure.
        let (mut all, mut diffs, mut failure) = (Vec::new(), Vec::new(), None);
        for (i, r) in reqs.into_iter().enumerate() {
            match self.exec(caller, vec![r]) {
                Ok((mut done, d)) => {
                    all.append(&mut done);
                    diffs.extend(d.lines);
                }
                Err(e) => {
                    failure = Some(format!("commands[{i}]: {}", err_text(&e)));
                    break;
                }
            }
        }
        let mut v = self.result_json(&all, &Diff { lines: diffs });
        if let Some(f) = failure {
            v["ok"] = json!(false);
            v["error"] = json!(format!("{f}; the earlier commands stay applied"));
        }
        ToolOutput {
            is_error: v["ok"] == json!(false),
            blocks: vec![Block::Text(v.to_string())],
        }
    }

    fn undo_redo(&self, caller: &Caller, args: &Value, undo: bool) -> ToolOutput {
        let mode = match args["mode"].as_str() {
            None | Some("stop") => UndoMode::Stop,
            Some("skip") => UndoMode::Skip,
            Some("force") => UndoMode::Force,
            Some(o) => return ToolOutput::error(format!("unknown mode {o:?}: stop, skip, force")),
        };
        let author = caller.author.clone();
        let r = self.studio.session.call(move |s, _| {
            let before = s.project().clone();
            let out = if undo {
                s.undo_with(&author, mode)
            } else {
                s.redo_with(&author, mode)
            };
            out.map(|o| (o, before, s.project().clone()))
        });
        match r {
            Ok(Ok((Some(rep), before, after))) => {
                let d = diff(&before, &after);
                ToolOutput::json(json!({
                    "ok": true,
                    "txn": rep.txn,
                    "conflicts": rep.conflicts,
                    "changes": d.lines,
                }))
            }
            Ok(Ok((None, ..))) => ToolOutput::json(json!({
                "ok": true,
                "note": if undo { "nothing of yours to undo" } else { "nothing to redo" },
            })),
            Ok(Err(e)) | Err(e) => ToolOutput::error(err_text(&e)),
        }
    }

    fn history_tool(&self, args: &Value) -> ToolOutput {
        let limit = args["limit"].as_u64().unwrap_or(15) as usize;
        match self.studio.session.history() {
            Ok(h) => {
                let total = h.len();
                let rows: Vec<Value> = h
                    .iter()
                    .skip(total.saturating_sub(limit))
                    .map(|e| {
                        json!({
                            "txn": e.txn,
                            "author": e.author,
                            "kind": e.kind,
                            "commands": e.commands.iter().map(|c| c.command.clone()).collect::<Vec<_>>(),
                            "summary": e.summary,
                        })
                    })
                    .collect();
                ToolOutput::json(json!({ "total": total, "recent": rows }))
            }
            Err(e) => ToolOutput::error(e.to_string()),
        }
    }

    // ---- state ------------------------------------------------------------------------

    fn get_state(&self, caller: &Caller, args: &Value) -> ToolOutput {
        let view = args["view"].as_str().unwrap_or("overview");
        let a = args.get("args").cloned().unwrap_or(json!({}));
        let p = self.studio.project();
        let warnings = self.studio.engine.stats().warnings;
        let r: Result<Value, String> = match view {
            "overview" => Ok(views::overview(&p, &warnings)),
            "notes" => views::notes_view(&p, &a),
            "pattern" => views::pattern_view(&p, &a),
            "sections" => Ok(views::sections_view(&p)),
            "mixer" => Ok(views::mixer_view(&p)),
            "channels" => Ok(views::channels_view(&p)),
            "warnings" => Ok(json!({ "warnings": warnings })),
            "transport" => {
                let st = self.studio.engine.status();
                let t = views::timing(&p);
                Ok(json!({
                    "playing": st.playing,
                    "position": t.position(st.position),
                    "position_ticks": st.position,
                    "mode": format!("{:?}", st.mode),
                    "loop": st.loop_range,
                    "metronome": st.metronome,
                }))
            }
            "session" => Ok(json!({
                "you": caller.author.to_string(),
                "grant": grant_text(&caller.grant),
                "grants_fixed": self.grants.is_fixed(),
                "project_dir": self.studio.config.project_dir,
                "exports_dir": self.exports_dir(),
                "known_clients": self.grants.known().iter().map(|(k, v)| (k.clone(), grant_text(v))).collect::<std::collections::BTreeMap<_, _>>(),
            })),
            "events" => match self.studio.events() {
                Ok(h) => {
                    let since = a["since"].as_u64().unwrap_or(0);
                    Ok(json!({
                        "last_seq": h.last_seq(),
                        "events": h.since(since).iter().map(event_line).collect::<Vec<_>>(),
                    }))
                }
                Err(e) => Err(e),
            },
            "raw" => {
                let Some(q) = a["query"].as_str() else {
                    return ToolOutput::error(
                        "raw needs args.query, e.g. { \"view\": \"raw\", \"args\": { \"query\": \"query.project\" } }",
                    );
                };
                match self.studio.registry.get(q) {
                    Ok(c) if c.spec().mutating => {
                        return ToolOutput::error(format!("{q} changes the project; use run"));
                    }
                    Ok(_) => {
                        return self.run_one(
                            caller,
                            Request::new(q, a.get("params").cloned().unwrap_or(json!({}))),
                        );
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            other => Err(format!(
                "unknown view {other:?}: overview, notes, pattern, sections, mixer, channels, transport, session, events, warnings, raw"
            )),
        };
        match r {
            Ok(v) => ToolOutput::json(v),
            Err(e) => ToolOutput::error(e),
        }
    }

    // ---- audio ------------------------------------------------------------------------

    fn exports_dir(&self) -> PathBuf {
        self.studio
            .config
            .exports_dir
            .clone()
            .unwrap_or_else(|| self.studio.config.project_dir.join("renders"))
    }

    fn parse_range(&self, p: &Project, v: &Value) -> Result<Option<(Tick, Tick)>, String> {
        if v.is_null() {
            return Ok(None);
        }
        let bar = p.settings.time_signature.ticks_per_bar();
        if let (Some(a), Some(b)) = (v["from_bar"].as_u64(), v["to_bar"].as_u64()) {
            if a == 0 || b < a {
                return Err("range needs 1 <= from_bar <= to_bar".into());
            }
            return Ok(Some(((a - 1) * bar, b * bar)));
        }
        if let (Some(a), Some(b)) = (v["from_tick"].as_u64(), v["to_tick"].as_u64()) {
            if b <= a {
                return Err("range needs from_tick < to_tick".into());
            }
            return Ok(Some((a, b)));
        }
        if v.as_object().is_some_and(|o| o.is_empty()) {
            return Ok(None);
        }
        Err("range is { from_bar, to_bar } (1-based, inclusive) or { from_tick, to_tick }".into())
    }

    fn render_audio(
        &self,
        project: &Arc<Project>,
        range: Option<(Tick, Tick)>,
        tail: bool,
        sample_rate: u32,
        only: Option<ChannelId>,
    ) -> Result<Arc<Rendered>, String> {
        let key: RenderKey = (Arc::as_ptr(project) as usize, range, tail, sample_rate);
        if only.is_none()
            && let Some((k, r)) = &*self.cache.lock().unwrap()
            && *k == key
        {
            return Ok(r.clone());
        }
        let mut proj: Arc<Project> = project.clone();
        if let Some(id) = only {
            let mut p = (**project).clone();
            let mut chans = (*p.channels).clone();
            for c in chans.values_mut() {
                c.muted = c.id != id;
            }
            p.channels = Arc::new(chans);
            proj = Arc::new(p);
        }
        let mode = if proj.clips.is_empty() {
            proj.pattern_order
                .first()
                .map_or(PlayMode::Song, |p| PlayMode::Pattern(*p))
        } else {
            PlayMode::Song
        };
        let settings = RenderSettings {
            mode,
            range,
            sample_rate,
            quality: Quality::Live,
            include_tail: tail,
            block_frames: 512,
        };
        let audio = ph_engine::render_offline(&proj, &settings, self.studio.assets.as_ref())
            .map_err(|e| format!("render failed: {e}"))?;
        let bar = project.settings.time_signature.ticks_per_bar();
        let r = Arc::new(Rendered {
            audio,
            first_bar: range.map_or(1, |r| (r.0 / bar) as u32 + 1),
        });
        if only.is_none() {
            *self.cache.lock().unwrap() = Some((key, r.clone()));
        }
        Ok(r)
    }

    fn render_tool(&self, args: &Value) -> ToolOutput {
        let p = self.studio.project();
        let range = match self.parse_range(&p, &args["range"]) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(e),
        };
        let sr = args["sample_rate"].as_u64().unwrap_or(48_000) as u32;
        let fmt = args["format"].as_str().unwrap_or("wav");
        let bits = args["bit_depth"].as_u64().unwrap_or(24);
        let name = sanitize(args["name"].as_str().unwrap_or("render"));
        let rendered = match self.render_audio(&p, range, true, sr, None) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(e),
        };
        let dir = self.exports_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return ToolOutput::error(format!("cannot create {}: {e}", dir.display()));
        }
        let path = dir.join(format!("{name}.{fmt}"));
        let a = &rendered.audio;
        let written = match (fmt, bits) {
            ("wav", 16) => write_wav(&path, &a.channels, a.sample_rate, WavFormat::Pcm16),
            ("wav", 24) => write_wav(&path, &a.channels, a.sample_rate, WavFormat::Pcm24),
            ("wav", 32) => write_wav(&path, &a.channels, a.sample_rate, WavFormat::Float32),
            ("flac", 16) => write_flac(&path, &a.channels, a.sample_rate, FlacDepth::Bits16),
            ("flac", 24) => write_flac(&path, &a.channels, a.sample_rate, FlacDepth::Bits24),
            _ => {
                return ToolOutput::error("unsupported format: wav 16/24/32 or flac 16/24");
            }
        };
        if let Err(e) = written {
            return ToolOutput::error(format!("could not write {}: {e}", path.display()));
        }
        let timing = views::timing(&p);
        let l = loudness::analyze(&a.channels, a.sample_rate);
        let h = health::analyze(&a.channels, a.sample_rate, &timing, rendered.first_bar);
        let pn = views::pitched_only(&p, song_range_notes(&p, range));
        let mut v = json!({
            "ok": true,
            "path": path,
            "seconds": (a.frames() as f64 / a.sample_rate as f64 * 100.0).round() / 100.0,
            "sample_rate": a.sample_rate,
            "analysis": {
                "integrated_lufs": l.integrated_lufs,
                "true_peak_dbtp": l.true_peak_dbtp,
                "rms_dbfs": l.rms_dbfs,
                "clipped_samples": h.clipped_samples,
                "key": key_from_notes(&pn, &timing).map(|k| k.key),
                "warnings": h.warnings,
            },
        });
        let w = self.studio.engine.stats().warnings;
        if !w.is_empty() {
            v["engine_warnings"] = json!(w);
        }
        if p.channels.is_empty() || p.patterns.values().all(|x| x.note_count() == 0) {
            v["note"] = json!("the project has no notes, so the render is silent");
        }
        ToolOutput::json(v)
    }

    fn analyze_tool(&self, args: &Value) -> ToolOutput {
        let p = self.studio.project();
        let range = match self.parse_range(&p, &args["range"]) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(e),
        };
        let mut what: Vec<String> = args["what"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if what.is_empty() {
            what = ["loudness", "spectrum", "health", "key", "chords", "clashes"]
                .map(String::from)
                .to_vec();
        }
        if what.iter().any(|w| w == "all") {
            what = [
                "loudness", "spectrum", "health", "onsets", "key", "chords", "clashes", "masking",
            ]
            .map(String::from)
            .to_vec();
        }
        const KNOWN: [&str; 8] = [
            "loudness", "spectrum", "health", "onsets", "key", "chords", "clashes", "masking",
        ];
        if let Some(bad) = what.iter().find(|w| !KNOWN.contains(&w.as_str())) {
            return ToolOutput::error(format!(
                "unknown analysis {bad:?}: {} or all",
                KNOWN.join(", ")
            ));
        }
        let needs_audio = what.iter().any(|w| !matches!(w.as_str(), "clashes"));
        let timing = views::timing(&p);
        let sr = 48_000;
        let rendered = if needs_audio {
            match self.render_audio(&p, range, false, sr, None) {
                Ok(r) => Some(r),
                Err(e) => return ToolOutput::error(e),
            }
        } else {
            None
        };
        let bar = timing.ticks_per_bar();
        let mut out = serde_json::Map::new();
        if let Some(r) = &rendered {
            let a = &r.audio;
            out.insert(
                "range".into(),
                json!({
                    "seconds": (a.frames() as f64 / a.sample_rate as f64 * 100.0).round() / 100.0,
                    "first_bar": r.first_bar,
                }),
            );
        }
        let notes = song_range_notes(&p, range);
        let pitched = views::pitched_only(&p, notes.clone());
        let (first_bar, last_bar) = match (range, &rendered) {
            (Some((a, b)), _) => ((a / bar) as u32 + 1, b.div_ceil(bar) as u32),
            (None, Some(r)) => (
                1,
                ((r.audio.frames() as f64 / r.audio.sample_rate as f64 / timing.seconds_per_bar())
                    .ceil() as u32)
                    .max(1),
            ),
            (None, None) => (
                1,
                (notes
                    .iter()
                    .map(|n| n.start + n.length)
                    .max()
                    .unwrap_or(bar)
                    .div_ceil(bar) as u32)
                    .max(1),
            ),
        };
        for w in &what {
            let ch = rendered.as_ref().map(|r| &r.audio.channels[..]);
            let sr = rendered.as_ref().map_or(sr, |r| r.audio.sample_rate);
            let fb = rendered.as_ref().map_or(1, |r| r.first_bar);
            let v = match (w.as_str(), ch) {
                ("loudness", Some(c)) => json!(loudness::analyze(c, sr)),
                ("spectrum", Some(c)) => json!(spectrum::analyze(c, sr)),
                ("health", Some(c)) => json!(health::analyze(c, sr, &timing, fb)),
                ("onsets", Some(c)) => {
                    let o = onsets::detect(c, sr, &timing, fb);
                    json!({ "count": o.len(), "onsets": o.iter().take(200).collect::<Vec<_>>() })
                }
                ("key", Some(c)) => json!({
                    "from_notes": key_from_notes(&pitched, &timing),
                    "from_audio": key_from_audio(c, sr, &timing),
                }),
                ("chords", Some(c)) => {
                    let n =
                        chords_from_notes(&pitched, &timing, first_bar, last_bar, ChordGrain::Auto);
                    let au = chords_from_audio(c, sr, &timing, fb, ChordGrain::Bar);
                    json!({
                        "from_notes": { "progression": progression(&n), "by_bar": n.iter().take(64).map(|s| format!("{}.{} {}", s.bar, s.beat, s.symbol)).collect::<Vec<_>>() },
                        "from_audio": { "progression": progression(&au), "by_bar": au.iter().take(64).map(|s| format!("{}.{} {}", s.bar, s.beat, s.symbol)).collect::<Vec<_>>() },
                    })
                }
                ("clashes", _) => {
                    let set = views::pitched_channels(&p, &notes);
                    let pp = p.clone();
                    let clashes = masking::note_clashes(
                        &notes,
                        &|c| set.contains(&c),
                        &|c| pp.channels.get(&c).map_or("?".into(), |x| x.name.clone()),
                        &timing,
                    );
                    json!({
                        "count": clashes.len(),
                        "clashes": clashes,
                        "note": "notes a semitone apart, sounding together, from different pitched channels",
                    })
                }
                ("masking", Some(_)) => match self.masking(&p, range, sr) {
                    Ok(v) => v,
                    Err(e) => return ToolOutput::error(e),
                },
                _ => json!(null),
            };
            out.insert(w.clone(), v);
        }
        let mut v = Value::Object(out);
        let w = self.studio.engine.stats().warnings;
        if !w.is_empty() {
            v["engine_warnings"] = json!(w);
        }
        ToolOutput::json(v)
    }

    fn masking(
        &self,
        p: &Arc<Project>,
        range: Option<(Tick, Tick)>,
        sr: u32,
    ) -> Result<Value, String> {
        let mut stems = Vec::new();
        for c in p.channels_in_order() {
            if c.muted || p.patterns.values().all(|x| !x.notes.contains_key(&c.id)) {
                continue;
            }
            let r = self.render_audio(p, range, false, sr, Some(c.id))?;
            stems.push(masking::stem_bands(
                &c.name,
                &r.audio.channels,
                r.audio.sample_rate,
            ));
        }
        let m = masking::band_masking(&stems);
        Ok(json!({ "stems": stems.len(), "overlaps": m }))
    }

    // ---- snapshots --------------------------------------------------------------------

    fn snapshot_tool(&self, args: &Value) -> ToolOutput {
        let Some(kind) = args["kind"].as_str() else {
            return ToolOutput::error(
                "kind is required: waveform, spectrogram, piano_roll, playlist",
            );
        };
        let p = self.studio.project();
        let range = match self.parse_range(&p, &args["range"]) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(e),
        };
        let width = args["width"].as_u64().unwrap_or(1400).clamp(480, 4000) as u32;
        let timing = views::timing(&p);
        let bar = timing.ticks_per_bar();
        let (png, caption) = match kind {
            "waveform" | "spectrogram" => {
                let r = match self.render_audio(&p, range, false, 48_000, None) {
                    Ok(r) => r,
                    Err(e) => return ToolOutput::error(e),
                };
                let secs = r.audio.frames() as f64 / r.audio.sample_rate as f64;
                if kind == "waveform" {
                    (
                        image::waveform(&image::WaveformSpec {
                            channels: &r.audio.channels,
                            sample_rate: r.audio.sample_rate,
                            timing,
                            range_s: None,
                            first_bar: r.first_bar,
                            width,
                        }),
                        format!(
                            "waveform, {secs:.1} s from bar {}, L over R; red = clipping",
                            r.first_bar
                        ),
                    )
                } else {
                    (
                        image::spectrogram(&image::SpectrogramSpec {
                            channels: &r.audio.channels,
                            sample_rate: r.audio.sample_rate,
                            timing,
                            range_s: None,
                            first_bar: r.first_bar,
                            width,
                        }),
                        format!(
                            "spectrogram, {secs:.1} s from bar {}, log frequency 30 Hz-20 kHz, colour is dBFS",
                            r.first_bar
                        ),
                    )
                }
            }
            "piano_roll" => match self.piano_roll(&p, args, range, width) {
                Ok(x) => x,
                Err(e) => return ToolOutput::error(e),
            },
            "playlist" => {
                let end = p.song_length().max(8 * bar);
                let rg = range.unwrap_or((0, end.div_ceil(bar) * bar));
                let used = p.clips.values().map(|c| c.track).max().unwrap_or(0) as usize + 1;
                let rows = used.max(4).min(p.tracks.len());
                let tracks: Vec<PlaylistTrackRow> = p
                    .tracks
                    .iter()
                    .take(rows)
                    .map(|t| PlaylistTrackRow {
                        name: t.name.clone(),
                        muted: t.muted,
                    })
                    .collect();
                let clips: Vec<PlaylistClipBox> = p
                    .clips
                    .values()
                    .map(|c| {
                        let pat = p.patterns.get(&c.pattern);
                        PlaylistClipBox {
                            track: c.track as usize,
                            start: c.start,
                            length: c.length,
                            label: pat.map_or("?".into(), |x| x.name.clone()),
                            color: pat.map_or(0x5f9ea0, |x| x.color),
                        }
                    })
                    .collect();
                (
                    image::playlist(&PlaylistSpec {
                        title: format!(
                            "PLAYLIST  BARS {}-{}  {} BPM  {}/{}",
                            rg.0 / bar + 1,
                            rg.1.div_ceil(bar),
                            p.settings.bpm,
                            p.settings.time_signature.numerator,
                            p.settings.time_signature.denominator
                        ),
                        tracks: &tracks,
                        clips: &clips,
                        timing,
                        range: rg,
                        width,
                    }),
                    format!(
                        "playlist overview, bars {}-{}",
                        rg.0 / bar + 1,
                        rg.1.div_ceil(bar)
                    ),
                )
            }
            other => {
                return ToolOutput::error(format!(
                    "unknown kind {other:?}: waveform, spectrogram, piano_roll, playlist"
                ));
            }
        };
        let mut blocks = vec![Block::Text(caption.clone())];
        if args["save"].as_bool().unwrap_or(false) {
            let dir = self.exports_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("snapshot-{kind}.png"));
            match std::fs::write(&path, &png) {
                Ok(()) => blocks.push(Block::Text(format!("saved {}", path.display()))),
                Err(e) => blocks.push(Block::Text(format!("could not save: {e}"))),
            }
        }
        blocks.push(Block::Image(png));
        ToolOutput {
            blocks,
            is_error: false,
        }
    }

    fn piano_roll(
        &self,
        p: &Arc<Project>,
        args: &Value,
        range: Option<(Tick, Tick)>,
        width: u32,
    ) -> Result<(Vec<u8>, String), String> {
        let timing = views::timing(p);
        let bar = timing.ticks_per_bar();
        let (mut notes, what): (Vec<SongNote>, String) =
            match args.get("pattern").filter(|v| !v.is_null()) {
                Some(v) => {
                    let id: PatternId = views::find_pattern(p, v)?;
                    (
                        ph_analysis::music::pattern_notes(p, id),
                        format!("PATTERN {}", p.patterns[&id].name.to_uppercase()),
                    )
                }
                None if p.clips.is_empty() => match p.pattern_order.first() {
                    Some(id) => (
                        ph_analysis::music::pattern_notes(p, *id),
                        format!("PATTERN {}", p.patterns[id].name.to_uppercase()),
                    ),
                    None => return Err("the project has no patterns yet".into()),
                },
                None => (song_notes(p), "SONG".to_string()),
            };
        if let Some(list) = args["channels"].as_array() {
            let mut keep = Vec::new();
            for c in list {
                keep.push(views::find_channel(p, c)?);
            }
            notes.retain(|n| keep.contains(&n.channel));
        }
        let end = notes
            .iter()
            .map(|n| n.start + n.length)
            .max()
            .unwrap_or(bar);
        let rg = range.unwrap_or((0, end.div_ceil(bar).max(1) * bar));
        // Legend: the channels that have notes, in rack order.
        let mut order: Vec<ChannelId> = Vec::new();
        for c in p.channels_in_order() {
            if notes.iter().any(|n| n.channel == c.id) {
                order.push(c.id);
            }
        }
        let legend: Vec<(String, u32)> = order
            .iter()
            .map(|id| (p.channels[id].name.clone(), p.channels[id].color))
            .collect();
        let group = |c: ChannelId| order.iter().position(|x| *x == c).unwrap_or(0);
        let rn = image::roll_notes(&notes, &group);
        let png = image::piano_roll(&PianoRollSpec {
            title: format!(
                "PIANO ROLL  {what}  BARS {}-{}  {} BPM  C4 = MIDI 60",
                rg.0 / bar + 1,
                rg.1.div_ceil(bar),
                p.settings.bpm
            ),
            notes: &rn,
            legend: &legend,
            timing,
            range: rg,
            width,
        });
        Ok((
            png,
            format!(
                "piano roll of {}, bars {}-{}, {} notes; colours per channel: {}",
                what.to_lowercase(),
                rg.0 / bar + 1,
                rg.1.div_ceil(bar),
                notes.len(),
                legend
                    .iter()
                    .map(|l| l.0.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))
    }
}

fn song_range_notes(p: &Project, range: Option<(Tick, Tick)>) -> Vec<SongNote> {
    let mut n = if p.clips.is_empty() {
        p.pattern_order
            .first()
            .map(|id| ph_analysis::music::pattern_notes(p, *id))
            .unwrap_or_default()
    } else {
        song_notes(p)
    };
    if let Some((a, b)) = range {
        n.retain(|x| x.start < b && x.start + x.length > a);
    }
    n
}

fn range_schema() -> Value {
    json!({
        "type": "object",
        "description": "{ from_bar, to_bar }, 1-based and inclusive; omit for the whole song",
        "properties": { "from_bar": { "type": "integer" }, "to_bar": { "type": "integer" } }
    })
}

fn tool(name: &str, description: &str, schema: Value, read_only: bool) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        read_only,
    }
}

/// One line for an event, for tool results and the events view.
pub fn event_line(e: &Event) -> String {
    match (&e.author, e.kind) {
        (Some(a), "edit") => {
            let detail = e.data["changes"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .take(3)
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|s| !s.is_empty());
            format!(
                "#{} {a} edited: {}{}",
                e.seq,
                e.summary,
                detail.map(|d| format!(" [{d}]")).unwrap_or_default()
            )
        }
        _ => format!("#{} {}: {}", e.seq, e.kind, e.summary),
    }
}
