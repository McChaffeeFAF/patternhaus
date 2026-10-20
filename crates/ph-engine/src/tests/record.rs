//! Recording and input monitoring (M2/T8), through the simulated device and simulated inputs.
//! No test opens a real audio device. Every pull runs under `assert_no_alloc` (`pull_into`),
//! so each test here also checks that the recorder allocates nothing on the audio thread.

use super::*;
use crate::record::*;

const IN_RATE: u32 = SR;

/// A stereo ramp: left = i / 1e5, right = -left (distinct values per frame, so alignment
/// errors show).
fn ramp(frames: usize) -> Vec<Vec<f32>> {
    let l: Vec<f32> = (0..frames).map(|i| (i as f32 + 1.0) / 1.0e5).collect();
    let r = l.iter().map(|v| -v).collect();
    vec![l, r]
}

fn provider(frames: usize, latency: u32) -> Box<SimulatedInputs> {
    Box::new(SimulatedInputs::new().with("Sim: In 1-2", IN_RATE, ramp(frames), latency))
}

fn pull(dev: &mut SimulatedDevice, frames: usize, block: usize) -> [Vec<f32>; 2] {
    let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut at = 0;
    while at < frames {
        let n = block.min(frames - at);
        let [l, r] = &mut out;
        dev.pull_into(&mut l[at..at + n], &mut r[at..at + n]);
        at += n;
    }
    out
}

fn read_wav(path: &std::path::Path) -> (hound::WavSpec, Vec<Vec<f32>>) {
    let mut r = hound::WavReader::open(path).unwrap();
    let spec = r.spec();
    let all: Vec<f32> = r.samples::<f32>().map(|s| s.unwrap()).collect();
    let ch = spec.channels as usize;
    let mut out = vec![Vec::new(); ch];
    for (i, s) in all.iter().enumerate() {
        out[i % ch].push(*s);
    }
    (spec, out)
}

/// An empty project at 120 BPM: a tick is 25 frames at 48 kHz.
fn bpm120() -> Arc<Project> {
    let mut e = Edit::new(&Project::empty("rec"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    Arc::new(e.finish().0)
}

fn project_with_channel() -> (Project, ChannelId) {
    let mut e = Edit::new(&Project::empty("rec"));
    e.update_settings(|s| s.bpm = 120.0).unwrap();
    let c = e.add_channel("Synth", synth(&[])).unwrap();
    e.update_channel(c, |ch| ch.insert = InsertId(2)).unwrap();
    (e.finish().0, c)
}

#[test]
fn a_take_is_the_input_written_to_a_new_wav_with_its_latency() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    engine.set_project(bpm120()).unwrap();
    engine.set_input_provider(Some(provider(20_000, 96)));
    let inputs = engine.inputs();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].name, "Sim: In 1-2");
    engine
        .arm_with(Arm {
            input: inputs[0].id,
            target: RecordTarget::Track(2),
            monitor: false,
        })
        .unwrap();
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    // recording plays the song; pull an awkward number of frames in odd blocks
    let _ = pull(&mut dev, 10_000, 333);
    assert_eq!(engine.record_status().phase, RecordPhase::Recording);
    assert!(engine.status().playing);
    let takes = engine.record_stop().unwrap();
    assert_eq!(takes.len(), 1);
    let t = &takes[0];
    assert_eq!(t.file, "Take 1.wav");
    assert_eq!(t.frames, 10_000);
    assert_eq!(t.start, 0);
    assert_eq!(
        t.latency_frames, 96,
        "the input reports 96 frames, the simulated output 0"
    );
    assert_eq!(t.target, RecordTarget::Track(2));
    // 10000 frames at 120 BPM and 48 kHz (25 per tick) = 400 ticks
    assert_eq!(t.length, 400);
    let (spec, audio) = read_wav(&t.path);
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (2, SR, 32)
    );
    assert_eq!(audio[0].len(), 10_000);
    let want = ramp(10_000);
    assert_eq!(audio[0], want[0], "left is the input, bit for bit");
    assert_eq!(audio[1], want[1]);
    // stopped recording leaves the transport alone; a second take gets the next number
    assert!(!engine.is_recording());
}

#[test]
fn takes_never_overwrite_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Take 1.wav"), b"precious").unwrap();
    std::fs::write(dir.path().join("Take 4.wav"), b"also precious").unwrap();
    let (engine, mut dev) = sim(128, 0);
    engine.set_input_provider(Some(provider(5_000, 0)));
    let id = engine.inputs()[0].id;
    engine.arm(id, RecordTarget::Track(0)).unwrap();
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    let _ = pull(&mut dev, 1_000, 128);
    let takes = engine.record_stop().unwrap();
    assert_eq!(takes[0].file, "Take 5.wav");
    assert_eq!(
        std::fs::read(dir.path().join("Take 1.wav")).unwrap(),
        b"precious"
    );
    assert_eq!(
        std::fs::read(dir.path().join("Take 4.wav")).unwrap(),
        b"also precious"
    );
}

#[test]
fn punch_records_only_inside_the_range_and_places_the_take_there() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    engine.set_project(bpm120()).unwrap();
    engine.set_input_provider(Some(provider(60_000, 0)));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor: false,
        })
        .unwrap();
    // punch in at beat 1 (ticks 960 = sample 24000) and out at beat 2 (sample 48000)
    let mut opts = RecordOptions::new(dir.path());
    opts.punch = Some((960, 1920));
    engine.record_start(opts).unwrap();
    let _ = pull(&mut dev, 60_000, 250);
    let takes = engine.record_stop().unwrap();
    let t = &takes[0];
    assert_eq!(t.start, 960, "the take sits at the punch-in");
    assert_eq!(t.frames, 24_000);
    assert_eq!(t.length, 960);
    let (_, audio) = read_wav(&t.path);
    // the input is read every block while rolling (a live stream does not wait), so the take
    // starts with the input frame that arrived at the punch-in sample 24000
    assert_eq!(audio[0][0], ramp(24_001)[0][24_000]);
    assert_eq!(audio[0].len(), 24_000);
}

#[test]
fn a_count_in_holds_the_song_and_clicks_then_records_from_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    let (p, _) = project_with_channel();
    engine.set_project(Arc::new(p)).unwrap();
    engine.set_input_provider(Some(provider(200_000, 0)));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor: false,
        })
        .unwrap();
    let mut opts = RecordOptions::new(dir.path());
    opts.count_in_bars = 1;
    engine.record_start(opts).unwrap();
    // one bar at 120 BPM and 48 kHz is 96000 frames
    let during = pull(&mut dev, 48_000, 256);
    assert_eq!(engine.record_status().phase, RecordPhase::CountIn);
    assert!(
        !engine.status().playing,
        "the transport waits for the count-in"
    );
    assert_eq!(engine.status().position, 0);
    assert!(peak(&during[0]) > 0.1, "the metronome clicks");
    // the click on the first beat is louder (higher) than none in between: silence between
    assert!(during[0][4000..20000].iter().all(|v| v.abs() < 1e-3));
    let after = pull(&mut dev, 48_000 + 5_000, 256);
    assert_eq!(engine.record_status().phase, RecordPhase::Recording);
    assert!(engine.status().playing);
    let _ = after;
    let takes = engine.record_stop().unwrap();
    let t = &takes[0];
    // recording began exactly when the count-in ended, at song position 0
    assert_eq!(t.start, 0);
    assert_eq!(t.frames, 5_000);
    let (_, audio) = read_wav(&t.path);
    assert_eq!(
        audio[0][0],
        ramp(1)[0][0],
        "the input was not consumed during the count-in"
    );
}

#[test]
fn a_mono_input_is_recorded_as_a_mono_file() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    let mono = vec![ramp(4_000).remove(0)];
    engine.set_input_provider(Some(Box::new(SimulatedInputs::new().with(
        "Mic",
        SR,
        mono.clone(),
        0,
    ))));
    let id = engine.inputs()[0].id;
    engine.arm(id, RecordTarget::Track(0)).unwrap();
    engine.set_monitor(id, false).unwrap();
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    let _ = pull(&mut dev, 3_000, 256);
    let t = engine.record_stop().unwrap().remove(0);
    assert_eq!(t.channels, 1);
    let (spec, audio) = read_wav(&t.path);
    assert_eq!(spec.channels, 1);
    assert_eq!(audio[0], mono[0][..3_000]);
}

#[test]
fn two_armed_inputs_make_two_named_takes() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    engine.set_input_provider(Some(Box::new(
        SimulatedInputs::new()
            .with("Mic 1", SR, ramp(4_000), 0)
            .with("Mic 2", SR, ramp(4_000), 0),
    )));
    for id in [InputId(0), InputId(1)] {
        engine
            .arm_with(Arm {
                input: id,
                target: RecordTarget::Track(id.0),
                monitor: false,
            })
            .unwrap();
    }
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    let _ = pull(&mut dev, 2_000, 256);
    let takes = engine.record_stop().unwrap();
    assert_eq!(takes.len(), 2);
    let mut names: Vec<_> = takes.iter().map(|t| t.file.clone()).collect();
    names.sort();
    assert_eq!(names, ["Take 1 - Mic 1.wav", "Take 2 - Mic 2.wav"]);
}

#[test]
fn an_empty_take_leaves_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    engine.set_input_provider(Some(provider(4_000, 0)));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor: false,
        })
        .unwrap();
    // punch range far in the future: nothing is captured
    let mut opts = RecordOptions::new(dir.path());
    opts.punch = Some((96_000, 97_000));
    engine.record_start(opts).unwrap();
    let _ = pull(&mut dev, 1_000, 256);
    assert!(engine.record_stop().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn recording_needs_an_armed_input_and_refuses_double_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _dev) = sim(256, 0);
    assert!(engine.inputs().is_empty(), "no provider, no inputs");
    assert!(engine.record_start(RecordOptions::new(dir.path())).is_err());
    assert!(engine.arm(InputId(0), RecordTarget::Track(0)).is_err());
    engine.set_input_provider(Some(provider(1000, 0)));
    engine.arm(InputId(0), RecordTarget::Track(0)).unwrap();
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    assert!(engine.record_start(RecordOptions::new(dir.path())).is_err());
    assert!(engine.record_stop().is_ok());
    assert!(engine.record_stop().is_err(), "not recording any more");
    assert!(engine.set_monitor(InputId(3), true).is_err());
}

#[test]
fn a_manual_latency_overrides_the_measurement() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 0);
    engine.set_input_provider(Some(provider(4_000, 500)));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor: false,
        })
        .unwrap();
    assert_eq!(engine.latency().input_frames, 500);
    assert_eq!(engine.latency().compensation_frames, 500);
    let mut opts = RecordOptions::new(dir.path());
    opts.latency_ms = Some(10.0);
    engine.record_start(opts).unwrap();
    let _ = pull(&mut dev, 1_000, 256);
    let t = engine.record_stop().unwrap().remove(0);
    assert_eq!(t.latency_frames, 480, "10 ms at 48 kHz");
}

// ---- monitoring

#[test]
fn monitoring_adds_the_input_to_the_target_insert_and_stays_out_of_offline_renders() {
    let (p, c) = project_with_channel();
    let p = Arc::new(p);
    let (engine, mut dev) = sim(256, 1);
    engine.set_project(p.clone()).unwrap();
    // a constant input, so the monitored level is easy to see
    let dc = vec![vec![0.25f32; 40_000]; 2];
    engine.set_input_provider(Some(Box::new(SimulatedInputs::new().with("DC", SR, dc, 0))));
    let id = engine.inputs()[0].id;
    // armed but not monitored: silent
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Channel(c),
            monitor: false,
        })
        .unwrap();
    let silent = pull(&mut dev, 2_048, 256);
    assert!(peak(&silent[0]) < 1e-6);
    // monitored: the input comes through insert 2 and the master at the strip's gain
    engine.set_monitor(id, true).unwrap();
    let heard = pull(&mut dev, 4_096, 256);
    let tail = &heard[0][1024..];
    assert!(rms(tail) > 0.05, "monitor is audible: {}", rms(tail));
    // insert 2's meter saw it (the channel's insert, not the master alone)
    let m = engine.meters();
    assert!(m.peak[2][0] > 0.1, "{:?}", m.peak[2]);
    // switching it off silences it again
    engine.set_monitor(id, false).unwrap();
    let _ = pull(&mut dev, 512, 256);
    let off = pull(&mut dev, 2_048, 256);
    assert!(peak(&off[0]) < 1e-6);
    // offline renders never include monitoring
    let rs = crate::RenderSettings {
        mode: crate::PlayMode::Song,
        range: Some((0, 960)),
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 256,
    };
    let a = crate::render_offline(&p, &rs, &*assets()).unwrap();
    assert!(a.channels[0].iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn an_armed_unmonitored_engine_is_bit_identical_to_one_without_inputs() {
    // live == offline when monitoring is off (acceptance 6)
    let p = Arc::new(beat());
    let script = [
        (
            0usize,
            Act::T(TransportCommand::SetMode(crate::PlayMode::Song)),
        ),
        (0, Act::T(TransportCommand::Play)),
    ];
    let plain = run_sim(&p, &script, 24_000, 256, 256, 1);
    let (engine, mut dev) = sim(256, 1);
    engine.set_project(p.clone()).unwrap();
    engine.set_input_provider(Some(provider(100_000, 0)));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor: false,
        })
        .unwrap();
    engine.transport(TransportCommand::Play);
    let armed = pull(&mut dev, 24_000, 256);
    assert_bits_eq(&plain, &armed, "armed, not monitored");
    // and while recording (capturing does not touch the output either)
    let dir = tempfile::tempdir().unwrap();
    let (engine, mut dev) = sim(256, 1);
    engine.set_project(p).unwrap();
    engine.set_input_provider(Some(provider(100_000, 0)));
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Track(0),
            monitor: false,
        })
        .unwrap();
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    let recorded = pull(&mut dev, 24_000, 256);
    assert_bits_eq(&plain, &recorded, "recording, not monitored");
    assert_eq!(engine.record_stop().unwrap()[0].frames, 24_000);
}

#[test]
fn recording_survives_plan_swaps_and_toggling_under_assert_no_alloc() {
    // a second thread swaps plans while the audio thread records and monitors; `pull_into`
    // aborts on any allocation inside the callback
    let dir = tempfile::tempdir().unwrap();
    let (p, c) = project_with_channel();
    let (engine, mut dev) = sim(192, 1);
    let engine = Arc::new(engine);
    engine.set_project(Arc::new(p.clone())).unwrap();
    engine.set_input_provider(Some(provider(400_000, 64)));
    let id = engine.inputs()[0].id;
    engine
        .arm_with(Arm {
            input: id,
            target: RecordTarget::Channel(c),
            monitor: true,
        })
        .unwrap();
    engine.record_start(RecordOptions::new(dir.path())).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let swapper = {
        let (e, stop, p) = (engine.clone(), stop.clone(), p.clone());
        std::thread::spawn(move || {
            let mut k = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let mut ed = Edit::new(&p);
                ed.update_channel(c, |ch| ch.volume = 0.3 + (k % 5) as f32 * 0.1)
                    .unwrap();
                e.set_project(Arc::new(ed.finish().0)).unwrap();
                let _ = e.set_monitor(id, k.is_multiple_of(2));
                k += 1;
                std::thread::sleep(std::time::Duration::from_micros(300));
            }
        })
    };
    let out = pull(&mut dev, 192_000, 192);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    swapper.join().unwrap();
    assert!(out[0].iter().all(|v| v.is_finite()));
    let t = engine.record_stop().unwrap().remove(0);
    assert_eq!(t.frames, 192_000);
    assert_eq!(t.overflow, 0);
    let (_, audio) = read_wav(&t.path);
    assert_eq!(audio[0].len(), 192_000);
    assert_eq!(audio[0][191_999], ramp(192_000)[0][191_999]);
}
