//! M2 criterion 6 in one project (M2/T10): audio clips in every stretch mode, tempo
//! automation, automation of every target kind (instrument, effect, mixer, channel pan, plugin
//! instrument and plugin effect parameters), the repo's test CLAP plugins, and an armed,
//! recording input path, all at once.
//!
//! - **Live ≡ offline:** with the input recording but not monitored, the simulated device
//!   (several buffer sizes and thread counts, through plan swaps and a seek) is bit-identical
//!   to the scripted offline render, and the take holds exactly the input.
//! - **The monitored path:** with monitoring on, live output is the offline render plus the
//!   input through the master strip (to float rounding), and nothing else changes: monitoring
//!   never reaches offline renders by design (DESIGN §4), so this is the strongest "live ≡
//!   offline" a monitored input allows.
//! - **No allocation:** every pull goes through `pull_into`, which runs the callback under
//!   `assert_no_alloc` (the test allocator aborts on any allocation on the audio thread).

use super::audio_oracle::audio_song;
use super::plugins::env;
use super::*;
use crate::plugins::{PluginEnv, PluginFormat, PluginSlot};
use crate::record::*;
use ph_plugin_host::testing::{GAIN_ID, SYNTH_ID};

/// [`audio_song`] (pattern clips, audio clips in every mode, tempo automation) plus the test
/// plugin synth on insert 5 through the test plugin gain, and six automation clips.
fn combined() -> Project {
    let base = audio_song();
    let mut e = Edit::new(&base);
    let bar = 4 * PPQ;
    let plug = e
        .add_channel(
            "Plug",
            InstrumentSpec::Plugin {
                format: PluginFormat::Clap,
                plugin_id: SYNTH_ID.into(),
                state: None,
                params: [("1".to_string(), 1.0)].into_iter().collect(),
            },
        )
        .unwrap();
    e.update_channel(plug, |c| {
        c.insert = InsertId(5);
        c.volume = 0.5;
    })
    .unwrap();
    e.update_insert(InsertId(5), |i| {
        i.effects.push(EffectSlot {
            effect: EffectSpec::Plugin {
                format: PluginFormat::Clap,
                plugin_id: GAIN_ID.into(),
                state: None,
                params: [("0".to_string(), 0.7)].into_iter().collect(),
            },
            bypass: false,
            mix: 1.0,
        });
    })
    .unwrap();
    let pat = e.add_pattern("Plug").unwrap();
    for (i, k) in [72u8, 76, 79, 84, 79, 76, 72, 67].iter().enumerate() {
        e.add_note(pat, plug, NoteSpec::new(*k, i as Tick * PPQ / 2, PPQ / 3))
            .unwrap();
    }
    for b in 0..4 {
        e.add_clip(9, pat, b * bar).unwrap();
    }
    let ch = |i: usize| base.channel_order[i];
    let mut auto = |track: u32, target: ParamPath, points: &[(Tick, f64, CurveKind)]| {
        let c = e.add_automation_clip(track, 0, 4 * bar, target).unwrap();
        for (t, v, k) in points {
            e.add_point(c, *t, *v, *k, 0.0).unwrap();
        }
    };
    use CurveKind::*;
    auto(
        10,
        ParamPath::ChannelParam(ch(3), "f1_cutoff".into()),
        &[
            (0, 300.0, Smooth),
            (2 * bar, 5000.0, Smooth),
            (4 * bar, 600.0, Hold),
        ],
    );
    auto(
        11,
        ParamPath::EffectParam {
            insert: InsertId(1),
            slot: 0,
            param: "b2_gain".into(),
        },
        &[(0, -6.0, SCurve), (4 * bar, 6.0, Hold)],
    );
    auto(
        12,
        ParamPath::InsertVolume(InsertId(2)),
        &[(0, 0.2, Linear), (bar, 1.0, Linear), (3 * bar, 0.5, Hold)],
    );
    auto(
        13,
        ParamPath::ChannelPan(ch(4)),
        &[
            (0, -0.8, Linear),
            (2 * bar, 0.8, Linear),
            (4 * bar, -0.8, Hold),
        ],
    );
    auto(
        14,
        ParamPath::ChannelParam(plug, "0".into()),
        &[(0, 0.2, Linear), (2 * bar, 0.9, Hold)],
    );
    auto(
        15,
        ParamPath::EffectParam {
            insert: InsertId(5),
            slot: 0,
            param: "0".into(),
        },
        &[(0, 0.3, Linear), (4 * bar, 1.5, Hold)],
    );
    e.finish().0
}

/// Two edits for plan swaps: a mixer change (nodes move across), then a moved automation
/// point and a plugin parameter (automation rebinds, the plugin node is reused).
fn edits(base: &Project) -> Vec<Arc<Project>> {
    let mut out = Vec::new();
    let mut e = Edit::new(base);
    e.update_insert(InsertId(3), |i| i.volume = 0.6).unwrap();
    out.push(Arc::new(e.finish().0));
    let mut e = Edit::new(out.last().unwrap());
    let p = out.last().unwrap();
    let (clip, point) = p
        .clips
        .values()
        .filter_map(|c| c.automation().map(|a| (c.id, a)))
        .find(|(_, a)| a.target == ParamPath::InsertVolume(InsertId(2)))
        .map(|(id, a)| (id, *a.points.keys().nth(1).unwrap()))
        .unwrap();
    e.update_point(clip, point, |pt| pt.value = 0.7).unwrap();
    let plug = *p.channel_order.last().unwrap();
    e.update_channel(plug, |c| {
        if let InstrumentSpec::Plugin { params, .. } = &mut c.instrument {
            params.insert("2".into(), 0.3);
        }
    })
    .unwrap();
    out.push(Arc::new(e.finish().0));
    out
}

fn script(base: &Project) -> Vec<(usize, Act)> {
    let ed = edits(base);
    vec![
        (0, Act::T(TransportCommand::Play)),
        (30_720, Act::P(ed[0].clone())),
        (61_440, Act::T(TransportCommand::Seek(6 * PPQ + 120))),
        (92_160, Act::P(ed[1].clone())),
    ]
}

const FRAMES: usize = 153_600;

/// The offline reference with plugin hosting: `initial`, then the script's actions at their
/// frames, rendered in blocks of `block`.
fn offline(initial: &Project, sc: &[(usize, Act)], env: &PluginEnv, block: usize) -> [Vec<f32>; 2] {
    let a = GenAssets;
    let mut off = Offline::new_with(initial, settings(block), &a, 0, Some(env)).unwrap();
    let mut out = [Vec::with_capacity(FRAMES), Vec::with_capacity(FRAMES)];
    let mut at = 0;
    let mut k = 0;
    while at < FRAMES {
        while k < sc.len() && sc[k].0 <= at {
            match &sc[k].1 {
                Act::T(t) => off.transport(t.clone()),
                Act::P(p) => off.set_project(p, &a).unwrap(),
                Act::Preview(..) => unreachable!(),
            }
            k += 1;
        }
        let next = sc.get(k).map_or(FRAMES, |s| s.0.min(FRAMES));
        let seg = off.render(next - at, block);
        out[0].extend_from_slice(&seg[0]);
        out[1].extend_from_slice(&seg[1]);
        at = next;
    }
    out
}

/// A stereo input with distinct values per frame: a slow sine on the left, its negation
/// scaled on the right.
fn input(frames: usize) -> Vec<Vec<f32>> {
    let l: Vec<f32> = (0..frames)
        .map(|i| 0.1 * (i as f32 * 0.003).sin())
        .collect();
    let r = l.iter().map(|v| -0.5 * v).collect();
    vec![l, r]
}

struct Live {
    out: [Vec<f32>; 2],
    take: Option<RecordedFile>,
}

/// The simulated device with plugins, an armed (and recording) input on the master track,
/// monitored or not, playing the script; every pull runs under `assert_no_alloc`.
fn live(
    base: &Project,
    sc: &[(usize, Act)],
    env: &PluginEnv,
    (buffer, pull, threads): (u32, usize, usize),
    monitor: bool,
    rec_dir: &std::path::Path,
) -> Live {
    let (engine, mut dev) = sim(buffer, threads);
    engine.set_plugins(Some(env.clone())).unwrap();
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.set_input_provider(Some(Box::new(SimulatedInputs::new().with(
        "Sim: In 1-2",
        SR,
        input(FRAMES + 4096),
        0,
    ))));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor,
        })
        .unwrap();
    engine.record_start(RecordOptions::new(rec_dir)).unwrap();
    let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
    let mut k = 0;
    let mut at = 0;
    while at < FRAMES {
        while k < sc.len() && sc[k].0 <= at {
            assert_eq!(sc[k].0 % pull, 0, "action off the pull grid");
            match &sc[k].1 {
                Act::T(t) => {
                    engine.transport(t.clone());
                }
                Act::P(p) => engine.set_project(p.clone()).unwrap(),
                Act::Preview(..) => unreachable!(),
            }
            k += 1;
        }
        let n = pull.min(FRAMES - at);
        let [l, r] = &mut out;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    // every plugin is live and nothing warns
    assert_eq!(engine.plugins().len(), 2);
    assert!(
        engine
            .plugin(PluginSlot::Effect { insert: 5, slot: 0 })
            .is_some()
    );
    assert!(
        engine.stats().warnings.is_empty(),
        "{:?}",
        engine.stats().warnings
    );
    let take = engine.record_stop().unwrap().into_iter().next();
    Live { out, take }
}

#[test]
fn audio_automation_plugins_and_a_recording_input_keep_live_equal_to_offline() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let base = combined();
    let sc = script(&base);
    let reference = offline(&base, &sc, &env, 1000);
    assert!(peak(&reference[0]) > 0.05, "reference render is silent");
    // the plugin node is in the mix: without hosting the render differs
    let a = GenAssets;
    let mut bare = Offline::new(&base, settings(1000), &a, 0).unwrap();
    bare.transport(TransportCommand::Play);
    let unhosted = bare.render(30_720, 1000);
    assert_ne!(unhosted[0], reference[0][..30_720], "the plugins play");
    // offline at another block size agrees with itself
    assert_bits_eq(
        &reference,
        &offline(&base, &sc, &env, 64),
        "offline block 64",
    );

    // recording, not monitored: bit-identical, and the take is the input
    for cfg in [(256u32, 256usize, 1usize), (128, 512, 3), (512, 1024, 2)] {
        let rec = tempfile::tempdir().unwrap();
        let l = live(&base, &sc, &env, cfg, false, rec.path());
        assert_bits_eq(
            &reference,
            &l.out,
            &format!("live {cfg:?} (recording, not monitored)"),
        );
        let take = l.take.expect("a take");
        assert_eq!(take.frames, FRAMES as u64);
        assert_eq!(take.overflow, 0);
    }

    // monitored: the offline render plus the input through the master strip
    let rec = tempfile::tempdir().unwrap();
    let m = live(&base, &sc, &env, (256, 256, 1), true, rec.path());
    let inp = input(FRAMES + 4096);
    for c in 0..2 {
        let diff: Vec<f64> = (0..FRAMES)
            .map(|i| m.out[c][i] as f64 - reference[c][i] as f64)
            .collect();
        let x = &inp[c][..FRAMES];
        let xx: f64 = x.iter().map(|v| (*v as f64).powi(2)).sum();
        let g = diff.iter().zip(x).map(|(d, v)| d * *v as f64).sum::<f64>() / xx;
        let resid = diff
            .iter()
            .zip(x)
            .map(|(d, v)| (d - g * *v as f64).abs())
            .fold(0.0f64, f64::max);
        println!("monitored channel {c}: input gain {g:.6}, residual {resid:e}");
        assert!(
            g > 0.5 && g < 1.5,
            "channel {c}: the monitored input is in the output at gain {g}"
        );
        assert!(
            resid < 1e-5,
            "channel {c}: live - offline is more than the monitored input (residual {resid})"
        );
    }
}

/// Follow-up (M2/T10): each analyser effect's window reads its own instance. Two analysers on
/// two inserts, a synth on one of them only: the engine hands out one feed per slot, the
/// sounding insert's feed sees the signal and the silent one's does not, and a parameter
/// edit (the node is reused across the plan swap) keeps the same feed.
#[test]
fn every_analyser_slot_has_its_own_feed() {
    let mut e = Edit::new(&Project::empty("analysers"));
    let c = e.add_channel("Synth", synth(&[])).unwrap();
    e.update_channel(c, |ch| ch.insert = InsertId(1)).unwrap();
    // a second channel, with no notes, keeps insert 2 in the graph
    let quiet = e.add_channel("Quiet", synth(&[])).unwrap();
    e.update_channel(quiet, |ch| ch.insert = InsertId(2))
        .unwrap();
    for i in [1u32, 2] {
        e.update_insert(InsertId(i), |ins| ins.effects.push(fx("analyzer", &[])))
            .unwrap();
    }
    let pat = e.add_pattern("P").unwrap();
    e.add_note(pat, c, NoteSpec::new(57, 0, 8 * PPQ)).unwrap();
    e.add_clip(0, pat, 0).unwrap();
    let p = e.finish().0;
    let (engine, mut dev) = sim(256, 1);
    engine.set_project(Arc::new(p.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let mut l = vec![0.0f32; 24_576];
    let mut r = vec![0.0f32; 24_576];
    for k in 0..96 {
        dev.pull_into(
            &mut l[k * 256..(k + 1) * 256],
            &mut r[k * 256..(k + 1) * 256],
        );
    }
    let a = engine
        .analyzer_feed(1, 0)
        .expect("insert 1 has an analyser");
    let b = engine
        .analyzer_feed(2, 0)
        .expect("insert 2 has an analyser");
    assert!(engine.analyzer_feed(3, 0).is_none());
    assert!(!Arc::ptr_eq(&a, &b), "one feed per analyser");
    assert!(a.updates() > 0 && a.peak()[0] > 0.01, "{:?}", a.peak());
    assert!(
        b.peak()[0] < 1e-6,
        "the silent insert's analyser: {:?}",
        b.peak()
    );
    assert!(a.short_term_lufs() > b.short_term_lufs());
    // a parameter edit swaps the plan but keeps the analyser node, and with it the feed
    let mut e = Edit::new(&p);
    e.update_insert(InsertId(1), |ins| {
        ins.effects[0] = fx("analyzer", &[("tilt", 1.0)]);
    })
    .unwrap();
    engine.set_project(Arc::new(e.finish().0)).unwrap();
    for _ in 0..8 {
        dev.pull(256);
    }
    let a2 = engine.analyzer_feed(1, 0).unwrap();
    assert!(Arc::ptr_eq(&a, &a2), "the reused node keeps its feed");
}
