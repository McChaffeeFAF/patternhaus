//! Typing keyboard to piano (FL convention): two rows of the computer keyboard play notes on
//! the selected channel through `Engine::preview_note`.

use egui::{Event, Key};
use std::collections::HashMap;

/// Semitone above the lower row's C for a typing key.
pub fn key_to_semitone(key: Key) -> Option<i32> {
    use Key::*;
    let lower = [Z, S, X, D, C, V, G, B, H, N, J, M];
    let upper = [Q, Num2, W, Num3, E, R, Num5, T, Num6, Y, Num7, U];
    if let Some(i) = lower.iter().position(|k| *k == key) {
        return Some(i as i32);
    }
    if let Some(i) = upper.iter().position(|k| *k == key) {
        return Some(12 + i as i32);
    }
    // the third octave continues on the upper row
    match key {
        I => Some(24),
        Num9 => Some(25),
        O => Some(26),
        Num0 => Some(27),
        P => Some(28),
        _ => None,
    }
}

/// MIDI key for a typing key at an octave shift (octave 5 puts the lower row's C at 60).
pub fn midi_key(key: Key, octave: i32) -> Option<u8> {
    let k = octave * 12 + key_to_semitone(key)?;
    (0..=127).contains(&k).then_some(k as u8)
}

pub struct TypingState {
    pub enabled: bool,
    pub octave: i32,
    pub velocity: f32,
    held: HashMap<Key, u8>,
}

impl Default for TypingState {
    fn default() -> Self {
        Self {
            enabled: true,
            octave: 5,
            velocity: 0.8,
            held: HashMap::new(),
        }
    }
}

impl TypingState {
    pub fn shift_octave(&mut self, delta: i32) {
        self.octave = (self.octave + delta).clamp(0, 8);
    }

    /// Turns key events into note on/off. `text_focus` is true while a text field has the
    /// keyboard; held notes are still released then.
    pub fn process(&mut self, events: &[Event], text_focus: bool) -> Vec<(u8, bool)> {
        let mut out = Vec::new();
        for e in events {
            let Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = e
            else {
                continue;
            };
            if !*pressed {
                if let Some(k) = self.held.remove(key) {
                    out.push((k, false));
                }
                continue;
            }
            if *repeat
                || !self.enabled
                || text_focus
                || modifiers.command
                || modifiers.alt
                || self.held.contains_key(key)
            {
                continue;
            }
            if let Some(k) = midi_key(*key, self.octave) {
                self.held.insert(*key, k);
                out.push((k, true));
            }
        }
        out
    }

    /// Releases everything (focus lost, window hidden).
    pub fn release_all(&mut self) -> Vec<(u8, bool)> {
        self.held.drain().map(|(_, k)| (k, false)).collect()
    }

    pub fn is_piano_key(&self, key: Key) -> bool {
        self.enabled && key_to_semitone(key).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn ev(key: Key, pressed: bool, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn layout_matches_the_two_row_convention() {
        assert_eq!(midi_key(Key::Z, 5), Some(60)); // C5
        assert_eq!(midi_key(Key::S, 5), Some(61));
        assert_eq!(midi_key(Key::M, 5), Some(71));
        assert_eq!(midi_key(Key::Q, 5), Some(72));
        assert_eq!(midi_key(Key::Num2, 5), Some(73));
        assert_eq!(midi_key(Key::U, 5), Some(83));
        assert_eq!(midi_key(Key::Z, 4), Some(48));
        assert_eq!(midi_key(Key::A, 5), None);
        assert_eq!(midi_key(Key::P, 9), None); // 9*12+28 > 127
    }

    #[test]
    fn press_release_pairs_and_ignores_repeats() {
        let mut t = TypingState::default();
        let out = t.process(&[ev(Key::Z, true, false), ev(Key::Z, true, true)], false);
        assert_eq!(out, vec![(60, true)]);
        // octave change while held: the release still uses the key that went down
        t.shift_octave(1);
        assert_eq!(
            t.process(&[ev(Key::Z, false, false)], false),
            vec![(60, false)]
        );
        assert_eq!(
            t.process(&[ev(Key::Z, true, false)], false),
            vec![(72, true)]
        );
    }

    #[test]
    fn text_focus_and_modifiers_block_notes() {
        let mut t = TypingState::default();
        assert!(t.process(&[ev(Key::Z, true, false)], true).is_empty());
        let mut ctrl = ev(Key::Z, true, false);
        if let Event::Key { modifiers, .. } = &mut ctrl {
            modifiers.command = true;
        }
        assert!(t.process(&[ctrl], false).is_empty());
        t.enabled = false;
        assert!(t.process(&[ev(Key::Z, true, false)], false).is_empty());
    }

    #[test]
    fn octave_is_clamped() {
        let mut t = TypingState::default();
        t.shift_octave(100);
        assert_eq!(t.octave, 8);
        t.shift_octave(-100);
        assert_eq!(t.octave, 0);
    }
}
