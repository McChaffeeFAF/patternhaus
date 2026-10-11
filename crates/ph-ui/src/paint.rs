//! Cheap paint helpers shared by the widgets: layered-stroke glow (never a blur pass),
//! LCD text, gradients and the recessed-well look.
//!
//! Glow is a handful of expanding, fading strokes: a fixed number of extra shapes whatever the
//! size, tessellated with the rest of the frame.

use crate::theme::Theme;
use egui::{
    Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
    Vec2, epaint::Vertex,
};

/// Layers of the glow: (expand in px, alpha factor).
const GLOW_RINGS: [(f32, f32); 3] = [(1.0, 0.55), (2.5, 0.3), (4.5, 0.14)];

/// A soft outer glow around `rect` in `colour`, scaled by `k` (0..=1; 0 draws nothing).
pub fn glow_rect(p: &Painter, rect: Rect, round: u8, colour: Color32, k: f32) {
    if k <= 0.01 {
        return;
    }
    for (grow, a) in GLOW_RINGS {
        p.rect_stroke(
            rect.expand(grow),
            CornerRadius::same(round.saturating_add(grow as u8)),
            Stroke::new(1.6, colour.gamma_multiply(a * k)),
            StrokeKind::Outside,
        );
    }
}

/// A glowing line: wide faint strokes under a thin bright one.
pub fn glow_line(p: &Painter, a: Pos2, b: Pos2, width: f32, colour: Color32, k: f32) {
    if k > 0.01 {
        for (grow, al) in GLOW_RINGS {
            p.line_segment(
                [a, b],
                Stroke::new(width + grow * 2.0, colour.gamma_multiply(al * 0.5 * k)),
            );
        }
    }
    p.line_segment([a, b], Stroke::new(width, colour));
}

/// A glowing polyline (arcs on knobs).
pub fn glow_path(p: &Painter, pts: Vec<Pos2>, width: f32, colour: Color32, k: f32) {
    if k > 0.01 {
        for (grow, al) in GLOW_RINGS.iter().take(2) {
            p.add(Shape::line(
                pts.clone(),
                Stroke::new(width + grow * 1.6, colour.gamma_multiply(al * 0.5 * k)),
            ));
        }
    }
    p.add(Shape::line(pts, Stroke::new(width, colour)));
}

/// A glowing dot (LEDs, playhead heads).
pub fn glow_dot(p: &Painter, c: Pos2, r: f32, colour: Color32, k: f32) {
    if k > 0.01 {
        for (grow, al) in GLOW_RINGS {
            p.circle_filled(c, r + grow, colour.gamma_multiply(al * 0.5 * k));
        }
    }
    p.circle_filled(c, r, colour);
}

/// The recessed look of a well (step pads, fader tracks, LCD): dark fill, a darker edge and a
/// lighter hairline under the bottom edge.
pub fn well(p: &Painter, rect: Rect, round: u8, fill: Color32, theme: &Theme) {
    p.rect_filled(rect, CornerRadius::same(round), fill);
    p.rect_stroke(
        rect,
        CornerRadius::same(round),
        Stroke::new(1.0, theme.colors.bg0.gamma_multiply(0.9)),
        StrokeKind::Inside,
    );
    let r = rect.shrink2(Vec2::new(round as f32 * 0.6, 0.0));
    p.line_segment(
        [
            Pos2::new(r.left(), rect.bottom() + 0.5),
            Pos2::new(r.right(), rect.bottom() + 0.5),
        ],
        Stroke::new(1.0, theme.colors.line_soft),
    );
}

/// LCD text: accent text with a faint glow behind it, on whatever is already painted.
pub fn lcd_text(
    p: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    colour: Color32,
    glow: Color32,
) -> Rect {
    for off in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        p.text(
            pos + Vec2::new(off.0, off.1),
            align,
            text,
            font.clone(),
            glow,
        );
    }
    p.text(pos, align, text, font, colour)
}

/// A vertical gradient rectangle from `top` to `bottom` colours (one quad).
pub fn vgradient(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    for (pos, color) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        m.vertices.push(Vertex { pos, uv, color });
    }
    m.indices.extend([0, 1, 2, 0, 2, 3]);
    p.add(Shape::mesh(m));
}

/// Colour stops sampled at `t` in 0..=1.
pub fn sample_stops(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        if t <= w[1].0 {
            let span = (w[1].0 - w[0].0).max(1e-6);
            return crate::theme::lerp(w[0].1, w[1].1, (t - w[0].0) / span);
        }
    }
    stops.last().map_or(Color32::TRANSPARENT, |s| s.1)
}

/// `colour` over `under` at opacity `a` (0..=1); an opaque result.
pub fn over(under: Color32, colour: Color32, a: f32) -> Color32 {
    crate::theme::lerp(under, colour, a)
}

/// Monospace readout text.
pub fn mono_text(p: &Painter, pos: Pos2, align: Align2, text: &str, size: f32, colour: Color32) {
    p.text(
        pos,
        align,
        text,
        FontId::new(size, crate::fonts::mono()),
        colour,
    );
}
