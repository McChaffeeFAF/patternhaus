//! The DESIGN §12 oracle extended to audio clips (M2 criterion 6): a song with pattern
//! clips and audio clips in every stretch mode, fades, reverse, pitch, a streamed source and
//! tempo changes renders bit-identically offline at block sizes 64, 512 and 37 (serial and
//! on the worker pool) and live on the simulated device, through plan swaps, seeks, a loop
//! region crossing clips, mode changes and a stop.

use super::audio::clip;
use super::*;

/// [`beat`] (128 BPM) plus audio clips on tracks 3..=6 and tempo automation on track 7.
pub fn audio_song() -> Project {
    let base = beat();
    let mut e = Edit::new(&base);
    let bar = 4 * PPQ;
    clip(&mut e, "gen:loop", Some(100.0), 3, 0, 2 * bar, |a| {
        a.tempo_sync = true;
        a.pitch_semitones = 2;
        a.fade_in = Fade {
            length: PPQ,
            curve: FadeCurve::Log,
        };
    });
    clip(&mut e, "gen:tone", Some(100.0), 4, 2 * PPQ, 6 * PPQ, |a| {
        a.tempo_sync = true;
        a.stretch = StretchMode::Stretch;
        a.pitch_semitones = -3;
        a.gain_db = -6.0;
        a.fade_out = Fade {
            length: PPQ,
            curve: FadeCurve::SCurve,
        };
    });
    clip(&mut e, "gen:clicks", Some(120.0), 5, bar, bar, |a| {
        a.tempo_sync = true;
        a.stretch = StretchMode::Slice;
        a.fine_cents = 30.0;
    });
    clip(&mut e, "gen:long", None, 6, PPQ, 2 * bar, |a| {
        a.reverse = true;
        a.source_offset = 100_000;
        a.gain_db = -4.0;
    });
    clip(&mut e, "gen:loop", Some(100.0), 3, 2 * bar, bar, |a| {
        a.tempo_sync = true;
        a.stretch = StretchMode::Stretch;
        a.reverse = true;
        a.fade_in = Fade {
            length: PPQ / 4,
            curve: FadeCurve::Exp,
        };
    });
    // tempo: 128, a ramp from 128 to 150 over bar 2, then 150
    let t = e
        .add_automation_clip(7, 0, 4 * bar, ParamPath::Tempo)
        .unwrap();
    e.add_point(t, 0, 128.0, CurveKind::Hold, 0.0).unwrap();
    e.add_point(t, bar, 128.0, CurveKind::Linear, 0.0).unwrap();
    e.add_point(t, 2 * bar, 150.0, CurveKind::Hold, 0.0)
        .unwrap();
    e.finish().0
}

fn edits(base: &Project) -> Vec<Arc<Project>> {
    let mut out = Vec::new();
    // 1: a channel change only (the audio tracks move across untouched)
    let mut e = Edit::new(base);
    e.update_channel(base.channel_order[3], |c| c.volume = 0.5)
        .unwrap();
    out.push(Arc::new(e.finish().0));
    // 2: the stretched clip's gain and the first loop's fade (those tracks are rebuilt)
    let mut e = Edit::new(out.last().unwrap());
    let ids: Vec<ClipId> = base
        .clips
        .values()
        .filter(|c| c.audio().is_some())
        .map(|c| c.id)
        .collect();
    for id in &ids[..2] {
        e.update_clip(*id, |c| {
            if let ClipKind::Audio(a) = &mut c.kind {
                a.gain_db -= 2.0;
                a.fade_in.length = PPQ / 2;
            }
        })
        .unwrap();
    }
    out.push(Arc::new(e.finish().0));
    // 3: a new sliced clip on a new track
    let mut e = Edit::new(out.last().unwrap());
    clip(&mut e, "gen:loop", Some(100.0), 8, 4 * PPQ, 8 * PPQ, |a| {
        a.tempo_sync = true;
        a.stretch = StretchMode::Slice;
    });
    out.push(Arc::new(e.finish().0));
    out
}

fn script(base: &Project) -> Vec<(usize, Act)> {
    let ed = edits(base);
    let pat = base.pattern_order[1];
    vec![
        (0, Act::T(TransportCommand::Play)),
        (20_480, Act::P(ed[0].clone())),
        (40_960, Act::T(TransportCommand::Seek(5 * PPQ + 240))),
        (61_440, Act::P(ed[1].clone())),
        (
            81_920,
            Act::T(TransportCommand::SetLoop(Some((6 * PPQ, 10 * PPQ)))),
        ),
        (102_400, Act::P(ed[2].clone())),
        (
            204_800,
            Act::T(TransportCommand::SetMode(crate::PlayMode::Pattern(pat))),
        ),
        (
            215_040,
            Act::T(TransportCommand::SetMode(crate::PlayMode::Song)),
        ),
        (225_280, Act::T(TransportCommand::Play)),
        (245_760, Act::T(TransportCommand::Stop)),
        (256_000, Act::T(TransportCommand::Play)),
    ]
}

const FRAMES: usize = 286_720;

#[test]
fn audio_clips_render_identically_live_and_offline() {
    let base = audio_song();
    let sc = script(&base);
    let reference = run_offline(&base, &sc, FRAMES, 1000, 0);
    assert!(peak(&reference[0]) > 0.05, "reference render is silent");
    for (block, workers) in [(64, 0), (512, 2), (37, 3)] {
        let o = run_offline(&base, &sc, FRAMES, block, workers);
        assert_bits_eq(
            &reference,
            &o,
            &format!("offline block {block} workers {workers}"),
        );
    }
    for (buffer, pull, threads) in [(256, 256, 1), (128, 512, 4), (512, 1024, 2)] {
        let s = run_sim(&base, &sc, FRAMES, buffer, pull, threads);
        assert_bits_eq(
            &reference,
            &s,
            &format!("simulated buffer {buffer} pull {pull} threads {threads}"),
        );
    }
}

#[test]
fn render_offline_of_an_audio_song_matches_simulated_playback() {
    let base = audio_song();
    let rs = super::audio::song(333);
    let off = crate::render_offline(&base, &rs, &GenAssets).unwrap();
    assert!(peak(&off.channels[0]) > 0.05);
    let sc = vec![(0, Act::T(TransportCommand::Play))];
    let s = run_sim(&base, &sc, off.frames(), 256, 256, 3);
    assert_bits_eq(
        &[off.channels[0].clone(), off.channels[1].clone()],
        &s,
        "render_offline vs simulated",
    );
    // and the song length follows the tempo automation
    let map = crate::automation::AutomationEvaluator::new(&base).tempo_map(
        128.0,
        SR,
        crate::plan::TEMPO_RESOLUTION,
    );
    assert!(map.segments() > 10, "the ramp is many tempo steps");
    assert_eq!(off.frames() as i64, map.sample_at(base.song_length()));
}
