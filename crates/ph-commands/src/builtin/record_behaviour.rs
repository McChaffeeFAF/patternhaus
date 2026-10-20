//! Behaviour tests for `record.*` and `midi.*` (M2/T8), on a scripted host: no device is
//! ever opened. The engine side is tested in `ph-engine` and end to end in `ph-control`.

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::sync::Arc;

/// A host whose devices are scripts.
#[derive(Default)]
struct RecHost {
    inner: NullHost,
    armed: Vec<ArmRequest>,
    disarmed: Vec<u32>,
    monitor: Vec<(u32, bool)>,
    started: Vec<RecordStart>,
    recording: bool,
    /// What `record_stop` hands back.
    outcome: Option<RecordOutcome>,
    learning: Option<ParamPath>,
    maps: Vec<MidiMappingEntry>,
    midi_target: Option<ChannelId>,
}

impl Host for RecHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        self.inner.transport(cmd)
    }
    fn transport_state(&self) -> TransportState {
        self.inner.transport_state()
    }
    fn render(&mut self, p: &Project, r: &RenderRequest) -> CmdResult<RenderReport> {
        self.inner.render(p, r)
    }
    fn resolve_path(&self, p: &str, a: FileAccess) -> CmdResult<std::path::PathBuf> {
        self.inner.resolve_path(p, a)
    }
    fn ui(&mut self, r: UiRequest) -> CmdResult<()> {
        self.inner.ui(r)
    }
    fn record_inputs(&mut self) -> CmdResult<RecordInputs> {
        Ok(RecordInputs {
            inputs: vec![RecordInput {
                id: 0,
                name: "Scripted: In 1-2".into(),
                channels: 2,
                sample_rate: 48_000,
                armed: !self.armed.is_empty(),
                monitor: true,
                track: self.armed.first().and_then(|a| a.track),
                channel: self.armed.first().and_then(|a| a.channel),
            }],
            latency: LatencyInfo {
                sample_rate: 48_000,
                input_frames: 96,
                output_frames: 144,
                monitor_ms: 5.0,
                compensation_ms: 5.0,
            },
            recording: self.recording,
        })
    }
    fn record_arm(&mut self, arm: ArmRequest) -> CmdResult<()> {
        if arm.input != 0 {
            return Err(CmdError::Host(format!("no audio input {}", arm.input)));
        }
        self.armed.push(arm);
        Ok(())
    }
    fn record_disarm(&mut self, input: u32) -> CmdResult<()> {
        self.disarmed.push(input);
        Ok(())
    }
    fn record_set_monitor(&mut self, input: u32, on: bool) -> CmdResult<()> {
        self.monitor.push((input, on));
        Ok(())
    }
    fn record_start(&mut self, start: RecordStart) -> CmdResult<RecordStarted> {
        let notes = start.notes.is_some();
        self.started.push(start);
        self.recording = true;
        Ok(RecordStarted {
            audio_inputs: self.armed.len() as u32,
            notes,
        })
    }
    fn record_stop(&mut self) -> CmdResult<RecordOutcome> {
        if !self.recording {
            return Err(CmdError::Host("not recording".into()));
        }
        self.recording = false;
        Ok(self.outcome.take().unwrap_or(RecordOutcome {
            takes: vec![],
            notes: vec![],
            note_target: None,
        }))
    }
    fn midi_target_channel(&self) -> Option<ChannelId> {
        self.midi_target
    }
    fn midi_inputs(&mut self) -> CmdResult<Vec<MidiInputEntry>> {
        Ok(vec![MidiInputEntry {
            name: "Scripted Keys".into(),
            connected: true,
        }])
    }
    fn midi_learn(&mut self, target: Option<ParamPath>) -> CmdResult<()> {
        self.learning = target;
        Ok(())
    }
    fn midi_mappings(&mut self) -> CmdResult<Vec<MidiMappingEntry>> {
        Ok(self.maps.clone())
    }
    fn midi_map(&mut self, m: MidiMappingEntry) -> CmdResult<Option<MidiMappingEntry>> {
        let old = self
            .maps
            .iter()
            .position(|x| x.controller == m.controller && x.device == m.device)
            .map(|i| self.maps.remove(i));
        self.maps.push(m);
        Ok(old)
    }
    fn midi_unmap(
        &mut self,
        controller: u8,
        device: Option<String>,
    ) -> CmdResult<Option<MidiMappingEntry>> {
        Ok(self
            .maps
            .iter()
            .position(|x| x.controller == controller && x.device == device)
            .map(|i| self.maps.remove(i)))
    }
}

struct Rig {
    s: Session,
    h: RecHost,
    who: Caller,
    pattern: PatternId,
    sampler: ChannelId,
    synth: ChannelId,
}

impl Rig {
    fn new() -> Self {
        let mut e = Edit::new(&Project::empty("Rec"));
        e.update_settings(|s| s.bpm = 120.0).unwrap();
        let sampler = e
            .add_channel(
                "Vocal",
                InstrumentSpec::Sampler {
                    sample: None,
                    root_key: 60,
                    params: Default::default(),
                },
            )
            .unwrap();
        let synth = e
            .add_channel(
                "Keys",
                InstrumentSpec::Synth {
                    preset: None,
                    params: Default::default(),
                },
            )
            .unwrap();
        let pattern = e.add_pattern("Riff").unwrap();
        let (project, _) = e.finish();
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), project),
            h: RecHost::default(),
            who: Caller::human(),
            pattern,
            sampler,
            synth,
        }
    }

    fn p(&self) -> &Project {
        self.s.project()
    }

    fn try_run(&mut self, cmd: &str, params: Value) -> CmdResult<Executed> {
        self.s
            .execute(&self.who, Request::new(cmd, params), &mut self.h)
    }

    fn run(&mut self, cmd: &str, params: Value) -> Value {
        match self.try_run(cmd, params) {
            Ok(o) => o.result,
            Err(e) => panic!("{cmd}: {e}"),
        }
    }

    fn err(&mut self, cmd: &str, params: Value) -> CmdError {
        match self.try_run(cmd, params) {
            Ok(o) => panic!("{cmd} should fail, got {:?}", o.result),
            Err(e) => e,
        }
    }

    fn take(&self, track: Option<u32>, channel: Option<ChannelId>, n: u32) -> TakeInfo {
        TakeInfo {
            input: "Scripted: In 1-2".into(),
            track,
            channel,
            reference: format!("samples/Take {n}.wav"),
            start: 3840,
            length: 3840,
            frames: 96_000,
            sample_rate: 48_000,
            channels: 2,
            latency_frames: 480,
            overflow: 0,
            error: None,
        }
    }
}

#[test]
fn list_inputs_reports_the_inputs_and_the_monitoring_latency() {
    let mut r = Rig::new();
    let v = r.run("record.list_inputs", json!({}));
    assert_eq!(v["inputs"][0]["name"], "Scripted: In 1-2");
    assert_eq!(v["inputs"][0]["armed"], false);
    assert_eq!(v["latency"]["monitor_ms"], 5.0);
    assert_eq!(v["recording"], false);
}

#[test]
fn arm_checks_where_the_take_goes() {
    let mut r = Rig::new();
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0 })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0, "track": 1, "channel": 0 })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0, "track": 9999 })),
        CmdError::Model(ModelError::NoTrack(_))
    ));
    // a synth cannot take a recording
    let synth = r.synth.0;
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0, "channel": synth })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0, "channel": 77 })),
        CmdError::Model(ModelError::NoChannel(_))
    ));
    // the host's own refusal comes through
    assert!(matches!(
        r.err("record.arm", json!({ "input": 7, "track": 0 })),
        CmdError::Host(_)
    ));
    assert!(matches!(
        r.err("record.arm", json!({ "input": 0, "trak": 2 })),
        CmdError::BadParams { .. }
    ));
    assert!(r.h.armed.is_empty());
    let out = r.run("record.arm", json!({ "input": 0, "track": 2 }));
    assert_eq!(out["monitor"], true, "monitoring is on by default");
    assert_eq!(r.h.armed[0].track, Some(2));
    let sampler = r.sampler.0;
    r.run(
        "record.arm",
        json!({ "input": 0, "channel": sampler, "monitor": false }),
    );
    assert_eq!(r.h.armed[1].channel, Some(r.sampler));
    assert!(!r.h.armed[1].monitor);
}

#[test]
fn disarm_and_monitor_reach_the_host() {
    let mut r = Rig::new();
    r.run("record.set_monitor", json!({ "input": 0, "on": false }));
    r.run("record.disarm", json!({ "input": 0 }));
    assert_eq!(r.h.monitor, [(0, false)]);
    assert_eq!(r.h.disarmed, [0]);
}

#[test]
fn start_validates_and_collects_midi_notes_only_in_pattern_mode() {
    let mut r = Rig::new();
    assert!(matches!(
        r.err("record.start", json!({ "punch": [960, 960] })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("record.start", json!({ "latency_ms": 5000.0 })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("record.start", json!({ "channel": 42 })),
        CmdError::Model(ModelError::NoChannel(_))
    ));
    // song mode: audio only
    r.h.midi_target = Some(r.synth);
    r.run("transport.set_mode", json!({ "mode": "song" }));
    let out = r.run(
        "record.start",
        json!({ "count_in_bars": 1, "punch": [0, 3840], "latency_ms": 12.5 }),
    );
    assert_eq!(out["notes"], false);
    let s = r.h.started.last().unwrap();
    assert_eq!((s.count_in_bars, s.punch, s.latency_ms), (1, Some((0, 3840)), Some(12.5)));
    assert!(s.notes.is_none());
    // pattern mode: the pattern and the MIDI target channel
    r.run(
        "transport.set_mode",
        json!({ "mode": "pattern", "pattern": r.pattern }),
    );
    let out = r.run("record.start", json!({}));
    assert_eq!(out["notes"], true);
    assert_eq!(
        r.h.started.last().unwrap().notes,
        Some(NoteCapture {
            pattern: r.pattern,
            channel: r.synth
        })
    );
    // an explicit channel wins; midi_notes: false turns it off
    let sampler = r.sampler.0;
    r.run("record.start", json!({ "channel": sampler }));
    assert_eq!(r.h.started.last().unwrap().notes.unwrap().channel, r.sampler);
    r.run("record.start", json!({ "midi_notes": false }));
    assert!(r.h.started.last().unwrap().notes.is_none());
    // no channel selected at all: no notes either
    r.h.midi_target = None;
    r.run("record.start", json!({}));
    assert!(r.h.started.last().unwrap().notes.is_none());
}

#[test]
fn stop_adds_the_take_as_a_registered_sample_and_a_latency_compensated_clip_in_one_undo_step() {
    let mut r = Rig::new();
    r.h.recording = true;
    r.h.outcome = Some(RecordOutcome {
        takes: vec![r.take(Some(1), None, 1)],
        notes: vec![],
        note_target: None,
    });
    let before = r.p().clone();
    let out = r.run("record.stop", json!({}));
    let clip = &out["takes"][0];
    assert_eq!(clip["track"], 1);
    assert_eq!(clip["start"], 3840);
    // 96000 frames minus the 480 of latency, at 120 BPM and 48 kHz (25 frames a tick)
    assert_eq!(clip["length"], 3821, "3820.8 ticks, rounded");
    let id = ClipId(clip["clip"].as_u64().unwrap() as u32);
    let c = r.p().clip(id).unwrap();
    let a = c.audio().unwrap();
    assert_eq!(a.source_offset, 480, "the clip plays from the compensation");
    assert_eq!((c.track, c.start), (1, 3840));
    let e = r.p().sample(a.sample).unwrap();
    assert_eq!(
        (e.path.as_str(), e.name.as_str(), e.frames, e.channels),
        ("samples/Take 1.wav", "Take 1.wav", 96_000, 2)
    );
    assert!(matches!(
        &e.provenance.origin,
        Origin::Recorded(Take { input, start: 3840, length: 3840 }) if input == "Scripted: In 1-2"
    ));
    // one step undoes both the clip and the registry entry
    r.run("history.undo", json!({}));
    assert_eq!(r.p().clips.len(), before.clips.len());
    assert_eq!(r.p().samples.len(), before.samples.len());
    r.run("history.redo", json!({}));
    assert_eq!(r.p().clips.len(), before.clips.len() + 1);
}

#[test]
fn stop_into_a_sampler_channel_replaces_its_sample() {
    let mut r = Rig::new();
    r.h.recording = true;
    r.h.outcome = Some(RecordOutcome {
        takes: vec![r.take(None, Some(r.sampler), 2)],
        notes: vec![],
        note_target: None,
    });
    let out = r.run("record.stop", json!({}));
    assert_eq!(out["takes"][0]["channel"], r.sampler.0);
    let InstrumentSpec::Sampler { sample, root_key, .. } =
        &r.p().channel(r.sampler).unwrap().instrument
    else {
        panic!()
    };
    assert_eq!(sample.as_deref(), Some("samples/Take 2.wav"));
    assert_eq!(*root_key, 60);
    assert!(r.p().sample_by_path("samples/Take 2.wav").is_some());
}

#[test]
fn stop_writes_played_notes_into_the_pattern_quantised_and_clamped() {
    let mut r = Rig::new();
    let pat = r.pattern;
    let len = r.p().pattern_length(pat).unwrap();
    r.h.recording = true;
    r.h.outcome = Some(RecordOutcome {
        takes: vec![r.take(Some(0), None, 3)],
        notes: vec![
            CapturedNote { key: 60, velocity: 0.5, start: 250, length: 100 },
            CapturedNote { key: 64, velocity: 1.0, start: 700, length: 5000 },
            CapturedNote { key: 67, velocity: 0.0, start: len + 500, length: 10 },
        ],
        note_target: Some(NoteCapture { pattern: pat, channel: r.synth }),
    });
    let before = r.p().clone();
    let out = r.run("record.stop", json!({ "quantize": 240 }));
    assert_eq!(out["notes"].as_array().unwrap().len(), 3);
    let notes = notes_of_channel(r.p(), pat, r.synth);
    assert_eq!(notes.len(), 3);
    assert_eq!((notes[0].key, notes[0].start), (60, 240), "250 snaps to 240");
    assert_eq!((notes[1].key, notes[1].start), (64, 720), "700 snaps to 720");
    assert_eq!(notes[1].start + notes[1].length, len, "a long note ends at the pattern end");
    assert!((notes[0].velocity - 0.5).abs() < 1e-6);
    assert!(notes[2].start < len, "a note past the end is pulled back inside");
    assert!(notes[2].velocity > 0.0);
    // the take and the notes are one undo step
    r.run("history.undo", json!({}));
    assert_eq!(r.p().clips.len(), before.clips.len());
    assert!(notes_of_channel(r.p(), pat, r.synth).is_empty());
    assert!(matches!(
        r.err("record.stop", json!({ "quantize": 0 })),
        CmdError::Model(ModelError::Invalid(_))
    ));
}

fn notes_of_channel(p: &Project, pat: PatternId, ch: ChannelId) -> Vec<Note> {
    let mut v: Vec<Note> = p
        .pattern(pat)
        .unwrap()
        .notes
        .get(&ch)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    v.sort_by_key(|n| (n.start, n.key));
    v
}

#[test]
fn stop_keeps_a_take_even_when_its_track_is_gone_and_reports_trouble() {
    let mut r = Rig::new();
    let mut t = r.take(Some(500), None, 4);
    t.overflow = 12;
    t.error = Some("disk full".into());
    r.h.recording = true;
    r.h.outcome = Some(RecordOutcome {
        takes: vec![t],
        notes: vec![],
        note_target: None,
    });
    let out = r.run("record.stop", json!({}));
    // placed on the last track instead of failing: the file must never be orphaned
    let track = out["takes"][0]["track"].as_u64().unwrap() as usize;
    assert_eq!(track, r.p().tracks.len() - 1);
    let w = out["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].as_str().unwrap().contains("disk full"));
    assert!(w[1].as_str().unwrap().contains("12 frames"));
}

#[test]
fn stop_with_nothing_recorded_changes_nothing_and_stop_when_idle_fails() {
    let mut r = Rig::new();
    assert!(matches!(r.err("record.stop", json!({})), CmdError::Host(_)));
    r.h.recording = true;
    let before = r.p().clone();
    let out = r.s.execute(
        &r.who,
        Request::new("record.stop", json!({})),
        &mut r.h,
    );
    let out = out.unwrap();
    assert_eq!(
        out.summary.as_deref(),
        Some("recording stopped, nothing was recorded")
    );
    assert_eq!(r.p().samples.len(), before.samples.len());
    assert_eq!(r.p().clips.len(), before.clips.len());
}

#[test]
fn midi_inputs_learn_map_unmap_and_list() {
    let mut r = Rig::new();
    let v = r.run("midi.list_inputs", json!({}));
    assert_eq!(v["inputs"][0]["name"], "Scripted Keys");
    assert_eq!(v["inputs"][0]["connected"], true);
    // learn: arms the host, again with nothing cancels
    let vol = format!("channel/{}/volume", r.synth.0);
    let out = r.run("midi.learn", json!({ "target": vol }));
    assert_eq!(out["learning"], vol);
    assert_eq!(r.h.learning, Some(ParamPath::ChannelVolume(r.synth)));
    r.run("midi.learn", json!({}));
    assert_eq!(r.h.learning, None);
    // targets must exist
    assert!(matches!(
        r.err("midi.learn", json!({ "target": "channel/99/volume" })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("midi.map", json!({ "controller": 74, "target": "insert/99/volume" })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(matches!(
        r.err("midi.map", json!({ "controller": 200, "target": "tempo" })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    // map, replace, list
    r.run("midi.map", json!({ "controller": 74, "target": "insert/1/volume" }));
    let out = r.run(
        "midi.map",
        json!({ "controller": 74, "target": "insert/1/pan", "device": null }),
    );
    assert_eq!(out["replaced"]["target"], "insert/1/volume");
    r.run(
        "midi.map",
        json!({ "controller": 74, "target": "tempo", "device": "Scripted Keys" }),
    );
    let l = r.run("midi.list_mappings", json!({}));
    assert_eq!(l["mappings"].as_array().unwrap().len(), 2);
    assert_eq!(l["mappings"][0]["target"], "insert/1/pan");
    // unmap: the device-specific one, then the general one, then nothing is left
    let out = r.run("midi.unmap", json!({ "controller": 74, "device": "Scripted Keys" }));
    assert_eq!(out["removed"]["target"], "tempo");
    r.run("midi.unmap", json!({ "controller": 74 }));
    assert!(matches!(
        r.err("midi.unmap", json!({ "controller": 74 })),
        CmdError::Model(ModelError::Invalid(_))
    ));
    assert!(r.h.maps.is_empty());
}

#[test]
fn midi_commands_need_the_full_scope_and_recording_the_compose_scope() {
    let reg = Registry::with_builtins();
    let scope = |id: &str| reg.get(id).unwrap().spec().scope;
    for id in ["midi.learn", "midi.map", "midi.unmap"] {
        assert_eq!(scope(id), Scope::Full, "{id} writes per-user config");
    }
    for id in ["midi.list_inputs", "midi.list_mappings", "record.list_inputs"] {
        assert_eq!(scope(id), Scope::Read, "{id}");
    }
    for id in ["record.arm", "record.disarm", "record.set_monitor", "record.start", "record.stop"] {
        assert_eq!(scope(id), Scope::Compose, "{id}");
    }
    // a hostless session says so plainly
    let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("x"));
    let e = s
        .execute(
            &Caller::human(),
            Request::new("record.list_inputs", json!({})),
            &mut NullHost::default(),
        )
        .unwrap_err();
    assert!(matches!(e, CmdError::Host(m) if m.contains("no audio input")));
}
