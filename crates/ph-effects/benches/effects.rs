//! Per-effect cost at 48 kHz, stereo, 256-frame blocks of noise.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ph_node::*;
use std::hint::black_box;

const BLOCK: usize = 256;

fn bench_effect(c: &mut Criterion, name: &str, q: Quality) {
    let mut p = ph_effects::builtin(name).unwrap();
    p.activate(&ActivateInfo {
        sample_rate: 48000.0,
        max_frames: BLOCK,
        quality: q,
        seed: 1,
    });
    let mut rng = ph_dsp::Rng::new(1);
    let il: Vec<f32> = (0..BLOCK).map(|_| rng.bipolar() * 0.5).collect();
    let ir: Vec<f32> = (0..BLOCK).map(|_| rng.bipolar() * 0.5).collect();
    let mut l = vec![0.0f32; BLOCK];
    let mut r = vec![0.0f32; BLOCK];
    let mut pos = 0i64;
    let label = format!(
        "{name}_{}",
        if q == Quality::Live { "live" } else { "high" }
    );
    let mut g = c.benchmark_group("effect");
    g.throughput(Throughput::Elements(BLOCK as u64));
    g.bench_function(&label, |b| {
        b.iter(|| {
            let ins: [&[f32]; 2] = [&il, &ir];
            let in_ports: [&[&[f32]]; 2] = [&ins, &ins];
            let n_in = p.descriptor().audio_inputs.len();
            let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
            let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
            let mut io = ProcessIo {
                inputs: &in_ports[..n_in],
                outputs: &mut ports,
                events: &[],
            };
            let ctx = ProcessContext {
                sample_rate: 48000.0,
                frames: BLOCK,
                song_pos_samples: pos,
                song_pos_ticks: pos as f64 * 0.04,
                bpm: 120.0,
                playing: true,
                live: true,
            };
            pos += BLOCK as i64;
            black_box(p.process(&ctx, &mut io));
        })
    });
    g.finish();
}

fn benches(c: &mut Criterion) {
    for name in ph_effects::BUILTIN {
        bench_effect(c, name, Quality::Live);
    }
    bench_effect(c, "filter", Quality::High);
    bench_effect(c, "saturator", Quality::High);
}

criterion_group!(group, benches);
criterion_main!(group);
