//! The Patternhaus mark and wordmark: a four-by-four pad grid with a few pads lit, and the
//! name in the condensed face. Drawn from theme tokens, so it follows the theme; the app icon
//! (`icon.rs`) is the same drawing rasterised.

use crate::paint;
use crate::theme::Theme;
use egui::{CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};

/// Which pads of the 4 x 4 grid are lit in the mark (row-major, 16 bits).
pub const LIT: u16 = 0b1000_0100_0010_1001_u16.reverse_bits();

/// The lit pads as a boolean grid.
pub fn lit(row: usize, col: usize) -> bool {
    (LIT >> (row * 4 + col)) & 1 == 1
}

/// Paints the mark in `rect` (square): a dark rounded tile with the pad grid, lit pads glowing.
pub fn paint_mark(p: &Painter, rect: Rect, theme: &Theme, glow: f32, alpha: f32) {
    let c = &theme.colors;
    let a = |col: egui::Color32| col.gamma_multiply(alpha);
    let side = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(side));
    let round = (side * 0.2).round() as u8;
    p.rect_filled(r, CornerRadius::same(round), a(c.bg0));
    p.rect_stroke(
        r,
        CornerRadius::same(round),
        Stroke::new((side * 0.04).max(1.0), a(c.accent_dim)),
        StrokeKind::Inside,
    );
    let pad = side * 0.16;
    let gap = side * 0.055;
    let grid = pad * 4.0 + gap * 3.0;
    let origin = r.center() - Vec2::splat(grid / 2.0);
    for row in 0..4 {
        for col in 0..4 {
            let min = origin + Vec2::new(col as f32 * (pad + gap), row as f32 * (pad + gap));
            let cell = Rect::from_min_size(min, Vec2::splat(pad));
            let rr = CornerRadius::same((pad * 0.22).round() as u8);
            if lit(row, col) {
                paint::glow_rect(p, cell, rr.nw, a(c.accent), 0.5 * glow);
                p.rect_filled(cell, rr, a(c.accent));
            } else {
                p.rect_filled(
                    cell,
                    rr,
                    a(if col / 2 % 2 == 0 { c.pad } else { c.pad_group }),
                );
            }
        }
    }
}

/// The wordmark: PATTERN in text colour, HAUS in the accent, in the condensed face with wide
/// tracking, after a small mark.
pub fn wordmark(ui: &mut Ui, theme: &Theme) {
    let c = &theme.colors;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
        paint_mark(ui.painter(), rect, theme, 0.6, 1.0);
        ui.add_space(7.0);
        ui.label(
            egui::RichText::new("PATTERN")
                .font(FontId::new(15.0, crate::fonts::cond_bold()))
                .extra_letter_spacing(2.2)
                .color(c.text),
        );
        ui.label(
            egui::RichText::new("HAUS")
                .font(FontId::new(15.0, crate::fonts::cond_bold()))
                .extra_letter_spacing(2.2)
                .color(c.accent),
        );
    });
}

/// Large centred wordmark for the splash and About: the mark above the name.
pub fn paint_logo(p: &Painter, centre: Pos2, scale: f32, theme: &Theme, alpha: f32) {
    let c = &theme.colors;
    let mark = Rect::from_center_size(
        centre - Vec2::new(0.0, 34.0 * scale),
        Vec2::splat(76.0 * scale),
    );
    paint_mark(p, mark, theme, 1.0, alpha);
    let y = centre.y + 32.0 * scale;
    let font = FontId::new(34.0 * scale, crate::fonts::cond_bold());
    let a = p.layout_no_wrap("PATTERN".into(), font.clone(), c.text);
    let b = p.layout_no_wrap("HAUS".into(), font.clone(), c.accent);
    let track = 7.0 * scale;
    let wa = a.size().x + track * 7.0;
    let wb = b.size().x + track * 4.0;
    let x0 = centre.x - (wa + wb) / 2.0;
    for (i, ch) in "PATTERN".chars().enumerate() {
        let g = p.layout_no_wrap(ch.to_string(), font.clone(), c.text.gamma_multiply(alpha));
        let x = x0 + i as f32 * track + prefix_width(p, "PATTERN", i, &font);
        p.galley(
            Pos2::new(x, y - g.size().y / 2.0),
            g,
            c.text.gamma_multiply(alpha),
        );
    }
    for (i, ch) in "HAUS".chars().enumerate() {
        let g = p.layout_no_wrap(ch.to_string(), font.clone(), c.accent.gamma_multiply(alpha));
        let x = x0 + wa + i as f32 * track + prefix_width(p, "HAUS", i, &font);
        p.galley(
            Pos2::new(x, y - g.size().y / 2.0),
            g,
            c.accent.gamma_multiply(alpha),
        );
    }
}

fn prefix_width(p: &Painter, word: &str, upto: usize, font: &FontId) -> f32 {
    let s: String = word.chars().take(upto).collect();
    if s.is_empty() {
        0.0
    } else {
        p.layout_no_wrap(s, font.clone(), egui::Color32::PLACEHOLDER)
            .size()
            .x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_lights_a_few_pads() {
        let n = (0..4)
            .flat_map(|r| (0..4).map(move |c| (r, c)))
            .filter(|&(r, c)| lit(r, c))
            .count();
        assert!((4..=8).contains(&n), "{n} lit pads");
    }
}
