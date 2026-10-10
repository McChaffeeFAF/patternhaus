//! DESIGN §12 oracle: simulated real-time output is bit-identical to the offline render,
//! across device buffer sizes, pull sizes, offline block sizes and thread counts, with
//! transport changes and plan swaps mid-render.

use super::*;
use crate::{PlayMode, RenderSettings, render_offline};

fn edits(base: &Project) -> Vec<Arc<Project>> {
    let mut out = Vec::new();
    // 1: param-only change on a reused synth + an effect param + a new note
    let mut e = Edit::new(base);
    let bass = base.channel_order[3];
    e.update_channel(bass, |c| {
        if let InstrumentSpec::Synth { params, .. } = &mut c.instrument {
            params.insert("f1_cutoff".into(), 2500.0);
        }
        c.volume = 0.6;
    })
    .unwrap();
    e.update_insert(InsertId(4), |i| {
        if let EffectSpec::Builtin { params, .. } = &mut i.effects[0].effect {
            params.insert("decay".into(), 3.0);
        }
    })
    .unwrap();
    let music = base.pattern_order[1];
    e.add_note(music, bass, NoteSpec::new(48, 3 * PPQ + STEP, STEP))
        .unwrap();
    out.push(Arc::new(e.finish().0));
    // 2: add a channel and an effect, remove a note
    let mut e = Edit::new(out.last().unwrap());
    let lead = e
        .add_channel("Lead", synth(&[("osc1_wave", 2.0), ("amp_release", 0.2)]))
        .unwrap();
    e.update_channel(lead, |c| c.insert = InsertId(2)).unwrap();
    e.add_note(music, lead, NoteSpec::new(72, 0, PPQ * 3))
        .unwrap();
    e.update_insert(InsertId(2), |i| {
        i.effects.push(fx("delay", &[("mix", 0.4)]))
    })
    .unwrap();
    let drums = base.pattern_order[0];
    let kick = base.channel_order[0];
    let first = *base.patterns[&drums].notes[&kick].keys().next().unwrap();
    e.remove_note(drums, kick, first).unwrap();
    out.push(Arc::new(e.finish().0));
    // 3: remove an effect and a channel, mute a channel, change the instrument kind
    let mut e = Edit::new(out.last().unwrap());
    e.update_insert(InsertId(1), |i| {
        i.effects.remove(0);
    })
    .unwrap();
    e.remove_channel(base.channel_order[2]).unwrap();
    e.update_channel(base.channel_order[1], |c| c.muted = true)
        .unwrap();
    e.update_channel(base.channel_order[4], |c| {
        c.instrument = sampler("gen:kick", &[]);
    })
    .unwrap();
    out.push(Arc::new(e.finish().0));
    out
}

/// Song playback with swaps, a seek, a loop region, mode changes and a stop.
fn script(base: &Project) -> Vec<(usize, Act)> {
    let ed = edits(base);
    let pat = base.pattern_order[1];
    vec![
        (0, Act::T(TransportCommand::Play)),
        (12_288, Act::P(ed[0].clone())),
        (24_576, Act::T(TransportCommand::Seek(3 * 4 * PPQ))),
        (36_864, Act::P(ed[1].clone())),
        (
            40_960,
            Act::T(TransportCommand::SetLoop(Some((4 * PPQ, 8 * PPQ)))),
        ),
        (61_440, Act::P(ed[2].clone())),
        (
            73_728,
            Act::T(TransportCommand::SetMode(PlayMode::Pattern(pat))),
        ),
        (77_824, Act::T(TransportCommand::Play)),
        (98_304, Act::T(TransportCommand::Stop)),
        (102_400, Act::T(TransportCommand::Play)),
        (106_496, Act::T(TransportCommand::SetMetronome(true))),
    ]
}

const FRAMES: usize = 122_880;

#[test]
fn simulated_matches_offline_across_buffers_blocks_and_threads() {
    let base = beat();
    let sc = script(&base);
    let reference = run_offline(&base, &sc, FRAMES, 1000, 0);
    assert!(peak(&reference[0]) > 0.05, "reference render is silent");
    // other offline block sizes and thread counts
    for (block, workers) in [(64, 0), (37, 3), (4096, 1)] {
        let o = run_offline(&base, &sc, FRAMES, block, workers);
        assert_bits_eq(
            &reference,
            &o,
            &format!("offline block {block} workers {workers}"),
        );
    }
    // simulated device: buffer sizes, pull sizes, thread counts
    for (buffer, pull, threads) in [(256, 256, 1), (256, 1024, 4), (128, 512, 2), (512, 128, 8)] {
        let s = run_sim(&base, &sc, FRAMES, buffer, pull, threads);
        assert_bits_eq(
            &reference,
            &s,
            &format!("simulated buffer {buffer} pull {pull} threads {threads}"),
        );
    }
}

#[test]
fn multithreaded_pool_actually_runs_workers() {
    let (_e, dev) = sim(256, 4);
    assert_eq!(dev.workers(), 3);
    let (_e, dev) = sim(256, 1);
    assert_eq!(dev.workers(), 0);
}

#[test]
fn render_offline_matches_simulated_playback() {
    let base = beat();
    let rs = RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 333,
    };
    let off = render_offline(&base, &rs, &GenAssets).unwrap();
    let len = off.frames();
    assert_eq!(
        len as i64,
        crate::tempo::TempoMap::constant(128.0, SR).sample_at(base.song_length())
    );
    let sc = vec![
        (0, Act::T(TransportCommand::Seek(0))),
        (0, Act::T(TransportCommand::Play)),
    ];
    let s = run_sim(&base, &sc, len, 256, 256, 2);
    let o = [off.channels[0].clone(), off.channels[1].clone()];
    assert_bits_eq(&o, &s, "render_offline vs simulated");
}

#[test]
fn block_size_and_threads_do_not_change_render_offline() {
    let base = beat();
    let mut outs = Vec::new();
    for (block, threads) in [(64usize, 1usize), (512, 4), (1031, 2)] {
        let rs = RenderSettings {
            mode: PlayMode::Song,
            range: Some((PPQ, 9 * PPQ)),
            sample_rate: 44_100,
            quality: Quality::High,
            include_tail: true,
            block_frames: block,
        };
        outs.push(crate::offline::render(&base, &rs, &GenAssets, threads - 1).unwrap());
    }
    for o in &outs[1..] {
        assert_eq!(
            o.frames(),
            outs[0].frames(),
            "tail length depends on block size"
        );
        assert_bits_eq(
            &[outs[0].channels[0].clone(), outs[0].channels[1].clone()],
            &[o.channels[0].clone(), o.channels[1].clone()],
            "render_offline block size",
        );
    }
}

#[test]
fn same_project_same_output_across_runs() {
    let base = beat();
    let sc = vec![(0, Act::T(TransportCommand::Play))];
    let a = run_sim(&base, &sc, 48_000, 256, 256, 4);
    let b = run_sim(&base, &sc, 48_000, 256, 256, 4);
    assert_bits_eq(&a, &b, "two runs");
}
