//! M1 acceptance criterion 7: the reference project at 48 kHz / 256 frames.
//!
//! 16 Haus Synth channels, each holding an 8-voice chord (128 voices), two channels per
//! insert on 8 inserts with one effect each (EQ, compressor, reverb, delay, filter,
//! saturator, limiter, EQ). The engine runs on the simulated device, so the figure is the
//! engine's own processing time ÷ buffer duration (the DSP load meter).
//!
//! M2 criterion 7 (audio part, M2/T1): the same project plus 8 tempo-synced audio clips in
//! Stretch mode on 8 playlist tracks, at time ratios from 0.75 to 1.33, playing all the way
//! through; the slower half of the sources are long enough (over 10 s) to stream from
//! "disk" through the prefetch thread.
//!
//! M2 criterion 7 (automation part, M2/T7): that project plus 4 automation clips (an
//! instrument parameter sweep, an effect parameter, a channel pan and a mixer volume ride,
//! all with curves) playing the whole time.
//!
//! `cargo bench -p ph-engine` prints the DSP load (all cores, and one thread), then runs
//! the criterion benchmarks.

use criterion::{Criterion, criterion_group, criterion_main};
use ph_core::*;
use ph_engine::{Engine, EngineConfig, TransportCommand};
use ph_node::{AssetResolver, SampleData, SampleStream, StreamError};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

const SR: u32 = 48_000;
const BUFFER: u32 = 256;

/// Sample BPMs of the 8 audio clips' sources (the project runs at 120).
const LOOP_BPMS: [f64; 8] = [90.0, 96.0, 100.0, 108.0, 112.0, 128.0, 140.0, 160.0];

/// Generated loop `gen:loop/<bpm>`: 16 beats at that tempo, stereo, a chord with a decaying
/// noise burst on every beat. Pure function of the frame, so it can also be streamed.
fn loop_frame(bpm: f64, ch: usize, i: u64) -> f32 {
    let t = i as f64 / SR as f64;
    let beat = (t * bpm / 60.0).fract() * 60.0 / bpm;
    let tau = std::f64::consts::TAU;
    let detune = if ch == 0 { 1.0 } else { 1.003 };
    let chord: f64 = [220.0, 277.18, 329.63]
        .iter()
        .map(|f| (tau * f * t * detune).sin())
        .sum();
    let h = (i.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as f64 / (1u64 << 24) as f64 - 0.5;
    (0.1 * chord + 0.6 * h * (-beat * 30.0).exp()) as f32
}

fn loop_bpm(r: &str) -> Option<f64> {
    r.strip_prefix("gen:loop/")?.parse().ok()
}

fn loop_frames(bpm: f64) -> u64 {
    (16.0 * 60.0 / bpm * SR as f64).round() as u64
}

struct BenchAssets;
impl AssetResolver for BenchAssets {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        let bpm = loop_bpm(r).ok_or_else(|| format!("no sample `{r}`"))?;
        let n = loop_frames(bpm);
        Ok(Arc::new(SampleData {
            sample_rate: SR as f64,
            channels: (0..2)
                .map(|c| (0..n).map(|i| loop_frame(bpm, c, i)).collect())
                .collect(),
            loop_range: None,
            root_key: None,
        }))
    }
    fn stream(&self, r: &str) -> Option<Result<Box<dyn SampleStream>, String>> {
        let bpm = loop_bpm(r)?;
        Some(Ok(Box::new(LoopStream(bpm))))
    }
}

struct LoopStream(f64);
impl SampleStream for LoopStream {
    fn frames(&self) -> u64 {
        loop_frames(self.0)
    }
    fn sample_rate(&self) -> f64 {
        SR as f64
    }
    fn channels(&self) -> usize {
        2
    }
    fn read(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, StreamError> {
        let n = out.iter().map(|c| c.len()).min().unwrap_or(0);
        let frames = self.frames();
        for (c, o) in out.iter_mut().enumerate() {
            for (k, v) in o.iter_mut().enumerate().take(n) {
                let i = start + k as u64;
                *v = if i < frames {
                    loop_frame(self.0, c, i)
                } else {
                    0.0
                };
            }
        }
        Ok(n.min(frames.saturating_sub(start) as usize))
    }
}

/// The reference project plus 8 stretched, tempo-synced audio clips (M2 criterion 7).
fn audio_project() -> Project {
    let mut e = Edit::new(&reference_project());
    for (k, bpm) in LOOP_BPMS.iter().enumerate() {
        let s = e
            .add_sample(SampleEntry {
                id: SampleId(0),
                path: format!("gen:loop/{bpm}"),
                name: format!("loop {bpm}"),
                frames: loop_frames(*bpm),
                sample_rate: SR,
                channels: 2,
                bpm: Some(SampleBpm {
                    value: *bpm,
                    source: BpmSource::User,
                }),
                root_key: None,
                analysis: Default::default(),
                provenance: Default::default(),
            })
            .unwrap();
        let mut a = AudioClip::new(s);
        a.tempo_sync = true;
        a.stretch = StretchMode::Stretch;
        a.gain_db = -12.0;
        e.add_audio_clip(1 + k as u32, 0, 16 * PPQ, a).unwrap();
    }
    e.finish().0
}

/// [`audio_project`] plus 4 automation clips over the whole song: a synth cutoff sweep
/// (smooth), a reverb parameter (S-curve), a channel pan and an insert volume ride (linear).
fn automated_project() -> Project {
    let base = audio_project();
    let mut e = Edit::new(&base);
    let ch = |i: usize| base.channel_order[i];
    let len = 16 * PPQ;
    let tracks = 20u32;
    let c = e
        .add_automation_clip(
            tracks,
            0,
            len,
            ParamPath::ChannelParam(ch(0), "f1_cutoff".into()),
        )
        .unwrap();
    for (k, v) in [
        (0, 800.0),
        (4, 6000.0),
        (8, 1200.0),
        (12, 9000.0),
        (16, 1500.0),
    ] {
        e.add_point(c, k * PPQ, v, CurveKind::Smooth, 0.0).unwrap();
    }
    let c = e
        .add_automation_clip(
            tracks + 1,
            0,
            len,
            ParamPath::EffectParam {
                insert: InsertId(3),
                slot: 0,
                param: "decay".into(),
            },
        )
        .unwrap();
    for (k, v) in [(0, 1.0), (8, 4.0), (16, 1.5)] {
        e.add_point(c, k * PPQ, v, CurveKind::SCurve, 0.0).unwrap();
    }
    let c = e
        .add_automation_clip(tracks + 2, 0, len, ParamPath::ChannelPan(ch(5)))
        .unwrap();
    for (k, v) in [(0, -0.8), (8, 0.8), (16, -0.8)] {
        e.add_point(c, k * PPQ, v, CurveKind::Linear, 0.0).unwrap();
    }
    let c = e
        .add_automation_clip(tracks + 3, 0, len, ParamPath::InsertVolume(InsertId(2)))
        .unwrap();
    for (k, v) in [(0, 0.8), (4, 0.4), (8, 0.8), (12, 0.5), (16, 0.8)] {
        e.add_point(c, k * PPQ, v, CurveKind::Linear, 0.0).unwrap();
    }
    e.finish().0
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

/// What the engine plays: the M1 reference project, plus 8 stretched audio clips, plus 4
/// automation clips.
#[derive(Copy, Clone, PartialEq)]
enum Load {
    Reference,
    Audio,
    AudioAutomation,
}

fn engine(threads: usize, load: Load) -> (Engine, ph_engine::SimulatedDevice) {
    let (engine, mut dev) = Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: BUFFER,
            threads,
            ..EngineConfig::default()
        },
        Arc::new(BenchAssets),
    )
    .unwrap();
    let project = match load {
        Load::Reference => reference_project(),
        Load::Audio => audio_project(),
        Load::AudioAutomation => automated_project(),
    };
    engine.set_project(Arc::new(project)).unwrap();
    engine.transport(TransportCommand::Play);
    // warm up: voices sounding, caches hot
    for _ in 0..200 {
        dev.pull(BUFFER as usize);
    }
    (engine, dev)
}

/// Block times over `blocks` callbacks, in seconds.
fn block_times(threads: usize, blocks: usize, load: Load) -> Vec<f64> {
    let (engine, mut dev) = engine(threads, load);
    let times = (0..blocks)
        .map(|_| {
            let t = Instant::now();
            let b = dev.pull(BUFFER as usize);
            let dt = t.elapsed().as_secs_f64();
            std::hint::black_box(&b);
            dt
        })
        .collect();
    let st = engine.stats();
    assert!(st.warnings.is_empty(), "{:?}", st.warnings);
    assert_eq!(st.underruns, 0, "streaming fell behind");
    times
}

/// Mean, median and worst DSP load (block time / buffer duration).
fn loads(times: &[f64]) -> (f64, f64, f64) {
    let budget = BUFFER as f64 / SR as f64;
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let median = sorted[sorted.len() / 2];
    let worst = *sorted.last().unwrap();
    (mean / budget, median / budget, worst / budget)
}

/// `PH_PERF_OUT=<file>`: measure only (no criterion run) and write the figures as JSON for
/// `cargo xtask perf`. Each configuration is measured in 5 runs of 2000 blocks; the
/// reported figure is the median of the runs, which keeps one noisy run from deciding.
fn perf_json(path: &str) {
    let mut fields = Vec::new();
    for (label, threads, load) in [
        ("all_cores", 0usize, Load::Reference),
        ("one_thread", 1, Load::Reference),
        ("audio_all_cores", 0, Load::Audio),
        ("audio_one_thread", 1, Load::Audio),
        ("automation_all_cores", 0, Load::AudioAutomation),
        ("automation_one_thread", 1, Load::AudioAutomation),
    ] {
        let mut means = Vec::new();
        let mut medians = Vec::new();
        let mut worsts = Vec::new();
        for _ in 0..5 {
            let (mean, median, worst) = loads(&block_times(threads, 2000, load));
            means.push(mean);
            medians.push(median);
            worsts.push(worst);
        }
        let mid = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2] * 100.0
        };
        fields.push(format!(
            "  \"{label}_dsp_load_mean_pct\": {:.2},
  \"{label}_dsp_load_median_pct\": {:.2},
  \"{label}_dsp_load_worst_pct\": {:.2}",
            mid(&mut means),
            mid(&mut medians),
            mid(&mut worsts)
        ));
    }
    let json = format!(
        "{{
  \"project\": \"reference: 16 synth channels x 8-voice chords, 8 inserts\",
  \"sample_rate\": {SR},
  \"buffer_frames\": {BUFFER},
{}
}}
",
        fields.join(
            ",
"
        )
    );
    std::fs::write(path, json).expect("write perf json");
}

fn bench(c: &mut Criterion) {
    if let Ok(path) = std::env::var("PH_PERF_OUT") {
        perf_json(&path);
        return;
    }
    for (label, threads, load) in [
        ("all cores", 0usize, Load::Reference),
        ("one thread", 1, Load::Reference),
        ("all cores, + 8 stretched audio clips", 0, Load::Audio),
        ("one thread, + 8 stretched audio clips", 1, Load::Audio),
        (
            "all cores, + 8 stretched clips + 4 automation clips",
            0,
            Load::AudioAutomation,
        ),
        (
            "one thread, + 8 stretched clips + 4 automation clips",
            1,
            Load::AudioAutomation,
        ),
    ] {
        let (mean, median, worst) = loads(&block_times(threads, 2000, load));
        println!(
            "reference project, 48 kHz / {BUFFER} frames, {label}: DSP load {:.1}% mean, {:.1}% median, {:.1}% worst block",
            mean * 100.0,
            median * 100.0,
            worst * 100.0
        );
    }
    let mut g = c.benchmark_group("reference_project_block_256");
    for (label, threads, load) in [
        ("all_cores", 0usize, Load::Reference),
        ("one_thread", 1, Load::Reference),
        ("audio_all_cores", 0, Load::Audio),
        ("audio_one_thread", 1, Load::Audio),
        ("automation_all_cores", 0, Load::AudioAutomation),
        ("automation_one_thread", 1, Load::AudioAutomation),
    ] {
        let (_engine, mut dev) = engine(threads, load);
        g.bench_function(label, |b| b.iter(|| dev.pull(BUFFER as usize)));
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
