//! Voice allocation with a voice limit and stealing.
//!
//! The pool holds more slots than the limit so a stolen voice can fade out (a few ms)
//! while its replacement starts in a fresh slot; only if every slot is busy is a fading
//! voice cut hard.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StealMode {
    Oldest,
    Quietest,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotState {
    Free,
    /// Sounding; `released` once its note-off arrived.
    Playing {
        released: bool,
    },
    /// Being faded out after a steal or choke; does not count towards the limit.
    Fading,
}

#[derive(Copy, Clone, Debug)]
pub struct Slot {
    pub state: SlotState,
    pub note_id: u32,
    pub key: u8,
    age: u64,
}

/// Result of [`VoiceAllocator::note_on`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    /// Slot for the new note.
    pub slot: usize,
    /// A playing voice to fade out (it is now `Fading`).
    pub fade: Option<usize>,
    /// True if `slot` was taken from a voice that was still sounding (cut hard).
    pub hard_cut: bool,
}

#[derive(Clone, Debug)]
pub struct VoiceAllocator {
    slots: Vec<Slot>,
    limit: usize,
    counter: u64,
    pub mode: StealMode,
}

impl VoiceAllocator {
    /// `capacity` slots in total; `limit` simultaneously playing voices.
    pub fn new(capacity: usize, limit: usize) -> Self {
        Self {
            slots: vec![
                Slot {
                    state: SlotState::Free,
                    note_id: 0,
                    key: 0,
                    age: 0,
                };
                capacity.max(1)
            ],
            limit: limit.clamp(1, capacity.max(1)),
            counter: 0,
            mode: StealMode::Oldest,
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.clamp(1, self.slots.len());
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn slot(&self, i: usize) -> &Slot {
        &self.slots[i]
    }

    pub fn playing_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s.state, SlotState::Playing { .. }))
            .count()
    }

    /// Allocate a slot for a new note. `level(i)` reports slot `i`'s current loudness (for
    /// [`StealMode::Quietest`]).
    pub fn note_on(&mut self, note_id: u32, key: u8, level: impl Fn(usize) -> f32) -> Allocation {
        self.counter += 1;
        let mut fade = None;
        if self.playing_count() >= self.limit {
            // prefer released voices, then by policy
            let victim = self
                .pick(
                    |s| matches!(s.state, SlotState::Playing { released: true }),
                    &level,
                )
                .or_else(|| self.pick(|s| matches!(s.state, SlotState::Playing { .. }), &level));
            if let Some(v) = victim {
                self.slots[v].state = SlotState::Fading;
                fade = Some(v);
            }
        }
        let (slot, hard_cut) = match self.slots.iter().position(|s| s.state == SlotState::Free) {
            Some(i) => (i, false),
            None => {
                // every slot busy: cut the quietest fading voice, else the faded victim
                let i = self
                    .pick_quietest(|s| s.state == SlotState::Fading, &level)
                    .or(fade)
                    .unwrap_or(0);
                if fade == Some(i) {
                    fade = None;
                }
                (i, true)
            }
        };
        self.slots[slot] = Slot {
            state: SlotState::Playing { released: false },
            note_id,
            key,
            age: self.counter,
        };
        Allocation {
            slot,
            fade,
            hard_cut,
        }
    }

    fn pick(&self, filter: impl Fn(&Slot) -> bool, level: &impl Fn(usize) -> f32) -> Option<usize> {
        match self.mode {
            StealMode::Oldest => self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| filter(s))
                .min_by_key(|(_, s)| s.age)
                .map(|(i, _)| i),
            StealMode::Quietest => self.pick_quietest(filter, level),
        }
    }

    fn pick_quietest(
        &self,
        filter: impl Fn(&Slot) -> bool,
        level: &impl Fn(usize) -> f32,
    ) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, s) in self.slots.iter().enumerate() {
            if filter(s) {
                let l = level(i);
                if best.is_none_or(|(_, bl)| l < bl) {
                    best = Some((i, l));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Mark the voice playing `note_id` as released; returns its slot.
    pub fn note_off(&mut self, note_id: u32) -> Option<usize> {
        let i = self.slots.iter().position(|s| {
            s.note_id == note_id && s.state == SlotState::Playing { released: false }
        })?;
        self.slots[i].state = SlotState::Playing { released: true };
        Some(i)
    }

    /// Mark a slot as fading (choke).
    pub fn set_fading(&mut self, i: usize) {
        if self.slots[i].state != SlotState::Free {
            self.slots[i].state = SlotState::Fading;
        }
    }

    /// The instrument calls this when a voice has gone silent.
    pub fn free(&mut self, i: usize) {
        self.slots[i].state = SlotState::Free;
    }

    pub fn free_all(&mut self) {
        for s in &mut self.slots {
            s.state = SlotState::Free;
        }
    }

    pub fn is_free(&self, i: usize) -> bool {
        self.slots[i].state == SlotState::Free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steals_oldest_beyond_limit() {
        let mut a = VoiceAllocator::new(6, 3);
        for n in 0..3 {
            let r = a.note_on(n, 60 + n as u8, |_| 1.0);
            assert_eq!(r.fade, None);
        }
        let r = a.note_on(3, 70, |_| 1.0);
        assert_eq!(r.fade, Some(0));
        assert!(!r.hard_cut);
        assert_eq!(a.playing_count(), 3);
    }

    #[test]
    fn prefers_released_then_quietest() {
        let mut a = VoiceAllocator::new(4, 2);
        a.mode = StealMode::Quietest;
        a.note_on(0, 60, |_| 1.0);
        a.note_on(1, 61, |_| 1.0);
        let lv = |i: usize| if i == 0 { 0.1 } else { 0.9 };
        let r = a.note_on(2, 62, lv);
        assert_eq!(r.fade, Some(0));
        a.note_off(2);
        let r = a.note_on(3, 63, |_| 1.0);
        // slot of note 2 was released, so it goes first
        assert_eq!(
            r.fade,
            Some(a.slots.iter().position(|s| s.note_id == 2).unwrap())
        );
    }

    #[test]
    fn hard_cut_when_full() {
        let mut a = VoiceAllocator::new(2, 2);
        a.note_on(0, 60, |_| 1.0);
        a.note_on(1, 61, |_| 1.0);
        let r = a.note_on(2, 62, |_| 1.0);
        assert!(r.hard_cut);
        assert_eq!(a.playing_count(), 2);
    }
}
