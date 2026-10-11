//! The whole audio callback runs under `assert_no_alloc` (the test allocator aborts on any
//! allocation inside it, on the audio thread and on the workers) while a second thread
//! swaps in new plans as fast as it can: notes, channels and effects come and go.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Variant `k` of the beat: different notes, channel count and effect chains.
fn variant(base: &Project, k: usize) -> Project {
    let mut e = Edit::new(base);
    let drums = base.pattern_order[0];
    let music = base.pattern_order[1];
    let chans: Vec<ChannelId> = base.channel_order.iter().copied().collect();
    let mut r = Rng::new(k as u64 + 1);
    for _ in 0..(1 + r.below(6)) {
        let ch = chans[r.below(chans.len() as u32) as usize];
        let start = r.below(16) as Tick * STEP;
        e.add_note(
            if r.below(2) == 0 { drums } else { music },
            ch,
            NoteSpec::new(
                36 + r.below(48) as u8,
                start,
                STEP * (1 + r.below(8) as Tick),
            ),
        )
        .unwrap();
    }
    if k % 3 == 1 {
        let c = e
            .add_channel("Extra", synth(&[("osc1_wave", 3.0)]))
            .unwrap();
        e.update_channel(c, |ch| ch.insert = InsertId(5)).unwrap();
        e.add_note(music, c, NoteSpec::new(55, 0, PPQ)).unwrap();
        e.update_insert(InsertId(5), |i| {
            i.effects
                .push(fx("filter", &[("cutoff", 500.0 + k as f64)]));
        })
        .unwrap();
    }
    if k % 4 == 2 {
        e.remove_channel(chans[2]).unwrap();
        e.update_insert(InsertId(1), |i| {
            i.effects.clear();
        })
        .unwrap();
    }
    if k % 5 == 3 {
        e.update_insert(InsertId(2), |i| {
            i.effects.insert(0, fx("limiter", &[]));
            i.effects.push(fx("eq", &[("b4_gain", -3.0)]));
        })
        .unwrap();
    }
    e.update_channel(chans[3], |c| c.volume = 0.3 + (k % 7) as f32 * 0.1)
        .unwrap();
    e.finish().0
}

#[test]
fn callback_never_allocates_while_plans_swap_every_block() {
    let base = beat();
    let variants: Vec<Arc<Project>> = (0..24).map(|k| Arc::new(variant(&base, k))).collect();
    let (engine, mut dev) = sim(128, 4);
    engine.set_project(Arc::new(base.clone())).unwrap();
    engine.transport(TransportCommand::Play);
    let stop = AtomicBool::new(false);
    let swaps = AtomicUsize::new(0);
    // Loose lockstep between the swapper and the device: the swapper posts one plan per pulled
    // block, and the device never runs more than one block ahead of it. So a swap is always in
    // flight while the next callback runs (the race under test), but the number of swaps, seeks
    // and previews per block no longer depends on how this machine schedules threads. Unpaced,
    // a slow Linux runner seeked many times per block (every seek silences all voices) and a
    // slow Windows runner starved the swapper below 300 swaps.
    let pulled = AtomicUsize::new(0);
    let blocks = 4000;
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
                if k % 50 == 0 {
                    engine.transport(TransportCommand::Seek((k as Tick % 16) * PPQ));
                }
                if k % 7 == 0 {
                    engine.preview_note(base.channel_order[3], 48, 0.8, k % 14 == 0);
                }
                swaps.fetch_add(1, Ordering::Release);
                k += 1;
            }
        });
        for b in 0..blocks {
            // stay at most one block ahead of the swapper (bounded wait so a bug cannot hang CI)
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
    assert!(loud > blocks / 4, "mostly silent ({loud} loud blocks)");
    let n = swaps.load(Ordering::Relaxed);
    assert!(n >= 300, "only {n} plan swaps posted");
    // the engine kept reporting
    let st = engine.stats();
    assert!(st.dsp_load > 0.0);
    assert!(!st.nodes.is_empty());
}

#[test]
fn swap_on_every_single_block_single_threaded() {
    // deterministic variant: one new plan before every pull
    let base = beat();
    let variants: Vec<Arc<Project>> = (0..8).map(|k| Arc::new(variant(&base, k))).collect();
    let (engine, mut dev) = sim(64, 1);
    engine.transport(TransportCommand::Play);
    let mut l = vec![0.0f32; 64];
    let mut r = vec![0.0f32; 64];
    for k in 0..600 {
        engine
            .set_project(variants[k % variants.len()].clone())
            .unwrap();
        dev.pull_into(&mut l, &mut r);
    }
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
}
