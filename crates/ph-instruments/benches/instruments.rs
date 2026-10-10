//! Per-voice cost at 48 kHz, 256-frame blocks (one held note).

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ph_node::*;
use std::hint::black_box;
use std::sync::Arc;

const BLOCK: usize = 256;

fn bench_voice(c: &mut Criterion, name: &str, mut p: Box<dyn Processor>, q: Quality) {
    p.activate(&ActivateInfo {
        sample_rate: 48000.0,
        max_frames: BLOCK,
        quality: q,
        seed: 1,
    });
    let mut l = vec![0.0f32; BLOCK];
    let mut r = vec![0.0f32; BLOCK];
    let mut pos = 0i64;
    let mut first = true;
    let mut g = c.benchmark_group("instrument");
    g.throughput(Throughput::Elements(BLOCK as u64));
    g.bench_function(name, |b| {
        b.iter(|| {
            let on = [Event {
                frame: 0,
                kind: EventKind::NoteOn {
                    note_id: 1,
                    key: 57,
                    velocity: 0.8,
                    pan: 0.0,
                    fine_pitch: 0.0,
                },
            }];
            let events: &[Event] = if first { &on } else { &[] };
            first = false;
            let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
            let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
            let mut io = ProcessIo {
                inputs: &[],
                outputs: &mut ports,
                events,
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

fn synth(params: &[(&str, f64)]) -> Box<dyn Processor> {
    let mut p: Box<dyn Processor> = Box::new(ph_instruments::synth::Synth::new());
    for (k, v) in params {
        let id = p.descriptor().param_id(k).unwrap();
        p.set_param(id, *v);
    }
    p
}

fn sampler() -> Box<dyn Processor> {
    let n = 48000 * 4;
    let data = Arc::new(SampleData {
        sample_rate: 44100.0,
        channels: vec![
            (0..n).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect(),
            (0..n).map(|i| ((i as f32) * 0.011).sin() * 0.5).collect(),
        ],
        loop_range: Some((1000, n - 1000)),
        root_key: None,
    });
    let mut p: Box<dyn Processor> =
        Box::new(ph_instruments::sampler::Sampler::new(Some(data), None));
    for (k, v) in [
        ("loop_mode", 1.0),
        ("filter_type", 1.0),
        ("cutoff", 4000.0),
        ("filter_env", 2.0),
    ] {
        let id = p.descriptor().param_id(k).unwrap();
        p.set_param(id, v);
    }
    p
}

fn benches(c: &mut Criterion) {
    let three_osc = [
        ("osc2_level", 0.7),
        ("osc3_level", 0.5),
        ("osc3_wave", 1.0),
        ("sub_level", 0.3),
    ];
    bench_voice(c, "synth_voice_1osc_live", synth(&[]), Quality::Live);
    bench_voice(
        c,
        "synth_voice_3osc_sub_live",
        synth(&three_osc),
        Quality::Live,
    );
    let mut full: Vec<(&str, f64)> = three_osc.to_vec();
    full.extend([
        ("f2_on", 1.0),
        ("noise_level", 0.1),
        ("mod1_src", 1.0),
        ("mod1_dst", 5.0),
        ("mod1_amt", 0.2),
    ]);
    bench_voice(
        c,
        "synth_voice_3osc_2filters_live",
        synth(&full),
        Quality::Live,
    );
    bench_voice(
        c,
        "synth_voice_3osc_2filters_high",
        synth(&full),
        Quality::High,
    );
    let mut uni = full.clone();
    uni.push(("unison_voices", 8.0));
    bench_voice(
        c,
        "synth_voice_3osc_unison8_live",
        synth(&uni),
        Quality::Live,
    );
    bench_voice(c, "sampler_voice_filtered_live", sampler(), Quality::Live);
    bench_voice(c, "sampler_voice_filtered_high", sampler(), Quality::High);
}

criterion_group!(group, benches);
criterion_main!(group);
