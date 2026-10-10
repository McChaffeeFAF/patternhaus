//! M1 acceptance criterion 7: the reference project at 48 kHz / 256 frames.
//!
//! 16 Haus Synth channels, each holding an 8-voice chord (128 voices), two channels per
//! insert on 8 inserts with one effect each (EQ, compressor, reverb, delay, filter,
//! saturator, limiter, EQ). The engine runs on the simulated device, so the figure is the
//! engine's own processing time ÷ buffer duration (the DSP load meter).
//!
//! `cargo bench -p ph-engine` prints the DSP load (all cores, and one thread), then runs
//! the criterion benchmarks.

use criterion::{Criterion, criterion_group, criterion_main};
use ph_core::*;
use ph_engine::{Engine, EngineConfig, TransportCommand};
use ph_node::{AssetResolver, SampleData};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

const SR: u32 = 48_000;
const BUFFER: u32 = 256;

struct NoAssets;
impl AssetResolver for NoAssets {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        Err(format!("no sample `{r}`"))
    }
}

fn reference_project() -> Project {
    let mut e = Edit::new(&Project::empty("reference"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let pat = e.add_pattern("Chords").unwrap();
    let chords: [[u8; 8]; 4] = [
        [48, 52, 55, 59, 62, 64, 67, 71],
        [45, 48, 52, 55, 59, 60, 64, 67],
        [41, 45, 48, 52, 55, 57, 60, 64],
        [43, 47, 50, 53, 57, 59, 62, 65],
    ];
    for c in 0..16u32 {
        let params: BTreeMap<String, f64> = [
            ("osc1_wave", (c % 3) as f64),
            ("osc2_level", 0.5),
            ("osc2_fine", 7.0),
            ("f1_cutoff", 1500.0 + 300.0 * c as f64),
            ("amp_release", 0.3),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect();
        let ch = e
            .add_channel(
                &format!("Synth {c}"),
                InstrumentSpec::Synth {
                    preset: None,
                    params,
                },
            )
            .unwrap();
        e.update_channel(ch, |x| {
            x.insert = InsertId(1 + c / 2);
            x.volume = 0.3;
        })
        .unwrap();
        for (bar, chord) in chords.iter().enumerate() {
            for k in chord {
                e.add_note(pat, ch, NoteSpec::new(*k, bar as Tick * 4 * PPQ, 4 * PPQ))
                    .unwrap();
            }
        }
    }
    let fx = [
        "eq",
        "compressor",
        "reverb",
        "delay",
        "filter",
        "saturator",
        "limiter",
        "eq",
    ];
    for (i, name) in fx.iter().enumerate() {
        e.update_insert(InsertId(1 + i as u32), |ins| {
            ins.effects.push(EffectSlot {
                effect: EffectSpec::Builtin {
                    name: name.to_string(),
                    params: BTreeMap::new(),
                },
                bypass: false,
                mix: 1.0,
            })
        })
        .unwrap();
    }
    e.add_clip(0, pat, 0).unwrap();
    e.finish().0
}

fn engine(threads: usize) -> (Engine, ph_engine::SimulatedDevice) {
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: BUFFER,
            threads,
            ..EngineConfig::default()
        },
        Arc::new(NoAssets),
    )
    .unwrap();
    engine.set_project(Arc::new(reference_project())).unwrap();
    engine.transport(TransportCommand::Play);
    // warm up: voices sounding, caches hot
    for _ in 0..200 {
        dev.pull(BUFFER as usize);
    }
    (engine, dev)
}

/// Mean and worst DSP load over `blocks` callbacks.
fn measure(threads: usize, blocks: usize) -> (f64, f64) {
    let (_engine, mut dev) = engine(threads);
    let budget = BUFFER as f64 / SR as f64;
    let mut total = 0.0;
    let mut worst: f64 = 0.0;
    for _ in 0..blocks {
        let t = Instant::now();
        let b = dev.pull(BUFFER as usize);
        let dt = t.elapsed().as_secs_f64();
        std::hint::black_box(&b);
        total += dt;
        worst = worst.max(dt);
    }
    (total / blocks as f64 / budget, worst / budget)
}

fn bench(c: &mut Criterion) {
    for (label, threads) in [("all cores", 0usize), ("one thread", 1)] {
        let (mean, worst) = measure(threads, 2000);
        println!(
            "reference project, 48 kHz / {BUFFER} frames, {label}: DSP load {:.1}% mean, {:.1}% worst block",
            mean * 100.0,
            worst * 100.0
        );
    }
    let mut g = c.benchmark_group("reference_project_block_256");
    for (label, threads) in [("all_cores", 0usize), ("one_thread", 1)] {
        let (_engine, mut dev) = engine(threads);
        g.bench_function(label, |b| b.iter(|| dev.pull(BUFFER as usize)));
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
