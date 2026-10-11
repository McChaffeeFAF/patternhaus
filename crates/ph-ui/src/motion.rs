//! Motion (DESIGN section 9): everything here runs on UI time, never on the audio thread, and
//! asks for repaints only while something is animating. A "reduce motion" preference turns
//! the decorative animation into jumps.

use egui::{Context, Id};

const REDUCE: &str = "ph_reduce_motion";

pub fn set_reduce(ctx: &Context, reduce: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new(REDUCE), reduce));
}

pub fn reduce(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new(REDUCE)))
        .unwrap_or(false)
}

/// Eases a value towards `target` over `secs` (egui animates and repaints while it moves).
/// With reduced motion the value jumps.
pub fn ease(ctx: &Context, id: Id, target: f32, secs: f32) -> f32 {
    ctx.animate_value_with_time(id, target, if reduce(ctx) { 0.0 } else { secs })
}

/// Eases a boolean to 0..1 (focus glow, hover fades).
pub fn ease_bool(ctx: &Context, id: Id, on: bool, secs: f32) -> f32 {
    ctx.animate_bool_with_time(id, on, if reduce(ctx) { 0.0 } else { secs })
}

/// Starts a one-shot pulse; [`pulse`] reads it back.
pub fn trigger(ctx: &Context, id: Id) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id, now));
}

/// The strength (1 -> 0 over `secs`) of a pulse started by [`trigger`]; 0 when idle or when
/// motion is reduced. Asks for a repaint for as long as the pulse lasts.
pub fn pulse(ctx: &Context, id: Id, secs: f64) -> f32 {
    if reduce(ctx) {
        return 0.0;
    }
    let Some(at) = ctx.data(|d| d.get_temp::<f64>(id)) else {
        return 0.0;
    };
    let k = 1.0 - (ctx.input(|i| i.time) - at) / secs;
    if k > 0.0 {
        ctx.request_repaint();
        k as f32
    } else {
        0.0
    }
}

/// Strength of a decay that started at `at` (UI time), without any stored state.
pub fn decay(now: f64, at: f64, secs: f64) -> f32 {
    (1.0 - (now - at) / secs).clamp(0.0, 1.0) as f32
}

/// Meter ballistics, in the UI only (the audio path is untouched): instant attack,
/// exponential release, a held peak, and a latched clip flag.
#[derive(Copy, Clone, Debug, Default)]
pub struct Ballistics {
    pub level: f32,
    pub peak: f32,
    pub peak_at: f64,
    pub clip: bool,
    pub last: f64,
}

impl Ballistics {
    pub const FALL_DB_PER_S: f32 = 26.0;
    pub const PEAK_FALL_DB_PER_S: f32 = 40.0;
    pub const HOLD_SECS: f64 = 1.5;

    /// Feeds a reading (linear amplitude) at `now`. Returns true while anything is still
    /// moving, so the caller can keep repainting only then.
    pub fn update(&mut self, reading: f32, now: f64, smooth: bool) -> bool {
        let dt = (now - self.last).clamp(0.0, 0.25) as f32;
        self.last = now;
        let r = reading.max(0.0);
        if r >= self.level || !smooth {
            self.level = r;
        } else {
            let db = crate::widgets::db_from_linear(self.level) - Self::FALL_DB_PER_S * dt;
            self.level = 10f32.powf(db / 20.0).max(r);
            if self.level < 1e-4 {
                self.level = 0.0;
            }
        }
        if r >= self.peak {
            self.peak = r;
            self.peak_at = now;
        } else if now - self.peak_at > Self::HOLD_SECS || !smooth {
            let db = crate::widgets::db_from_linear(self.peak) - Self::PEAK_FALL_DB_PER_S * dt;
            self.peak = if smooth { 10f32.powf(db / 20.0) } else { r }.max(self.level);
            if self.peak < 1e-4 {
                self.peak = 0.0;
            }
        }
        if r > 1.0 {
            self.clip = true;
        }
        self.level > 0.0 || self.peak > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meters_attack_instantly_and_fall_smoothly() {
        let mut b = Ballistics::default();
        b.update(1.0, 0.0, true);
        assert_eq!(b.level, 1.0);
        b.update(0.0, 0.1, true);
        assert!(
            b.level < 1.0 && b.level > 0.5,
            "falls, not drops: {}",
            b.level
        );
        let mut jump = Ballistics::default();
        jump.update(1.0, 0.0, false);
        jump.update(0.0, 0.1, false);
        assert_eq!(jump.level, 0.0);
    }

    #[test]
    fn peaks_hold_then_release() {
        let mut b = Ballistics::default();
        b.update(0.9, 0.0, true);
        b.update(0.0, 1.0, true);
        assert!((b.peak - 0.9).abs() < 1e-6, "held at 1.0 s");
        for i in 0..60 {
            b.update(0.0, 1.6 + i as f64 * 0.05, true);
        }
        assert!(b.peak < 0.5);
    }

    #[test]
    fn clipping_latches() {
        let mut b = Ballistics::default();
        b.update(1.2, 0.0, true);
        assert!(b.clip);
    }

    #[test]
    fn it_settles_so_the_ui_can_go_idle() {
        let mut b = Ballistics::default();
        b.update(1.0, 0.0, true);
        let mut t = 0.0;
        while b.update(0.0, t, true) {
            t += 0.05;
            assert!(t < 30.0, "never settles");
        }
        assert_eq!((b.level, b.peak), (0.0, 0.0));
    }

    #[test]
    fn decay_runs_from_one_to_zero() {
        assert_eq!(decay(10.0, 10.0, 2.0), 1.0);
        assert_eq!(decay(11.0, 10.0, 2.0), 0.5);
        assert_eq!(decay(13.0, 10.0, 2.0), 0.0);
    }
}
