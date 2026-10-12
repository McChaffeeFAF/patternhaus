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

/// A horizontal gradient rectangle from `left` to `right` colours (one quad).
pub fn hgradient(p: &Painter, rect: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    for (pos, color) in [
        (rect.left_top(), left),
        (rect.right_top(), right),
        (rect.right_bottom(), right),
        (rect.left_bottom(), left),
    ] {
        m.vertices.push(Vertex { pos, uv, color });
    }
    m.indices.extend([0, 1, 2, 0, 2, 3]);
    p.add(Shape::mesh(m));
}

/// The playhead: a glowing line with a soft trail behind it. `reduce` drops the trail and the
/// glow (reduced motion keeps the line).
pub fn playhead(p: &Painter, x: f32, top: f32, bottom: f32, theme: &Theme, reduce: bool) {
    let c = &theme.colors;
    let area = p.clip_rect();
    if x < area.left() - 40.0 || x > area.right() + 4.0 {
        return;
    }
    if !reduce {
        let trail = Rect::from_min_max(
            Pos2::new((x - 26.0).max(area.left()), top),
            Pos2::new(x, bottom),
        );
        hgradient(p, trail, c.tint.gamma_multiply(0.0), c.glow_soft);
    }
    glow_line(
        p,
        Pos2::new(x, top),
        Pos2::new(x, bottom),
        1.5,
        c.playhead,
        if reduce { 0.0 } else { 0.8 },
    );
}

/// Lays `text` out on one row no wider than `max_w`: text that fits is returned as laid out by
/// [`Painter::layout_no_wrap`] (identical to what `Painter::text` draws); longer text is cut
/// and ends in "…". Returns the galley and whether it was shortened. egui caches galleys, so
/// the fits-already case costs one cache lookup.
pub fn fit_galley(
    p: &Painter,
    text: &str,
    font: FontId,
    colour: Color32,
    max_w: f32,
) -> (std::sync::Arc<egui::Galley>, bool) {
    let plain = p.layout_no_wrap(text.to_owned(), font.clone(), colour);
    if plain.size().x <= max_w.max(0.0) + 0.01 {
        return (plain, false);
    }
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, colour);
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    (p.layout_job(job), true)
}

/// As [`Painter::text`] but never wider than `max_w`: too-long text is shortened with "…".
/// Returns the rect drawn and whether the text was truncated (show the full text on hover).
pub fn text_fit(
    p: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    colour: Color32,
    max_w: f32,
) -> (Rect, bool) {
    let (galley, cut) = fit_galley(p, text, font, colour, max_w);
    let rect = align.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, colour);
    (rect, cut)
}

/// [`text_fit`] on an interactive area: when the text was shortened, hovering `resp` shows the
/// full text (tooltip and the hint bar).
pub fn text_fit_on(
    p: &Painter,
    resp: &egui::Response,
    (pos, align): (Pos2, Align2),
    text: &str,
    font: FontId,
    colour: Color32,
    max_w: f32,
) -> Rect {
    let (rect, cut) = text_fit(p, pos, align, text, font, colour, max_w);
    if cut {
        use crate::hint::HintExt;
        let _ = resp.clone().on_hover_text(text).hint(text, "", "");
    }
    rect
}

/// `text` shortened with "…" to fit `max_w` at `font` (for places that need a `String`, such as
/// window titles). Text that fits comes back unchanged.
pub fn fit_str(p: &Painter, text: &str, font: FontId, max_w: f32) -> String {
    let (g, cut) = fit_galley(p, text, font, Color32::TRANSPARENT, max_w);
    if cut {
        g.text().to_owned()
    } else {
        text.to_owned()
    }
}

/// `text` cut to at most `max` characters, ending in "…" when it was cut. For widgets that
/// measure their text whole (badges) and so cannot be given a width.
pub fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut t: String = text.chars().take(max.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}

/// Shortens a path-like `text` in the middle so the file name stays visible: `C:\…\Kick 808.wav`.
/// `fits` says whether a candidate is short enough; text that fits comes back unchanged. The
/// kept head is as long as still fits; when even `…\name` is too long the name itself is cut
/// at its end. Separators are `/` and `\`.
pub fn middle_ellipsis(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    let Some(sep_at) = text.rfind(['/', '\\']) else {
        return cut_end(text, &fits);
    };
    let (head, tail) = text.split_at(sep_at); // tail starts with the separator
    let chars: Vec<char> = head.chars().collect();
    let make = |n: usize| format!("{}\u{2026}{tail}", chars[..n].iter().collect::<String>());
    if !fits(&make(0)) {
        // not even "…\name": keep the start of the name
        return cut_end(&format!("\u{2026}{tail}"), &fits);
    }
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&make(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    make(lo)
}

fn cut_end(text: &str, fits: &impl Fn(&str) -> bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let make = |n: usize| format!("{}\u{2026}", chars[..n].iter().collect::<String>());
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&make(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    make(lo)
}

/// [`middle_ellipsis`] measured in pixels: `text` at `font` no wider than `max_w`.
pub fn fit_path(p: &Painter, text: &str, font: FontId, max_w: f32) -> String {
    middle_ellipsis(text, |s| {
        p.layout_no_wrap(s.to_owned(), font.clone(), Color32::TRANSPARENT)
            .size()
            .x
            <= max_w.max(0.0) + 0.01
    })
}

/// [`middle_ellipsis`] counted in characters, for places that cannot measure (dialog text).
pub fn ellipsize_middle(text: &str, max: usize) -> String {
    middle_ellipsis(text, |s| s.chars().count() <= max)
}

#[cfg(test)]
mod middle_tests {
    use super::*;

    #[test]
    fn keeps_the_file_name_and_the_start() {
        let p = r"C:\Users\someone\Music\Samples\Drums\Kick 808.wav";
        assert_eq!(ellipsize_middle(p, 100), p);
        let s = ellipsize_middle(p, 20);
        assert!(s.chars().count() <= 20, "{s}");
        assert!(s.ends_with(r"\Kick 808.wav"), "{s}");
        assert!(s.starts_with("C:"), "{s}");
        assert!(s.contains('\u{2026}'));
        assert_eq!(ellipsize_middle(p, 14), "…\\Kick 808.wav");
    }

    #[test]
    fn a_name_longer_than_the_room_is_cut_at_its_end() {
        let s = ellipsize_middle("/a/b/a-very-long-file-name.wav", 8);
        assert_eq!(s.chars().count(), 8);
        assert!(s.starts_with('\u{2026}') && s.ends_with('\u{2026}'), "{s}");
        assert_eq!(
            ellipsize_middle("no-separators-here.wav", 8)
                .chars()
                .count(),
            8
        );
    }
}
