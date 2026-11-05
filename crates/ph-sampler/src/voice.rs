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
        ((self.next_u64() >> 32) * n as u64 >> 32) as u32
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

/// One sounding zone.
#[derive(Copy, Clone, Debug, Default)]
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
    /// Read position in frames of the file; `step` frames per output frame (negative when
    /// reversed).
    pub pos: f64,
    pub step: f64,
    pub gain: [f32; 2],
    /// Start order, for stealing (higher = newer).
    pub order: u64,
    /// Held: a note-off has not arrived yet.
    pub held: bool,
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
    pub step: f64,
    pub gain: [f32; 2],
}

/// A fixed pool of voices, allocated in `activate`.
pub struct VoicePool {
    pub voices: Box<[Voice]>,
    order: u64,
}

impl VoicePool {
    pub fn new(n: usize) -> Self {
        Self {
            voices: vec![Voice::default(); n.max(1)].into_boxed_slice(),
            order: 0,
        }
    }

    pub fn reset(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
        }
        self.order = 0;
    }

    pub fn active(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Starts a voice, stealing one when the group's or the instrument's limit is reached
    /// (the group's [`StealMode`] decides which; `None` refuses the new voice). Returns the
    /// voice index. Real-time.
    pub fn trigger(&mut self, plan: &Plan, t: Trigger) -> Option<usize> {
        let g = &plan.groups[t.group];
        if let Some(limit) = g.voice_limit {
            let in_group = self
                .voices
                .iter()
                .filter(|v| v.active && v.group == t.group)
                .count();
            if in_group >= limit as usize {
                let victim = self.steal(g.stealing, Some(t.group))?;
                return Some(self.start(victim, t));
            }
        }
        let limit = plan.voice_limit.min(self.voices.len());
        let free = self.voices[..limit].iter().position(|v| !v.active);
        let slot = match free {
            Some(i) => i,
            None => self.steal(g.stealing, None)?,
        };
        Some(self.start(slot, t))
    }

    fn start(&mut self, i: usize, t: Trigger) -> usize {
        self.order += 1;
        self.voices[i] = Voice {
            active: true,
            note_id: t.note_id,
            key: t.key,
            zone: t.zone,
            group: t.group,
            bus: t.bus,
            pos: t.pos,
            step: t.step,
            gain: t.gain,
            order: self.order,
            held: true,
        };
        i
    }

    /// The voice to give way, among active voices (of `group` when given). Real-time.
    pub fn steal(&mut self, mode: StealMode, group: Option<usize>) -> Option<usize> {
        let candidates = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.active && group.is_none_or(|g| v.group == g));
        let pick = match mode {
            StealMode::None => None,
            StealMode::Oldest => candidates.min_by_key(|(_, v)| v.order),
            StealMode::Newest => candidates.max_by_key(|(_, v)| v.order),
            StealMode::Quietest => candidates.min_by(|(_, a), (_, b)| {
                let (ga, gb) = (a.gain[0] + a.gain[1], b.gain[0] + b.gain[1]);
                ga.total_cmp(&gb).then(a.order.cmp(&b.order))
            }),
        };
        pick.map(|(i, _)| i)
    }

    /// Note-off for `note_id`: held voices are released. Without envelopes (M3/T0) a released
    /// voice that is not one-shot stops at once; T2 starts its amp-envelope release instead.
    /// Real-time.
    pub fn release(&mut self, plan: &Plan, note_id: u32) {
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.active && v.note_id == note_id)
        {
            v.held = false;
            if !plan.zones[v.zone].one_shot {
                v.active = false; // T2: amp envelope release
            }
        }
    }

    /// Mono and legato groups: the voices of `group` give way to a new note. Real-time.
    pub fn choke_group(&mut self, group: usize) {
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.active && v.group == group)
        {
            v.active = false; // T2: a short fade, and legato glides instead
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
