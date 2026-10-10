//! `Engine::audition`: the browser's preview voice, outside the project graph.

use super::*;
use crate::{PlayMode, RenderSettings, render_offline};

/// Pulls `frames` frames in device-sized blocks, calling `before(block_index, engine)`
/// before each pull. Every pull runs under `assert_no_alloc` (`pull_into`).
fn drive(
    engine: &Engine,
    dev: &mut SimulatedDevice,
    frames: usize,
    block: usize,
    mut before: impl FnMut(usize, &Engine),
) -> [Vec<f32>; 2] {
    let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut at = 0;
    let mut k = 0;
    while at < frames {
        before(k, engine);
        let n = block.min(frames - at);
        let [l, r] = &mut out;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
        k += 1;
    }
    out
}

#[test]
fn audition_plays_outside_the_project_at_the_preview_level() {
    // empty project, transport stopped: the output is the preview voice alone
    let (engine, mut dev) = sim(256, 1);
    engine.audition("gen:dc", 1.0).unwrap();
    let out = drive(&engine, &mut dev, 4096, 256, |_, _| {});
    // gen:dc is 0.5 for one second; the preview level is 0.5 at velocity 1
    assert!((out[0][100] - 0.25).abs() < 1e-6, "{}", out[0][100]);
    assert!((out[1][4000] - 0.25).abs() < 1e-6);
    // the mixer meters never see it
    let m = engine.meters();
    assert!(m.peak.iter().all(|p| p[0] == 0.0 && p[1] == 0.0), "{m:?}");
    // velocity scales it
    engine.audition("gen:dc", 0.5).unwrap();
    let out = drive(&engine, &mut dev, 2048, 256, |_, _| {});
    // the old voice fades over 5 ms; after that only the new one is heard
    assert!((out[0][1024] - 0.125).abs() < 1e-6, "{}", out[0][1024]);
    assert!(matches!(
        engine.audition("gen:missing", 1.0),
        Err(crate::EngineError::Asset(_))
    ));
}

#[test]
fn stop_audition_fades_out_quickly() {
    let (engine, mut dev) = sim(256, 1);
    engine.audition("gen:dc", 1.0).unwrap();
    let _ = drive(&engine, &mut dev, 1024, 256, |_, _| {});
    engine.stop_audition();
    let out = drive(&engine, &mut dev, 1024, 256, |_, _| {});
    // 5 ms fade at 48 kHz = 240 frames, monotonic, then silence
    assert!(out[0][0] > 0.2 && out[0][0] <= 0.25);
    assert!(out[0][100] < out[0][0]);
    assert!(out[0][300..].iter().all(|v| *v == 0.0));
}

#[test]
fn audition_never_allocates_on_the_audio_thread() {
    // the test allocator aborts on any allocation inside `pull_into` (audio thread and
    // workers); auditions are posted, replaced, stopped and collected between pulls while
    // the beat plays on four threads with plan swaps
    let base = beat();
    let (engine, mut dev) = sim(128, 4);
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let samples = ["gen:kick", "gen:snare", "gen:hat", "gen:dc"];
    let out = drive(&engine, &mut dev, 48_000, 128, |k, e| {
        if k % 7 == 0 {
            e.audition(samples[(k / 7) % samples.len()], 0.8).unwrap();
        }
        if k % 11 == 5 {
            e.stop_audition();
        }
        if k % 13 == 0 {
            let _ = e.stats(); // collects voices handed back by the audio thread
        }
        if k % 50 == 25 {
            let mut p = base.clone();
            p.settings.swing = (k % 100) as f32 / 100.0;
            e.set_project(Arc::new(p)).unwrap();
        }
    });
    assert!(peak(&out[0]) > 0.0);
}

#[test]
fn audition_mixes_in_without_disturbing_the_project() {
    let base = beat();
    let frames = 48_000;
    // project alone, through the simulated device
    let (engine, mut dev) = sim(256, 2);
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let project_only = drive(&engine, &mut dev, frames, 256, |_, _| {});
    // the same with auditions during playback
    let (engine, mut dev) = sim(256, 2);
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let with = drive(&engine, &mut dev, frames, 256, |k, e| {
        if k == 10 {
            e.audition("gen:snare", 1.0).unwrap();
        }
        if k == 60 {
            e.audition("gen:kick", 0.7).unwrap();
        }
    });
    // the preview voice alone, on an empty engine with the same timing
    let (engine, mut dev) = sim(256, 1);
    let voice = drive(&engine, &mut dev, frames, 256, |k, e| {
        if k == 10 {
            e.audition("gen:snare", 1.0).unwrap();
        }
        if k == 60 {
            e.audition("gen:kick", 0.7).unwrap();
        }
    });
    assert!(peak(&voice[0]) > 0.1, "the audition is audible");
    for c in 0..2 {
        for i in 0..frames {
            let expect = project_only[c][i] + voice[c][i];
            assert!(
                (with[c][i] - expect).abs() <= 1e-6,
                "channel {c} frame {i}: {} vs {expect}",
                with[c][i]
            );
        }
    }
    // before the first audition the output is the project, bit for bit
    assert_eq!(
        with[0][..2560]
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        project_only[0][..2560]
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn audition_never_reaches_offline_renders_or_exports() {
    let base = beat();
    let settings = RenderSettings {
        mode: PlayMode::Song,
        range: Some((0, 4 * PPQ * 4)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    let before = render_offline(&base, &settings, &GenAssets).unwrap();
    // a live engine auditioning while the offline render (the export path) runs
    let (engine, mut dev) = sim(256, 1);
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.audition("gen:dc", 1.0).unwrap();
    let _ = drive(&engine, &mut dev, 2048, 256, |_, _| {});
    let during = render_offline(&base, &settings, &GenAssets).unwrap();
    let _ = drive(&engine, &mut dev, 2048, 256, |_, _| {});
    assert_eq!(before, during, "an audition changed the offline render");
}

#[test]
fn audition_instrument_renders_a_preset_note_off_thread() {
    let (engine, mut dev) = sim(256, 1);
    let before = engine.status();
    engine
        .audition_instrument(&synth(&[]), 60, 1.0, 0.3)
        .unwrap();
    let out = drive(&engine, &mut dev, 24_000, 256, |_, _| {});
    assert!(peak(&out[0]) > 0.01, "the synth note is heard");
    assert_eq!(
        engine.status(),
        before,
        "the transport and project are untouched"
    );
}
