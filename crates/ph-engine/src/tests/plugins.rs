//! Plugin hosting in the engine (M2/T3) with the repo's test plugins: plugin instruments and
//! effects compile like built-ins, the test plugins keep live output bit-identical to the
//! offline render, latency is reported, states load, and a crashing plugin goes silent, is
//! reported once and is isolated while the session keeps playing.

use super::*;
use crate::plugins::*;
use crate::{PlayMode, RenderSettings, render_offline_with};
use ph_plugin_host::manager::{HostConfig, PluginHost};
use ph_plugin_host::testing::{CRASH_ID, GAIN_ID, GAIN_LATENCY, SYNTH_ID, test_plugins};
use std::sync::OnceLock;

/// One host per test binary, scanned once through the helper process.
fn host() -> Arc<PluginHost> {
    static HOST: OnceLock<Arc<PluginHost>> = OnceLock::new();
    HOST.get_or_init(|| {
        let p = test_plugins();
        let h = PluginHost::new(HostConfig {
            cache_file: None,
            extra_paths: vec![],
            scan_helper: Some(p.scan_helper.clone()),
            timeout: None,
        });
        let r = h.scan(std::slice::from_ref(&p.dir), |_| {}).unwrap();
        assert_eq!(r.found.len(), 3, "{:?}", r.failed);
        Arc::new(h)
    })
    .clone()
}

fn env(dir: &std::path::Path) -> PluginEnv {
    PluginEnv::new(host(), dir)
}

fn plugin(id: &str, state: Option<&str>, p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Plugin {
        format: PluginFormat::Clap,
        plugin_id: id.into(),
        state: state.map(str::to_string),
        params: p.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    }
}

fn plugin_fx(id: &str, p: &[(&str, f64)]) -> EffectSlot {
    EffectSlot {
        effect: EffectSpec::Plugin {
            format: PluginFormat::Clap,
            plugin_id: id.into(),
            state: None,
            params: p.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        },
        bypass: false,
        mix: 1.0,
    }
}

/// A plugin synth on insert 1 through the plugin gain, and a Haus Synth on insert 2.
fn song(synth_state: Option<&str>) -> (Project, ChannelId, ChannelId) {
    let mut e = Edit::new(&Project::empty("plugins"));
    let ps = e
        .add_channel("Plugin synth", plugin(SYNTH_ID, synth_state, &[("1", 1.0)]))
        .unwrap();
    let hs = e
        .add_channel("Haus synth", synth(&[("amp_release", 0.1)]))
        .unwrap();
    e.update_channel(ps, |c| c.insert = InsertId(1)).unwrap();
    e.update_channel(hs, |c| c.insert = InsertId(2)).unwrap();
    e.update_insert(InsertId(1), |i| {
        i.effects.push(plugin_fx(GAIN_ID, &[("0", 0.5)]));
    })
    .unwrap();
    let pat = e.add_pattern("P").unwrap();
    for (i, k) in [60u8, 64, 67, 72].iter().enumerate() {
        e.add_note(pat, ps, NoteSpec::new(*k, i as Tick * PPQ, PPQ / 2))
            .unwrap();
        e.add_note(pat, hs, NoteSpec::new(*k - 12, i as Tick * PPQ, PPQ / 2))
            .unwrap();
    }
    for b in 0..2 {
        e.add_clip(0, pat, b * 4 * PPQ).unwrap();
    }
    (e.finish().0, ps, hs)
}

fn render(p: &Project, env: Option<&PluginEnv>) -> [Vec<f32>; 2] {
    let a = render_offline_with(
        p,
        &RenderSettings {
            mode: PlayMode::Song,
            range: None,
            sample_rate: SR,
            quality: Quality::Live,
            include_tail: false,
            block_frames: 512,
        },
        &GenAssets,
        env,
    )
    .unwrap();
    let [l, r]: [Vec<f32>; 2] = a.channels.try_into().unwrap();
    [l, r]
}

#[test]
fn plugin_instruments_and_effects_play_and_live_matches_offline_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let (p, ps, _) = song(None);
    let off = render(&p, Some(&env));
    let frames = off[0].len();
    assert!(peak(&off[0]) > 0.05);
    // without hosting the plugin channel is silent: hosting changes the output
    let without = render(&p, None);
    assert_ne!(without, off);
    // the live engine on the simulated device, with its own plugin instances
    let (engine, mut dev) = sim(256, 2);
    engine.set_plugins(Some(env.clone())).unwrap();
    engine.set_project(Arc::new(p.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let mut live = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut at = 0;
    while at < frames {
        let n = 256.min(frames - at);
        let [l, r] = &mut live;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    assert_bits_eq(&live, &off, "test plugins live vs offline");
    // the engine knows its plugin nodes, their latency and live values
    let synth = engine.plugin(PluginSlot::Channel(ps)).unwrap();
    assert_eq!(synth.info.id, SYNTH_ID);
    assert_eq!(synth.values().unwrap()[1], 1.0, "spec params applied");
    let gain = engine
        .plugin(PluginSlot::Effect { insert: 1, slot: 0 })
        .unwrap();
    assert_eq!(gain.latency(), GAIN_LATENCY);
    assert_eq!(gain.values().unwrap(), vec![0.5]);
    assert_eq!(engine.plugins().len(), 2);
    assert!(
        engine.stats().warnings.is_empty(),
        "{:?}",
        engine.stats().warnings
    );
}

#[test]
fn param_edits_reuse_the_plugin_node_and_reach_it() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let (p, ps, _) = song(None);
    let (engine, mut dev) = sim(256, 1);
    engine.set_plugins(Some(env)).unwrap();
    engine.set_project(Arc::new(p.clone())).unwrap();
    dev.pull(256);
    let before = engine.plugin(PluginSlot::Channel(ps)).unwrap();
    let mut e = Edit::new(&p);
    e.update_channel(ps, |c| {
        if let InstrumentSpec::Plugin { params, .. } = &mut c.instrument {
            params.insert("0".into(), 0.125);
        }
    })
    .unwrap();
    engine.set_project(Arc::new(e.finish().0)).unwrap();
    dev.pull(256);
    dev.pull(256);
    let after = engine.plugin(PluginSlot::Channel(ps)).unwrap();
    assert!(Arc::ptr_eq(&before.fault, &after.fault), "same instance");
    assert_eq!(after.values().unwrap()[0], 0.125);
}

#[test]
fn states_load_from_plugins_files_and_from_live_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let (p, ps, _) = song(Some("plugins/synth.state"));
    // no file yet: defaults
    let (engine, mut dev) = sim(256, 1);
    engine.set_plugins(Some(env.clone())).unwrap();
    engine.set_project(Arc::new(p.clone())).unwrap();
    dev.pull(256);
    let live = engine.plugin(PluginSlot::Channel(ps)).unwrap();
    assert_eq!(live.state_ref.as_deref(), Some("plugins/synth.state"));
    // the live state, as the file the host would write
    let states = engine.plugin_states();
    let file = states.get("plugins/synth.state").unwrap();
    let st = PluginState::from_file_bytes(file).unwrap();
    assert_eq!(st.plugin_id, SYNTH_ID);
    assert!(st.data.starts_with(b"PHTP1"));
    // a state file with gain 0 silences the plugin synth (but not the Haus synth)
    let mut silent = PluginState::from_file_bytes(file).unwrap();
    silent.data = b"PHTP1\n0=0\n1=1\n2=0.05\n".to_vec();
    std::fs::create_dir_all(dir.path().join("plugins")).unwrap();
    std::fs::write(
        dir.path().join("plugins/synth.state"),
        silent.to_file_bytes(),
    )
    .unwrap();
    let (only_plugin, _, hs) = song(Some("plugins/synth.state"));
    let mut e = Edit::new(&only_plugin);
    e.update_channel(hs, |c| c.muted = true).unwrap();
    let only_plugin = e.finish().0;
    assert_eq!(peak(&render(&only_plugin, Some(&env))[0]), 0.0);
    // an override (the live session's state) wins over the file
    let live_env = env.with_states(states);
    assert!(peak(&render(&only_plugin, Some(&live_env))[0]) > 0.05);
    // a state of another plugin is refused: the channel plays silence and warns nothing new
    let mut wrong = silent.clone();
    wrong.plugin_id = GAIN_ID.into();
    std::fs::write(
        dir.path().join("plugins/synth.state"),
        wrong.to_file_bytes(),
    )
    .unwrap();
    assert_eq!(peak(&render(&only_plugin, Some(&env))[0]), 0.0);
}

#[test]
fn unscanned_plugins_warn_and_play_silence() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = Edit::new(&Project::empty("x"));
    e.add_channel("Ghost", plugin("com.example.ghost", None, &[]))
        .unwrap();
    let p = e.finish().0;
    let w = crate::project_warnings_with(&p, &GenAssets, Some(&env(dir.path())));
    assert_eq!(w.len(), 1);
    assert!(
        w[0].contains("has not been found by a plugin scan"),
        "{}",
        w[0]
    );
    let w = crate::project_warnings(&p, &GenAssets);
    assert!(w[0].contains("not enabled"), "{}", w[0]);
}

#[test]
fn a_crashing_plugin_is_caught_reported_isolated_and_the_session_keeps_playing() {
    let dir = tempfile::tempdir().unwrap();
    // its own host, so marking the crash plugin isolate does not affect other tests
    let p = test_plugins();
    let h = PluginHost::new(HostConfig {
        scan_helper: Some(p.scan_helper.clone()),
        cache_file: Some(dir.path().join("cache.json")),
        ..Default::default()
    });
    h.scan(std::slice::from_ref(&p.dir), |_| {}).unwrap();
    let env = PluginEnv::new(Arc::new(h), dir.path());
    let (mut song, _, hs) = song(None);
    let mut e = Edit::new(&song);
    e.update_insert(InsertId(1), |i| i.effects.push(plugin_fx(CRASH_ID, &[])))
        .unwrap();
    song = e.finish().0;
    let (engine, mut dev) = sim(256, 2);
    engine.set_plugins(Some(env.clone())).unwrap();
    engine.set_project(Arc::new(song.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    for _ in 0..8 {
        dev.pull(256);
    }
    assert!(engine.plugin_faults().is_empty());
    // switch the crash on through the project, like plugin.set_param would
    let mut e = Edit::new(&song);
    e.update_insert(InsertId(1), |i| {
        if let EffectSpec::Plugin { params, .. } = &mut i.effects[1].effect {
            params.insert("0".into(), 1.0);
        }
    })
    .unwrap();
    engine.set_project(Arc::new(e.finish().0)).unwrap();
    let mut after = [Vec::new(), Vec::new()];
    for _ in 0..40 {
        let b = dev.pull(256);
        after[0].extend_from_slice(&b.channels[0]);
        after[1].extend_from_slice(&b.channels[1]);
    }
    let faults = engine.plugin_faults();
    assert_eq!(faults.len(), 1, "reported once");
    let f = &faults[0];
    assert_eq!(f.slot, PluginSlot::Effect { insert: 1, slot: 1 });
    assert_eq!(f.plugin.id, CRASH_ID);
    assert_eq!(f.fault.call, ph_plugin_host::PluginCall::Process);
    assert!(f.message().contains("isolate"));
    // isolated in the scan cache (persisted), and the engine says so
    assert_eq!(
        env.host.status(PluginFormat::Clap, CRASH_ID),
        PluginStatus::Isolate
    );
    let cache = std::fs::read_to_string(dir.path().join("cache.json")).unwrap();
    assert!(cache.contains("isolate"));
    assert!(
        engine
            .stats()
            .warnings
            .iter()
            .any(|w| w.contains("crashed"))
    );
    assert_eq!(engine.plugin_faults().len(), 1, "still once");
    // the session keeps running: the Haus synth on insert 2 still sounds
    assert!(peak(&after[0]) > 0.01, "the rest of the mix still plays");
    let _ = hs;
}

#[test]
fn plugin_parameters_are_automated_through_param_events() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let (p, _, _) = song(None);
    let mut e = Edit::new(&p);
    let c = e
        .add_automation_clip(
            1,
            4 * PPQ,
            4 * PPQ,
            ParamPath::EffectParam {
                insert: InsertId(1),
                slot: 0,
                param: "0".into(),
            },
        )
        .unwrap();
    e.add_point(c, 0, 0.1, CurveKind::Hold, 0.0).unwrap();
    let auto = e.finish().0;
    let plain = render(&p, Some(&env));
    let out = render(&auto, Some(&env));
    let bar = (SR as f64 * 60.0 / p.settings.bpm * 4.0).round() as usize;
    let part = |x: &[Vec<f32>; 2], b: usize| rms(&x[0][b * bar..(b + 1) * bar]);
    // the first bar is untouched; the second plays through the automated gain
    assert_eq!(plain[0][..bar], out[0][..bar]);
    let ratio = (part(&out, 1) / part(&plain, 1)) as f64;
    assert!(
        (ratio - 1.0).abs() > 0.2,
        "the plugin parameter was not automated: ratio {ratio}"
    );
    // live equals offline with automation on a plugin node
    let frames = out[0].len();
    let (engine, mut dev) = sim(256, 2);
    engine.set_plugins(Some(env.clone())).unwrap();
    engine.set_project(Arc::new(auto.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let mut live = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut at = 0;
    while at < frames {
        let n = 256.min(frames - at);
        let [l, r] = &mut live;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    assert_bits_eq(&out, &live, "automated plugin: live vs offline");
}
