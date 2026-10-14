//! Audio clips in the engine (M2/T0 interfaces, M2/T1 playback).
//!
//! - [`plan_audio_clips`] compiles the model's audio clips into [`AudioClipPlan`]s: song
//!   positions in samples and ticks, the played source range, how the source position
//!   follows the song (tempo-synced clips follow the [`TempoMap`], so they track tempo
//!   changes), gain and fades. Control thread.
//! - The render plan gives every playlist track with audio clips one **audio-track node**
//!   (`crate::player`), summed into the master insert after the channels. Its voices play
//!   the clips sample-accurately: [`StretchMode::Resample`] and [`StretchMode::Slice`] by
//!   windowed-sinc interpolation (`ph_dsp::interp`), [`StretchMode::Stretch`] through a
//!   [`Stretcher`] (Signalsmith Stretch, MIT, via `ph-stretch`).
//! - Long sources stream from disk through the prefetch thread ([`Prefetcher`],
//!   `crate::stream`); short ones are decoded whole.
//!
//! Everything a voice computes depends only on the song position, never on the block size
//! or the thread, so live playback is bit-identical to an offline render.

use crate::EngineResult;
use crate::tempo::TempoMap;
use ph_core::*;

pub use crate::stream::{BLOCK_FRAMES, Prefetcher, STREAM_MIN_SECONDS, StreamPolicy};

/// One audio clip, ready to play.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioClipPlan {
    pub clip: ClipId,
    pub track: u32,
    pub sample: SampleId,
    /// Asset reference of the registry entry.
    pub path: String,
    /// Song samples covered: `start..end`.
    pub start: i64,
    pub end: i64,
    /// Song ticks covered: `start_tick..end_tick`.
    pub start_tick: Tick,
    pub end_tick: Tick,
    /// First source frame played (the start of the played range, also when reversed).
    pub source_start: u64,
    /// Tempo-synced (and the sample has a tempo): the source position follows song *ticks*,
    /// [`AudioClipPlan::source_per_tick`] frames per tick, so it follows tempo changes.
    /// Otherwise it follows song *samples* at [`AudioClipPlan::speed`].
    pub synced: bool,
    /// Source frames per song tick when `synced` (pitch factor included for `Resample`).
    pub source_per_tick: f64,
    /// Source frames per output frame, sample-rate conversion included:
    /// `AudioClip::time_ratio × source rate / engine rate`, at the tempo at the clip start
    /// (exact for unsynced clips; a synced clip's speed changes with the tempo).
    pub speed: f64,
    /// Transposition not carried by `speed`, in semitones: 0 for `Resample` (the speed
    /// carries it), the clip's pitch for `Stretch` (the time-stretcher shifts it) and `Slice`
    /// (each slice is played faster or slower by it, unstretched, from its own start).
    pub transpose: f64,
    /// Linear gain from `gain_db`.
    pub gain: f32,
    /// Fade lengths in output samples, with their curves.
    pub fade_in: (u64, FadeCurve),
    pub fade_out: (u64, FadeCurve),
    pub reverse: bool,
    pub stretch: StretchMode,
    /// The file's sample rate and length according to the registry.
    pub file_rate: f64,
    pub file_frames: u64,
}

impl AudioClipPlan {
    /// Source frames the clip covers from `source_start` (the played range), before clipping
    /// to the file: the source position at the clip end.
    pub fn span(&self, tempo: &TempoMap) -> f64 {
        self.pos(tempo, self.end)
    }

    /// Source frames the clip plays (the span rounded up, clipped to the file).
    pub fn source_frames(&self, tempo: &TempoMap, file_frames: u64) -> u64 {
        let want = self.span(tempo).max(0.0).ceil() as u64;
        want.min(file_frames.saturating_sub(self.source_start))
    }

    /// Position in the played range (0 = the first frame played, counting in play order,
    /// so backwards through the file when reversed), in source frames, at song sample `s`.
    /// Valid for any `s` (negative before the clip start).
    pub fn pos(&self, tempo: &TempoMap, s: i64) -> f64 {
        if self.synced {
            (tempo.tick_at(s) - self.start_tick as f64) * self.source_per_tick
        } else {
            (s - self.start) as f64 * self.speed
        }
    }

    /// Fade gain at output sample `i` of the clip (0-based), fade-in and fade-out combined.
    pub fn fade_gain(&self, i: u64) -> f64 {
        let len = (self.end - self.start) as u64;
        let mut g = 1.0;
        let (fi, ci) = self.fade_in;
        if fi > 0 && i < fi {
            g *= ci.gain(i as f64 / fi as f64);
        }
        let (fo, co) = self.fade_out;
        if fo > 0 && i + fo >= len {
            g *= co.gain(len.saturating_sub(i) as f64 / fo as f64);
        }
        g
    }
}

/// Compiles every audio clip on an unmuted track, sorted by (start, clip id), plus a warning
/// for each clip that cannot play as asked (a missing registry entry, a synced clip whose
/// sample has no tempo). Off the audio thread.
pub fn plan_audio_clips(project: &Project, tempo: &TempoMap) -> (Vec<AudioClipPlan>, Vec<String>) {
    let rate = tempo.sample_rate() as f64;
    let mut plans = Vec::new();
    let mut warnings = Vec::new();
    for c in project.clips.values() {
        let Some(a) = c.audio() else { continue };
        if project
            .tracks
            .get(c.track as usize)
            .is_some_and(|t| t.muted)
        {
            continue;
        }
        let Ok(entry) = project.sample(a.sample) else {
            warnings.push(format!(
                "audio clip {}: sample {} is not registered",
                c.id, a.sample
            ));
            continue;
        };
        let has_tempo = entry.bpm().is_some_and(|b| b > 0.0);
        if a.tempo_sync && !has_tempo {
            warnings.push(format!(
                "audio clip {}: {} has no tempo, so it plays unsynced",
                c.id, entry.name
            ));
        }
        let start = tempo.sample_at(c.start);
        let end = tempo.sample_at(c.start + c.length);
        let to_samples = |t: Tick| (tempo.sample_at(c.start + t.min(c.length)) - start) as u64;
        let file_rate = entry.sample_rate.max(1) as f64;
        let synced = a.tempo_sync && has_tempo;
        // at 60 BPM a beat is one second, so the ratio there is source seconds per beat
        let source_per_tick = if synced {
            a.time_ratio(entry, 60.0) * file_rate / PPQ as f64
        } else {
            0.0
        };
        let ratio = a.time_ratio(entry, tempo.bpm_at(c.start));
        plans.push(AudioClipPlan {
            clip: c.id,
            track: c.track,
            sample: a.sample,
            path: entry.path.clone(),
            start,
            end,
            start_tick: c.start,
            end_tick: c.start + c.length,
            source_start: a.source_offset,
            synced,
            source_per_tick,
            speed: ratio * file_rate / rate,
            transpose: match a.stretch {
                StretchMode::Resample => 0.0,
                StretchMode::Stretch | StretchMode::Slice => a.pitch(),
            },
            gain: 10f32.powf(a.gain_db / 20.0),
            fade_in: (to_samples(a.fade_in.length), a.fade_in.curve),
            fade_out: (to_samples(a.fade_out.length), a.fade_out.curve),
            reverse: a.reverse,
            stretch: a.stretch,
            file_rate,
            file_frames: entry.frames,
        });
    }
    plans.sort_by_key(|p| (p.start, p.clip));
    (plans, warnings)
}

/// A time-stretch and pitch-shift engine (shaped after Signalsmith Stretch). One instance
/// per playing clip voice.
pub trait Stretcher: std::marker::Send {
    /// Allocates for `channels` at `sample_rate`, for blocks of up to `max_frames` output
    /// frames. Off the audio thread.
    fn configure(&mut self, channels: usize, sample_rate: f64, max_frames: usize);
    /// Real-time. Forgets all buffered audio (seek, loop).
    fn reset(&mut self);
    /// Input frames to feed ahead of the first output frame.
    fn input_latency(&self) -> usize;
    /// Output frames before the input's first frame appears.
    fn output_latency(&self) -> usize;
    /// Real-time. Pitch shift applied from now on, in semitones.
    fn set_transpose(&mut self, semitones: f64);
    /// Real-time. Consumes all of `input` (`[channel][frame]`) and produces all of `output`;
    /// the time ratio of this call is `input frames / output frames`.
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]);
    /// Real-time. Pre-roll after a [`Stretcher::reset`]: the [`Stretcher::preroll`] frames
    /// just before the next input, fed without producing output, for playback at `rate`
    /// (input ÷ output frames). Added in M2/T1; the default ignores it.
    fn seek(&mut self, input: &[&[f32]], rate: f64) {
        let _ = (input, rate);
    }
    /// Frames [`Stretcher::seek`] wants (0 = no pre-roll).
    fn preroll(&self) -> usize {
        0
    }
}

/// Signalsmith Stretch (MIT) as a [`Stretcher`]: the `Stretch` mode.
pub struct SignalsmithStretcher {
    inner: Option<ph_stretch::Stretch>,
    preset: ph_stretch::Preset,
    seed: u32,
}

impl SignalsmithStretcher {
    /// Unconfigured; [`Stretcher::configure`] allocates. `seed` drives the phase
    /// randomisation Signalsmith uses for stretches slower than 0.5×.
    pub fn new(preset: ph_stretch::Preset, seed: u32) -> Self {
        Self {
            inner: None,
            preset,
            seed,
        }
    }
}

impl Stretcher for SignalsmithStretcher {
    fn configure(&mut self, channels: usize, sample_rate: f64, _max_frames: usize) {
        self.inner = ph_stretch::Stretch::new(channels, sample_rate, self.preset, self.seed);
    }
    fn reset(&mut self) {
        if let Some(s) = self.inner.as_mut() {
            s.reset();
        }
    }
    fn input_latency(&self) -> usize {
        self.inner.as_ref().map_or(0, |s| s.input_latency())
    }
    fn output_latency(&self) -> usize {
        self.inner.as_ref().map_or(0, |s| s.output_latency())
    }
    fn set_transpose(&mut self, semitones: f64) {
        if let Some(s) = self.inner.as_mut() {
            s.set_transpose(semitones);
        }
    }
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        match self.inner.as_mut() {
            Some(s) => s.process(input, output),
            None => {
                for c in output.iter_mut() {
                    c.fill(0.0);
                }
            }
        }
    }
    fn seek(&mut self, input: &[&[f32]], rate: f64) {
        if let Some(s) = self.inner.as_mut() {
            s.seek(input, rate);
        }
    }
    fn preroll(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, |s| s.block_samples() + s.interval_samples())
    }
}

/// Varispeed (turntable) as a [`Stretcher`]: the time ratio changes speed and pitch together
/// (cubic Hermite interpolation; `set_transpose` has no effect). The `Resample` and `Slice`
/// modes' behaviour; the clip player itself interpolates from absolute source positions.
pub struct Varispeed {
    hist: Vec<[f32; 3]>,
    phase: f64,
}

impl Default for Varispeed {
    fn default() -> Self {
        Self::new()
    }
}

impl Varispeed {
    pub fn new() -> Self {
        Self {
            hist: Vec::new(),
            phase: 0.0,
        }
    }
}

impl Stretcher for Varispeed {
    fn configure(&mut self, channels: usize, _sample_rate: f64, _max_frames: usize) {
        self.hist = vec![[0.0; 3]; channels.max(1)];
        self.phase = 0.0;
    }
    fn reset(&mut self) {
        self.hist.fill([0.0; 3]);
        self.phase = 0.0;
    }
    fn input_latency(&self) -> usize {
        0
    }
    fn output_latency(&self) -> usize {
        2
    }
    fn set_transpose(&mut self, _semitones: f64) {}
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let n_in = input.first().map_or(0, |c| c.len());
        let n_out = output.first().map_or(0, |c| c.len());
        let step = if n_out > 0 {
            n_in as f64 / n_out as f64
        } else {
            0.0
        };
        let start_phase = self.phase;
        for (c, out) in output.iter_mut().enumerate() {
            let Some(inp) = input.get(c.min(input.len().saturating_sub(1))) else {
                out.fill(0.0);
                continue;
            };
            let h = self.hist.get(c).copied().unwrap_or([0.0; 3]);
            // virtual signal: h[0], h[1], h[2], inp[0], inp[1], ...; output reads from h[1]
            let at = |k: i64| -> f32 {
                if k < 3 {
                    h[k.max(0) as usize]
                } else {
                    inp.get((k - 3) as usize)
                        .copied()
                        .unwrap_or_else(|| inp.last().copied().unwrap_or(0.0))
                }
            };
            for (i, o) in out.iter_mut().enumerate() {
                let p = 1.0 + start_phase + i as f64 * step;
                let k = p.floor() as i64;
                let t = (p - k as f64) as f32;
                *o = ph_dsp::interp::hermite(at(k - 1), at(k), at(k + 1), at(k + 2), t);
            }
            if let Some(hc) = self.hist.get_mut(c) {
                let len = n_in as i64 + 3;
                *hc = [at(len - 3), at(len - 2), at(len - 1)];
            }
        }
        self.phase = (start_phase + n_out as f64 * step) - n_in as f64;
    }
}

/// The stretcher for a mode, unconfigured: Signalsmith Stretch for `Stretch`, varispeed
/// for `Resample` and `Slice`.
pub fn stretcher_for(mode: StretchMode) -> EngineResult<Box<dyn Stretcher>> {
    Ok(match mode {
        StretchMode::Stretch => Box::new(SignalsmithStretcher::new(ph_stretch::Preset::Default, 1)),
        StretchMode::Resample | StretchMode::Slice => Box::new(Varispeed::new()),
    })
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    fn project() -> (Project, ClipId) {
        let mut ed = Edit::new(&Project::empty("a"));
        let s = ed
            .add_sample(SampleEntry {
                id: SampleId(0),
                path: "samples/ramp.wav".into(),
                name: "ramp.wav".into(),
                frames: 1000,
                sample_rate: 44_100,
                channels: 2,
                bpm: Some(SampleBpm {
                    value: 100.0,
                    source: BpmSource::User,
                }),
                root_key: None,
                analysis: Default::default(),
                provenance: Default::default(),
            })
            .unwrap();
        let mut a = AudioClip::new(s);
        a.tempo_sync = true;
        a.gain_db = -6.0;
        a.fade_in = Fade {
            length: PPQ,
            curve: FadeCurve::Linear,
        };
        a.stretch = StretchMode::Stretch;
        a.pitch_semitones = 3;
        a.source_offset = 100;
        let c = ed.add_audio_clip(0, PPQ, 4 * PPQ, a).unwrap();
        (ed.finish().0, c)
    }

    #[test]
    fn plans_carry_units_the_player_needs() {
        let (p, c) = project();
        let tempo = TempoMap::constant(125.0, 48_000);
        let (plans, warnings) = plan_audio_clips(&p, &tempo);
        assert!(warnings.is_empty());
        let pl = &plans[0];
        assert_eq!(pl.clip, c);
        assert_eq!((pl.start, pl.end), (23_040, 115_200));
        assert_eq!((pl.start_tick, pl.end_tick), (PPQ, 5 * PPQ));
        assert_eq!(pl.source_start, 100);
        // 125 / 100 BPM, and 44.1 kHz source into a 48 kHz engine
        assert!((pl.speed - 1.25 * 44_100.0 / 48_000.0).abs() < 1e-12);
        assert!(pl.synced);
        // a beat of a 100 BPM sample at 44.1 kHz is 26 460 frames
        assert!((pl.source_per_tick * PPQ as f64 - 26_460.0).abs() < 1e-9);
        assert!((pl.pos(&tempo, pl.end) - 4.0 * 26_460.0).abs() < 1e-6);
        assert_eq!(pl.pos(&tempo, pl.start), 0.0);
        assert_eq!(pl.transpose, 3.0);
        assert!((pl.gain - 0.501_187).abs() < 1e-5);
        assert_eq!(pl.fade_in, (23_040, FadeCurve::Linear));
        assert_eq!(pl.fade_gain(0), 0.0);
        assert_eq!(pl.fade_gain(11_520), 0.5);
        assert_eq!(pl.fade_gain(23_040), 1.0);
        assert_eq!(pl.source_frames(&tempo, 1000), 900);
        // a tempo change halfway: the synced clip's source follows ticks, not samples
        let mut map = TempoMap::constant(125.0, 48_000);
        map.push_change(3 * PPQ, 62.5);
        let (plans2, _) = plan_audio_clips(&p, &map);
        let q = &plans2[0];
        assert!(
            (q.pos(&map, q.end) - 4.0 * 26_460.0).abs() < 1e-6,
            "same beats played"
        );
        assert!(q.end - q.start > pl.end - pl.start, "but it takes longer");
        // no tempo: warned, plays unsynced
        let mut ed = Edit::new(&p);
        ed.set_sample_bpm(SampleId(0), None).unwrap();
        let (plans, warnings) = plan_audio_clips(ed.project(), &tempo);
        assert_eq!(warnings.len(), 1);
        assert!(!plans[0].synced);
        assert!((plans[0].speed - 44_100.0 / 48_000.0).abs() < 1e-12);
    }

    #[test]
    fn every_mode_has_a_stretcher() {
        for mode in [
            StretchMode::Resample,
            StretchMode::Stretch,
            StretchMode::Slice,
        ] {
            let mut s = stretcher_for(mode).unwrap();
            s.configure(2, 48_000.0, 512);
            s.reset();
            s.set_transpose(2.0);
            let x: Vec<f32> = (0..600).map(|i| (i as f32 * 0.05).sin()).collect();
            let (mut l, mut r) = (vec![0.0; 500], vec![0.0; 500]);
            s.process(&[&x, &x], &mut [&mut l, &mut r]);
            assert!(l.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn varispeed_resamples_continuously_across_calls() {
        let mut v = Varispeed::new();
        v.configure(1, 48_000.0, 64);
        // a ramp read at half speed: output steps by 0.5, delayed by the latency
        let x: Vec<f32> = (0..200).map(|i| i as f32).collect();
        let mut out = Vec::new();
        for k in 0..10 {
            let mut o = vec![0.0; 40];
            v.process(&[&x[k * 20..(k + 1) * 20]], &mut [&mut o]);
            out.extend(o);
        }
        for (i, w) in out.windows(2).enumerate().skip(8) {
            assert!((w[1] - w[0] - 0.5).abs() < 1e-4, "at {i}: {w:?}");
        }
    }
}
