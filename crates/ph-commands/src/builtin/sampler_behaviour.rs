//! Behaviour tests for the M3 `sampler.*` commands (model-only ones implemented in M3/T0,
//! and the stubs that name their track until it lands).

use crate::*;
use ph_core::*;
use serde_json::{Value, json};
use std::sync::Arc;

struct Rig {
    s: Session,
    h: NullHost,
    who: Caller,
}

impl Rig {
    fn new() -> Self {
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("M3")),
            h: NullHost::default(),
            who: Caller::human(),
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

    fn inst(&self) -> &HausInstrument {
        self.p().instrument(InstrumentId(0)).unwrap()
    }

    /// An instrument (0) on a new channel (0), group 0, and two zones (0: C3, 1: C4).
    fn piano() -> Self {
        let mut r = Rig::new();
        let out = r.run("sampler.new", json!({ "name": "Piano", "group": "Main" }));
        assert_eq!(out, json!({ "instrument": 0, "group": 0, "channel": 0 }));
        let out = r.run(
            "sampler.zone_add",
            json!({ "instrument": 0, "zones": [
                { "group": 0, "sample": "samples/C3.wav", "root_key": 48, "keys": [42, 53] },
                { "group": 0, "sample": "samples/C4.wav", "root_key": 60, "keys": [54, 66],
                  "velocity": [1, 127], "loop": { "start": 100, "end": 5000 } },
            ] }),
        );
        assert_eq!(out["zones"], json!([0, 1]));
        r
    }
}

#[test]
fn sampler_new_makes_an_instrument_on_a_channel() {
    let r = Rig::piano();
    let ch = r.p().channel(ChannelId(0)).unwrap();
    assert_eq!(ch.name, "Piano");
    assert!(matches!(
        ch.instrument,
        InstrumentSpec::MultiSampler {
            instrument: Some(InstrumentId(0)),
            ..
        }
    ));
    let inst = r.inst();
    assert_eq!(inst.group(GroupId(0)).unwrap().name, "Main");
    let z = inst.zone(ZoneId(1)).unwrap();
    assert_eq!((z.keys.lo, z.keys.hi, z.root_key), (54, 66, 60));
    assert_eq!(z.looping.as_ref().unwrap().end, 5000);
    // on an existing channel: replaces what it plays, adds no channel
    let mut r = Rig::new();
    r.run(
        "channel.add",
        json!({ "name": "Lead", "instrument": { "kind": "synth", "preset": null } }),
    );
    let out = r.run("sampler.new", json!({ "name": "Kit", "channel": 0 }));
    assert_eq!(out["channel"], 0);
    assert_eq!(r.p().channels.len(), 1);
    assert_eq!(r.inst().groups[&GroupId(0)].name, "Group 1");
    // one undo removes it all
    r.s.undo(&Author::Human).unwrap();
    assert!(r.p().instruments.is_empty());
    assert!(matches!(
        r.p().channel(ChannelId(0)).unwrap().instrument,
        InstrumentSpec::Synth { .. }
    ));
}

#[test]
fn sampler_zone_add_refuses_bad_zones() {
    let mut r = Rig::piano();
    let e = r.err(
        "sampler.zone_add",
        json!({ "instrument": 0, "zones": [{ "group": 0, "sample": "x.wav" }] }),
    );
    assert!(e.to_string().contains("root_key"), "{e}");
    let e = r.err(
        "sampler.zone_add",
        json!({ "instrument": 0, "zones": [{ "group": 0, "sample": "x.wav", "root_key": 60, "tuning": 3 }] }),
    );
    assert!(e.to_string().contains("tuning"), "{e}");
    let e = r.err(
        "sampler.zone_add",
        json!({ "instrument": 0, "zones": [{ "group": 9, "sample": "x.wav", "root_key": 60 }] }),
    );
    assert!(e.to_string().contains("group"), "{e}");
    let e = r.err(
        "sampler.zone_add",
        json!({ "instrument": 0, "zones": [{ "group": 0, "sample": 4, "root_key": 60 }] }),
    );
    assert!(e.to_string().contains("sample"), "{e}");
    assert_eq!(r.inst().zones.len(), 2, "nothing was added");
}

#[test]
fn sampler_zone_move_shifts_splits_and_regroups() {
    let mut r = Rig::piano();
    // shift both zones up a tone: ranges and roots follow
    r.run(
        "sampler.zone_move",
        json!({ "instrument": 0, "zones": [0, 1], "shift": 2 }),
    );
    let z = r.inst().zone(ZoneId(0)).unwrap();
    assert_eq!((z.keys.lo, z.keys.hi, z.root_key), (44, 55, 50));
    // a velocity split on zone 1, keys kept, root kept
    r.run(
        "sampler.zone_move",
        json!({ "instrument": 0, "zones": [1], "velocity": [0, 63], "keys": [60, 72], "root_follows": false }),
    );
    let z = r.inst().zone(ZoneId(1)).unwrap();
    assert_eq!((z.keys.lo, z.keys.hi, z.root_key), (60, 72, 62));
    assert_eq!(z.velocity, VelRange { lo: 0, hi: 63 });
    // into another group
    let g = r.run(
        "sampler.group_add",
        json!({ "instrument": 0, "name": "Soft" }),
    );
    assert_eq!(g["group"], 1);
    r.run(
        "sampler.zone_move",
        json!({ "instrument": 0, "zones": [1], "group": 1 }),
    );
    assert_eq!(r.inst().zone(ZoneId(1)).unwrap().group, GroupId(1));
    // off the keyboard, or both shift and keys: refused
    r.err(
        "sampler.zone_move",
        json!({ "instrument": 0, "zones": [0], "shift": 100 }),
    );
    r.err(
        "sampler.zone_move",
        json!({ "instrument": 0, "zones": [0], "shift": 1, "keys": [0, 1] }),
    );
}

#[test]
fn sampler_zone_set_changes_and_clears_fields() {
    let mut r = Rig::piano();
    r.run(
        "sampler.zone_set",
        json!({ "instrument": 0, "zones": [0, 1], "set": {
            "tune": -12, "fine": 5.5, "volume_db": -6, "round_robin": 1,
            "trigger": "release", "release_decay_db": 3, "loop": null } }),
    );
    for z in r.inst().zones.values() {
        assert_eq!((z.tune, z.fine, z.volume_db), (-12, 5.5, -6.0));
        assert_eq!(z.trigger, ZoneTrigger::Release);
        assert_eq!(z.round_robin, Some(1));
        assert!(z.looping.is_none());
    }
    let e = r.err(
        "sampler.zone_set",
        json!({ "instrument": 0, "zones": [0], "set": { "id": 7 } }),
    );
    assert!(e.to_string().contains("no field"), "{e}");
    r.err(
        "sampler.zone_set",
        json!({ "instrument": 0, "zones": [0], "set": { "fine": 300 } }),
    );
    r.run(
        "sampler.zone_remove",
        json!({ "instrument": 0, "zones": [0] }),
    );
    assert_eq!(
        r.inst().zones.keys().copied().collect::<Vec<_>>(),
        [ZoneId(1)]
    );
}

#[test]
fn sampler_groups_set_and_remove() {
    let mut r = Rig::piano();
    r.run(
        "sampler.group_set",
        json!({ "instrument": 0, "group": 0, "set": {
            "amp_env": { "attack": 0.01, "release": 0.4 },
            "filter": { "kind": "low_pass24", "cutoff": 5000, "resonance": 2 },
            "polyphony": "legato", "portamento": 0.05, "round_robin": "random",
            "voice_limit": 8, "stealing": "quietest", "output": 1,
            "effects": [{ "name": "eq", "params": { "low_gain": -3 } }] } }),
    );
    let g = r.inst().group(GroupId(0)).unwrap().clone();
    assert_eq!(g.amp_env.attack, 0.01);
    assert_eq!(
        g.amp_env.sustain, 1.0,
        "unset envelope fields keep their defaults"
    );
    assert_eq!(g.filter.unwrap().cutoff, 5000.0);
    assert_eq!(
        (g.polyphony, g.round_robin, g.stealing),
        (
            Polyphony::Legato,
            RoundRobinMode::Random,
            StealMode::Quietest
        )
    );
    assert_eq!((g.voice_limit, g.output), (Some(8), 1));
    assert_eq!(g.effects[0].mix, 1.0);
    r.err(
        "sampler.group_set",
        json!({ "instrument": 0, "group": 0, "set": { "output": 99 } }),
    );
    let out = r.run(
        "sampler.group_remove",
        json!({ "instrument": 0, "group": 0 }),
    );
    assert_eq!(out["zones_removed"], 2);
    assert!(r.inst().groups.is_empty() && r.inst().zones.is_empty());
}

#[test]
fn sampler_set_changes_name_and_settings() {
    let mut r = Rig::piano();
    r.run(
        "sampler.set",
        json!({ "instrument": 0, "name": "Grand", "set": { "transpose": -12, "bend_up": 12, "preload_frames": 65536 } }),
    );
    let i = r.inst();
    assert_eq!(i.name, "Grand");
    assert_eq!(
        (
            i.settings.transpose,
            i.settings.bend_up,
            i.settings.preload_frames
        ),
        (-12, 12.0, Some(65536))
    );
    r.err(
        "sampler.set",
        json!({ "instrument": 0, "set": { "voice_limit": 0 } }),
    );
    r.err("sampler.set", json!({ "instrument": 3, "name": "x" }));
}

#[test]
fn sampler_keyswitches_and_membership() {
    let mut r = Rig::piano();
    r.run(
        "sampler.group_add",
        json!({ "instrument": 0, "name": "Staccato" }),
    );
    let a = r.run(
        "sampler.keyswitch_set",
        json!({ "instrument": 0, "name": "sustain", "trigger": { "kind": "key", "key": 24 },
                "groups": [0], "default": true }),
    );
    assert_eq!(a["articulation"], 0);
    let b = r.run(
        "sampler.keyswitch_set",
        json!({ "instrument": 0, "name": "staccato", "trigger": { "kind": "key", "key": 25 }, "groups": [1] }),
    );
    assert_eq!(b["articulation"], 1);
    let i = r.inst();
    assert_eq!(i.settings.default_articulation, Some(ArticulationId(0)));
    assert_eq!(
        i.group(GroupId(0)).unwrap().articulations,
        [ArticulationId(0)]
    );
    assert_eq!(
        i.group(GroupId(1)).unwrap().articulations,
        [ArticulationId(1)]
    );
    // change: a CC trigger, both groups play it
    r.run(
        "sampler.keyswitch_set",
        json!({ "instrument": 0, "articulation": 1,
                "trigger": { "kind": "cc", "controller": 32, "lo": 0.5, "hi": 1.0 }, "groups": [0, 1] }),
    );
    assert_eq!(
        r.inst().group(GroupId(0)).unwrap().articulations,
        [ArticulationId(0), ArticulationId(1)]
    );
    // a new one needs a name and a trigger
    r.err(
        "sampler.keyswitch_set",
        json!({ "instrument": 0, "name": "x" }),
    );
    // remove: groups leave it, the default is cleared
    r.run(
        "sampler.keyswitch_set",
        json!({ "instrument": 0, "articulation": 0, "remove": true }),
    );
    let i = r.inst();
    assert_eq!(i.articulations.len(), 1);
    assert_eq!(i.settings.default_articulation, None);
    assert_eq!(
        i.group(GroupId(0)).unwrap().articulations,
        [ArticulationId(1)]
    );
    // the whole thing undoes step by step
    r.s.undo(&Author::Human).unwrap();
    assert_eq!(r.inst().articulations.len(), 2);
}

#[test]
fn sampler_modulation_set_replaces_the_matrix() {
    let mut r = Rig::piano();
    let out = r.run(
        "sampler.modulation_set",
        json!({ "instrument": 0, "routes": [
            { "source": { "kind": "velocity" }, "target": "volume", "depth": -18, "curve": "concave" },
            { "source": { "kind": "cc", "controller": 1 }, "target": "cutoff", "depth": 2400, "group": 0 },
        ] }),
    );
    assert_eq!(out["routes"], 2);
    let m = &r.inst().modulation;
    assert_eq!(m[1].source, ModSource::Cc { controller: 1 });
    assert_eq!(m[1].group, Some(GroupId(0)));
    // a route scoped to a missing group is refused
    r.err(
        "sampler.modulation_set",
        json!({ "instrument": 0, "routes": [
            { "source": { "kind": "random" }, "target": "pan", "depth": 0.5, "group": 7 }] }),
    );
    // removing the group drops its scoped route with it
    r.run(
        "sampler.group_remove",
        json!({ "instrument": 0, "group": 0 }),
    );
    assert_eq!(r.inst().modulation.len(), 1);
}

#[test]
fn sampler_report_describes_the_instrument() {
    let mut r = Rig::piano();
    let out = r.run("sampler.report", json!({ "instrument": 0 }));
    assert_eq!(out["name"], "Piano");
    assert_eq!(out["zones"], 2);
    assert_eq!(out["groups"][0]["zones"], 2);
    assert_eq!(out["channels"], json!([0]));
    assert_eq!(out["import_notes"], json!([]));
    r.err("sampler.report", json!({ "instrument": 5 }));
}

#[test]
fn sampler_import_of_later_formats_names_their_track() {
    let mut r = Rig::piano();
    for (cmd, params, track) in [("sampler.import", json!({ "path": "C:/Lib/x.nki" }), "M3/T6")] {
        match r.err(cmd, params) {
            CmdError::Unavailable { track: t, .. } => assert_eq!(t, track, "{cmd}"),
            e => panic!("{cmd}: {e}"),
        }
    }
    let e = r.err("sampler.import", json!({ "path": "C:/x.txt" }));
    assert!(e.to_string().contains("not an instrument format"), "{e}");
    // schemas refuse unknown parameters
    assert!(matches!(
        r.err("sampler.load", json!({ "path": "x", "linked": true })),
        CmdError::BadParams { .. }
    ));
}

// ------------------------------------------------------------------------- files (M3/T3)

/// A host over a temp folder: the project is `<root>/project`, a library `<root>/lib`.
/// Agents may read and write inside the project and read the library; the human may read
/// any absolute path.
struct FsHost {
    inner: NullHost,
    project: std::path::PathBuf,
    lib: std::path::PathBuf,
}

impl Host for FsHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        self.inner.transport(cmd)
    }
    fn transport_state(&self) -> TransportState {
        self.inner.transport_state()
    }
    fn render(&mut self, p: &Project, r: &RenderRequest) -> CmdResult<RenderReport> {
        self.inner.render(p, r)
    }
    fn resolve_path(&self, path: &str, access: FileAccess) -> CmdResult<std::path::PathBuf> {
        let p = std::path::Path::new(path);
        if p.is_absolute() {
            if p.starts_with(&self.project)
                || (access == FileAccess::Read && p.starts_with(&self.lib))
            {
                return Ok(p.to_path_buf());
            }
            return Err(CmdError::Host(format!(
                "{path} is outside the allowed folders"
            )));
        }
        if p.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(CmdError::Host(format!("{path} leaves the project")));
        }
        Ok(self.project.join(p))
    }
    fn ui(&mut self, _: UiRequest) -> CmdResult<()> {
        Ok(())
    }
}

struct FsRig {
    s: Session,
    h: FsHost,
    who: Caller,
    dir: tempfile::TempDir,
}

fn tone(path: &std::path::Path, frames: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ch: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    ph_formats::encode::write_wav(path, &[ch], 48_000, ph_formats::encode::WavFormat::Pcm16)
        .unwrap();
}

impl FsRig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (project, lib) = (dir.path().join("project"), dir.path().join("lib"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("M3")),
            h: FsHost {
                inner: NullHost::default(),
                project,
                lib,
            },
            who: Caller::human(),
            dir,
        }
    }

    /// `lib/Piano/piano.sfz` with two velocity layers, a release sample and an unknown opcode.
    fn sfz_library(&self) -> std::path::PathBuf {
        let d = self.h.lib.join("Piano");
        for f in ["Samples/c4_p.wav", "Samples/c4_f.wav", "Samples/rel.wav"] {
            tone(&d.join(f), 4800);
        }
        std::fs::write(
            d.join("piano.sfz"),
            "<control> default_path=Samples/\n<group> lokey=48 hikey=72 pitch_keycenter=60 frobnicate=1\n\
             <region> sample=c4_p.wav hivel=80\n<region> sample=c4_f.wav lovel=81\n\
             <group> lokey=48 hikey=72 pitch_keycenter=60 trigger=release rt_decay=6\n<region> sample=rel.wav\n",
        )
        .unwrap();
        d.join("piano.sfz")
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

    fn inst(&self, i: u32) -> &HausInstrument {
        self.s.project().instrument(InstrumentId(i)).unwrap()
    }
}

type Shape = (String, u8, u8, u8, u8, u8, ZoneTrigger);

/// Zones without ids or group ids, for comparing instruments across load and save.
fn zone_shapes(i: &HausInstrument) -> Vec<Shape> {
    i.zones
        .values()
        .map(|z| {
            let SampleRef::File(f) = &z.sample else {
                panic!("registry sample")
            };
            (
                f.rsplit('/').next().unwrap().to_string(),
                z.keys.lo,
                z.keys.hi,
                z.velocity.lo,
                z.velocity.hi,
                z.root_key,
                z.trigger,
            )
        })
        .collect()
}

#[test]
fn sampler_import_links_an_sfz_library_and_reports_what_it_skipped() {
    let mut r = FsRig::new();
    let sfz = r.sfz_library();
    let out = r.run("sampler.import", json!({ "path": sfz.to_string_lossy() }));
    assert_eq!(
        (out["instrument"].as_u64(), out["channel"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(out["zones"], 3);
    assert_eq!(out["collected"], false);
    let lib = r.h.lib.join("Piano").to_string_lossy().replace('\\', "/");
    assert_eq!(out["folder"].as_str().unwrap(), lib);
    assert_eq!(out["report"]["unknown"][0]["what"], "opcode:frobnicate");
    let inst = r.inst(0).clone();
    assert_eq!(inst.source.as_ref().unwrap().format, "sfz");
    assert_eq!(
        zone_shapes(&inst),
        vec![
            ("c4_p.wav".into(), 48, 72, 0, 80, 60, ZoneTrigger::Attack),
            ("c4_f.wav".into(), 48, 72, 81, 127, 60, ZoneTrigger::Attack),
            ("rel.wav".into(), 48, 72, 0, 127, 60, ZoneTrigger::Release),
        ]
    );
    assert!(
        inst.zones
            .values()
            .all(|z| matches!(&z.sample, SampleRef::File(f) if f.starts_with("Samples/")))
    );
    // nothing was written into the library
    assert_eq!(std::fs::read_dir(r.h.lib.join("Piano")).unwrap().count(), 2);
    // one undo removes the instrument and its channel
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert!(r.s.project().instruments.is_empty() && r.s.project().channels.is_empty());
}

#[test]
fn sampler_collect_copies_samples_into_the_project_as_one_undo() {
    let mut r = FsRig::new();
    let sfz = r.sfz_library();
    r.run("sampler.import", json!({ "path": sfz.to_string_lossy() }));
    let before = r.inst(0).clone();
    let out = r.run("sampler.collect", json!({ "instrument": 0 }));
    assert_eq!(out["copied"], 3);
    assert_eq!(out["folder"], "instruments/piano");
    let inst = r.inst(0).clone();
    assert_eq!(inst.folder.as_deref(), Some("instruments/piano"));
    for z in inst.zones.values() {
        let SampleRef::File(f) = &z.sample else {
            panic!()
        };
        assert!(f.starts_with("samples/"), "{f}");
        assert!(
            r.h.project.join("instruments/piano").join(f).is_file(),
            "{f}"
        );
    }
    assert_eq!(zone_shapes(&inst), zone_shapes(&before));
    // collecting again is a no-op
    assert_eq!(
        r.run("sampler.collect", json!({ "instrument": 0 }))["copied"],
        0
    );
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert_eq!(r.inst(0), &before);
}

#[test]
fn sampler_save_native_is_byte_stable_and_load_reopens_it() {
    let mut r = FsRig::new();
    let sfz = r.sfz_library();
    r.run("sampler.import", json!({ "path": sfz.to_string_lossy() }));
    let out = r.run(
        "sampler.save_native",
        json!({ "instrument": 0, "path": "native/Piano" }),
    );
    assert_eq!(out["samples"], 3);
    let file = r.h.project.join("native/Piano/instrument.toml");
    let first = std::fs::read(&file).unwrap();
    let again = r.run(
        "sampler.save_native",
        json!({ "instrument": 0, "path": "native/Piano" }),
    );
    assert_eq!(again["samples"], 0, "identical samples are reused");
    assert_eq!(std::fs::read(&file).unwrap(), first, "byte-stable");
    // the library folder an instrument links is never written
    let lib = r.h.lib.join("Piano").to_string_lossy().into_owned();
    assert!(
        r.try_run(
            "sampler.save_native",
            json!({ "instrument": 0, "path": lib })
        )
        .is_err()
    );
    // reopen it linked (project-relative folder) and collected
    let out = r.run("sampler.load", json!({ "path": "native/Piano" }));
    assert_eq!(out["folder"], "native/Piano");
    let linked = r.inst(1).clone();
    assert_eq!(zone_shapes(&linked), zone_shapes(r.inst(0)));
    assert_eq!(linked.import_notes, r.inst(0).import_notes);
    let out = r.run(
        "sampler.load",
        json!({ "path": "native/Piano/instrument.toml", "collect": true, "channel": 0 }),
    );
    assert_eq!(out["channel"], 0);
    assert_eq!(out["folder"], "instruments/piano");
    assert_eq!(out["copied"], 3);
    assert_eq!(zone_shapes(r.inst(2)), zone_shapes(&linked));
    assert!(matches!(
        r.s.project().channel(ChannelId(0)).unwrap().instrument,
        InstrumentSpec::MultiSampler {
            instrument: Some(InstrumentId(2)),
            ..
        }
    ));
    r.s.undo(&Author::Human).unwrap().unwrap();
    assert!(r.s.project().instrument(InstrumentId(2)).is_err());
}

/// `lib/kit.sf2`: a drum kit (bank 128) and a bass preset (bank 0, program 33).
fn sf2_library(r: &FsRig) -> std::path::PathBuf {
    use ph_sampler_formats::build::{Sf2Builder, Sf2Sample, Sf2Zone, tagged_tone};
    let mut b = Sf2Builder::new();
    let tone = |n: u32| {
        Sf2Sample::from_f32(
            &format!("t{n}"),
            &tagged_tone(n, 200.0 + 50.0 * n as f64, 3000, 44_100),
            44_100,
            60,
        )
    };
    let kick = b.add_sample(tone(1));
    let hat = b.add_sample(tone(2));
    let kit = b.add_instrument(
        "Kit",
        vec![
            Sf2Zone::new().keys(36, 36).sample(kick),
            Sf2Zone::new().keys(42, 42).set(57, 1).sample(hat),
        ],
    );
    b.add_preset("Kit", 128, 0, vec![Sf2Zone::new().instrument(kit)]);
    let bass = b.add_instrument("Bass", vec![Sf2Zone::new().sample(kick)]);
    b.add_preset("Bass", 0, 33, vec![Sf2Zone::new().instrument(bass)]);
    let path = r.h.lib.join("kit.sf2");
    std::fs::write(&path, b.build()).unwrap();
    path
}

#[test]
fn sampler_import_collects_an_sf2_preset_and_lists_the_others() {
    let mut r = FsRig::new();
    let sf2 = sf2_library(&r);
    let p = sf2.to_string_lossy().into_owned();
    // default: the first preset by bank, then program
    let out = r.run("sampler.import", json!({ "path": p }));
    assert_eq!(out["report"]["format"], "sf2");
    assert_eq!(
        out["collected"], true,
        "SoundFont samples are always written out"
    );
    assert_eq!(r.inst(0).name, "Bass");
    let presets = out["report"]["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 2);
    assert_eq!(
        presets[1],
        json!({ "bank": 0, "program": 33, "name": "Bass", "imported": true })
    );
    // the kit by bank:program; samples written as WAV beside the report
    let out = r.run("sampler.import", json!({ "path": p, "preset": "128:0" }));
    assert_eq!(
        (out["zones"].as_u64(), out["copied"].as_u64()),
        (Some(2), Some(2))
    );
    let inst = r.inst(1).clone();
    assert_eq!(inst.name, "Kit");
    let folder = r.h.project.join(inst.folder.as_deref().unwrap());
    assert!(folder.join("import-report.md").is_file());
    for z in inst.zones.values() {
        let SampleRef::File(f) = &z.sample else {
            panic!()
        };
        let wav = ph_formats::decode::decode_file(&folder.join(f)).unwrap();
        assert_eq!((wav.frames(), wav.sample_rate), (3000, 44_100.0), "{f}");
    }
    // by name; an unknown preset and a preset for another format are refused
    r.run("sampler.import", json!({ "path": p, "preset": "kit" }));
    assert_eq!(r.inst(2).name, "Kit");
    let e = r
        .try_run("sampler.import", json!({ "path": p, "preset": "9:9" }))
        .unwrap_err();
    assert!(e.to_string().contains("128:0 Kit"), "{e}");
    let sfz = r.sfz_library();
    let e = r
        .try_run(
            "sampler.import",
            json!({ "path": sfz.to_string_lossy(), "preset": "0:0" }),
        )
        .unwrap_err();
    assert!(e.to_string().contains("SF2/SF3 files only"), "{e}");
}

#[test]
fn sampler_import_links_exs_and_decentsampler_libraries() {
    use ph_sampler_formats::build::{ExsBuilder, ExsGroup, ExsSample, ExsZone};
    let mut r = FsRig::new();
    // EXS: the sample is found beside the file (its stored macOS path does not exist)
    let d = r.h.lib.join("Strings");
    tone(&d.join("vln_c4.wav"), 4800);
    let mut b = ExsBuilder::new("Strings");
    b.groups.push(ExsGroup::new("Sustain"));
    b.samples
        .push(ExsSample::new("vln_c4.wav", "/Users/someone/Samples"));
    b.zones.push(ExsZone::new(0, 0, (55, 72), 60));
    std::fs::write(d.join("strings.exs"), b.build()).unwrap();
    let out = r.run(
        "sampler.import",
        json!({ "path": d.join("strings.exs").to_string_lossy() }),
    );
    assert_eq!(
        (out["zones"].as_u64(), out["collected"].as_bool()),
        (Some(1), Some(false))
    );
    assert_eq!(out["report"]["format"], "exs");
    assert_eq!(
        out["folder"].as_str().unwrap(),
        d.to_string_lossy().replace('\\', "/")
    );
    assert_eq!(
        zone_shapes(r.inst(0)),
        vec![("vln_c4.wav".into(), 55, 72, 0, 127, 60, ZoneTrigger::Attack)]
    );
    // DecentSampler: a .dspreset, and a .dsbundle folder (not a folder of samples)
    let bundle = r.h.lib.join("Keys.dsbundle");
    tone(&bundle.join("Samples/k.wav"), 4800);
    let preset = r#"<DecentSampler><groups><group name="K"><sample path="Samples/k.wav" rootNote="60" loNote="48" hiNote="72" trigger="release"/></group></groups></DecentSampler>"#;
    std::fs::write(bundle.join("keys.dspreset"), preset).unwrap();
    let out = r.run(
        "sampler.import",
        json!({ "path": bundle.join("keys.dspreset").to_string_lossy() }),
    );
    assert_eq!(out["report"]["format"], "dspreset");
    assert_eq!(
        zone_shapes(r.inst(1)),
        vec![("k.wav".into(), 48, 72, 0, 127, 60, ZoneTrigger::Release)]
    );
    let out = r.run(
        "sampler.import",
        json!({ "path": bundle.to_string_lossy() }),
    );
    assert_eq!(out["report"]["presets"][0]["name"], "keys.dspreset");
    assert_eq!(r.inst(2).zones.len(), 1);
    // collected on request
    let out = r.run(
        "sampler.import",
        json!({ "path": d.join("strings.exs").to_string_lossy(), "collect": true }),
    );
    assert_eq!(out["copied"], 1);
}

#[test]
fn sampler_import_maps_a_folder_of_samples_by_note_names() {
    let mut r = FsRig::new();
    let d = r.h.lib.join("Rhodes");
    for f in [
        "rh_C3.wav",
        "rh_C4_soft.wav",
        "rh_C4_zloud.wav",
        "rh_G4.wav",
        "notes.wav",
    ] {
        tone(&d.join(f), 2400);
    }
    let out = r.run("sampler.import", json!({ "path": d.to_string_lossy() }));
    assert_eq!(out["zones"], 4);
    assert_eq!(out["report"]["notes"][0]["what"], "file:notes.wav");
    let shapes = zone_shapes(r.inst(0));
    assert_eq!(
        shapes
            .iter()
            .map(|z| (z.0.as_str(), z.1, z.2, z.3, z.4, z.5))
            .collect::<Vec<_>>(),
        vec![
            ("rh_C3.wav", 0, 54, 0, 127, 48),
            ("rh_C4_soft.wav", 55, 63, 0, 63, 60),
            ("rh_C4_zloud.wav", 55, 63, 64, 127, 60),
            ("rh_G4.wav", 64, 127, 0, 127, 67),
        ]
    );
    // collected on request, with the report beside it
    let out = r.run(
        "sampler.import",
        json!({ "path": d.to_string_lossy(), "collect": true }),
    );
    assert_eq!(out["copied"], 4);
    assert!(
        r.h.project
            .join("instruments/Rhodes/import-report.md")
            .is_file()
    );
}

#[test]
fn sampler_import_keeps_agents_inside_their_folders() {
    let mut r = FsRig::new();
    let sfz = r.sfz_library();
    // an instrument file outside the allowed folders is refused for an agent
    let outside = r.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("piano.sfz"), "<region> sample=x.wav").unwrap();
    r.who = Caller::agent("claude", Grant::full());
    let path = outside.join("piano.sfz").to_string_lossy().into_owned();
    assert!(
        r.try_run("sampler.import", json!({ "path": path }))
            .is_err()
    );
    // samples outside the allowed folders are refused too
    let secret = r.dir.path().join("secret.wav");
    tone(&secret, 100);
    std::fs::write(
        r.h.lib.join("sneaky.sfz"),
        format!("<region> sample={}", secret.display()),
    )
    .unwrap();
    let path = r.h.lib.join("sneaky.sfz").to_string_lossy().into_owned();
    assert!(
        r.try_run("sampler.import", json!({ "path": path }))
            .is_err()
    );
    // inside the library it works, and agents can collect into the project
    let out = r.run(
        "sampler.import",
        json!({ "path": sfz.to_string_lossy(), "collect": true }),
    );
    assert_eq!(out["copied"], 3);
}

#[test]
fn sampler_edits_survive_project_new_and_undo() {
    let mut r = Rig::piano();
    r.run(
        "sampler.modulation_set",
        json!({ "instrument": 0, "routes": [
            { "source": { "kind": "velocity" }, "target": "volume", "depth": -6, "group": 0 }] }),
    );
    let before = r.p().clone();
    let mut req = Request::new("project.new", json!({}));
    req.confirm = true;
    r.s.execute(&r.who, req, &mut r.h).unwrap();
    assert!(r.p().instruments.is_empty() && r.p().channels.is_empty());
    r.s.undo(&Author::Human).unwrap();
    let mut expected = before.clone();
    expected.next_ids = r.p().next_ids.clone(); // ids are never freed
    assert_eq!(r.p(), &expected);
}
