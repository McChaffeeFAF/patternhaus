//! Step sequencer view model: how notes map to step buttons and back.
//!
//! A lit step is a note exactly one step long (240 ticks) starting on a step boundary
//! (`ph_core::STEP`); `step.toggle` / `step.set` create and remove exactly those. Other notes
//! in the row are shown dimmed as "other" so the row never lies about what is in the pattern.

use ph_core::{ChannelId, NoteId, PatternId, Project, STEP, Tick};
use std::collections::BTreeSet;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Step {
    Off,
    On {
        note: NoteId,
        velocity: f32,
        key: u8,
    },
    /// A longer or off-grid note starts in this step (edit it in the piano roll).
    Other,
}

impl Step {
    pub fn is_on(&self) -> bool {
        matches!(self, Step::On { .. })
    }
}

/// Steps shown for a pattern: at least 16, more when the pattern is longer.
pub fn step_count(project: &Project, pattern: PatternId) -> u32 {
    let len = project.pattern_length(pattern).unwrap_or(STEP * 16);
    (len / STEP).max(16) as u32
}

/// The row of `n` steps for a channel in a pattern.
pub fn row(project: &Project, pattern: PatternId, channel: ChannelId, n: u32) -> Vec<Step> {
    let mut out = vec![Step::Off; n as usize];
    let Some(notes) = project
        .patterns
        .get(&pattern)
        .and_then(|p| p.notes.get(&channel))
    else {
        return out;
    };
    for note in notes.values() {
        let step = (note.start / STEP) as usize;
        if step >= out.len() {
            continue;
        }
        let exact = note.start % STEP == 0 && note.length == STEP;
        out[step] = match (exact, out[step]) {
            (true, _) => Step::On {
                note: note.id,
                velocity: note.velocity,
                key: note.key,
            },
            (false, Step::Off) => Step::Other,
            (false, keep) => keep,
        };
    }
    out
}

/// The row after painting `value` onto `touched` steps: what `step.set` should send.
pub fn painted(current: &[Step], touched: &BTreeSet<u32>, value: bool) -> Vec<bool> {
    current
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if touched.contains(&(i as u32)) {
                value
            } else {
                s.is_on()
            }
        })
        .collect()
}

/// Horizontal layout of the step buttons: `w` wide with an extra `gap` after every 4.
#[derive(Copy, Clone, Debug)]
pub struct StepLayout {
    pub origin_x: f32,
    pub w: f32,
    pub gap: f32,
}

impl StepLayout {
    pub fn x_of(&self, step: u32) -> f32 {
        self.origin_x + step as f32 * self.w + (step / 4) as f32 * self.gap
    }

    /// Step under an x position, or None in the gaps and outside the row.
    pub fn step_at(&self, x: f32, n: u32) -> Option<u32> {
        if x < self.origin_x {
            return None;
        }
        // groups of four steps plus one gap
        let group_w = self.w * 4.0 + self.gap;
        let rel = x - self.origin_x;
        let group = (rel / group_w).floor() as u32;
        let within = rel - group as f32 * group_w;
        let in_group = (within / self.w).floor() as u32;
        if in_group > 3 {
            return None; // in the gap
        }
        let step = group * 4 + in_group;
        (step < n).then_some(step)
    }

    pub fn total_width(&self, n: u32) -> f32 {
        self.x_of(n.saturating_sub(1)) + self.w - self.origin_x
    }
}

/// Step index of a tick (for the playhead).
pub fn step_of_tick(t: Tick) -> u32 {
    (t / STEP) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, NullHost, Registry, Request, Session};
    use serde_json::json;
    use std::sync::Arc;

    fn project() -> (Project, PatternId, ChannelId) {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("t"));
        let mut h = NullHost::default();
        let who = Caller::human();
        let mut run =
            |c: &str, p: serde_json::Value| s.execute(&who, Request::new(c, p), &mut h).unwrap();
        let pat = run("pattern.add", json!({})).result["pattern"]
            .as_u64()
            .unwrap() as u32;
        let ch = run(
            "channel.add",
            json!({"name": "Kick", "instrument": {"kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60}}),
        )
        .result["channel"]
            .as_u64()
            .unwrap() as u32;
        run(
            "step.set",
            json!({"pattern": pat, "channel": ch, "steps": [true, false, false, false, true]}),
        );
        run(
            "note.add",
            json!({"pattern": pat, "channel": ch, "notes": [{"key": 60, "start": 480, "length": 960}]}),
        );
        (s.project().as_ref().clone(), PatternId(pat), ChannelId(ch))
    }

    #[test]
    fn rows_distinguish_steps_from_other_notes() {
        let (p, pat, ch) = project();
        let r = row(&p, pat, ch, 16);
        assert!(r[0].is_on() && r[4].is_on());
        assert_eq!(r[2], Step::Other); // the long note at tick 480
        assert_eq!(r[1], Step::Off);
        assert_eq!(r.len(), 16);
    }

    #[test]
    fn step_count_follows_pattern_length() {
        let (p, pat, _) = project();
        assert_eq!(step_count(&p, pat), 16);
    }

    #[test]
    fn painting_keeps_unrelated_steps() {
        let (p, pat, ch) = project();
        let r = row(&p, pat, ch, 16);
        let touched: BTreeSet<u32> = [1, 2, 3].into();
        let on = painted(&r, &touched, true);
        assert!(on[0] && on[1] && on[2] && on[3] && on[4] && !on[5]);
        let off = painted(&r, &[0u32].into(), false);
        assert!(!off[0] && off[4]);
    }

    #[test]
    fn layout_maps_x_to_steps_and_skips_gaps() {
        let l = StepLayout {
            origin_x: 100.0,
            w: 20.0,
            gap: 6.0,
        };
        assert_eq!(l.x_of(0), 100.0);
        assert_eq!(l.x_of(4), 100.0 + 80.0 + 6.0);
        for s in [0, 3, 4, 7, 15] {
            assert_eq!(l.step_at(l.x_of(s) + 5.0, 16), Some(s));
        }
        assert_eq!(l.step_at(100.0 + 80.0 + 2.0, 16), None); // the gap
        assert_eq!(l.step_at(50.0, 16), None);
        assert_eq!(l.step_at(l.x_of(15) + 30.0, 16), None);
    }
}
