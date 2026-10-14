//! The VST3 host against the repo's VST3 test plugins (`test-plugins/ph-test-vst3`): scanning
//! in and out of process, an instrument with events, a list parameter and MIDI mapping, an
//! effect with latency, parameters and state. Skipped on macOS (bundle loading not done).

use assert_no_alloc::AllocDisabler;
use ph_node::*;
use ph_plugin_host::scan::{ScanOutcome, scan_in_child};
use ph_plugin_host::testing::*;
use ph_plugin_host::vst3::Vst3Format;
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

fn infos() -> Option<Vec<PluginInfo>> {
    let path = test_plugins().vst3.clone()?;
    Some(Vst3Format::new().scan(&path).unwrap())
}

fn activated(name: &str) -> Option<Box<dyn PluginInstance>> {
    let info = infos()?.into_iter().find(|i| i.name == name).unwrap();
    let mut i = Vst3Format::new().instantiate(&info).unwrap();
    i.activate(&ActivateInfo {
        sample_rate: SR,
        max_frames: 128,
        quality: Quality::Live,
        seed: 1,
    });
    Some(i)
}

fn run(
    p: &mut dyn Processor,
    input: Option<&[f32]>,
    frames: usize,
    block: usize,
    events: &[(usize, EventKind)],
) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    let mut r = vec![0.0f32; frames];
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
        let zeros = vec![0.0f32; n];
        let src = input.map_or(&zeros[..], |x| &x[start..start + n]);
        let ins: [&[f32]; 2] = [src, src];
        let ports: [&[&[f32]]; 1] = [&ins];
        let mut ch: [&mut [f32]; 2] = [&mut out[start..start + n], &mut r[start..start + n]];
        let mut outs: [&mut [&mut [f32]]; 1] = [&mut ch];
        let mut io = ProcessIo {
            inputs: if input.is_some() { &ports } else { &[] },
            outputs: &mut outs,
            events: &evbuf,
        };
        let c = ProcessContext {
            sample_rate: SR,
            frames: n,
            song_pos_samples: start as i64,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        assert_no_alloc::assert_no_alloc(|| p.process(&c, &mut io));
        start += n;
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn note_on(key: u8) -> EventKind {
    EventKind::NoteOn {
        note_id: 3,
        key,
        velocity: 1.0,
        pan: 0.0,
        fine_pitch: 0.0,
    }
}

#[test]
fn scan_lists_both_classes_in_and_out_of_process() {
    let Some(found) = infos() else { return };
    let names: Vec<&str> = found.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, [VST3_SYNTH_NAME, VST3_GAIN_NAME]);
    assert_eq!(found[0].kind, PluginKind::Instrument);
    assert_eq!(found[1].kind, PluginKind::Effect);
    assert_eq!(found[0].vendor, "Patternhaus");
    assert_eq!(found[0].version, "1.0.0");
    assert_eq!(found[0].id.len(), 32);
    assert!(
        found
            .iter()
            .all(|i| !i.has_editor && i.format == PluginFormat::Vst3)
    );
    let p = test_plugins();
    let (outcome, r) = scan_in_child(
        &p.scan_helper,
        Duration::from_secs(60),
        PluginFormat::Vst3,
        p.vst3.as_ref().unwrap(),
    );
    assert_eq!(outcome, ScanOutcome::Ok, "{:?}", r.error);
    assert_eq!(r.found, found);
}

#[test]
fn the_synth_plays_notes_and_maps_cc1_to_its_list_parameter() {
    let Some(mut s) = activated(VST3_SYNTH_NAME) else {
        return;
    };
    let d = s.descriptor().clone();
    assert!(d.note_input);
    assert_eq!(d.params[0].name, "Volume");
    assert_eq!((d.params[0].min, d.params[0].max), (0.0, 1.0));
    assert_eq!(d.params[1].choices, ["Low", "Mid", "High"]);
    assert_eq!(s.param(ParamId(1)), 1.0, "Mid, as a step index");
    let out = run(s.as_mut(), None, 2048, 128, &[(100, note_on(69))]);
    assert!(out[..100].iter().all(|x| *x == 0.0));
    assert!(peak(&out) > 0.3);
    // block-size independent
    let mut t = activated(VST3_SYNTH_NAME).unwrap();
    assert_eq!(run(t.as_mut(), None, 2048, 37, &[(100, note_on(69))]), out);
    // note off releases (480 samples)
    let off = EventKind::NoteOff {
        note_id: 3,
        key: 69,
        velocity: 0.0,
    };
    let mut t = activated(VST3_SYNTH_NAME).unwrap();
    let o = run(
        t.as_mut(),
        None,
        4096,
        128,
        &[(0, note_on(69)), (1000, off)],
    );
    assert_eq!(peak(&o[1600..]), 0.0);
    // all notes off releases everything
    let mut t = activated(VST3_SYNTH_NAME).unwrap();
    let o = run(
        t.as_mut(),
        None,
        4096,
        128,
        &[
            (0, note_on(60)),
            (0, note_on(64)),
            (1000, EventKind::AllNotesOff),
        ],
    );
    assert_eq!(peak(&o[1600..]), 0.0);
    // CC 1 goes through IMidiMapping to "Octave": the waveform changes
    let mut t = activated(VST3_SYNTH_NAME).unwrap();
    let cc = EventKind::Cc {
        controller: 1,
        value: 1.0,
    };
    let o = run(t.as_mut(), None, 2048, 128, &[(0, cc), (100, note_on(69))]);
    assert_ne!(o, out);
    assert_eq!(t.control().unwrap().param_values().unwrap()[1], 2.0, "High");
    // a sample-accurate Param event silences it
    let mut t = activated(VST3_SYNTH_NAME).unwrap();
    let mute = EventKind::Param {
        id: ParamId(0),
        value: 0.0,
    };
    let o = run(
        t.as_mut(),
        None,
        2048,
        128,
        &[(0, note_on(69)), (1000, mute)],
    );
    assert!(peak(&o[900..1000]) > 0.1);
    assert_eq!(peak(&o[1000..]), 0.0);
}

#[test]
fn the_gain_has_its_latency_params_text_and_state() {
    let Some(mut g) = activated(VST3_GAIN_NAME) else {
        return;
    };
    assert_eq!(g.latency(), VST3_GAIN_LATENCY);
    let c = g.control().unwrap();
    assert_eq!(c.latency(), VST3_GAIN_LATENCY);
    g.set_param(ParamId(0), 0.25); // plain gain 0.5
    let input: Vec<f32> = (0..600).map(|i| ((i % 40) as f32 / 40.0) - 0.5).collect();
    let out = run(g.as_mut(), Some(&input), 600, 128, &[]);
    let l = VST3_GAIN_LATENCY as usize;
    assert!(out[..l].iter().all(|x| *x == 0.0));
    for i in l..600 {
        assert_eq!(out[i], input[i - l] * 0.5, "frame {i}");
    }
    assert_eq!(c.param_values().unwrap(), vec![0.25]);
    assert_eq!(c.param_text(0, 0.25).as_deref(), Some("0.50x"));
    let state = c.save_state().unwrap();
    assert!(state.starts_with(b"PHVST3S1"));
    let mut h = activated(VST3_GAIN_NAME).unwrap();
    assert_eq!(h.param(ParamId(0)), 0.5);
    h.load_state(&state, &NoAssets).unwrap();
    assert_eq!(h.param(ParamId(0)), 0.25);
    assert!(h.load_state(b"junk", &NoAssets).is_err());
    let ps = PluginState::save(g.as_ref());
    assert_eq!(ps.format, PluginFormat::Vst3);
    // through the crash-catch node, like the engine
    let node = PluginNode::new(h);
    assert_eq!(node.save_state(), state);
    assert!(!node.fault_handle().is_dead());
}
