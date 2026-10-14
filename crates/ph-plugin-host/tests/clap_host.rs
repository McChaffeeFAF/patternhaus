//! The CLAP host against the repo's test plugins (`test-plugins/ph-test-plugins`): scanning in
//! and out of process, instruments and effects, parameters, events, state, latency, the
//! real-time rules and the crash catch.

use assert_no_alloc::AllocDisabler;
use ph_node::*;
use ph_plugin_host::clap::ClapFormat;
use ph_plugin_host::scan::{ScanOutcome, scan_in_child};
use ph_plugin_host::testing::*;
use ph_plugin_host::*;
use std::sync::Arc;
use std::time::Duration;

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

struct NoAssets;
impl AssetResolver for NoAssets {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        Err(format!("no {r}"))
    }
}

const SR: f64 = 48_000.0;

fn info(id: &str) -> PluginInfo {
    let p = test_plugins();
    let found = ClapFormat::new().scan(&p.clap).unwrap();
    found.into_iter().find(|i| i.id == id).unwrap()
}

fn activated(id: &str, max_frames: usize) -> Box<dyn PluginInstance> {
    let mut i = ClapFormat::new().instantiate(&info(id)).unwrap();
    i.activate(&ActivateInfo {
        sample_rate: SR,
        max_frames,
        quality: Quality::Live,
        seed: 1,
    });
    i
}

fn ctx(frames: usize, pos: i64) -> ProcessContext {
    ProcessContext {
        sample_rate: SR,
        frames,
        song_pos_samples: pos,
        song_pos_ticks: 0.0,
        bpm: 120.0,
        playing: true,
        live: true,
    }
}

/// Renders an instrument for `frames` frames in blocks of `block`, with `events` (absolute
/// frames), allocation-free.
fn render_instrument(
    p: &mut dyn Processor,
    frames: usize,
    block: usize,
    events: &[(usize, EventKind)],
) -> [Vec<f32>; 2] {
    let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut evbuf: Vec<Event> = Vec::with_capacity(events.len());
    let mut start = 0;
    while start < frames {
        let n = block.min(frames - start);
        evbuf.clear();
        for (f, k) in events {
            if *f >= start && *f < start + n {
                evbuf.push(Event {
                    frame: (*f - start) as u32,
                    kind: *k,
                });
            }
        }
        let [l, r] = &mut out;
        let mut ch: [&mut [f32]; 2] = [&mut l[start..start + n], &mut r[start..start + n]];
        let mut ports: [&mut [&mut [f32]]; 1] = [&mut ch];
        let mut io = ProcessIo {
            inputs: &[],
            outputs: &mut ports,
            events: &evbuf,
        };
        let c = ctx(n, start as i64);
        assert_no_alloc::assert_no_alloc(|| p.process(&c, &mut io));
        start += n;
    }
    out
}

fn render_effect(p: &mut dyn Processor, input: &[f32], block: usize) -> (Vec<f32>, ProcessStatus) {
    let mut out = vec![0.0f32; input.len()];
    let mut r = vec![0.0f32; input.len()];
    let mut status = ProcessStatus::Continue;
    let mut start = 0;
    while start < input.len() {
        let n = block.min(input.len() - start);
        let ins: [&[f32]; 2] = [&input[start..start + n], &input[start..start + n]];
        let ports: [&[&[f32]]; 1] = [&ins];
        let mut ch: [&mut [f32]; 2] = [&mut out[start..start + n], &mut r[start..start + n]];
        let mut outs: [&mut [&mut [f32]]; 1] = [&mut ch];
        let mut io = ProcessIo {
            inputs: &ports,
            outputs: &mut outs,
            events: &[],
        };
        let c = ctx(n, start as i64);
        status = assert_no_alloc::assert_no_alloc(|| p.process(&c, &mut io));
        start += n;
    }
    (out, status)
}

fn note_on(key: u8) -> EventKind {
    EventKind::NoteOn {
        note_id: 7,
        key,
        velocity: 1.0,
        pan: 0.0,
        fine_pitch: 0.0,
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

#[test]
fn scan_finds_the_three_test_plugins_in_and_out_of_process() {
    let p = test_plugins();
    let found = ClapFormat::new().scan(&p.clap).unwrap();
    let ids: Vec<&str> = found.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, [SYNTH_ID, GAIN_ID, CRASH_ID]);
    assert_eq!(found[0].kind, PluginKind::Instrument);
    assert_eq!(found[1].kind, PluginKind::Effect);
    assert_eq!(found[0].vendor, "Patternhaus");
    assert_eq!(found[0].version, "1.0.0");
    assert!(
        found
            .iter()
            .all(|i| !i.has_editor && i.format == PluginFormat::Clap)
    );
    // the same through the helper process
    let (outcome, r) = scan_in_child(
        &p.scan_helper,
        Duration::from_secs(60),
        PluginFormat::Clap,
        &p.clap,
    );
    assert_eq!(outcome, ScanOutcome::Ok, "{:?}", r.error);
    assert_eq!(r.found, found);
    // a file that is not a plugin fails without taking the scan down
    let bogus = p.dir.join("..").join("not-a-plugin.clap");
    std::fs::write(&bogus, b"not a library").unwrap();
    let (outcome, r) = scan_in_child(
        &p.scan_helper,
        Duration::from_secs(60),
        PluginFormat::Clap,
        &bogus,
    );
    assert_eq!(outcome, ScanOutcome::Failed);
    assert!(r.error.is_some());
}

#[test]
fn the_synth_describes_itself_and_plays_notes_sample_accurately() {
    let mut s = activated(SYNTH_ID, 256);
    let d = s.descriptor().clone();
    assert_eq!(d.id, format!("clap:{SYNTH_ID}"));
    assert_eq!(d.kind, NodeKind::Instrument);
    assert!(d.note_input);
    assert!(d.audio_inputs.is_empty());
    let ids: Vec<&str> = d.params.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["0", "1", "2"]);
    assert_eq!(d.params[0].name, "Gain");
    assert_eq!(d.params[1].choices, ["Sine", "Saw"]);
    assert!(d.params[1].stepped);
    assert_eq!(s.param(ParamId(0)), 0.5);
    let out = render_instrument(s.as_mut(), 1024, 256, &[(100, note_on(69))]);
    assert!(
        out[0][..100].iter().all(|x| *x == 0.0),
        "silent before the note"
    );
    assert!(out[0][101] != 0.0, "sounds right after it");
    assert_eq!(out[0], out[1]);
    assert!(peak(&out[0]) > 0.3 && peak(&out[0]) <= 0.5 + 1e-6);
    // block-size independent: any block size, the same samples
    for block in [1usize, 37, 256] {
        let mut t = activated(SYNTH_ID, 256);
        let o = render_instrument(t.as_mut(), 1024, block, &[(100, note_on(69))]);
        assert_eq!(o, out, "block {block}");
    }
    // note off releases (50 ms default release), then the synth sleeps
    let mut t = activated(SYNTH_ID, 512);
    let off = EventKind::NoteOff {
        note_id: 7,
        key: 69,
        velocity: 0.0,
    };
    let o = render_instrument(t.as_mut(), 48_000, 512, &[(0, note_on(69)), (4800, off)]);
    assert!(peak(&o[0][4000..4800]) > 0.3);
    assert_eq!(peak(&o[0][4800 + 2500..]), 0.0, "released within 50 ms");
}

#[test]
fn params_cc_bend_and_expressions_reach_the_plugin() {
    // set_param before processing (gain 0 = silence)
    let mut s = activated(SYNTH_ID, 256);
    s.set_param(ParamId(0), 0.0);
    let o = render_instrument(s.as_mut(), 2048, 256, &[(0, note_on(60))]);
    assert_eq!(peak(&o[0]), 0.0);
    assert_eq!(s.param(ParamId(0)), 0.0);
    // a sample-accurate Param event (automation)
    let mut s = activated(SYNTH_ID, 256);
    let auto = EventKind::Param {
        id: ParamId(0),
        value: 0.0,
    };
    let o = render_instrument(s.as_mut(), 2048, 256, &[(0, note_on(60)), (1000, auto)]);
    assert!(peak(&o[0][900..1000]) > 0.1);
    assert_eq!(peak(&o[0][1000..]), 0.0);
    assert_eq!(s.param(ParamId(0)), 0.0);
    // mod wheel (CC 1) halves the level
    let base = render_instrument(
        activated(SYNTH_ID, 256).as_mut(),
        4096,
        256,
        &[(0, note_on(60))],
    );
    let cc = EventKind::Cc {
        controller: 1,
        value: 1.0,
    };
    let modded = render_instrument(
        activated(SYNTH_ID, 256).as_mut(),
        4096,
        256,
        &[(0, cc), (0, note_on(60))],
    );
    let ratio = peak(&modded[0][2048..]) / peak(&base[0][2048..]);
    assert!((ratio - 0.5).abs() < 1e-3, "{ratio}");
    // pitch bend and tuning change the waveform; volume expression scales it
    let bend = render_instrument(
        activated(SYNTH_ID, 256).as_mut(),
        4096,
        256,
        &[(0, EventKind::PitchBend { value: 1.0 }), (0, note_on(60))],
    );
    assert_ne!(bend[0], base[0]);
    let vol = render_instrument(
        activated(SYNTH_ID, 256).as_mut(),
        4096,
        256,
        &[
            (0, note_on(60)),
            (
                0,
                EventKind::NoteExpression {
                    note_id: 7,
                    kind: NoteExpressionKind::Volume,
                    value: 0.25,
                },
            ),
        ],
    );
    let ratio = peak(&vol[0][2048..]) / peak(&base[0][2048..]);
    assert!((ratio - 0.25).abs() < 1e-3, "{ratio}");
    // AllNotesOff chokes
    let o = render_instrument(
        activated(SYNTH_ID, 256).as_mut(),
        4096,
        256,
        &[(0, note_on(60)), (2000, EventKind::AllNotesOff)],
    );
    assert_eq!(peak(&o[0][2000..]), 0.0);
}

#[test]
fn control_reads_values_and_text_and_state_round_trips() {
    let mut a = activated(SYNTH_ID, 64);
    let c = a.control().unwrap();
    assert_eq!(c.info().id, SYNTH_ID);
    a.set_param(ParamId(1), 1.0);
    a.set_param(ParamId(2), 0.25);
    // queued changes reach the plugin with the next block
    render_instrument(a.as_mut(), 64, 64, &[]);
    assert_eq!(c.param_values().unwrap(), vec![0.5, 1.0, 0.25]);
    assert_eq!(c.param_text(1, 1.0).as_deref(), Some("Saw"));
    assert_eq!(c.param_text(2, 0.25).as_deref(), Some("250 ms"));
    let state = c.save_state().unwrap();
    assert!(state.starts_with(b"PHTP1\n"));
    assert_eq!(a.save_state(), state);
    let ps = PluginState::save(a.as_ref());
    assert_eq!(ps.plugin_id, SYNTH_ID);
    assert_eq!(ps.version, "1.0.0");
    let mut b = activated(SYNTH_ID, 64);
    ps.load_into(b.as_mut(), &NoAssets).unwrap();
    assert_eq!(b.param(ParamId(1)), 1.0);
    assert_eq!(b.param(ParamId(2)), 0.25);
    assert_eq!(
        b.control().unwrap().param_values().unwrap(),
        vec![0.5, 1.0, 0.25]
    );
    assert!(b.load_state(b"garbage", &NoAssets).is_err());
    // a state loads before activation too
    let mut fresh = ClapFormat::new().instantiate(&info(SYNTH_ID)).unwrap();
    fresh.load_state(&state, &NoAssets).unwrap();
    assert_eq!(fresh.param(ParamId(1)), 1.0);
}

#[test]
fn the_gain_reports_and_has_its_latency() {
    let mut g = activated(GAIN_ID, 128);
    assert_eq!(g.latency(), GAIN_LATENCY);
    assert_eq!(g.control().unwrap().latency(), GAIN_LATENCY);
    let d = g.descriptor().clone();
    assert_eq!(d.kind, NodeKind::Effect);
    assert_eq!(d.audio_inputs.len(), 1);
    assert!(!d.note_input);
    g.set_param(ParamId(0), 0.5);
    let input: Vec<f32> = (0..1000).map(|i| ((i % 50) as f32 / 50.0) - 0.5).collect();
    let (out, _) = render_effect(g.as_mut(), &input, 128);
    let l = GAIN_LATENCY as usize;
    assert!(out[..l].iter().all(|x| *x == 0.0));
    for i in l..1000 {
        assert_eq!(out[i], input[i - l] * 0.5, "frame {i}");
    }
}

#[test]
fn a_native_crash_while_processing_is_caught_and_the_node_goes_silent() {
    let inst = ClapFormat::new().instantiate(&info(CRASH_ID)).unwrap();
    let mut node = PluginNode::new(inst);
    node.activate(&ActivateInfo {
        sample_rate: SR,
        max_frames: 64,
        quality: Quality::Live,
        seed: 1,
    });
    let fault = node.fault_handle();
    let input = vec![0.5f32; 256];
    let (out, status) = render_effect(&mut node, &input, 64);
    assert_eq!(out, input, "a pass-through while healthy");
    assert_eq!(status, ProcessStatus::Continue);
    let crash = node.descriptor().param_id("0").unwrap();
    node.set_param(crash, 1.0);
    let (out, status) = render_effect(&mut node, &input, 64);
    assert!(out.iter().all(|x| *x == 0.0), "silence after the fault");
    assert_eq!(status, ProcessStatus::Sleep);
    assert!(fault.is_dead());
    let f = fault.fault().unwrap();
    assert_eq!(f.call, PluginCall::Process);
    assert_eq!(f.plugin_id, CRASH_ID);
    if ph_plugin_host::guard::supported() {
        assert!(
            f.message.contains("access violation")
                || f.message.contains("SIGSEGV")
                || f.message.contains("SIGBUS"),
            "{}",
            f.message
        );
    }
    // never called again; dropping it does not call the plugin either
    node.set_param(crash, 0.0);
    assert!(
        render_effect(&mut node, &input, 64)
            .0
            .iter()
            .all(|x| *x == 0.0)
    );
    assert!(node.control().is_none());
    drop(node);
    // the rest of the host keeps working
    let mut g = activated(GAIN_ID, 64);
    assert_eq!(g.latency(), GAIN_LATENCY);
    let _ = render_effect(g.as_mut(), &input, 64);
}

#[test]
fn a_native_crash_while_saving_state_is_caught() {
    let mut inst = ClapFormat::new().instantiate(&info(CRASH_ID)).unwrap();
    let c = inst.control().unwrap();
    inst.set_param(ParamId(0), 2.0);
    // not active: the change goes through params.flush on the main thread
    assert_eq!(c.param_values().unwrap(), vec![2.0]);
    let e = c.save_state().unwrap_err();
    assert!(e.to_string().contains("crashed"), "{e}");
    assert!(c.fault().is_dead());
    assert!(matches!(c.param_values(), Err(PluginError::Dead)));
    assert!(inst.save_state().is_empty());
    drop(inst);
}
