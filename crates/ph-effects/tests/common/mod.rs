//! Shared processor test harness (used by ph-effects and ph-instruments tests).
//!
//! Renders a `Processor` offline with absolute-frame events cut into blocks of any size,
//! generates seeded random events, and provides the three real-time contract checks every
//! processor must pass: no allocation, determinism, and block-size independence.
#![allow(dead_code)]

use ph_dsp::Rng;
use ph_node::*;

pub const SR: f64 = 48000.0;
pub const BPM: f64 = 120.0;

pub fn activate(p: &mut dyn Processor, max_frames: usize, quality: Quality, seed: u64) {
    p.activate(&ActivateInfo {
        sample_rate: SR,
        max_frames,
        quality,
        seed,
    });
}

pub fn ctx(start: usize, frames: usize) -> ProcessContext {
    ProcessContext {
        sample_rate: SR,
        frames,
        song_pos_samples: start as i64,
        song_pos_ticks: start as f64 * BPM / 60.0 * 960.0 / SR,
        bpm: BPM,
        playing: true,
        live: true,
    }
}

/// Core renderer; allocation-free given pre-sized buffers. `next_block` picks each block's
/// size; `between` runs before each block (for `set_param`/`reset` torture).
#[allow(clippy::too_many_arguments)]
pub fn render_with(
    p: &mut dyn Processor,
    frames: usize,
    next_block: &mut dyn FnMut() -> usize,
    between: &mut dyn FnMut(&mut dyn Processor, usize),
    events: &[(usize, EventKind)],
    main: Option<&[Vec<f32>]>,
    sc: Option<&[Vec<f32>]>,
    out: &mut [Vec<f32>],
    ev_buf: &mut Vec<Event>,
    zeros: &[f32],
) -> usize {
    let nin = p.descriptor().audio_inputs.len().min(2);
    let mut sleeps = 0;
    let mut ei = 0;
    let mut start = 0;
    let (ol, or) = out.split_at_mut(1);
    while start < frames {
        let n = next_block().clamp(1, zeros.len()).min(frames - start);
        between(p, start);
        ev_buf.clear();
        while ei < events.len() && events[ei].0 < start + n {
            ev_buf.push(Event {
                frame: (events[ei].0 - start) as u32,
                kind: events[ei].1,
            });
            ei += 1;
        }
        let main_ch: [&[f32]; 2] = [
            chan(main, zeros, 0, start, n),
            chan(main, zeros, 1, start, n),
        ];
        let sc_ch: [&[f32]; 2] = [chan(sc, zeros, 0, start, n), chan(sc, zeros, 1, start, n)];
        let ports: [&[&[f32]]; 2] = [&main_ch, &sc_ch];
        let mut och: [&mut [f32]; 2] = [&mut ol[0][start..start + n], &mut or[0][start..start + n]];
        let mut oports: [&mut [&mut [f32]]; 1] = [&mut och];
        let mut io = ProcessIo {
            inputs: &ports[..nin],
            outputs: &mut oports,
            events: ev_buf,
        };
        if p.process(&ctx(start, n), &mut io) == ProcessStatus::Sleep {
            sleeps += 1;
        }
        start += n;
    }
    sleeps
}

fn chan<'a>(
    src: Option<&'a [Vec<f32>]>,
    zeros: &'a [f32],
    c: usize,
    start: usize,
    n: usize,
) -> &'a [f32] {
    match src {
        Some(s) => &s[c.min(s.len() - 1)][start..start + n],
        None => &zeros[..n],
    }
}

/// Render `frames` in fixed blocks of `block`. Returns `[left, right]`.
pub fn render(
    p: &mut dyn Processor,
    frames: usize,
    block: usize,
    events: &[(usize, EventKind)],
    main: Option<&[Vec<f32>]>,
    sc: Option<&[Vec<f32>]>,
) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; frames]; 2];
    let mut ev_buf = Vec::with_capacity(events.len() + 1);
    let zeros = vec![0.0f32; block];
    render_with(
        p,
        frames,
        &mut || block,
        &mut |_, _| {},
        events,
        main,
        sc,
        &mut out,
        &mut ev_buf,
        &zeros,
    );
    out
}

/// Seeded white noise with silent gaps (so tails and sleep paths are exercised).
pub fn noise(frames: usize, seed: u64, level: f32) -> Vec<Vec<f32>> {
    let mut r = Rng::new(seed);
    (0..2)
        .map(|_| {
            (0..frames)
                .map(|i| {
                    let gap = (i / 9000) % 3 == 2;
                    if gap { 0.0 } else { r.bipolar() * level }
                })
                .collect()
        })
        .collect()
}

/// Random parameter automation (and, for instruments, notes) sorted by frame.
pub fn random_events(
    desc: &NodeDescriptor,
    frames: usize,
    seed: u64,
    param_changes: usize,
    notes: usize,
) -> Vec<(usize, EventKind)> {
    let mut r = Rng::new(seed);
    let mut ev = Vec::new();
    for _ in 0..param_changes {
        let i = r.below(desc.params.len() as u32) as usize;
        let p = &desc.params[i];
        let v = p.min + (p.max - p.min) * r.next_f64();
        ev.push((
            r.below(frames as u32) as usize,
            EventKind::Param {
                id: ParamId(i as u32),
                value: v,
            },
        ));
    }
    for k in 0..notes {
        let at = r.below(frames as u32) as usize;
        let len = 1 + r.below(24000) as usize;
        let key = 36 + r.below(60) as u8;
        ev.push((
            at,
            EventKind::NoteOn {
                note_id: k as u32,
                key,
                velocity: 0.2 + 0.8 * r.next_f32(),
                pan: r.bipolar() * 0.5,
                fine_pitch: 0.0,
            },
        ));
        if at + len < frames {
            ev.push((
                at + len,
                EventKind::NoteOff {
                    note_id: k as u32,
                    key,
                    velocity: 0.5,
                },
            ));
        }
        if r.below(40) == 0 {
            ev.push((r.below(frames as u32) as usize, EventKind::AllNotesOff));
        }
    }
    ev.sort_by_key(|e| e.0);
    ev
}

pub fn bits(x: &[Vec<f32>]) -> Vec<Vec<u32>> {
    x.iter()
        .map(|c| c.iter().map(|v| v.to_bits()).collect())
        .collect()
}

/// First index where two renders differ, for diagnostics.
pub fn first_diff(a: &[Vec<f32>], b: &[Vec<f32>]) -> Option<(usize, usize, f32, f32)> {
    for c in 0..a.len() {
        for i in 0..a[c].len() {
            if a[c][i].to_bits() != b[c][i].to_bits() {
                return Some((c, i, a[c][i], b[c][i]));
            }
        }
    }
    None
}

/// Block 64 ≡ block 512 ≡ block 37, bit for bit, with random notes and automation.
pub fn assert_block_independent(
    make: &dyn Fn() -> Box<dyn Processor>,
    input: bool,
    notes: usize,
    quality: Quality,
) {
    let frames = 48000;
    let desc = make().descriptor().clone();
    let ev = random_events(&desc, frames, 11, 60, notes);
    let inp = noise(frames, 5, 0.5);
    let main = input.then_some(&inp[..]);
    let mut outs = Vec::new();
    for block in [64, 512, 37] {
        let mut p = make();
        activate(p.as_mut(), 512, quality, 3);
        outs.push(render(p.as_mut(), frames, block, &ev, main, main));
    }
    assert!(
        outs[0].iter().flatten().any(|v| *v != 0.0),
        "{}: render is silent",
        desc.id
    );
    for k in 1..outs.len() {
        assert!(
            bits(&outs[0]) == bits(&outs[k]),
            "{}: block size changes output; first diff {:?}",
            desc.id,
            first_diff(&outs[0], &outs[k])
        );
    }
}

/// Two fresh instances with the same seed render identical bits.
pub fn assert_deterministic(make: &dyn Fn() -> Box<dyn Processor>, input: bool, notes: usize) {
    let frames = 24000;
    let desc = make().descriptor().clone();
    let ev = random_events(&desc, frames, 21, 30, notes);
    let inp = noise(frames, 9, 0.5);
    let main = input.then_some(&inp[..]);
    let mut outs = Vec::new();
    for _ in 0..2 {
        let mut p = make();
        activate(p.as_mut(), 256, Quality::Live, 42);
        outs.push(render(p.as_mut(), frames, 256, &ev, main, main));
    }
    assert!(
        bits(&outs[0]) == bits(&outs[1]),
        "{} is not deterministic",
        desc.id
    );
}

/// Thousands of random-size blocks with random notes, automation, `set_param` and
/// `reset` calls, all under `assert_no_alloc`. The test binary must install
/// `assert_no_alloc::AllocDisabler` as its global allocator.
pub fn stress_no_alloc(make: &dyn Fn() -> Box<dyn Processor>, input: bool, notes: usize) {
    for quality in [Quality::Live, Quality::High] {
        let frames = 48000 * 12;
        let mut p = make();
        activate(p.as_mut(), 512, quality, 7);
        let desc = p.descriptor().clone();
        let ev = random_events(&desc, frames, 99, frames / 300, notes);
        let inp = noise(frames, 1, 0.8);
        let main = input.then_some(&inp[..]);
        let mut out = vec![vec![0.0f32; frames]; 2];
        let mut ev_buf = Vec::with_capacity(ev.len() + 1);
        let zeros = vec![0.0f32; 512];
        let mut r1 = Rng::new(5);
        let mut r2 = Rng::new(6);
        let ranges: Vec<(f64, f64)> = desc.params.iter().map(|p| (p.min, p.max)).collect();
        let mut blocks = 0usize;
        assert_no_alloc::assert_no_alloc(|| {
            render_with(
                p.as_mut(),
                frames,
                &mut || {
                    blocks += 1;
                    1 + r1.below(512) as usize
                },
                &mut |p, _| {
                    if r2.below(8) == 0 {
                        let i = r2.below(ranges.len() as u32) as usize;
                        let (lo, hi) = ranges[i];
                        // include out-of-range values: set_param must clamp
                        let v = lo - 0.5 * (hi - lo) + 2.0 * (hi - lo) * r2.next_f64();
                        p.set_param(ParamId(i as u32), v);
                        let _ = p.param(ParamId(i as u32));
                    }
                    if r2.below(500) == 0 {
                        p.reset();
                    }
                    let _ = (p.latency(), p.tail());
                },
                &ev,
                main,
                main,
                &mut out,
                &mut ev_buf,
                &zeros,
            );
        });
        assert!(blocks > 2000, "only {blocks} blocks");
        assert!(
            out.iter().flatten().all(|v| v.is_finite()),
            "{}: non-finite output",
            desc.id
        );
    }
}

/// Feed a loud signal, then a long silence. The decaying tail must never produce a subnormal
/// sample (subnormals are 10–100× slower on x86 and mean state is not being flushed).
///
/// That check is deterministic. The wall-clock comparison (silence must not cost more than
/// 2× the loud part) is noisy on shared CI runners, so it only runs when
/// `PH_TIMING_TESTS=1` is set, e.g. locally before a DSP change is merged.
pub fn assert_no_denormal_slowdown(make: &dyn Fn() -> Box<dyn Processor>, notes: bool) {
    use std::time::Instant;
    let block = 256;
    let loud_frames = 48000;
    let quiet_frames = 48000 * 4;
    let mut p = make();
    activate(p.as_mut(), block, Quality::Live, 1);
    let mut ev = Vec::new();
    if notes {
        for k in 0..8u32 {
            ev.push((
                k as usize * 100,
                EventKind::NoteOn {
                    note_id: k,
                    key: 48 + 3 * k as u8,
                    velocity: 1.0,
                    pan: 0.0,
                    fine_pitch: 0.0,
                },
            ));
        }
        for k in 0..8u32 {
            ev.push((
                30000 + k as usize,
                EventKind::NoteOff {
                    note_id: k,
                    key: 48 + 3 * k as u8,
                    velocity: 0.0,
                },
            ));
        }
    }
    let loud = noise(loud_frames, 3, 1.0);
    let quiet = vec![vec![0.0f32; quiet_frames]; 2];
    let t0 = Instant::now();
    let _ = render(p.as_mut(), loud_frames, block, &ev, Some(&loud), None);
    let loud_per_sample = t0.elapsed().as_secs_f64() / loud_frames as f64;
    // skip the first second of the tail (still audible), time the long decay
    let first_tail = render(p.as_mut(), 48000, block, &[], Some(&quiet), None);
    let t1 = Instant::now();
    let long_tail = render(
        p.as_mut(),
        quiet_frames - 48000,
        block,
        &[],
        Some(&quiet),
        None,
    );
    let quiet_per_sample = t1.elapsed().as_secs_f64() / (quiet_frames - 48000) as f64;
    let id = &p.descriptor().id;
    for (c, ch) in first_tail.iter().chain(long_tail.iter()).enumerate() {
        if let Some(i) = ch.iter().position(|x| x.is_subnormal()) {
            panic!(
                "{id}: subnormal sample {:e} in the tail (channel {c}, frame {i})",
                ch[i]
            );
        }
    }
    if std::env::var("PH_TIMING_TESTS").as_deref() == Ok("1") {
        let ratio = quiet_per_sample / loud_per_sample;
        assert!(
            ratio < 2.0,
            "{id}: silence costs {ratio:.2}× the loud signal (denormals?)"
        );
    }
}
