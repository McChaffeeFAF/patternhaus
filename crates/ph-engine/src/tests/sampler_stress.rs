//! M3 criterion 4: Haus Sampler disk streaming under load.
//!
//! A generated library of WAV files is written to a temporary folder and played as one
//! instrument: 128 voices at 48 kHz in 256-frame buffers, every voice streaming its own file
//! at its own pitch, under the real device's policy ([`StreamPolicy::Live`]: a block that is
//! not loaded yet is silence and an underrun), paced in real time so the prefetch thread
//! races the audio clock as it does on a device. It measures:
//!
//! - **underruns** (must be 0);
//! - **resident memory**: the sample data the sampler holds (every file's head) plus its
//!   stream windows and block cache, against the preload budget (files × `preload_frames` ×
//!   channels × 4 bytes) plus a fixed margin (the cache and windows, set by the voice count
//!   alone). This is the sampler's own accounting of what it allocates, not the process's
//!   resident set: nothing else loads the files (streamed files never go through
//!   `AssetResolver::sample`);
//! - **load time**: building the instrument, which reads only the heads.
//!
//! `streaming_stress_ci` (about 50 MB) runs with the other tests. The full library (about
//! 2 GB) is `#[ignore]`d because writing it takes a while: run it with
//! `cargo xtask sampler-stress`.

use super::*;
use crate::player::{AudioCtx, SAMPLER_BLOCK_FRAMES, SAMPLER_LOOKAHEAD_BLOCKS};
use crate::stream::StreamPolicy;
use ph_node::{Event, EventKind, Processor as _};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

const VOICES: usize = 128;
const BUFFER: usize = 256;

/// What a run measured.
#[derive(Debug)]
struct Report {
    library_bytes: u64,
    files: usize,
    load: Duration,
    underruns: u64,
    resident: usize,
    budget: usize,
    margin: usize,
    dsp_load_mean: f64,
    blocks: usize,
}

/// Writes `files` stereo 16-bit WAVs of `seconds` each: a decaying partial series per file
/// (each file a different pitch), like a sampled instrument.
fn write_library(dir: &std::path::Path, files: usize, seconds: f64) -> u64 {
    let frames = (seconds * SR as f64) as usize;
    let mut total = 0;
    for f in 0..files {
        let hz = 55.0 * (f as f64 / 12.0).exp2();
        let mut l = vec![0.0f32; frames];
        let mut r = vec![0.0f32; frames];
        // phasor recurrences (fast enough for gigabytes)
        let (mut c1, mut s1) = (1.0f64, 0.0f64);
        let (dc, ds) = (
            (std::f64::consts::TAU * hz / SR as f64).cos(),
            (std::f64::consts::TAU * hz / SR as f64).sin(),
        );
        let decay = (-1.0 / (4.0 * SR as f64)).exp();
        let mut amp = 0.5f64;
        for i in 0..frames {
            let (c, s) = (c1 * dc - s1 * ds, s1 * dc + c1 * ds);
            c1 = c;
            s1 = s;
            // fundamental and second partial (from the double angle), slowly decaying
            let x = s1 + 0.3 * 2.0 * s1 * c1;
            l[i] = (amp * x) as f32;
            r[i] = (amp * (s1 - 0.3 * 2.0 * s1 * c1)) as f32;
            amp = (amp * decay).max(0.05);
        }
        let path = dir.join(format!("{f:03}.wav"));
        ph_formats::encode::write_wav(&path, &[l, r], SR, ph_formats::encode::WavFormat::Pcm16)
            .unwrap();
        total += std::fs::metadata(&path).unwrap().len();
    }
    total
}

/// One key per voice: key `k` plays file `k % files` at a pitch near the file's own.
fn library_project(dir: &std::path::Path, files: usize) -> (Project, InstrumentId) {
    let mut e = Edit::new(&Project::empty("stress"));
    let mut inst = HausInstrument::new("Stress");
    inst.folder = Some(dir.to_string_lossy().replace('\\', "/"));
    inst.settings.voice_limit = VOICES as u32;
    let i = e.add_instrument(inst).unwrap();
    let g = e.add_group(i, Group::new("all")).unwrap();
    for k in 0..VOICES as u8 {
        let mut z = Zone::new(
            g,
            SampleRef::File(format!("{:03}.wav", k as usize % files)),
            KeyRange { lo: k, hi: k },
            k.saturating_sub(k % 3),
        );
        z.one_shot = true;
        e.add_zone(i, z).unwrap();
    }
    (e.finish().0, i)
}

/// Plays the library for `play_seconds`, paced in real time (see the loop for the pattern).
fn stress(files: usize, file_seconds: f64, play_seconds: f64) -> Report {
    let tmp = tempfile::tempdir().unwrap();
    let library_bytes = write_library(tmp.path(), files, file_seconds);
    let (project, id) = library_project(tmp.path(), files);
    let assets = crate::assets::FileResolver::new().allow_root(tmp.path());
    let mut audio = AudioCtx::new(StreamPolicy::Live);
    let t0 = Instant::now();
    let (mut s, warnings) =
        ph_sampler::create_streamed(&project, id, &assets, &mut |streams, voices| {
            Some(audio.sampler_cache(streams, voices) as Arc<dyn ph_sampler::StreamSource>)
        })
        .unwrap();
    s.activate(&ph_node::ActivateInfo {
        sample_rate: SR as f64,
        max_frames: BUFFER,
        quality: Quality::Live,
        seed: 7,
    });
    let load = t0.elapsed();
    assert!(warnings.is_empty(), "{warnings:?}");
    let slots = ph_sampler::voice_slots(&project.instruments[&id]);
    let preload = ph_sampler::DEFAULT_PRELOAD_FRAMES as usize;
    let budget = files * preload * 2 * 4;
    let cache = (slots * (SAMPLER_LOOKAHEAD_BLOCKS + 2) + 2) * SAMPLER_BLOCK_FRAMES * 2 * 4;
    let margin = cache + slots * 2 * ph_sampler::render::WINDOW * 4;
    let resident = s.resident_bytes() + cache;

    let blocks = (play_seconds * SR as f64 / BUFFER as f64) as usize;
    let period = Duration::from_secs_f64(BUFFER as f64 / SR as f64);
    let mut l = vec![0.0f32; BUFFER];
    let mut r = vec![0.0f32; BUFFER];
    let mut events = Vec::with_capacity(4);
    let mut busy = Duration::ZERO;
    let mut peak = 0.0f32;
    let start = Instant::now();
    for b in 0..blocks {
        events.clear();
        // all 128 notes start in the first 128 blocks; then one restarts every 8 blocks
        // (each from its head again), so every voice streams for about 5 s at a time
        let k = if b < VOICES {
            Some(b)
        } else if b.is_multiple_of(8) {
            Some((b / 8) % VOICES)
        } else {
            None
        };
        if let Some(k) = k {
            let key = k as u8;
            if b >= VOICES {
                events.push(Event {
                    frame: 0,
                    kind: EventKind::NoteOff {
                        note_id: key as u32,
                        key,
                        velocity: 0.0,
                    },
                });
            }
            events.push(Event {
                frame: 1,
                kind: EventKind::NoteOn {
                    note_id: key as u32,
                    key,
                    velocity: 0.8,
                    pan: 0.0,
                    fine_pitch: 0.0,
                },
            });
        }
        let ctx = ph_node::ProcessContext {
            sample_rate: SR as f64,
            frames: BUFFER,
            song_pos_samples: (b * BUFFER) as i64,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        let t = Instant::now();
        assert_no_alloc::assert_no_alloc(|| {
            let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
            let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
            let mut io = ph_node::ProcessIo {
                inputs: &[],
                outputs: &mut ports,
                events: &events,
            };
            s.process(&ctx, &mut io);
        });
        busy += t.elapsed();
        peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
        // the next callback comes when the device needs it
        let due = start + period * (b as u32 + 1);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
    }
    assert!(peak > 0.05, "the library plays");
    Report {
        library_bytes,
        files,
        load,
        underruns: audio.underruns.load(Relaxed),
        resident,
        budget,
        margin,
        dsp_load_mean: busy.as_secs_f64() / (blocks as f64 * period.as_secs_f64()),
        blocks,
    }
}

fn check(r: &Report, max_load: Duration) {
    println!(
        "sampler streaming stress: {} files, {:.1} MB; {VOICES} voices, {} blocks of {BUFFER} at {SR} Hz; \
         load {:.0} ms; underruns {}; resident {:.1} MB against preload budget {:.1} MB + margin {:.1} MB; \
         mean DSP load {:.1}% (one thread)",
        r.files,
        r.library_bytes as f64 / 1e6,
        r.blocks,
        r.load.as_secs_f64() * 1000.0,
        r.underruns,
        r.resident as f64 / 1e6,
        r.budget as f64 / 1e6,
        r.margin as f64 / 1e6,
        r.dsp_load_mean * 100.0,
    );
    assert_eq!(r.underruns, 0, "streaming fell behind");
    assert!(
        r.resident <= r.budget + r.margin,
        "resident {} above budget {} + margin {}",
        r.resident,
        r.budget,
        r.margin
    );
    assert!(r.load < max_load, "loading took {:?}", r.load);
}

/// About 50 MB: 32 files of 8.5 s, 4 s of 128 streamed voices.
#[test]
fn streaming_stress_ci() {
    let r = stress(32, 8.5, 4.0);
    check(&r, Duration::from_secs(2));
}

/// About 2 GB: 128 files of 87 s, 20 s of 128 streamed voices. Slow to set up (it writes
/// the library first): `cargo xtask sampler-stress`.
#[test]
#[ignore = "writes a 2 GB library; run with `cargo xtask sampler-stress`"]
fn streaming_stress_2gb() {
    let r = stress(128, 87.0, 20.0);
    check(&r, Duration::from_secs(2));
    assert!(
        (r.resident as u64) < r.library_bytes / 20,
        "the sampler holds a small fraction of the library"
    );
}
