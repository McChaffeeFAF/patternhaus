//! Engine tests: oracle, real-time safety, timing, routing, reuse.
//!
//! The test binary installs `AllocDisabler`, so every `rt::no_alloc` section (the device
//! callback, worker processing) aborts on allocation in these tests.

mod audio;
mod audio_oracle;
mod audition;
mod automation;
mod oracle;
mod plugins;
mod routing;
mod rt_miri;
mod rt_safety;
mod stretch_quality;
mod timing;
mod transport;

use crate::offline::Offline;
use crate::plan::Settings;
use crate::runtime::Command;
use crate::{Engine, EngineConfig, SimulatedDevice, TransportCommand};
use ph_core::*;
use ph_dsp::Rng;
use ph_node::{AssetResolver, Quality, SampleData, SampleStream, StreamError};
use std::collections::BTreeMap;
use std::sync::Arc;

// Not under Miri: interpreting std's Windows `System` allocator behind a custom global
// allocator trips a Stacked/Tree Borrows false positive on over-aligned frees (reproduced
// without any engine code). Miri checks memory safety; the allocation ban is checked by the
// normal test runs.
#[cfg_attr(not(miri), global_allocator)]
#[cfg_attr(miri, allow(dead_code))]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

pub const SR: u32 = 48_000;

/// Generated test samples: `gen:impulse`, `gen:dc`, `gen:kick`, `gen:snare`, `gen:hat`,
/// `gen:silence`.
pub struct GenAssets;

/// Generated audio-clip material (M2/T1), `[channel][frame]` at 48 kHz:
/// - `gen:tone`: stereo 440 Hz sine at 0.5, 3 s;
/// - `gen:harm`: a 220 Hz tone with five harmonics, mono, 3 s;
/// - `gen:clicks`: mono 2 ms decaying clicks every 0.5 s (120 BPM quarters), 4 s;
/// - `gen:loop`: a stereo two-bar drum loop at 100 BPM (4.8 s);
/// - `gen:long`: 12 s stereo (long enough to stream from "disk"), also streamable;
/// - `gen:ramp`: stereo, left = frame / 48000, right = −left, 2 s.
pub fn gen_audio(reference: &str) -> Option<Vec<Vec<f32>>> {
    let sr = SR as f64;
    let tau = std::f64::consts::TAU;
    Some(match reference {
        "gen:tone" => {
            let l: Vec<f32> = (0..3 * SR as usize)
                .map(|i| (0.5 * (tau * 440.0 * i as f64 / sr).sin()) as f32)
                .collect();
            vec![l.clone(), l]
        }
        "gen:harm" => vec![
            (0..3 * SR as usize)
                .map(|i| {
                    let t = i as f64 / sr;
                    (1..=6)
                        .map(|h| 0.3 / h as f64 * (tau * 220.0 * h as f64 * t).sin())
                        .sum::<f64>() as f32
                })
                .collect(),
        ],
        "gen:clicks" => {
            let mut x = vec![0.0f32; 4 * SR as usize];
            let mut rng = Rng::new(5);
            for k in 0..8 {
                let s = k * SR as usize / 2;
                for i in 0..(0.01 * sr) as usize {
                    x[s + i] = rng.bipolar() * 0.8 * (-(i as f32) / 96.0).exp();
                }
            }
            vec![x]
        }
        "gen:loop" => {
            let n = (4.8 * sr) as usize;
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            let mut rng = Rng::new(11);
            let step = (0.15 * sr) as usize; // a 16th at 100 BPM
            for k in 0..32 {
                let s = k * step;
                let kick = k % 4 == 0;
                let snare = k % 8 == 4;
                for i in 0..(0.12 * sr) as usize {
                    if s + i >= n {
                        break;
                    }
                    let t = i as f64 / sr;
                    let mut v = 0.0f64;
                    if kick {
                        v += 0.8
                            * (tau * (55.0 + 90.0 * (-t * 40.0).exp()) * t).sin()
                            * (-t * 18.0).exp();
                    }
                    if snare {
                        v += 0.4 * rng.bipolar() as f64 * (-t * 30.0).exp();
                    }
                    v += 0.08 * rng.bipolar() as f64 * (-t * 120.0).exp();
                    l[s + i] += v as f32;
                    r[s + i] += (v * 0.9) as f32;
                }
            }
            vec![l, r]
        }
        "gen:long" => {
            let n = 12 * SR as usize;
            vec![
                (0..n).map(|i| long_frame(0, i as u64)).collect(),
                (0..n).map(|i| long_frame(1, i as u64)).collect(),
            ]
        }
        "gen:ramp" => {
            let l: Vec<f32> = (0..2 * SR as usize).map(|i| i as f32 / sr as f32).collect();
            let r = l.iter().map(|v| -v).collect();
            vec![l, r]
        }
        _ => return None,
    })
}

/// `gen:long`, frame by frame (so the stream and the decoded copy agree exactly).
pub fn long_frame(ch: usize, i: u64) -> f32 {
    let t = i as f64 / SR as f64;
    let tau = std::f64::consts::TAU;
    let f = if ch == 0 { 220.0 } else { 330.0 };
    let beat = (t * 2.0).fract();
    (0.3 * (tau * f * t).sin() * (0.6 + 0.4 * (-beat * 8.0).exp())) as f32
}

/// `gen:long` as a stream (a "file" read in blocks).
pub struct GenStream;

impl SampleStream for GenStream {
    fn frames(&self) -> u64 {
        12 * SR as u64
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
            for i in 0..n {
                let f = start + i as u64;
                o[i] = if f < frames { long_frame(c, f) } else { 0.0 };
            }
        }
        Ok(n.min(frames.saturating_sub(start) as usize))
    }
}

impl AssetResolver for GenAssets {
    fn stream(&self, reference: &str) -> Option<Result<Box<dyn SampleStream>, String>> {
        (reference == "gen:long").then(|| Ok(Box::new(GenStream) as Box<dyn SampleStream>))
    }

    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        if let Some(channels) = gen_audio(reference) {
            return Ok(Arc::new(SampleData {
                sample_rate: SR as f64,
                channels,
                loop_range: None,
                root_key: None,
            }));
        }
        let sr = SR as f64;
        let mut rng = Rng::new(9);
        let data: Vec<f32> = match reference {
            "gen:impulse" => {
                let mut v = vec![0.0; 64];
                v[0] = 1.0;
                v
            }
            "gen:dc" => vec![0.5; 48_000],
            "gen:silence" => vec![0.0; 48_000],
            "gen:kick" => {
                let mut ph = 0.0f64;
                (0..(0.4 * sr) as usize)
                    .map(|i| {
                        let t = i as f64 / sr;
                        let f = 45.0 + 110.0 * (-t * 30.0).exp();
                        ph += f / sr;
                        ((ph * std::f64::consts::TAU).sin() * (-t * 7.0).exp()) as f32 * 0.9
                    })
                    .collect()
            }
            "gen:snare" => (0..(0.25 * sr) as usize)
                .map(|i| {
                    let t = i as f64 / sr;
                    let tone = (t * 180.0 * std::f64::consts::TAU).sin() * (-t * 25.0).exp();
                    let noise = rng.bipolar() as f64 * (-t * 14.0).exp();
                    (0.4 * tone + 0.5 * noise) as f32
                })
                .collect(),
            "gen:hat" => {
                let mut prev = 0.0f32;
                (0..(0.08 * sr) as usize)
                    .map(|i| {
                        let t = i as f64 / sr;
                        let n = rng.bipolar();
                        let hp = n - prev;
                        prev = n;
                        hp * 0.3 * (-t * 50.0).exp() as f32
                    })
                    .collect()
            }
            other => return Err(format!("no generated sample `{other}`")),
        };
        Ok(Arc::new(SampleData {
            sample_rate: sr,
            channels: vec![data],
            loop_range: None,
            root_key: None,
        }))
    }
}

pub fn assets() -> Arc<dyn AssetResolver> {
    Arc::new(GenAssets)
}

fn params(p: &[(&str, f64)]) -> BTreeMap<String, f64> {
    p.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

pub fn synth(p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Synth {
        preset: None,
        params: params(p),
    }
}

pub fn sampler(sample: &str, p: &[(&str, f64)]) -> InstrumentSpec {
    InstrumentSpec::Sampler {
        sample: Some(sample.into()),
        root_key: 60,
        params: params(p),
    }
}

pub fn fx(name: &str, p: &[(&str, f64)]) -> EffectSlot {
    EffectSlot {
        effect: EffectSpec::Builtin {
            name: name.into(),
            params: params(p),
        },
        bypass: false,
        mix: 1.0,
    }
}

/// A small song: kick + snare + hats (samplers, the last two in choke group 1) on insert
/// 1 (EQ, compressor sidechained from insert 3, reverb send to insert 4), bass synth on
/// insert 2 (saturator), chord synth on insert 3; two patterns, 4 bars of clips, swing.
pub fn beat() -> Project {
    let mut e = Edit::new(&Project::empty("beat"));
    e.update_settings(|s| {
        s.bpm = 128.0;
        s.swing = 0.3;
    })
    .unwrap();
    let kick = e.add_channel("Kick", sampler("gen:kick", &[])).unwrap();
    let snare = e
        .add_channel("Snare", sampler("gen:snare", &[("choke_group", 1.0)]))
        .unwrap();
    let hat = e
        .add_channel("Hat", sampler("gen:hat", &[("choke_group", 1.0)]))
        .unwrap();
    let bass = e
        .add_channel(
            "Bass",
            synth(&[("osc1_wave", 0.0), ("f1_cutoff", 900.0), ("sub_level", 0.5)]),
        )
        .unwrap();
    let chords = e
        .add_channel(
            "Chords",
            synth(&[
                ("osc1_wave", 1.0),
                ("unison_voices", 3.0),
                ("amp_release", 0.4),
            ]),
        )
        .unwrap();
    for (c, i) in [(kick, 1), (snare, 1), (hat, 1), (bass, 2), (chords, 3)] {
        e.update_channel(c, |ch| ch.insert = InsertId(i)).unwrap();
    }
    e.update_insert(InsertId(1), |ins| {
        ins.effects.push(fx("eq", &[("b2_gain", 3.0)]));
        ins.effects.push(fx(
            "compressor",
            &[("threshold", -20.0), ("sidechain", 1.0)],
        ));
        ins.sends.push(ph_core::Send {
            to: InsertId(4),
            level: 0.3,
        });
        ins.sidechain_from = Some(InsertId(3));
    })
    .unwrap();
    e.update_insert(InsertId(4), |ins| {
        ins.effects
            .push(fx("reverb", &[("mix", 1.0), ("decay", 1.5)]));
    })
    .unwrap();
    e.update_insert(InsertId(2), |ins| {
        ins.effects.push(fx("saturator", &[("drive", 4.0)]));
        ins.pan = -0.3;
    })
    .unwrap();
    let drums = e.add_pattern("Drums").unwrap();
    for k in 0..4 {
        e.add_note(drums, kick, NoteSpec::new(60, k * PPQ, STEP))
            .unwrap();
        e.add_note(
            drums,
            snare,
            NoteSpec::new(60, k * PPQ + PPQ / 2 + STEP, STEP),
        )
        .unwrap();
    }
    for s in 0..16 {
        let mut n = NoteSpec::new(60, s * STEP, STEP / 2);
        n.velocity = if s % 2 == 0 { 0.8 } else { 0.5 };
        e.add_note(drums, hat, n).unwrap();
    }
    let music = e.add_pattern("Music").unwrap();
    for (i, k) in [36u8, 36, 43, 41].iter().enumerate() {
        e.add_note(music, bass, NoteSpec::new(*k, i as Tick * PPQ, PPQ - STEP))
            .unwrap();
    }
    for k in [60u8, 64, 67, 71] {
        e.add_note(music, chords, NoteSpec::new(k, 0, 2 * PPQ))
            .unwrap();
        e.add_note(music, chords, NoteSpec::new(k - 3, 2 * PPQ + STEP, 2 * PPQ))
            .unwrap();
    }
    let bar = 4 * PPQ;
    for b in 0..4 {
        e.add_clip(0, drums, b * bar).unwrap();
        if b > 0 {
            e.add_clip(1, music, b * bar).unwrap();
        }
    }
    e.finish().0
}

pub fn settings(block: usize) -> Settings {
    Settings {
        sample_rate: SR,
        max_frames: block,
        quality: Quality::Live,
    }
}

/// A scripted action at a frame offset.
#[derive(Clone)]
pub enum Act {
    T(TransportCommand),
    P(Arc<Project>),
    Preview(ChannelId, u8, f32, bool),
}

/// Offline reference: `initial` loaded, actions applied at their frames, rendered in
/// blocks of `block` with `workers` graph threads.
pub fn run_offline(
    initial: &Project,
    script: &[(usize, Act)],
    frames: usize,
    block: usize,
    workers: usize,
) -> [Vec<f32>; 2] {
    let a = GenAssets;
    let mut off = Offline::new(initial, settings(block), &a, workers).unwrap();
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut at = 0;
    let mut k = 0;
    while at < frames {
        while k < script.len() && script[k].0 <= at {
            match &script[k].1 {
                Act::T(t) => off.transport(t.clone()),
                Act::P(p) => off.set_project(p, &a).unwrap(),
                Act::Preview(c, key, v, on) => off.command(Command::Preview {
                    channel: *c,
                    key: *key,
                    velocity: *v,
                    on: *on,
                }),
            }
            k += 1;
        }
        let next = script.get(k).map_or(frames, |s| s.0.min(frames));
        let seg = off.render(next - at, block);
        out[0].extend_from_slice(&seg[0]);
        out[1].extend_from_slice(&seg[1]);
        at = next;
    }
    out
}

pub fn sim(buffer: u32, threads: usize) -> (Engine, SimulatedDevice) {
    Engine::start_simulated(
        EngineConfig {
            sample_rate: SR,
            buffer_frames: buffer,
            threads,
            ..EngineConfig::default()
        },
        assets(),
    )
    .unwrap()
}

/// The simulated device: `initial` set, then pulls of `pull` frames with actions applied
/// before the pull that starts at their frame (frames must be multiples of `pull`).
pub fn run_sim(
    initial: &Project,
    script: &[(usize, Act)],
    frames: usize,
    buffer: u32,
    pull: usize,
    threads: usize,
) -> [Vec<f32>; 2] {
    let (engine, mut dev) = sim(buffer, threads);
    engine.set_project(Arc::new(initial.clone())).unwrap();
    let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut k = 0;
    let mut at = 0;
    while at < frames {
        while k < script.len() && script[k].0 <= at {
            assert_eq!(script[k].0 % pull, 0, "action off the pull grid");
            match &script[k].1 {
                Act::T(t) => {
                    engine.transport(t.clone());
                }
                Act::P(p) => engine.set_project(p.clone()).unwrap(),
                Act::Preview(c, key, v, on) => engine.preview_note(*c, *key, *v, *on),
            }
            k += 1;
        }
        let n = pull.min(frames - at);
        let [l, r] = &mut out;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    out
}

pub fn assert_bits_eq(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2], what: &str) {
    assert_eq!(a[0].len(), b[0].len(), "{what}: lengths differ");
    for c in 0..2 {
        for i in 0..a[c].len() {
            if a[c][i].to_bits() != b[c][i].to_bits() {
                panic!(
                    "{what}: first difference at channel {c} frame {i}: {} vs {}",
                    a[c][i], b[c][i]
                );
            }
        }
    }
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt() as f32
}
