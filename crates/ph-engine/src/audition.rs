//! The browser's preview voice ([`crate::Engine::audition`]).
//!
//! - It sits outside the project graph: the audio thread adds it to the device output after
//!   the master insert (and after the metronome), at a fixed preview level. Meters, the
//!   offline render and exports never see it.
//! - The control thread resolves and decodes the sample, resamples it to the engine rate and
//!   bakes in the gain, all off the audio thread, then posts the finished [`Voice`] through a
//!   [`Mailbox`]. The audio thread only copies samples.
//! - Voices are never freed on the audio thread: a replaced or finished voice goes back
//!   through a second mailbox and the control thread frees it on its next audition call.

use crate::rt::Mailbox;
use crate::{AudioBuffer, EngineError, EngineResult, PlayMode, RenderSettings};
use ph_core::{Edit, InstrumentSpec, NoteSpec, PPQ, Project, Tick};
use ph_node::{AssetResolver, Quality, SampleData};

/// Fixed preview level (about -6 dB), scaled by the velocity.
pub(crate) const PREVIEW_LEVEL: f32 = 0.5;
/// Longest preview kept (seconds); longer files are cut, with a short fade.
pub(crate) const MAX_SECONDS: f64 = 30.0;
/// Fade used by `stop_audition`, and at a cut end (seconds).
const FADE_SECONDS: f64 = 0.005;

/// A decoded, resampled preview, ready to copy out.
pub(crate) struct Voice {
    /// Left and right at the engine rate, gain applied.
    pub frames: [Vec<f32>; 2],
    pub pos: usize,
    /// Frames left in a stop fade, and its length.
    pub fade: Option<(usize, usize)>,
    fade_len: usize,
}

impl Voice {
    /// Builds a voice from decoded audio. Off the audio thread (allocates).
    pub fn new(data: &SampleData, engine_rate: u32, gain: f32) -> Voice {
        let src_rate = if data.sample_rate > 0.0 {
            data.sample_rate
        } else {
            engine_rate as f64
        };
        let ratio = src_rate / engine_rate.max(1) as f64;
        let src_frames = data.frames();
        let max_out = (MAX_SECONDS * engine_rate as f64) as usize;
        let out_frames = ((src_frames as f64 / ratio).floor() as usize).min(max_out);
        let left = data.channels.first();
        let right = data.channels.get(1).or(left);
        let resample = |ch: Option<&Vec<f32>>| -> Vec<f32> {
            let Some(ch) = ch else {
                return vec![0.0; out_frames];
            };
            (0..out_frames)
                .map(|i| {
                    let x = i as f64 * ratio;
                    let j = x as usize;
                    let t = (x - j as f64) as f32;
                    let a = ch.get(j).copied().unwrap_or(0.0);
                    let b = ch.get(j + 1).copied().unwrap_or(0.0);
                    (a + (b - a) * t) * gain
                })
                .collect()
        };
        let mut frames = [resample(left), resample(right)];
        let fade_len = ((FADE_SECONDS * engine_rate as f64) as usize).max(1);
        if out_frames == max_out && src_frames as f64 / ratio > max_out as f64 {
            // cut: fade the last few milliseconds instead of clicking
            let n = fade_len.min(out_frames);
            for ch in &mut frames {
                for k in 0..n {
                    ch[out_frames - n + k] *= 1.0 - (k + 1) as f32 / n as f32;
                }
            }
        }
        Voice {
            frames,
            pos: 0,
            fade: None,
            fade_len,
        }
    }

    pub fn finished(&self) -> bool {
        self.pos >= self.frames[0].len() || matches!(self.fade, Some((0, _)))
    }

    /// An empty voice: posting it stops the current one.
    pub fn silence() -> Voice {
        Voice {
            frames: [Vec::new(), Vec::new()],
            pos: 0,
            fade: None,
            fade_len: 1,
        }
    }

    /// Starts the stop fade (audio thread).
    pub fn stop(&mut self) {
        if self.fade.is_none() {
            self.fade = Some((self.fade_len, self.fade_len));
        }
    }

    /// Adds the next `l.len()` frames into `l`/`r` (audio thread; no allocation).
    pub fn mix_into(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let [a, b] = &self.frames;
        for i in 0..n {
            if self.finished() {
                return;
            }
            let mut g = 1.0;
            if let Some((left, len)) = &mut self.fade {
                g = *left as f32 / *len as f32;
                *left -= 1;
            }
            l[i] += a[self.pos] * g;
            r[i] += b[self.pos] * g;
            self.pos += 1;
        }
    }
}

/// The two mailboxes between `Engine` (control side) and the runtime (audio side).
pub(crate) struct Port {
    /// New voices, control → audio.
    pub incoming: Mailbox<Voice>,
    /// Voices to free, audio → control.
    pub retired: Mailbox<Voice>,
}

impl Port {
    pub fn new() -> Self {
        Self {
            incoming: Mailbox::new(),
            retired: Mailbox::new(),
        }
    }

    /// Control side: frees a retired voice, then posts `voice` (freeing one that was posted
    /// but never picked up).
    pub fn post(&self, voice: Box<Voice>) {
        drop(self.retired.take());
        drop(self.incoming.put(voice));
    }

    /// Control side: frees whatever the audio thread handed back.
    pub fn collect(&self) {
        drop(self.retired.take());
    }
}

/// Audio side: the playing voice, one fading out after being replaced or stopped, and one
/// waiting to be installed.
pub(crate) struct Player {
    pub current: Option<Box<Voice>>,
    fading: Option<Box<Voice>>,
    pending: Option<Box<Voice>>,
}

impl Player {
    pub fn new() -> Self {
        Self {
            current: None,
            fading: None,
            pending: None,
        }
    }

    /// Once per callback, before mixing. Real-time safe: nothing is freed here.
    ///
    /// - A newly posted voice replaces the current one, which fades out over a few
    ///   milliseconds (a stop is posted as an empty voice, so a stop and a new audition are
    ///   ordered exactly as the control thread posted them).
    /// - A faded or finished voice goes back to the control side through the return slot. If
    ///   that slot is still full, the voice is kept (silent) and a new one waits; the control
    ///   side empties the slot on every `audition`, `stop_audition` and `stats` call.
    pub fn poll(&mut self, port: &Port) {
        if self.fading.as_ref().is_some_and(|v| v.finished())
            && let Some(f) = self.fading.take()
            && let Err(f) = port.retired.try_put(f)
        {
            self.fading = Some(f);
        }
        if self.pending.is_none() {
            self.pending = port.incoming.take();
        }
        if self.pending.is_some() && self.fading.is_none() {
            if let Some(mut old) = self.current.take() {
                old.stop();
                self.fading = Some(old);
            }
            self.current = self.pending.take();
        }
    }

    pub fn mix_into(&mut self, l: &mut [f32], r: &mut [f32]) {
        if let Some(v) = self.fading.as_mut() {
            v.mix_into(l, r);
        }
        if let Some(v) = self.current.as_mut() {
            v.mix_into(l, r);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // the runtime is dropped off the audio thread (device teardown or offline)
        drop(self.current.take());
        drop(self.fading.take());
        drop(self.pending.take());
    }
}

/// Renders one note of `instrument` for an instrument audition (off the audio thread).
pub(crate) fn render_instrument(
    instrument: &InstrumentSpec,
    key: u8,
    seconds: f64,
    sample_rate: u32,
    assets: &dyn AssetResolver,
) -> EngineResult<AudioBuffer> {
    let base = Project::empty("audition");
    let mut e = Edit::new(&base);
    let bad = |err: ph_core::ModelError| EngineError::Plan(err.to_string());
    let ch = e.add_channel("audition", instrument.clone()).map_err(bad)?;
    e.update_channel(ch, |c| c.volume = 1.0).map_err(bad)?;
    let pat = e.add_pattern("audition").map_err(bad)?;
    let bpm = base.settings.bpm;
    let len = ((seconds.max(0.05) * bpm / 60.0) * PPQ as f64) as Tick;
    e.add_note(pat, ch, NoteSpec::new(key, 0, len))
        .map_err(bad)?;
    let (project, _) = e.finish();
    let settings = RenderSettings {
        mode: PlayMode::Pattern(pat),
        range: Some((0, len)),
        sample_rate,
        quality: Quality::Live,
        include_tail: true,
        block_frames: 512,
    };
    let mut audio = crate::render_offline(&project, &settings, assets)?;
    let cap = ((seconds.max(0.05) + 3.0) * sample_rate as f64) as usize;
    for c in &mut audio.channels {
        c.truncate(cap);
    }
    Ok(audio)
}
