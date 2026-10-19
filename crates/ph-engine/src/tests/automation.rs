//! Automation playback (M2/T7): every target kind reaches the audio, parameters return to the
//! project's value when automation stops, and events do not depend on how a render is split
//! into blocks.

use super::audio::song;
use super::*;
use crate::automation::{AutomationEvaluator, BoundTarget};
use crate::tempo::TempoMap;
use crate::{PlayMode, render_offline};
use ph_node::AUTOMATION_INTERVAL;

/// One beat at 120 BPM, in samples.
const BEAT: usize = 24_000;

/// A sustained sine on channel 0 (insert 1), one long note, 120 BPM; `f` adds automation.
fn tone(f: impl FnOnce(&mut Edit, ChannelId)) -> Project {
    let mut e = Edit::new(&Project::empty("auto"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let lead = e
        .add_channel(
            "Lead",
            synth(&[
                ("osc1_wave", 0.0),
                ("amp_attack", 0.001),
                ("amp_sustain", 1.0),
                ("volume", -6.0),
                ("f1_cutoff", 20_000.0),
            ]),
        )
        .unwrap();
    e.update_channel(lead, |c| c.insert = InsertId(1)).unwrap();
    let pat = e.add_pattern("P").unwrap();
    e.add_note(pat, lead, NoteSpec::new(69, 0, 12 * PPQ))
        .unwrap();
    e.add_clip(0, pat, 0).unwrap();
    f(&mut e, lead);
    e.finish().0
}

/// A clip on `track` holding `value` between ticks `from` and `to` (a hold point at each end).
fn hold(e: &mut Edit, track: u32, target: ParamPath, from: Tick, to: Tick, value: f64) -> ClipId {
    let c = e
        .add_automation_clip(track, from, to - from, target)
        .unwrap();
    e.add_point(c, 0, value, CurveKind::Hold, 0.0).unwrap();
    c
}

fn render(p: &Project, block: usize) -> [Vec<f32>; 2] {
    let b = render_offline(p, &song(block), &GenAssets).unwrap();
    [b.channels[0].clone(), b.channels[1].clone()]
}

/// RMS of channel `c` over beat `from` (+ a margin that clears onsets and smoothing) to `to`.
fn level(out: &[Vec<f32>; 2], c: usize, from: usize, to: usize) -> f32 {
    rms(&out[c][from * BEAT + 3000..to * BEAT - 1000])
}

fn assert_ratio(got: f32, want: f32, tol: f32, what: &str) {
    assert!(
        (got / want - 1.0).abs() <= tol,
        "{what}: got {got}, want {want}"
    );
}

fn project_tempo(p: &Project) -> TempoMap {
    AutomationEvaluator::new(p).tempo_map(p.settings.bpm, SR, crate::plan::TEMPO_RESOLUTION)
}

fn bound(p: &Project) -> Vec<BoundTarget> {
    AutomationEvaluator::new(p).bind(&project_tempo(p))
}

#[test]
fn an_instrument_parameter_follows_its_clip_and_returns_to_the_project_value() {
    let plain = render(&tone(|_, _| {}), 256);
    let p = tone(|e, c| {
        hold(
            e,
            1,
            ParamPath::ChannelParam(c, "volume".into()),
            2 * PPQ,
            4 * PPQ,
            -30.0,
        );
    });
    let out = render(&p, 256);
    let before = level(&out, 0, 0, 2);
    let inside = level(&out, 0, 2, 4);
    let after = level(&out, 0, 4, 6);
    // −6 dB → −30 dB inside (24 dB down), then back
    assert_ratio(inside / before, 10f32.powf(-24.0 / 20.0), 0.06, "inside");
    assert_ratio(after / before, 1.0, 0.02, "after the clip ends");
    assert_ratio(before / level(&plain, 0, 0, 2), 1.0, 0.01, "before");
}

#[test]
fn stopping_returns_automated_parameters_to_the_project_value() {
    // the clip never ends within the first 3 beats; stop inside it
    let p = tone(|e, c| {
        hold(
            e,
            1,
            ParamPath::ChannelParam(c, "volume".into()),
            0,
            8 * PPQ,
            -30.0,
        );
    });
    let lead = p.channel_order[0];
    let a = GenAssets;
    let mut off = crate::offline::Offline::new(&p, settings(256), &a, 0).unwrap();
    off.transport(TransportCommand::Play);
    let playing = off.render(2 * BEAT, 256);
    off.transport(TransportCommand::Stop);
    off.render(2048, 256);
    off.command(Command::Preview {
        channel: lead,
        key: 69,
        velocity: 1.0,
        on: true,
    });
    let stopped = off.render(BEAT, 256);
    let reference = {
        let mut off = crate::offline::Offline::new(&tone(|_, _| {}), settings(256), &a, 0).unwrap();
        off.command(Command::Preview {
            channel: lead,
            key: 69,
            velocity: 1.0,
            on: true,
        });
        off.render(BEAT, 256)
    };
    let plain = {
        let mut off = crate::offline::Offline::new(&tone(|_, _| {}), settings(256), &a, 0).unwrap();
        off.transport(TransportCommand::Play);
        off.render(2 * BEAT, 256)
    };
    let win = |x: &[Vec<f32>; 2]| rms(&x[0][BEAT / 2..BEAT - 1000]);
    assert_ratio(
        rms(&playing[0][BEAT / 2..2 * BEAT]) / rms(&plain[0][BEAT / 2..2 * BEAT]),
        10f32.powf(-24.0 / 20.0),
        0.06,
        "automated while playing",
    );
    assert_ratio(
        win(&stopped) / win(&reference),
        1.0,
        0.02,
        "a preview after stop plays at the project value",
    );
}

#[test]
fn channel_volume_and_pan_automation_drive_the_strip() {
    let p = tone(|e, c| {
        hold(e, 1, ParamPath::ChannelVolume(c), 2 * PPQ, 4 * PPQ, 0.4);
        hold(e, 2, ParamPath::ChannelPan(c), 4 * PPQ, 6 * PPQ, -1.0);
    });
    let out = render(&p, 100);
    let model = p.channels[&p.channel_order[0]].volume;
    let want = crate::plan::channel_gain(0.4, 0.0, false)[0]
        / crate::plan::channel_gain(model, 0.0, false)[0];
    assert_ratio(
        level(&out, 0, 2, 4) / level(&out, 0, 0, 2),
        want,
        0.03,
        "volume",
    );
    assert_ratio(
        level(&out, 0, 3, 4) / level(&out, 1, 3, 4),
        1.0,
        0.01,
        "centred",
    );
    // hard left: the right side is silent, the left is at full pan gain
    assert!(level(&out, 1, 4, 6) < 1e-4, "right is silent");
    let full = crate::plan::channel_gain(model, -1.0, false)[0]
        / crate::plan::channel_gain(model, 0.0, false)[0];
    assert_ratio(
        level(&out, 0, 4, 6) / level(&out, 0, 0, 2),
        full,
        0.03,
        "left",
    );
    assert_ratio(
        level(&out, 0, 6, 8) / level(&out, 0, 0, 2),
        1.0,
        0.02,
        "after",
    );
}

#[test]
fn mixer_volume_pan_mute_and_send_automation_drive_the_inserts() {
    let p = tone(|e, c| {
        // a second route: insert 1 sends to insert 2 only, not to the master
        e.update_insert(InsertId(1), |i| {
            i.route_to_master = false;
            i.sends.push(ph_core::Send {
                to: InsertId(2),
                level: 1.0,
            });
        })
        .unwrap();
        let _ = c;
        hold(
            e,
            1,
            ParamPath::InsertSend {
                insert: InsertId(1),
                to: InsertId(2),
            },
            2 * PPQ,
            4 * PPQ,
            0.25,
        );
        hold(
            e,
            2,
            ParamPath::InsertVolume(InsertId(2)),
            4 * PPQ,
            6 * PPQ,
            0.5,
        );
        hold(
            e,
            3,
            ParamPath::InsertMute(InsertId(2)),
            6 * PPQ,
            8 * PPQ,
            1.0,
        );
        hold(
            e,
            4,
            ParamPath::InsertPan(InsertId(2)),
            8 * PPQ,
            10 * PPQ,
            1.0,
        );
    });
    let out = render(&p, 64);
    let base = level(&out, 0, 0, 2);
    assert!(base > 0.01);
    assert_ratio(level(&out, 0, 2, 4) / base, 0.25, 0.02, "send level");
    let model = p.mixer[2].volume as f32;
    assert_ratio(
        level(&out, 0, 4, 6) / base,
        0.5 / model,
        0.02,
        "insert volume",
    );
    assert!(level(&out, 0, 6, 8) < 1e-5, "muted");
    assert!(level(&out, 0, 8, 10) < 1e-5, "panned right: left is silent");
    assert_ratio(
        level(&out, 1, 8, 10) / level(&out, 1, 0, 2),
        1.0,
        0.02,
        "right at full",
    );
    assert_ratio(level(&out, 0, 10, 12) / base, 1.0, 0.02, "all back");
}

#[test]
fn an_effect_parameter_follows_its_clip() {
    let p = tone(|e, c| {
        e.update_channel(c, |ch| {
            if let InstrumentSpec::Synth { params, .. } = &mut ch.instrument {
                params.insert("osc1_wave".into(), 1.0);
            }
        })
        .unwrap();
        e.update_insert(InsertId(1), |i| {
            i.effects.push(fx("filter", &[("cutoff", 20_000.0)]));
        })
        .unwrap();
        hold(
            e,
            1,
            ParamPath::EffectParam {
                insert: InsertId(1),
                slot: 0,
                param: "cutoff".into(),
            },
            2 * PPQ,
            4 * PPQ,
            120.0,
        );
    });
    let out = render(&p, 64);
    let open = level(&out, 0, 0, 2);
    assert!(
        level(&out, 0, 2, 4) < 0.35 * open,
        "closed filter: {} vs {open}",
        level(&out, 0, 2, 4)
    );
    assert_ratio(level(&out, 0, 4, 6) / open, 1.0, 0.03, "filter open again");
}

#[test]
fn removing_automation_resets_a_reused_processor() {
    let a = GenAssets;
    let automated = tone(|e, c| {
        hold(
            e,
            1,
            ParamPath::ChannelParam(c, "volume".into()),
            0,
            12 * PPQ,
            -30.0,
        );
    });
    let plain = tone(|_, _| {});
    let mut off = crate::offline::Offline::new(&automated, settings(256), &a, 0).unwrap();
    off.transport(TransportCommand::Play);
    let first = off.render(2 * BEAT, 256);
    // the clip is deleted while playing: the synth is reused and must return to −6 dB
    off.set_project(&Arc::new(plain.clone()), &a).unwrap();
    let second = off.render(2 * BEAT, 256);
    let w = |x: &[Vec<f32>; 2]| rms(&x[0][BEAT / 2..2 * BEAT]);
    assert_ratio(
        w(&second) / w(&first),
        10f32.powf(24.0 / 20.0),
        0.08,
        "level after the clip is removed",
    );
}

#[test]
fn parameter_events_meet_the_contract_and_do_not_depend_on_blocks() {
    let p = tone(|e, c| {
        let k = e
            .add_automation_clip(1, PPQ, 4 * PPQ, ParamPath::ChannelParam(c, "volume".into()))
            .unwrap();
        e.add_point(k, 0, -30.0, CurveKind::Smooth, 0.0).unwrap();
        e.add_point(k, PPQ, -6.0, CurveKind::Linear, 0.0).unwrap();
        e.add_point(k, 2 * PPQ, -20.0, CurveKind::SCurve, 0.0)
            .unwrap();
        e.add_point(k, 3 * PPQ, -10.0, CurveKind::Hold, 0.0)
            .unwrap();
    });
    let tempo = project_tempo(&p);
    let b = &bound(&p)[0];
    let total = 7 * BEAT as i64;
    let run = |block: i64| -> Vec<(i64, f64)> {
        let mut out = Vec::new();
        let mut last = f64::NAN;
        let mut a = 0;
        while a < total {
            let e = (a + block).min(total);
            b.schedule(&tempo, -6.0, a, e, true, a == 0, &mut last, |f, v| {
                out.push((a + f as i64, v))
            });
            a = e;
        }
        out
    };
    let whole = run(total);
    for block in [37, 64, 512, 1000] {
        assert_eq!(whole, run(block), "block {block}");
    }
    // exact values at their own frame; gaps at most AUTOMATION_INTERVAL inside a moving curve
    for (s, v) in &whole {
        if let Some(want) = b.value_at(&tempo, *s) {
            assert_eq!(*v, want);
        }
    }
    let moving: Vec<i64> = whole
        .iter()
        .map(|e| e.0)
        .filter(|s| (PPQ as i64 * 24..3 * PPQ as i64 * 24).contains(s))
        .collect();
    assert!(
        moving
            .windows(2)
            .all(|w| w[1] - w[0] <= AUTOMATION_INTERVAL as i64)
    );
    // clip start (value arrives), the hold segment, and the project value after the end
    assert_eq!(whole.first().unwrap().0, 0);
    assert_eq!(
        whole.first().unwrap().1,
        -6.0,
        "the project value at the start"
    );
    assert_eq!(whole.last().unwrap(), &(5 * BEAT as i64, -6.0));
    // stopped: only the project value, once
    let mut last = f64::NAN;
    let mut n = Vec::new();
    for a in (0..4096).step_by(512) {
        b.schedule(
            &tempo,
            -6.0,
            a,
            a + 512,
            false,
            a == 0,
            &mut last,
            |f, v| n.push((a + f as i64, v)),
        );
    }
    assert_eq!(n, vec![(0, -6.0)]);
}

// ------------------------------------------------------------------------ the oracle

/// [`beat`] with automation on every target kind: instrument params (synth, sampler), an
/// effect param, channel volume and pan, insert volume, pan, mute and send level, with
/// overlapping clips, a clip offset, a jump and every curve type, plus the tempo ramp.
pub fn automated_song() -> Project {
    let base = beat();
    let mut e = Edit::new(&base);
    let [kick, snare, hat, bass, chords] = [0, 1, 2, 3, 4].map(|i| base.channel_order[i]);
    let _ = hat;
    let bar = 4 * PPQ;
    // synth cutoff: smooth sweep with a jump in the middle
    let c = e
        .add_automation_clip(
            3,
            0,
            4 * bar,
            ParamPath::ChannelParam(bass, "f1_cutoff".into()),
        )
        .unwrap();
    e.add_point(c, 0, 200.0, CurveKind::Smooth, 0.0).unwrap();
    e.add_point(c, bar, 6000.0, CurveKind::SCurve, 0.3).unwrap();
    e.add_point(c, 2 * bar, 400.0, CurveKind::Linear, -0.5)
        .unwrap();
    e.add_point(c, 2 * bar, 3000.0, CurveKind::Hold, 0.0)
        .unwrap();
    e.add_point(c, 3 * bar, 800.0, CurveKind::Linear, 0.0)
        .unwrap();
    // a second, later clip over the first: wins where they overlap
    let c = e
        .add_automation_clip(
            4,
            bar + PPQ,
            2 * PPQ,
            ParamPath::ChannelParam(bass, "f1_cutoff".into()),
        )
        .unwrap();
    e.add_point(c, 0, 1500.0, CurveKind::Linear, 0.0).unwrap();
    e.add_point(c, 2 * PPQ, 300.0, CurveKind::Hold, 0.0)
        .unwrap();
    // sampler parameter (volume in dB) with a clip offset into the point timeline
    let c = e
        .add_automation_clip(
            5,
            0,
            2 * bar,
            ParamPath::ChannelParam(kick, "volume".into()),
        )
        .unwrap();
    e.add_point(c, 0, 0.0, CurveKind::Linear, 0.0).unwrap();
    e.add_point(c, 2 * bar, -18.0, CurveKind::Hold, 0.0)
        .unwrap();
    e.update_clip(c, |k| k.offset = bar / 2).unwrap();
    // effect param: the saturator's drive on insert 2
    let c = e
        .add_automation_clip(
            6,
            PPQ,
            3 * bar,
            ParamPath::EffectParam {
                insert: InsertId(2),
                slot: 0,
                param: "drive".into(),
            },
        )
        .unwrap();
    e.add_point(c, 0, 1.0, CurveKind::Smooth, 0.0).unwrap();
    e.add_point(c, 2 * bar, 14.0, CurveKind::Linear, 0.0)
        .unwrap();
    e.add_point(c, 3 * bar, 2.0, CurveKind::Hold, 0.0).unwrap();
    // strips
    let c = e
        .add_automation_clip(8, bar, 2 * bar, ParamPath::ChannelVolume(chords))
        .unwrap();
    e.add_point(c, 0, 0.8, CurveKind::Linear, 0.0).unwrap();
    e.add_point(c, bar, 0.2, CurveKind::SCurve, 0.0).unwrap();
    e.add_point(c, 2 * bar, 0.8, CurveKind::Hold, 0.0).unwrap();
    let c = e
        .add_automation_clip(9, 0, 3 * bar, ParamPath::ChannelPan(snare))
        .unwrap();
    e.add_point(c, 0, -1.0, CurveKind::Smooth, 0.0).unwrap();
    e.add_point(c, 3 * bar, 1.0, CurveKind::Hold, 0.0).unwrap();
    let c = e
        .add_automation_clip(10, PPQ, 3 * bar, ParamPath::InsertVolume(InsertId(2)))
        .unwrap();
    e.add_point(c, 0, 1.0, CurveKind::Linear, 0.0).unwrap();
    e.add_point(c, bar, 0.3, CurveKind::Hold, 0.0).unwrap();
    e.add_point(c, 2 * bar, 1.4, CurveKind::Linear, 0.0)
        .unwrap();
    let c = e
        .add_automation_clip(11, 0, 4 * bar, ParamPath::InsertPan(InsertId(3)))
        .unwrap();
    e.add_point(c, 0, 0.8, CurveKind::SCurve, 0.0).unwrap();
    e.add_point(c, 2 * bar, -0.8, CurveKind::Linear, 0.0)
        .unwrap();
    let c = e
        .add_automation_clip(12, 2 * bar, 2 * PPQ, ParamPath::InsertMute(InsertId(3)))
        .unwrap();
    e.add_point(c, 0, 1.0, CurveKind::Hold, 0.0).unwrap();
    let c = e
        .add_automation_clip(
            13,
            0,
            4 * bar,
            ParamPath::InsertSend {
                insert: InsertId(1),
                to: InsertId(4),
            },
        )
        .unwrap();
    e.add_point(c, 0, 0.0, CurveKind::Linear, 0.0).unwrap();
    e.add_point(c, 2 * bar, 1.2, CurveKind::SCurve, 0.0)
        .unwrap();
    e.add_point(c, 4 * bar, 0.1, CurveKind::Hold, 0.0).unwrap();
    // tempo: a gentle ramp
    let t = e
        .add_automation_clip(14, 0, 4 * bar, ParamPath::Tempo)
        .unwrap();
    e.add_point(t, 0, 128.0, CurveKind::Linear, 0.0).unwrap();
    e.add_point(t, 3 * bar, 140.0, CurveKind::Hold, 0.0)
        .unwrap();
    e.finish().0
}

fn edits(base: &Project) -> Vec<Arc<Project>> {
    let mut out = Vec::new();
    // 1: a channel change only (every automated processor is reused)
    let mut e = Edit::new(base);
    e.update_channel(base.channel_order[2], |c| c.volume = 0.5)
        .unwrap();
    out.push(Arc::new(e.finish().0));
    // 2: delete the cutoff sweep's second clip and add a hold on the saturator
    let mut e = Edit::new(out.last().unwrap());
    let victim = base
        .clips
        .values()
        .find(|c| {
            c.automation().is_some_and(|a| {
                a.points.len() == 2
                    && matches!(&a.target, ParamPath::ChannelParam(_, p) if p == "f1_cutoff")
            })
        })
        .unwrap()
        .id;
    e.remove_clip(victim).unwrap();
    hold(
        &mut e,
        15,
        ParamPath::EffectParam {
            insert: InsertId(2),
            slot: 0,
            param: "drive".into(),
        },
        2 * PPQ,
        12 * PPQ,
        20.0,
    );
    out.push(Arc::new(e.finish().0));
    // 3: remove every automation clip on the saturator (it must return to the project value)
    let mut e = Edit::new(out.last().unwrap());
    let ids: Vec<ClipId> = out
        .last()
        .unwrap()
        .clips
        .values()
        .filter(|c| {
            c.automation().is_some_and(|a| {
                matches!(&a.target, ParamPath::EffectParam { insert, .. } if *insert == InsertId(2))
            })
        })
        .map(|c| c.id)
        .collect();
    for id in ids {
        e.remove_clip(id).unwrap();
    }
    out.push(Arc::new(e.finish().0));
    out
}

fn script(base: &Project) -> Vec<(usize, Act)> {
    let ed = edits(base);
    let pat = base.pattern_order[1];
    vec![
        (0, Act::T(TransportCommand::Play)),
        (20_480, Act::P(ed[0].clone())),
        (40_960, Act::T(TransportCommand::Seek(5 * PPQ + 240))),
        (61_440, Act::P(ed[1].clone())),
        (
            81_920,
            Act::T(TransportCommand::SetLoop(Some((6 * PPQ, 10 * PPQ)))),
        ),
        (102_400, Act::P(ed[2].clone())),
        (
            143_360,
            Act::T(TransportCommand::SetMode(PlayMode::Pattern(pat))),
        ),
        (153_600, Act::T(TransportCommand::SetMode(PlayMode::Song))),
        (163_840, Act::T(TransportCommand::Play)),
        (184_320, Act::T(TransportCommand::Stop)),
        (194_560, Act::T(TransportCommand::Play)),
    ]
}

const FRAMES: usize = 225_280;

#[test]
fn automation_renders_identically_live_and_offline() {
    let base = automated_song();
    let sc = script(&base);
    let reference = run_offline(&base, &sc, FRAMES, 1000, 0);
    assert!(peak(&reference[0]) > 0.05, "reference render is silent");
    // automation changes the sound (this is not the plain beat)
    let plain = run_offline(&beat(), &sc[..1], 61_440, 1000, 0);
    assert!(
        (0..61_440)
            .step_by(7)
            .any(|i| plain[0][i].to_bits() != reference[0][i].to_bits()),
        "the automated song sounds like the plain one"
    );
    for (block, workers) in [(64, 0), (512, 2), (37, 3)] {
        let o = run_offline(&base, &sc, FRAMES, block, workers);
        assert_bits_eq(
            &reference,
            &o,
            &format!("offline block {block} workers {workers}"),
        );
    }
    for (buffer, pull, threads) in [(256, 256, 1), (128, 512, 4), (512, 1024, 2)] {
        let s = run_sim(&base, &sc, FRAMES, buffer, pull, threads);
        assert_bits_eq(
            &reference,
            &s,
            &format!("simulated buffer {buffer} pull {pull} threads {threads}"),
        );
    }
}

#[test]
fn render_offline_of_an_automated_song_matches_simulated_playback() {
    let base = automated_song();
    let off = render_offline(&base, &song(333), &GenAssets).unwrap();
    assert!(peak(&off.channels[0]) > 0.05);
    let sc = vec![(0, Act::T(TransportCommand::Play))];
    let s = run_sim(&base, &sc, off.frames(), 256, 256, 3);
    assert_bits_eq(
        &[off.channels[0].clone(), off.channels[1].clone()],
        &s,
        "render_offline vs simulated",
    );
}

#[test]
fn the_callback_never_allocates_with_automation_while_plans_swap() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let base = automated_song();
    let ed = edits(&base);
    let mut variants: Vec<Arc<Project>> = vec![Arc::new(base.clone())];
    variants.extend(ed);
    let (engine, mut dev) = sim(128, 4);
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let stop = AtomicBool::new(false);
    let swaps = AtomicUsize::new(0);
    let pulled = AtomicUsize::new(0);
    let blocks = 3000;
    let mut l = vec![0.0f32; 128];
    let mut r = vec![0.0f32; 128];
    let mut finite = true;
    let mut loud = 0usize;
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut k = 0;
            let mut seen = 0;
            while !stop.load(Ordering::Relaxed) {
                let p = pulled.load(Ordering::Acquire);
                if p == seen {
                    std::thread::yield_now();
                    continue;
                }
                seen = p;
                engine
                    .set_project(variants[k % variants.len()].clone())
                    .unwrap();
                if k % 40 == 0 {
                    engine.transport(TransportCommand::Seek((k as Tick % 16) * PPQ));
                }
                if k % 97 == 0 {
                    engine.transport(TransportCommand::Stop);
                    engine.transport(TransportCommand::Play);
                }
                swaps.fetch_add(1, Ordering::Release);
                k += 1;
            }
        });
        for b in 0..blocks {
            let t0 = std::time::Instant::now();
            while swaps.load(Ordering::Acquire) + 1 < b
                && t0.elapsed() < std::time::Duration::from_secs(5)
            {
                std::thread::yield_now();
            }
            // `pull_into` wraps the callback in `assert_no_alloc`
            dev.pull_into(&mut l, &mut r);
            pulled.fetch_add(1, Ordering::Release);
            finite &= l.iter().chain(&r).all(|v| v.is_finite());
            if peak(&l) > 1e-3 {
                loud += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
    });
    assert!(finite, "non-finite output");
    assert!(
        loud > blocks / 4,
        "the song was silent ({loud} loud blocks)"
    );
    assert!(swaps.load(Ordering::Relaxed) > 300, "too few swaps");
}
