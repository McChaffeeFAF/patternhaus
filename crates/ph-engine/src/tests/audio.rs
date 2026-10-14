//! Audio-clip playback (M2/T1): offsets, trim and slip, gain, fades and their curves,
//! reverse, pitch, mute, the three stretch modes, tempo sync through tempo changes, seeks
//! and loop wraps inside clips, and disk streaming.

use super::*;
use crate::{PlayMode, RenderSettings, render_offline};

pub fn entry(path: &str, bpm: Option<f64>) -> SampleEntry {
    let ch = GenAssets
        .sample(path)
        .expect("generated material")
        .channels
        .clone();
    SampleEntry {
        id: SampleId(0),
        path: path.into(),
        name: path.trim_start_matches("gen:").into(),
        frames: ch[0].len() as u64,
        sample_rate: SR,
        channels: ch.len() as u16,
        bpm: bpm.map(|value| SampleBpm {
            value,
            source: BpmSource::User,
        }),
        root_key: None,
        analysis: Default::default(),
        provenance: Default::default(),
    }
}

pub fn audio_project(bpm: f64, f: impl FnOnce(&mut Edit)) -> Project {
    let mut e = Edit::new(&Project::empty("audio"));
    e.update_settings(|s| s.bpm = bpm).unwrap();
    f(&mut e);
    e.finish().0
}

/// One clip of `path` on track 0 at tick `start`, `len` ticks, configured by `f`.
pub fn clip(
    e: &mut Edit,
    path: &str,
    bpm: Option<f64>,
    track: u32,
    start: Tick,
    len: Tick,
    f: impl FnOnce(&mut AudioClip),
) -> ClipId {
    let s = match e.project().sample_by_path(path) {
        Some(x) => x.id,
        None => e.add_sample(entry(path, bpm)).unwrap(),
    };
    let mut a = AudioClip::new(s);
    f(&mut a);
    e.add_audio_clip(track, start, len, a).unwrap()
}

pub fn song(block: usize) -> RenderSettings {
    RenderSettings {
        mode: PlayMode::Song,
        range: None,
        sample_rate: SR,
        quality: Quality::Live,
        include_tail: false,
        block_frames: block,
    }
}

fn render(p: &Project) -> [Vec<f32>; 2] {
    let b = render_offline(p, &song(256), &GenAssets).unwrap();
    [b.channels[0].clone(), b.channels[1].clone()]
}

/// Frames per beat at `bpm`.
fn beat(bpm: f64) -> usize {
    (SR as f64 * 60.0 / bpm).round() as usize
}

fn assert_close(got: f32, want: f32, tol: f32, what: &str) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got} vs {want} (tolerance {tol})"
    );
}

/// Frequency by counting rising zero crossings over the middle half of `x`.
pub fn zero_cross_hz(x: &[f32]) -> f64 {
    let (a, b) = (x.len() / 4, 3 * x.len() / 4);
    let mut first: Option<f64> = None;
    let mut last = 0.0;
    let mut count = 0;
    for i in a + 1..b {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            // linear interpolation of the crossing
            let frac = -x[i - 1] as f64 / (x[i] - x[i - 1]) as f64;
            let t = i as f64 - 1.0 + frac;
            first.get_or_insert(t);
            last = t;
            count += 1;
        }
    }
    let first = first.expect("crossings");
    (count - 1) as f64 * SR as f64 / (last - first)
}

/// Sample indices where `|x|` first rises above `thr` after at least `gap` quiet samples.
pub fn onsets(x: &[f32], thr: f32, gap: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quiet = gap;
    for (i, v) in x.iter().enumerate() {
        if v.abs() > thr {
            if quiet >= gap {
                out.push(i);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    out
}

#[test]
fn resample_at_unity_reproduces_the_source_with_offset_and_gain() {
    let bpm = 120.0;
    let src = gen_audio("gen:ramp").unwrap();
    let p = audio_project(bpm, |e| {
        clip(e, "gen:ramp", None, 0, 0, PPQ, |a| {
            a.source_offset = 4800;
            a.gain_db = -6.0;
        });
    });
    let out = render(&p);
    assert_eq!(out[0].len(), beat(bpm));
    let g = 10f32.powf(-6.0 / 20.0);
    for i in (0..out[0].len()).step_by(997) {
        assert_close(out[0][i], g * src[0][4800 + i], 2e-4, &format!("left {i}"));
        assert_close(out[1][i], g * src[1][4800 + i], 2e-4, &format!("right {i}"));
    }
}

#[test]
fn fades_follow_their_curves() {
    let bpm = 120.0;
    let b = beat(bpm);
    let p = audio_project(bpm, |e| {
        clip(e, "gen:dc", None, 0, 0, 2 * PPQ, |a| {
            a.fade_in = Fade {
                length: PPQ / 2,
                curve: FadeCurve::Exp,
            };
            a.fade_out = Fade {
                length: PPQ,
                curve: FadeCurve::SCurve,
            };
        });
    });
    // gen:dc is in the registry at its real length
    let out = render(&p);
    assert_eq!(out[0].len(), 2 * b);
    let fi = b / 2;
    for i in [0, fi / 4, fi / 2, fi - 1, fi, b, b + b / 3, 2 * b - 1] {
        let mut g = 1.0;
        if i < fi {
            g *= FadeCurve::Exp.gain(i as f64 / fi as f64);
        }
        if i + b >= 2 * b {
            g *= FadeCurve::SCurve.gain((2 * b - i) as f64 / b as f64);
        }
        // mono source: both sides; the sinc edge at frame 0 reads silence before the file
        let tol = if i < 8 { 0.05 } else { 2e-4 };
        assert_close(out[0][i], 0.5 * g as f32, tol, &format!("frame {i}"));
        assert_eq!(out[0][i], out[1][i]);
    }
}

#[test]
fn reverse_plays_the_range_backwards() {
    let bpm = 120.0;
    let src = gen_audio("gen:ramp").unwrap();
    let len = beat(bpm) / 2;
    let p = audio_project(bpm, |e| {
        clip(e, "gen:ramp", None, 0, 0, PPQ / 2, |a| {
            a.source_offset = 1000;
            a.reverse = true;
        });
    });
    let out = render(&p);
    for i in (16..len - 16).step_by(331) {
        assert_close(
            out[0][i],
            src[0][1000 + len - 1 - i],
            2e-4,
            &format!("frame {i}"),
        );
    }
}

#[test]
fn resample_pitch_moves_speed_and_pitch_together() {
    let bpm = 120.0;
    let p = audio_project(bpm, |e| {
        clip(e, "gen:tone", None, 0, 0, 2 * PPQ, |a| {
            a.pitch_semitones = 12
        });
    });
    let out = render(&p);
    // 3 s of tone at double speed lasts 1.5 s: the 1 s clip is all tone
    let f = zero_cross_hz(&out[0]);
    assert!((1200.0 * (f / 880.0).log2()).abs() < 1.0, "{f} Hz");
    let p = audio_project(bpm, |e| {
        clip(e, "gen:tone", None, 0, 0, 2 * PPQ, |a| a.fine_cents = -50.0);
    });
    let f = zero_cross_hz(&render(&p)[0]);
    assert!((1200.0 * (f / 440.0).log2() + 50.0).abs() < 1.0, "{f} Hz");
}

#[test]
fn muted_tracks_and_pattern_mode_are_silent() {
    let p = audio_project(120.0, |e| {
        clip(e, "gen:tone", None, 2, 0, PPQ, |_| {});
        e.update_track(2, |t| t.muted = true).unwrap();
        // something audible elsewhere so the song has a length
        clip(e, "gen:dc", None, 0, PPQ, PPQ, |a| a.gain_db = -60.0);
    });
    let out = render(&p);
    assert!(peak(&out[0][..beat(120.0)]) == 0.0, "muted track played");
    // pattern mode: an audio-only project plays nothing
    let mut e = Edit::new(&p);
    let pat = e.add_pattern("P").unwrap();
    let p2 = e.finish().0;
    let mut rs = song(256);
    rs.mode = PlayMode::Pattern(pat);
    let b = render_offline(&p2, &rs, &GenAssets).unwrap();
    assert!(peak(&b.channels[0]) == 0.0);
}

#[test]
fn synced_clips_follow_tempo_changes() {
    // clicks every beat of a 120 BPM sample; the song changes from 120 to 80 BPM at beat 4
    let p = audio_project(120.0, |e| {
        clip(e, "gen:clicks", Some(120.0), 0, 0, 8 * PPQ, |a| {
            a.tempo_sync = true
        });
        let t = e
            .add_automation_clip(1, 0, 8 * PPQ, ParamPath::Tempo)
            .unwrap();
        e.add_point(t, 0, 120.0, CurveKind::Hold, 0.0).unwrap();
        e.add_point(t, 4 * PPQ, 80.0, CurveKind::Hold, 0.0).unwrap();
    });
    let out = render(&p);
    let mut map = crate::tempo::TempoMap::constant(120.0, SR);
    map.push_change(4 * PPQ, 80.0);
    let found = onsets(&out[0], 0.05, 2000);
    assert_eq!(found.len(), 8, "{found:?}");
    for (k, f) in found.iter().enumerate() {
        let want = map.sample_at(k as Tick * PPQ);
        assert!(
            (*f as i64 - want).abs() <= 24,
            "click {k} at {f}, beat at {want}"
        );
    }
    assert_eq!(out[0].len() as i64, map.sample_at(8 * PPQ));
}

#[test]
fn stretch_keeps_pitch_while_following_the_tempo() {
    for (sample_bpm, harm) in [(100.0, false), (140.0, true)] {
        let path = if harm { "gen:harm" } else { "gen:tone" };
        let p = audio_project(120.0, |e| {
            clip(e, path, Some(sample_bpm), 0, 0, 4 * PPQ, |a| {
                a.tempo_sync = true;
                a.stretch = StretchMode::Stretch;
            });
        });
        let out = render(&p);
        assert_eq!(out[0].len(), 4 * beat(120.0));
        let want = if harm { 220.0 } else { 440.0 };
        let f = zero_cross_hz(&out[0]);
        let cents = 1200.0 * (f / want).log2();
        assert!(
            cents.abs() < 3.0,
            "{path} at {sample_bpm}: {f} Hz ({cents} cents)"
        );
    }
    // transposing a stretched clip moves only the pitch
    let p = audio_project(120.0, |e| {
        clip(e, "gen:tone", Some(120.0), 0, 0, 4 * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Stretch;
            a.pitch_semitones = -12;
        });
    });
    let f = zero_cross_hz(&render(&p)[0]);
    assert!((1200.0 * (f / 220.0).log2()).abs() < 10.0, "{f} Hz");
}

#[test]
fn slice_retimes_each_transient_without_stretching_it() {
    // the 120 BPM click loop in a 150 BPM song: each click lands on its beat, unchanged
    let p = audio_project(150.0, |e| {
        clip(e, "gen:clicks", Some(120.0), 0, 0, 8 * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Slice;
        });
    });
    let out = render(&p);
    let b = beat(150.0);
    let found = onsets(&out[0], 0.05, 2000);
    assert_eq!(found.len(), 8, "{found:?}");
    let src = &gen_audio("gen:clicks").unwrap()[0];
    let src_on = onsets(src, 0.05, 2000);
    for (k, f) in found.iter().enumerate() {
        assert!(
            (*f as i64 - (k * b + src_on[0]) as i64).abs() <= 8,
            "click {k} at {f}, beat at {}",
            k * b
        );
        // unstretched: the click's first 5 ms match the source sample for sample
        for i in 0..240 {
            assert_close(
                out[0][f + i],
                src[src_on[k] + i],
                1e-3,
                &format!("click {k} frame {i}"),
            );
        }
    }
}

#[test]
fn seeking_and_looping_inside_clips_matches_continuous_playback() {
    let p = audio_project(120.0, |e| {
        clip(e, "gen:loop", None, 0, 0, 4 * PPQ, |a| a.gain_db = -3.0);
        clip(e, "gen:clicks", Some(120.0), 1, PPQ, 4 * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Slice;
        });
    });
    let b = beat(120.0);
    let full = run_offline(&p, &[(0, Act::T(TransportCommand::Play))], 5 * b, 256, 0);
    // seek to beat 2 after half a beat
    let sc = vec![
        (0, Act::T(TransportCommand::Play)),
        (b / 2, Act::T(TransportCommand::Seek(2 * PPQ))),
    ];
    let seek = run_offline(&p, &sc, 3 * b, 256, 0);
    for c in 0..2 {
        assert_eq!(seek[c][b / 2..3 * b], full[c][2 * b..2 * b + 5 * b / 2]);
    }
    // a loop region from beat 1 to 3: both clips cross its end
    let sc = vec![
        (0, Act::T(TransportCommand::SetLoop(Some((PPQ, 3 * PPQ))))),
        (0, Act::T(TransportCommand::Seek(PPQ))),
        (0, Act::T(TransportCommand::Play)),
    ];
    let looped = run_offline(&p, &sc, 5 * b, 333, 0);
    for c in 0..2 {
        assert_eq!(looped[c][..2 * b], full[c][b..3 * b], "first pass");
        assert_eq!(looped[c][2 * b..4 * b], full[c][b..3 * b], "after the wrap");
    }
}

/// Resolves like [`GenAssets`] but never streams (everything is preloaded).
struct Preloaded;
impl AssetResolver for Preloaded {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        GenAssets.sample(r)
    }
}

#[test]
fn a_streamed_clip_plays_exactly_like_a_preloaded_one() {
    let p = audio_project(120.0, |e| {
        clip(e, "gen:long", None, 0, 0, 16 * PPQ, |a| {
            a.source_offset = 12_345
        });
        clip(e, "gen:long", None, 1, 2 * PPQ, 8 * PPQ, |a| {
            a.reverse = true;
            a.pitch_semitones = 3;
        });
    });
    let a = render_offline(&p, &song(512), &GenAssets).unwrap();
    let b = render_offline(&p, &song(512), &Preloaded).unwrap();
    assert!(peak(&a.channels[0]) > 0.1);
    assert_eq!(
        a.channels, b.channels,
        "streamed (offline: synchronous) vs preloaded"
    );
    // and the simulated device (prefetch thread, waiting reader) agrees
    let frames = a.frames();
    let s = run_sim(
        &p,
        &[(0, Act::T(TransportCommand::Play))],
        frames,
        256,
        256,
        2,
    );
    assert_bits_eq(
        &[a.channels[0].clone(), a.channels[1].clone()],
        &s,
        "simulated",
    );
}

/// A stream that takes 30 ms per read: a disk that cannot keep up.
struct SlowAssets;
struct SlowStream;
impl SampleStream for SlowStream {
    fn frames(&self) -> u64 {
        GenStream.frames()
    }
    fn sample_rate(&self) -> f64 {
        SR as f64
    }
    fn channels(&self) -> usize {
        2
    }
    fn read(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, StreamError> {
        std::thread::sleep(std::time::Duration::from_millis(30));
        GenStream.read(start, out)
    }
}
impl AssetResolver for SlowAssets {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        GenAssets.sample(r)
    }
    fn stream(&self, r: &str) -> Option<Result<Box<dyn SampleStream>, String>> {
        (r == "gen:long").then(|| Ok(Box::new(SlowStream) as Box<dyn SampleStream>))
    }
}

#[test]
fn a_live_reader_counts_underruns_instead_of_waiting() {
    let p = audio_project(120.0, |e| {
        clip(e, "gen:long", None, 0, 0, 16 * PPQ, |_| {});
    });
    let mut off = crate::offline::Offline::with_policy(
        &p,
        settings(256),
        &SlowAssets,
        0,
        crate::audio_clips::StreamPolicy::Live,
    )
    .unwrap();
    off.transport(TransportCommand::Play);
    let t0 = std::time::Instant::now();
    let out = off.render(48_000, 256);
    assert!(
        t0.elapsed() < std::time::Duration::from_millis(1500),
        "the reader waited for the disk"
    );
    let underruns = off.underruns();
    assert!(underruns > 0, "no underrun counted");
    assert!(
        out[0][..256].iter().all(|v| *v == 0.0),
        "missing audio is silence"
    );
    // a fast disk: none
    let mut off = crate::offline::Offline::with_policy(
        &p,
        settings(256),
        &GenAssets,
        0,
        crate::audio_clips::StreamPolicy::Wait,
    )
    .unwrap();
    off.transport(TransportCommand::Play);
    off.render(48_000, 256);
    assert_eq!(off.underruns(), 0);
}

#[test]
fn engine_stats_report_streaming_underruns() {
    let (engine, mut dev) = sim(256, 1);
    let p = audio_project(120.0, |e| {
        clip(e, "gen:long", None, 0, 0, 4 * PPQ, |_| {});
    });
    engine.set_project(Arc::new(p)).unwrap();
    engine.transport(TransportCommand::Play);
    for _ in 0..50 {
        dev.pull(256);
    }
    let st = engine.stats();
    assert_eq!(st.underruns, 0);
    assert!(st.warnings.is_empty(), "{:?}", st.warnings);
    assert!(
        st.nodes
            .iter()
            .any(|n| n.owner == Some(ObjectPath::Track(0))),
        "the audio track has a CPU meter"
    );
}

#[test]
fn an_unchanged_track_keeps_its_voices_across_plan_swaps() {
    // a stretched clip, then an edit elsewhere mid-clip: the track node moves across, so the
    // output is the same as without the edit
    let p = audio_project(120.0, |e| {
        clip(e, "gen:tone", Some(100.0), 0, 0, 4 * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Stretch;
        });
        clip(e, "gen:dc", None, 1, 0, 2 * PPQ, |a| a.gain_db = -40.0);
    });
    let mut e = Edit::new(&p);
    let dc = *p.clips.keys().last().unwrap();
    e.update_clip(dc, |c| {
        if let ClipKind::Audio(a) = &mut c.kind {
            a.gain_db = -30.0;
        }
    })
    .unwrap();
    // gain edit on another track; this track is untouched
    let edited = Arc::new(e.finish().0);
    let b = beat(120.0);
    let plain = run_offline(&p, &[(0, Act::T(TransportCommand::Play))], 3 * b, 128, 0);
    let sc = vec![
        (0, Act::T(TransportCommand::Play)),
        (b + 128, Act::P(edited)),
    ];
    let swapped = run_offline(&p, &sc, 3 * b, 128, 0);
    // the stretched track is identical; only the dc track's level changed in the second beat
    let dc0 = 0.5f32;
    let g40 = 10f32.powf(-40.0 / 20.0) * dc0;
    let g30 = 10f32.powf(-30.0 / 20.0) * dc0;
    for i in (b + 256..2 * b - 256).step_by(101) {
        let tone_plain = plain[0][i] - g40;
        let tone_swapped = swapped[0][i] - g30;
        assert_close(tone_swapped, tone_plain, 1e-5, &format!("frame {i}"));
    }
}

#[test]
fn stretch_voices_are_prewarmed_before_the_loop_wraps() {
    // a stretched clip from the loop start: the second pass (pre-warmed during the first)
    // is bit-identical to the first (started at the clip start), at any block size
    let p = audio_project(120.0, |e| {
        clip(e, "gen:tone", Some(100.0), 0, 0, 8 * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Stretch;
        });
        clip(e, "gen:loop", Some(100.0), 1, 0, 8 * PPQ, |a| {
            a.tempo_sync = true;
            a.stretch = StretchMode::Stretch;
            a.reverse = true;
        });
    });
    let b = beat(120.0);
    let sc = vec![
        (0, Act::T(TransportCommand::SetLoop(Some((0, 4 * PPQ))))),
        (0, Act::T(TransportCommand::Play)),
    ];
    let mut first = None;
    for block in [64, 512, 37] {
        let out = run_offline(&p, &sc, 12 * b, block, 0);
        assert!(peak(&out[0]) > 0.1);
        for ch in &out {
            assert_eq!(ch[..4 * b], ch[4 * b..8 * b], "second pass, block {block}");
            assert_eq!(ch[..4 * b], ch[8 * b..], "third pass, block {block}");
        }
        match &first {
            None => first = Some(out),
            Some(f) => assert_bits_eq(f, &out, &format!("block {block}")),
        }
    }
}

#[test]
fn the_loop_wrap_does_not_warm_every_stretch_voice_in_one_block() {
    // four stretched clips from the loop start: without pre-warming, the wrap block would
    // compute an output latency of quanta for each of them at once
    let p = audio_project(120.0, |e| {
        for t in 0..4 {
            clip(
                e,
                "gen:tone",
                Some(90.0 + 10.0 * t as f64),
                t,
                0,
                8 * PPQ,
                |a| {
                    a.tempo_sync = true;
                    a.stretch = StretchMode::Stretch;
                },
            );
        }
    });
    let mut off = crate::offline::Offline::new(&p, settings(256), &GenAssets, 0).unwrap();
    off.transport(TransportCommand::SetLoop(Some((0, 4 * PPQ))));
    off.transport(TransportCommand::Play);
    let lead_quanta = {
        let mut st = crate::audio_clips::stretcher_for(StretchMode::Stretch).unwrap();
        st.configure(1, SR as f64, 256);
        st.output_latency() / crate::player::QUANTUM
    };
    let mut worst = 0;
    // the first block warms everything up from the start (playback starts there); measure
    // the next 3 passes
    off.render(256, 256);
    for _ in 0..(3 * 4 * beat(120.0) / 256) {
        let before = crate::player::QUANTA.with(|q| q.get());
        off.render(256, 256);
        worst = worst.max(crate::player::QUANTA.with(|q| q.get()) - before);
    }
    assert!(lead_quanta >= 8, "latency {lead_quanta} quanta");
    // each voice: its own quantum plus at most a 3-quantum pre-warm budget
    assert!(
        worst <= 4 * 4,
        "{worst} quanta in one block (one warm-up is {lead_quanta})"
    );
}
