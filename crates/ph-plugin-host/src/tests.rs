use crate::fake::*;
use crate::*;
use std::sync::Arc;

struct NoAssets;
impl AssetResolver for NoAssets {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        Err(format!("no {r}"))
    }
}

fn node() -> PluginNode {
    let f = FakeFormat;
    let info = f.scan(Path::new(FAKE_FILE)).unwrap().remove(0);
    let mut n = PluginNode::new(f.instantiate(&info).unwrap());
    n.activate(&ActivateInfo {
        sample_rate: 48_000.0,
        max_frames: 64,
        quality: Quality::Live,
        seed: 1,
    });
    n
}

/// Runs one block of `[1, -0.5, ...]` through `n` and returns the left output.
fn run(n: &mut PluginNode) -> (ProcessStatus, Vec<f32>) {
    let input = [1.0f32, -0.5, 0.25, 0.0];
    let (mut l, mut r) = ([9.0f32; 4], [9.0f32; 4]);
    let ins: [&[f32]; 2] = [&input, &input];
    let in_ports: [&[&[f32]]; 1] = [&ins];
    let mut outs: [&mut [f32]; 2] = [&mut l, &mut r];
    let mut out_ports: [&mut [&mut [f32]]; 1] = [&mut outs];
    let ctx = ProcessContext {
        sample_rate: 48_000.0,
        frames: 4,
        song_pos_samples: 0,
        song_pos_ticks: 0.0,
        bpm: 120.0,
        playing: true,
        live: true,
    };
    let mut io = ProcessIo {
        inputs: &in_ports,
        outputs: &mut out_ports,
        events: &[],
    };
    let s = n.process(&ctx, &mut io);
    (s, l.to_vec())
}

#[test]
fn scan_reports_found_and_failed() {
    let r = FakeFormat.scan_all(&[PathBuf::from("a/ph-test-gain.clap"), PathBuf::from("b.dll")]);
    assert_eq!(r.found.len(), 1);
    assert_eq!(r.found[0].id, FAKE_ID);
    assert_eq!(r.found[0].kind, PluginKind::Effect);
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].path, PathBuf::from("b.dll"));
    let mut other = r.found[0].clone();
    other.id = "x".into();
    assert!(matches!(
        FakeFormat.instantiate(&other),
        Err(PluginError::NotFound(_))
    ));
}

#[test]
fn a_plugin_node_is_a_processor() {
    let mut n = node();
    let gain = n.descriptor().param_id("gain").unwrap();
    n.set_param(gain, 0.5);
    assert_eq!(n.param(gain), 0.5);
    assert_eq!(
        run(&mut n),
        (ProcessStatus::Continue, vec![0.5, -0.25, 0.125, 0.0])
    );
    let mut boxed: Box<dyn Processor> = Box::new(n);
    boxed.set_param(gain, 9.0);
    assert_eq!(boxed.param(gain), 2.0, "clamped");
}

#[test]
fn state_round_trips_and_refuses_another_plugin() {
    let f = FakeFormat;
    let info = FakeFormat::info(Path::new(FAKE_FILE));
    let mut a = f.instantiate(&info).unwrap();
    let gain = a.descriptor().param_id("gain").unwrap();
    a.set_param(gain, 1.5);
    let state = PluginState::save(a.as_ref());
    assert_eq!(state.plugin_id, FAKE_ID);
    let mut b = f.instantiate(&info).unwrap();
    state.load_into(b.as_mut(), &NoAssets).unwrap();
    assert_eq!(b.param(gain), 1.5);
    let mut wrong = state.clone();
    wrong.plugin_id = "other".into();
    assert!(wrong.load_into(b.as_mut(), &NoAssets).is_err());
    // through the node
    let mut n = PluginNode::new(b);
    n.load_state(b"0.25", &NoAssets).unwrap();
    assert_eq!(n.save_state(), b"0.25");
    assert!(n.load_state(b"loud", &NoAssets).is_err());
}

#[test]
fn a_crash_is_caught_and_the_node_goes_silent_for_good() {
    let mut n = node();
    let fault = n.fault_handle();
    let gain = n.descriptor().param_id("gain").unwrap();
    let crash = n.descriptor().param_id("crash").unwrap();
    n.set_param(gain, 2.0);
    n.set_param(crash, 1.0);
    assert!(!fault.is_dead());
    let (status, out) = run(&mut n);
    assert_eq!(status, ProcessStatus::Sleep);
    assert_eq!(out, vec![0.0; 4], "silence, not the stale buffer");
    assert!(fault.is_dead());
    let f = fault.fault().unwrap();
    assert_eq!(f.call, PluginCall::Process);
    assert_eq!(f.plugin_id, FAKE_ID);
    assert!(f.message.contains("crashed on demand"));
    // never called again: params read as defaults, state is empty, loads fail
    n.set_param(crash, 0.0);
    assert_eq!(run(&mut n).1, vec![0.0; 4]);
    assert_eq!(n.param(gain), 1.0);
    assert!(n.save_state().is_empty());
    assert!(n.load_state(b"1", &NoAssets).is_err());
    assert!(n.info().is_none());
    // a host-reported fault (a native crash in T3) kills a node the same way
    let mut m = node();
    m.fault_handle().report(PluginFault {
        plugin_id: FAKE_ID.into(),
        call: PluginCall::Process,
        message: "access violation".into(),
    });
    assert_eq!(run(&mut m), (ProcessStatus::Sleep, vec![0.0; 4]));
}

#[test]
fn editor_handle_opens_resizes_and_closes() {
    let mut inst = FakeFormat
        .instantiate(&FakeFormat::info(Path::new(FAKE_FILE)))
        .unwrap();
    let mut ed = inst.editor().unwrap();
    assert!(ed.can_embed() && !ed.is_open());
    assert!(
        ed.set_size(EditorSize {
            width: 1,
            height: 1
        })
        .is_err()
    );
    let s = ed.open(None).unwrap();
    assert_eq!((s.width, s.height), (400, 300));
    let s = ed
        .set_size(EditorSize {
            width: 100,
            height: 600,
        })
        .unwrap();
    assert_eq!((s.width, s.height), (200, 600));
    ed.close();
    assert!(!ed.is_open() && ed.size().is_none());
}

#[test]
fn infos_and_status_serialise_for_the_scan_cache() {
    let info = FakeFormat::info(Path::new("x/ph-test-gain.clap"));
    let v = serde_json::to_value(&info).unwrap();
    assert_eq!(
        (v["format"].as_str(), v["kind"].as_str()),
        (Some("clap"), Some("effect"))
    );
    assert_eq!(serde_json::from_value::<PluginInfo>(v).unwrap(), info);
    assert_eq!(
        serde_json::to_value(PluginStatus::Isolate).unwrap(),
        "isolate"
    );
    assert_eq!(PluginStatus::default(), PluginStatus::Ok);
}

#[test]
fn state_files_round_trip_and_reject_garbage() {
    let s = PluginState {
        format: PluginFormat::Clap,
        plugin_id: FAKE_ID.into(),
        version: "1.0.0".into(),
        data: b"line one\nbinary \x00\xff".to_vec(),
    };
    let bytes = s.to_file_bytes();
    assert!(bytes.starts_with(b"PATTERNHAUS-PLUGIN-STATE 1\n{"));
    assert_eq!(PluginState::from_file_bytes(&bytes).unwrap(), s);
    assert!(PluginState::from_file_bytes(b"junk").is_err());
    assert!(PluginState::from_file_bytes(&bytes[..bytes.len() - 1]).is_err());
}
