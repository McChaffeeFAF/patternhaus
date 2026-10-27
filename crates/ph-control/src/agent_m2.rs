//! The M2 additions to the agent toolset (M2/T9): convenience tools for audio clips,
//! automation and plugins, the proposal (preview mode) tools and views, a rendered audio
//! excerpt for audio-capable models, and reference-track comparison.
//!
//! A child module of `agent`, so it shares `Agent`'s private state.

use super::{Agent, Block, ToolOutput, ToolSpec, tool};
use crate::diff::diff;
use ph_analysis::reference;
use ph_commands::{Author, ProposalInfo};
use ph_core::{Project, SampleId, Tick};
use ph_formats::encode::{WavFormat, encode_wav};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Longest audio excerpt `render` returns, in seconds.
pub const MAX_LISTEN_SECONDS: f64 = 15.0;
/// Largest WAV a `render` excerpt may be, in bytes (base64 adds a third on the wire).
pub const MAX_LISTEN_BYTES: usize = 1_500_000;
/// Default excerpt length, in seconds.
const DEFAULT_LISTEN_SECONDS: f64 = 8.0;
/// Excerpts are rendered at this rate (16-bit): enough for judging a mix, small enough to send.
const LISTEN_RATE: u32 = 24_000;
/// Reference files longer than this are cut (seconds).
const MAX_REFERENCE_SECONDS: f64 = 900.0;
/// Longest `proposal_status` wait, in seconds.
const MAX_WAIT_SECONDS: f64 = 60.0;

/// Better descriptions than the command summary for the convenience tools.
pub(super) fn direct_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "add_audio_clip" => {
            "Place an audio clip on a playlist track (audio.add_clip). Give `sample` (a registered sample id: get_state view audio lists them) or `path` (factory:..., samples/loop.wav, or a file inside the project or library folders; it is copied into the project). `start` is in ticks (960 per quarter note); `track` is 0-based. For a loop that follows the project tempo use tempo_sync: true with stretch \"stretch\" (keeps pitch) once the sample has a bpm (sample.set_bpm or sample.detect_bpm), or call fit_to_tempo. Shape it afterwards with audio.set_gain, set_fade, set_pitch, set_reverse, set_range, split, duplicate (list_commands \"audio\")."
        }
        "fit_to_tempo" => {
            "Make an audio clip follow the project tempo using its sample's bpm, sized to the whole loop (audio.fit_to_tempo). Needs the sample's bpm: sample.detect_bpm or sample.set_bpm first."
        }
        "add_automation" => {
            "Create an automation clip that moves a parameter over time (automation.add_clip). `target` is one of tempo, channel/<id>/volume, channel/<id>/pan, channel/<id>/param/<param>, insert/<n>/volume, insert/<n>/pan, insert/<n>/mute, insert/<n>/send/<m>, insert/<n>/slot/<s>/param/<param> (node_params lists parameter ids). `start` and `length` are ticks; each point is { tick (from the clip start), value (in the target's own unit), curve: linear | hold | smooth | s_curve }. Read it back with get_state view automation; edit with automation.add_point, move_point, set_curve, remove_points."
        }
        "add_plugin_instrument" => {
            "Add a channel that plays an installed CLAP or VST3 instrument (plugin.add_instrument). Run `run plugin.scan` once and `run plugin.list` to find the plugin id; node_params or plugin.get_params list its parameters; set them with plugin.set_param. Plugins are hosted out of process: a crash silences the plugin, not the session."
        }
        "add_plugin_effect" => {
            "Add an installed CLAP or VST3 effect to a mixer insert (plugin.add_effect). Run `run plugin.scan` and `run plugin.list` for the plugin id; plugin.set_param changes its parameters."
        }
        "propose" => {
            "Propose a batch of commands instead of applying it (preview mode, proposal.submit). The person sees your change as ghost notes and clips in your colour and accepts or rejects it; nothing changes until they accept, and an accepted proposal is one undo step attributed to you. Use it for bigger or riskier changes (a new section, a rewrite, a mix move). Returns the proposal id and a would_change preview of the musical diff. Then read the decision with proposal_status (wait_seconds to wait for it) or in the proposal_decisions block of a later reply. Transport, file, record, MIDI and UI commands cannot be proposed. A proposal made stale by someone else's edit to the same objects is reported stale, never force-applied: propose again."
        }
        _ => return None,
    })
}

/// Tools that are not plain command shortcuts.
pub(super) fn extra_tools() -> Vec<ToolSpec> {
    vec![tool(
        "proposal_status",
        "Read the decision on your proposals (see propose). With id: that proposal; without: all pending and recently decided ones. status.state is pending, accepted (with the transaction), rejected (with the reason) or stale (with why it could not be applied). wait_seconds (max 60) waits for a pending proposal to be decided.",
        json!({ "type": "object", "properties": {
            "id": { "type": "integer", "description": "proposal id from propose; omit to list all" },
            "wait_seconds": { "type": "number", "description": "wait up to this long for a decision (0-60, default 0)" } } }),
        true,
    )]
}

/// A reference track, loaded.
pub(super) struct Reference {
    label: String,
    audio: Vec<Vec<f32>>,
    rate: u32,
    seconds: f64,
    truncated: bool,
}

fn decided(p: &ProposalInfo) -> bool {
    !p.pending()
}

impl Agent {
    // ---- proposals --------------------------------------------------------------------

    fn proposals(&self) -> Result<Vec<ProposalInfo>, String> {
        self.studio.session.proposals().map_err(|e| e.to_string())
    }

    /// `get_state { view: "proposals" }`.
    pub(super) fn proposals_view(&self) -> Result<Value, String> {
        let list = self.proposals()?;
        let pending = list.iter().filter(|p| p.pending()).count();
        Ok(json!({
            "pending": pending,
            "proposals": list.iter().map(|p| p.to_json()).collect::<Vec<_>>(),
            "text": if list.is_empty() {
                "no proposals (submit one with propose)".to_string()
            } else {
                list.iter().map(|p| format!("#{} \"{}\" by {}: {}", p.id, p.title, p.author, p.status.name())).collect::<Vec<_>>().join("; ")
            },
        }))
    }

    /// The `proposal_status` tool.
    pub(super) fn proposal_status(&self, client: &str, args: &Value) -> ToolOutput {
        let id = args["id"].as_u64();
        let wait = args["wait_seconds"]
            .as_f64()
            .unwrap_or(0.0)
            .clamp(0.0, MAX_WAIT_SECONDS);
        let deadline = Instant::now() + Duration::from_secs_f64(wait);
        loop {
            let list = match self.proposals() {
                Ok(l) => l,
                Err(e) => return ToolOutput::error(e),
            };
            let mine = Author::Agent(client.to_string());
            let out = match id {
                Some(id) => {
                    let Some(p) = list.iter().find(|p| p.id == id) else {
                        return ToolOutput::error(format!(
                            "no proposal {id} (it may be older than the kept decisions); status without id lists them"
                        ));
                    };
                    if p.pending() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    if decided(p) && p.author == mine {
                        self.mark_decision_seen(client, p.decided_seq);
                    }
                    json!({ "proposal": p.to_json(), "decided": decided(p) })
                }
                None => {
                    let pending = list.iter().filter(|p| p.pending()).count();
                    if pending > 0 && list.iter().all(|p| p.pending()) && Instant::now() < deadline
                    {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    for p in list.iter().filter(|p| decided(p) && p.author == mine) {
                        self.mark_decision_seen(client, p.decided_seq);
                    }
                    json!({ "pending": pending, "proposals": list.iter().map(|p| p.to_json()).collect::<Vec<_>>() })
                }
            };
            return ToolOutput::json(out);
        }
    }

    fn mark_decision_seen(&self, client: &str, seq: u64) {
        let mut g = self.seen_decisions.lock().unwrap();
        let e = g.entry(client.to_string()).or_insert(0);
        *e = (*e).max(seq);
    }

    /// Appends the decisions made on this client's proposals since it last heard.
    pub(super) fn with_decisions(&self, client: &str, mut out: ToolOutput) -> ToolOutput {
        let Ok(list) = self.studio.session.proposals() else {
            return out;
        };
        let me = Author::Agent(client.to_string());
        let seen = self
            .seen_decisions
            .lock()
            .unwrap()
            .get(client)
            .copied()
            .unwrap_or(0);
        let news: Vec<&ProposalInfo> = list
            .iter()
            .filter(|p| p.author == me && decided(p) && p.decided_seq > seen)
            .collect();
        let Some(last) = news.iter().map(|p| p.decided_seq).max() else {
            return out;
        };
        self.mark_decision_seen(client, last);
        out.blocks.push(Block::Text(
            json!({ "proposal_decisions": news.iter().map(|p| p.to_json()).collect::<Vec<_>>() })
                .to_string(),
        ));
        out
    }

    /// Adds `would_change` (the musical diff of the preview) to each `proposal.submit` result.
    pub(super) fn attach_previews(&self, v: &mut Value, done: &[ph_commands::Executed]) {
        for (i, e) in done.iter().enumerate() {
            if e.command != "proposal.submit" {
                continue;
            }
            let Some(id) = e.result["proposal"].as_u64() else {
                continue;
            };
            if let Ok(Some(info)) = self.studio.session.call(move |s, _| s.proposal(id)) {
                let d = diff(&info.base, &info.preview);
                v["results"][i]["would_change"] = d.to_json();
            }
        }
    }

    // ---- audio excerpt for audio-capable models ----------------------------------------

    /// Finishes a `render` reply: with `listen: true`, adds a short WAV excerpt as audio
    /// content, bounded by [`MAX_LISTEN_SECONDS`] and [`MAX_LISTEN_BYTES`].
    pub(super) fn attach_listen(
        &self,
        p: &Arc<Project>,
        range: Option<(Tick, Tick)>,
        args: &Value,
        mut v: Value,
    ) -> ToolOutput {
        if !args["listen"].as_bool().unwrap_or(false) {
            return ToolOutput::json(v);
        }
        let limits = json!({ "max_seconds": MAX_LISTEN_SECONDS, "max_bytes": MAX_LISTEN_BYTES, "sample_rate": LISTEN_RATE, "format": "16-bit PCM WAV" });
        let t = crate::views::timing(p);
        let (start, mut end) = match self.parse_range(p, &args["listen_range"]) {
            Ok(Some(r)) => r,
            Ok(None) => (range.map_or(0, |r| r.0), Tick::MAX),
            Err(e) => {
                v["listen"] = json!({ "error": e, "limits": limits });
                return ToolOutput::json(v);
            }
        };
        let seconds = args["listen_seconds"]
            .as_f64()
            .unwrap_or(DEFAULT_LISTEN_SECONDS)
            .clamp(0.5, MAX_LISTEN_SECONDS);
        let cap = start + t.seconds_to_ticks(seconds);
        let mut clipped = end > cap;
        end = end.min(cap);
        if let Some(r) = range {
            clipped |= end > r.1;
            end = end.min(r.1);
        }
        if end <= start {
            v["listen"] = json!({ "error": "the excerpt range is empty (it starts after the end of the render range)", "limits": limits });
            return ToolOutput::json(v);
        }
        let r = match self.render_audio(p, Some((start, end)), false, LISTEN_RATE, None) {
            Ok(r) => r,
            Err(e) => {
                v["listen"] = json!({ "error": e, "limits": limits });
                return ToolOutput::json(v);
            }
        };
        let a = &r.audio;
        // stereo while it fits, else mono, else cut
        let mut chans: Vec<&[f32]> = a.channels.iter().take(2).map(|c| &c[..]).collect();
        let mut mono: Vec<f32> = Vec::new();
        let max_frames_stereo = (MAX_LISTEN_BYTES - 64) / (2 * chans.len().max(1));
        if chans.len() == 2 && a.frames() > max_frames_stereo {
            mono = (0..a.frames())
                .map(|i| 0.5 * (a.channels[0][i] + a.channels[1][i]))
                .collect();
            chans = vec![&mono[..]];
        }
        let max_frames = (MAX_LISTEN_BYTES - 64) / (2 * chans.len().max(1));
        let frames = a.frames().min(max_frames);
        let truncated = clipped || frames < a.frames();
        let cut: Vec<&[f32]> = chans.iter().map(|c| &c[..frames.min(c.len())]).collect();
        let _ = &mono;
        let wav = match encode_wav(&cut, a.sample_rate, WavFormat::Pcm16, 0x5eed) {
            Ok(w) => w,
            Err(e) => {
                v["listen"] = json!({ "error": format!("could not encode the excerpt: {e}"), "limits": limits });
                return ToolOutput::json(v);
            }
        };
        let peak = cut
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |m, x| m.max(x.abs()));
        let bar = t.ticks_per_bar();
        v["listen"] = json!({
            "attached": "audio/wav",
            "from": t.position(start),
            "to": t.position(start + t.seconds_to_ticks(frames as f64 / a.sample_rate as f64)),
            "from_bar": start / bar + 1,
            "seconds": (frames as f64 / a.sample_rate as f64 * 100.0).round() / 100.0,
            "sample_rate": a.sample_rate,
            "channels": cut.len(),
            "bytes": wav.len(),
            "truncated": truncated,
            "silent": peak < 1e-4,
            "limits": limits,
            "note": if truncated {
                "the excerpt was cut to the limits; ask for another part with listen_range"
            } else {
                "the audio block follows this text"
            },
        });
        ToolOutput {
            blocks: vec![Block::Text(v.to_string()), Block::Audio(wav)],
            is_error: false,
        }
    }

    // ---- reference-track comparison ----------------------------------------------------

    /// Loads the reference named by `analyze`'s `reference` argument: a registered sample (its
    /// id), or a file reference such as `factory:...`, `samples/x.wav` or a path inside the
    /// folders agents may read. Nothing is copied anywhere.
    pub(super) fn load_reference(&self, p: &Project, v: &Value) -> Result<Reference, String> {
        const HELP: &str = "analyze what: [\"reference\"] needs `reference`: a registered sample id (get_state view audio) or an audio file inside the project or library folders";
        let (label, path) = match v {
            Value::Null => return Err(HELP.into()),
            Value::Number(n) => {
                let id = SampleId(n.as_u64().ok_or(HELP)? as u32);
                let e = p.sample(id).map_err(|_| {
                    format!("no registered sample {id} (get_state view audio lists them)")
                })?;
                (e.name.clone(), e.path.clone())
            }
            Value::String(s) => match s
                .parse::<u32>()
                .ok()
                .and_then(|n| p.sample(SampleId(n)).ok())
            {
                Some(e) => (e.name.clone(), e.path.clone()),
                None => (
                    s.rsplit(['/', '\\']).next().unwrap_or(s).to_string(),
                    s.clone(),
                ),
            },
            _ => return Err(HELP.into()),
        };
        let d = self
            .studio
            .assets
            .sample(&path)
            .map_err(|e| format!("cannot read the reference {path}: {e}"))?;
        let rate = d.sample_rate.round().max(1.0) as u32;
        let max_frames = (MAX_REFERENCE_SECONDS * rate as f64) as usize;
        let truncated = d.frames() > max_frames;
        let audio: Vec<Vec<f32>> = d
            .channels
            .iter()
            .map(|c| c[..c.len().min(max_frames)].to_vec())
            .collect();
        let seconds = audio.first().map_or(0, |c| c.len()) as f64 / rate as f64;
        if audio.is_empty() || seconds <= 0.0 {
            return Err(format!("the reference {path} holds no audio"));
        }
        Ok(Reference {
            label,
            audio,
            rate,
            seconds,
            truncated,
        })
    }

    /// The `reference` part of an `analyze` reply.
    pub(super) fn reference_report(&self, mix: &[Vec<f32>], mix_rate: u32, r: &Reference) -> Value {
        let c = reference::compare(mix, mix_rate, &r.audio, r.rate);
        let mut v = serde_json::to_value(&c).unwrap_or(Value::Null);
        v["reference"] = json!({
            "name": r.label,
            "seconds": (r.seconds * 10.0).round() / 10.0,
            "sample_rate": r.rate,
            "cut_to_first_seconds": r.truncated.then_some(MAX_REFERENCE_SECONDS),
        });
        v
    }
}
