//! The voice model: trigger, release and steal; round-robin state; the seeded RNG.
//!
//! # Determinism (live = offline, bit for bit)
//!
//! All randomness (random round robin, `ModSource::Random`, random LFOs) comes from one
//! [`Rng`] per sampler node, seeded from `ActivateInfo::seed` (the engine derives it from the
//! channel id). The RNG and every round-robin counter return to that seed on
//! `Processor::activate`, `Processor::reset` and `EventKind::AllNotesOff` (the engine sends
//! it when the transport stops). Draws happen only in event order (one per triggering
//! note-on, never per block), so the sequence depends only on the events since the last
//! reset: an offline render (fresh nodes) and live playback from the same start hear the
//! same samples, at any block size.

use crate::plan::Plan;
use ph_core::{RoundRobinMode, StealMode};

/// SplitMix64: small, fast, good enough for musical randomness, and trivially reseedable.
#[derive(Copy, Clone, Debug)]
pub struct Rng {
    seed: u64,
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { seed, state: seed }
    }

    /// Back to the seed. Real-time.
    pub fn reset(&mut self) {
        self.state = self.seed;
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }

    /// Uniform in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Per-group round-robin state.
#[derive(Clone, Debug)]
pub struct RoundRobin {
    /// Next sequential position, per group.
    next: Box<[u32]>,
    /// Last position played, per group (random mode avoids repeating it).
    last: Box<[u32]>,
}

impl RoundRobin {
    pub fn new(groups: usize) -> Self {
        Self {
            next: vec![0; groups].into_boxed_slice(),
            last: vec![u32::MAX; groups].into_boxed_slice(),
        }
    }

    pub fn reset(&mut self) {
        self.next.fill(0);
        self.last.fill(u32::MAX);
    }

    /// The position group `g` plays for this note (call once per note per group). Real-time.
    pub fn advance(&mut self, g: usize, len: u32, mode: RoundRobinMode, rng: &mut Rng) -> u32 {
        if len == 0 {
            return 0;
        }
        let pos = match mode {
            RoundRobinMode::Sequential => {
                let p = self.next[g] % len;
                self.next[g] = (p + 1) % len;
                p
            }
            RoundRobinMode::Random if len == 1 => 0,
            RoundRobinMode::Random => {
                // never the same position twice in a row
                let last = self.last[g];
                if last < len {
                    let r = rng.below(len - 1);
                    if r >= last { r + 1 } else { r }
                } else {
                    rng.below(len)
                }
            }
        };
        self.last[g] = pos;
        pos
    }
}

/// Modulation sums for one voice, in the targets' units (see [`ph_core::ModTarget`]).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Mods {
    /// Cents.
    pub pitch: f32,
    /// dB.
    pub volume: f32,
    /// Pan units.
    pub pan: f32,
    /// Cents.
    pub cutoff: f32,
    /// dB.
    pub resonance: f32,
    /// Frames of the file.
    pub start: f32,
}

/// One sounding zone.
#[derive(Clone, Debug)]
pub struct Voice {
    pub active: bool,
    /// The note it belongs to (`EventKind::NoteOn::note_id`).
    pub note_id: u32,
    pub key: u8,
    /// Index into [`Plan::zones`].
    pub zone: usize,
    /// Index into [`Plan::groups`].
    pub group: usize,
    /// Output bus.
    pub bus: u32,
    /// Read position in frames of the file.
    pub pos: f64,
    /// Frames of the file per output frame at the note's own pitch (always positive).
    pub step: f64,
    /// +1 forwards, -1 backwards (reverse zones, the way back of a ping-pong loop).
    pub dir: f64,
    /// Static gain per channel: levels × velocity × pan (release decay included).
    pub gain: [f32; 2],
    /// Start order, for stealing (higher = newer).
    pub order: u64,
    /// Held: a note-off has not arrived yet.
    pub held: bool,
    /// Note velocity 0..=1, the note's random draw 0..=1, its poly pressure 0..=1.
    pub velocity: f32,
    pub random: f32,
    pub pressure: f32,
    /// Linear gain of the zone, group, instrument and velocity (release decay included),
    /// and the note's own pan: before modulation and parameters.
    pub level: f32,
    pub note_pan: f32,
    /// Crossfade gain from velocity (fixed for the note).
    pub vel_xfade: f32,
    /// Routes with sources fixed at note-on, summed once.
    pub fixed: Mods,
    pub amp: crate::env::Env,
    pub fenv: crate::env::Env,
    pub lfo_phase: [f64; crate::plan::MAX_LFOS],
    pub lfo_hold: [f32; crate::plan::MAX_LFOS],
    pub lfo_rng: u64,
    /// Frames since the note started (LFO delay and fade).
    pub age: u64,
    /// Two stages per channel: `[l1, r1, l2, r2]`.
    pub filt: [ph_dsp::filter::Svf; 4],
    /// Glide offset in semitones (moves to 0 by `glide_step` per frame).
    pub glide: f64,
    pub glide_step: f64,
    /// Frames until the next control update.
    pub ctrl: u32,
    /// Output gain per channel now, and its per-frame ramp to the control target.
    pub g_now: [f32; 2],
    pub g_inc: [f32; 2],
    /// Frames of the file per output frame now (pitch modulation included).
    pub rate: f64,
    /// The zone loop is in effect (false once a sustain loop is left at note-off).
    pub looping: bool,
    /// A steal or choke fade: frames left of `fade_len` (0 = none).
    pub fade: u32,
    pub fade_len: u32,
    /// First file frame held in this voice's stream window (`i64::MIN` = empty).
    pub win_start: i64,
    /// Output level at the last control update (for `quietest` stealing).
    pub loudness: f32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            active: false,
            note_id: 0,
            key: 0,
            zone: 0,
            group: 0,
            bus: 0,
            pos: 0.0,
            step: 1.0,
            dir: 1.0,
            gain: [0.0; 2],
            order: 0,
            held: false,
            velocity: 0.0,
            random: 0.0,
            pressure: 0.0,
            level: 0.0,
            note_pan: 0.0,
            vel_xfade: 1.0,
            fixed: Mods::default(),
            amp: Default::default(),
            fenv: Default::default(),
            lfo_phase: [0.0; crate::plan::MAX_LFOS],
            lfo_hold: [0.0; crate::plan::MAX_LFOS],
            lfo_rng: 0,
            age: 0,
            filt: Default::default(),
            glide: 0.0,
            glide_step: 0.0,
            ctrl: 0,
            g_now: [0.0; 2],
            g_inc: [0.0; 2],
            rate: 1.0,
            looping: false,
            fade: 0,
            fade_len: 0,
            win_start: i64::MIN,
            loudness: 0.0,
        }
    }
}

impl Voice {
    /// True while it sounds and has not been told to give way.
    pub fn counts(&self) -> bool {
        self.active && self.fade == 0
    }

    /// Starts a short fade-out (stolen or choked). Real-time.
    pub fn fade_out(&mut self, frames: u32) {
        if self.active && self.fade == 0 {
            self.fade = frames.max(1);
            self.fade_len = frames.max(1);
        }
    }
}

/// What a trigger asks for.
#[derive(Copy, Clone, Debug)]
pub struct Trigger {
    pub note_id: u32,
    pub key: u8,
    pub zone: usize,
    pub group: usize,
    pub bus: u32,
    pub pos: f64,
    /// Negative when the zone plays backwards.
    pub step: f64,
    pub gain: [f32; 2],
}

/// Spare voices beyond the instrument's limit, for the fades of stolen voices.
pub const SPARE_VOICES: usize = 16;

/// A fixed pool of voices, allocated in `activate`: the instrument's limit plus
/// [`SPARE_VOICES`] so a stolen voice can fade while its successor starts.
pub struct VoicePool {
    pub voices: Box<[Voice]>,
    order: u64,
    /// Steal and choke fade, frames.
    pub fade_frames: u32,
}

impl VoicePool {
    /// A pool for an instrument limit of `n` voices.
    pub fn new(n: usize) -> Self {
        Self {
            voices: vec![Voice::default(); n.max(1) + SPARE_VOICES].into_boxed_slice(),
            order: 0,
            fade_frames: 144,
        }
    }

    /// Slots in a pool for an instrument limit of `n` (what a stream source must serve).
    pub fn capacity_for(n: usize) -> usize {
        n.max(1) + SPARE_VOICES
    }

    pub fn reset(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
            v.fade = 0;
        }
        self.order = 0;
    }

    /// Voices sounding (fading ones included).
    pub fn active(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Starts a voice, stealing one when the group's or the instrument's limit is reached
    /// (the group's [`StealMode`] decides which; `None` refuses the new voice). A stolen voice
    /// fades out over `fade_frames` while its successor starts in a free slot. Returns the
    /// voice index; the caller sets up the rest of the voice. Real-time.
    pub fn trigger(&mut self, plan: &Plan, t: Trigger) -> Option<usize> {
        let g = &plan.groups[t.group];
        if let Some(limit) = g.voice_limit {
            let in_group = self
                .voices
                .iter()
                .filter(|v| v.counts() && v.group == t.group)
                .count();
            if in_group >= limit as usize {
                let victim = self.steal(g.stealing, Some(t.group))?;
                let f = self.fade_frames;
                self.voices[victim].fade_out(f);
            }
        }
        let counting = self.voices.iter().filter(|v| v.counts()).count();
        if counting >= plan.voice_limit {
            let victim = self.steal(g.stealing, None)?;
            let f = self.fade_frames;
            self.voices[victim].fade_out(f);
        }
        let slot = match self.voices.iter().position(|v| !v.active) {
            Some(i) => i,
            // every spare is still fading: cut the fade nearest its end
            None => self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.fade > 0)
                .min_by_key(|(_, v)| (v.fade, v.order))
                .map(|(i, _)| i)?,
        };
        Some(self.start(slot, t))
    }

    fn start(&mut self, i: usize, t: Trigger) -> usize {
        self.order += 1;
        let v = &mut self.voices[i];
        v.active = true;
        v.note_id = t.note_id;
        v.key = t.key;
        v.zone = t.zone;
        v.group = t.group;
        v.bus = t.bus;
        v.pos = t.pos;
        v.step = t.step.abs();
        v.dir = if t.step < 0.0 { -1.0 } else { 1.0 };
        v.gain = t.gain;
        v.order = self.order;
        v.held = true;
        v.fade = 0;
        v.fade_len = 0;
        v.win_start = i64::MIN;
        i
    }

    /// The voice to give way, among sounding voices that are not already fading (of `group`
    /// when given). Real-time.
    pub fn steal(&mut self, mode: StealMode, group: Option<usize>) -> Option<usize> {
        let candidates = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.counts() && group.is_none_or(|g| v.group == g));
        let pick = match mode {
            StealMode::None => None,
            StealMode::Oldest => candidates.min_by_key(|(_, v)| v.order),
            StealMode::Newest => candidates.max_by_key(|(_, v)| v.order),
            StealMode::Quietest => candidates.min_by(|(_, a), (_, b)| {
                a.loudness
                    .total_cmp(&b.loudness)
                    .then(a.order.cmp(&b.order))
            }),
        };
        pick.map(|(i, _)| i)
    }

    /// Note-off for `note_id`: held voices release their amp and filter envelopes (one-shot
    /// zones play on), and sustain loops (`exit`) are left. Real-time.
    pub fn release(&mut self, plan: &Plan, note_id: u32) {
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.active && v.held && v.note_id == note_id)
        {
            v.held = false;
            let z = &plan.zones[v.zone];
            if z.looping.as_ref().is_some_and(|l| l.exit_on_release) {
                v.looping = false;
            }
            if !z.one_shot {
                v.amp.release();
                v.fenv.release();
            }
        }
    }

    /// Mono and legato groups: the voices of `group` give way to a new note with a short
    /// fade. Real-time.
    pub fn choke_group(&mut self, group: usize) {
        let f = self.fade_frames;
        for v in self.voices.iter_mut().filter(|v| v.group == group) {
            v.fade_out(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_reproducible_and_resettable() {
        let mut a = Rng::new(7);
        let first: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        a.reset();
        let again: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        assert_eq!(first, again);
        assert_ne!(Rng::new(8).next_u64(), first[0]);
        for _ in 0..1000 {
            assert!(a.below(3) < 3);
            let u = a.unit();
            assert!((0.0..1.0).contains(&u));
        }
    }

    #[test]
    fn round_robin_cycles_and_random_never_repeats() {
        let mut rng = Rng::new(1);
        let mut rr = RoundRobin::new(2);
        let seq: Vec<u32> = (0..7)
            .map(|_| rr.advance(0, 3, RoundRobinMode::Sequential, &mut rng))
            .collect();
        assert_eq!(seq, [0, 1, 2, 0, 1, 2, 0]);
        let mut last = u32::MAX;
        let mut seen = [0; 4];
        for _ in 0..400 {
            let p = rr.advance(1, 4, RoundRobinMode::Random, &mut rng);
            assert_ne!(p, last);
            seen[p as usize] += 1;
            last = p;
        }
        assert!(seen.iter().all(|n| *n > 50), "{seen:?}");
        rr.reset();
        assert_eq!(rr.advance(0, 3, RoundRobinMode::Sequential, &mut rng), 0);
    }
}
