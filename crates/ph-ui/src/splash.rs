//! The loading screen: the mark and wordmark on the deepest surface with a sweeping bar. It
//! shows over the first frames of the real app (the engine is already running by then), fades
//! out, and any click or key skips it. With reduced motion it is static and short.

use crate::theme::Theme;
use egui::{Align2, Color32, Context, FontId, Id, Order, Pos2, Rect, Stroke, Vec2};

/// Seconds the splash holds before it fades.
pub const HOLD: f64 = 1.1;
/// Seconds of the fade.
pub const FADE: f64 = 0.3;
/// The same, with reduced motion.
pub const HOLD_REDUCED: f64 = 0.5;

#[derive(Copy, Clone, Debug, Default)]
pub struct Splash {
    started: Option<f64>,
    /// When the person skipped it (the fade runs from here).
    skipped_at: Option<f64>,
}

impl Splash {
    pub fn new() -> Self {
        Self::default()
    }

    /// How opaque the splash is `elapsed` seconds in (None once it is over).
    pub fn opacity(elapsed: f64, reduce: bool) -> Option<f32> {
        let (hold, fade) = if reduce {
            (HOLD_REDUCED, 0.0)
        } else {
            (HOLD, FADE)
        };
        if elapsed < hold {
            Some(1.0)
        } else if fade > 0.0 && elapsed < hold + fade {
            Some(1.0 - ((elapsed - hold) / fade) as f32)
        } else {
            None
        }
    }

    /// Draws the splash; returns false once it is over.
    pub fn show(&mut self, ctx: &Context, theme: &Theme, reduce: bool) -> bool {
        let now = ctx.input(|i| i.time);
        let started = *self.started.get_or_insert(now);
        let skip = ctx.input(|i| {
            i.pointer.any_pressed()
                || i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
        });
        if skip && self.skipped_at.is_none() && now - started > 0.15 {
            self.skipped_at = Some(now);
        }
        let mut elapsed = now - started;
        if let Some(at) = self.skipped_at {
            // jump to the start of the fade
            let hold = if reduce { HOLD_REDUCED } else { HOLD };
            elapsed = elapsed.max(hold) + (now - at);
        }
        let Some(alpha) = Self::opacity(elapsed, reduce) else {
            return false;
        };
        let c = &theme.colors;
        let screen = ctx.content_rect();
        egui::Area::new(Id::new("ph_splash"))
            .order(Order::Tooltip)
            .fixed_pos(screen.min)
            .interactable(false)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 0.0, c.bg0.gamma_multiply(alpha));
                let centre = rect.center() - Vec2::new(0.0, 10.0);
                let fade = |col: Color32| col.gamma_multiply(alpha);
                // the mark and the name
                let scale = (rect.height() / 520.0).clamp(0.8, 1.6);
                crate::brand::paint_logo(p, centre, scale, theme, alpha);
                // the loading bar
                let bar_w = 240.0 * scale;
                let bar = Rect::from_center_size(
                    Pos2::new(centre.x, centre.y + 96.0 * scale),
                    Vec2::new(bar_w, 3.0),
                );
                p.rect_filled(bar, 1.5, fade(c.bg3));
                let t = if reduce {
                    0.5
                } else {
                    ((elapsed * 0.9) % 1.0) as f32
                };
                let head = bar.left() + bar_w * t;
                let seg = Rect::from_min_max(
                    Pos2::new((head - 60.0 * scale).max(bar.left()), bar.top()),
                    Pos2::new(head.min(bar.right()), bar.bottom()),
                );
                crate::paint::glow_rect(p, seg, 1, fade(c.accent), 0.8);
                p.rect_filled(seg, 1.5, fade(c.accent));
                p.text(
                    Pos2::new(centre.x, bar.bottom() + 16.0),
                    Align2::CENTER_CENTER,
                    "STARTING THE AUDIO ENGINE",
                    FontId::new(11.0, crate::fonts::cond()),
                    fade(c.dim),
                );
                p.text(
                    Pos2::new(rect.center().x, rect.bottom() - 22.0),
                    Align2::CENTER_CENTER,
                    format!("v{}", env!("CARGO_PKG_VERSION")),
                    FontId::new(11.0, crate::fonts::mono()),
                    fade(c.faint),
                );
                p.hline(
                    rect.x_range(),
                    rect.top() + 1.0,
                    Stroke::new(1.0, fade(c.accent_dim)),
                );
            });
        if !reduce {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_holds_then_fades_then_ends() {
        assert_eq!(Splash::opacity(0.0, false), Some(1.0));
        assert_eq!(Splash::opacity(HOLD - 0.01, false), Some(1.0));
        let mid = Splash::opacity(HOLD + FADE / 2.0, false).unwrap();
        assert!((mid - 0.5).abs() < 0.01);
        assert_eq!(Splash::opacity(HOLD + FADE + 0.01, false), None);
    }

    #[test]
    fn reduced_motion_is_short_and_has_no_fade() {
        assert_eq!(Splash::opacity(HOLD_REDUCED - 0.01, true), Some(1.0));
        assert_eq!(Splash::opacity(HOLD_REDUCED + 0.01, true), None);
    }
}
