//! The icon set: one consistent stroke family on a 16 px grid with a 1.5 px stroke, drawn as
//! vectors by us (no icon font, no third-party artwork, nothing to license). Each icon is a
//! short list of strokes in grid coordinates, scaled to whatever size it is painted at.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// One drawing primitive, in 16 x 16 grid units.
#[derive(Copy, Clone, Debug)]
pub enum Op {
    /// A line.
    L(f32, f32, f32, f32),
    /// An open polyline.
    P(&'static [(f32, f32)]),
    /// A closed polygon outline.
    Pc(&'static [(f32, f32)]),
    /// A filled polygon.
    F(&'static [(f32, f32)]),
    /// A rounded rectangle outline: x, y, w, h, radius.
    R(f32, f32, f32, f32, f32),
    /// A circle outline: centre and radius.
    C(f32, f32, f32),
    /// A filled dot.
    D(f32, f32, f32),
    /// An arc: centre, radius, start and end angle in degrees (0 = right, clockwise).
    A(f32, f32, f32, f32, f32),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // transport
    Play,
    Stop,
    Pause,
    Record,
    Loop,
    Metronome,
    // edit and file
    Undo,
    Redo,
    Save,
    Export,
    Clone,
    Rename,
    Plus,
    Search,
    // tools
    Draw,
    Paint,
    Delete,
    Mute,
    Slice,
    Select,
    Zoom,
    Snap,
    Quantize,
    Ghost,
    Keys,
    // window buttons
    Detach,
    Maximize,
    Restore,
    Close,
    Options,
    Chevron,
    // browser
    Folder,
    Sample,
    Preset,
    Plugin,
    // misc
    Bypass,
    Solo,
    Agent,
    Gear,
    Help,
    Menu,
    Pattern,
    Left,
    Right,
    Up,
    Down,
    Minus,
}

impl Icon {
    pub const ALL: &'static [Icon] = &[
        Icon::Play,
        Icon::Stop,
        Icon::Pause,
        Icon::Record,
        Icon::Loop,
        Icon::Metronome,
        Icon::Undo,
        Icon::Redo,
        Icon::Save,
        Icon::Export,
        Icon::Clone,
        Icon::Rename,
        Icon::Plus,
        Icon::Search,
        Icon::Draw,
        Icon::Paint,
        Icon::Delete,
        Icon::Mute,
        Icon::Slice,
        Icon::Select,
        Icon::Zoom,
        Icon::Snap,
        Icon::Quantize,
        Icon::Ghost,
        Icon::Keys,
        Icon::Detach,
        Icon::Maximize,
        Icon::Restore,
        Icon::Close,
        Icon::Options,
        Icon::Chevron,
        Icon::Folder,
        Icon::Sample,
        Icon::Preset,
        Icon::Plugin,
        Icon::Bypass,
        Icon::Solo,
        Icon::Agent,
        Icon::Gear,
        Icon::Help,
        Icon::Menu,
        Icon::Pattern,
        Icon::Left,
        Icon::Right,
        Icon::Up,
        Icon::Down,
        Icon::Minus,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Icon::Play => "play",
            Icon::Stop => "stop",
            Icon::Pause => "pause",
            Icon::Record => "record",
            Icon::Loop => "loop",
            Icon::Metronome => "metronome",
            Icon::Undo => "undo",
            Icon::Redo => "redo",
            Icon::Save => "save",
            Icon::Export => "export",
            Icon::Clone => "clone",
            Icon::Rename => "rename",
            Icon::Plus => "plus",
            Icon::Search => "search",
            Icon::Draw => "draw",
            Icon::Paint => "paint",
            Icon::Delete => "delete",
            Icon::Mute => "mute",
            Icon::Slice => "slice",
            Icon::Select => "select",
            Icon::Zoom => "zoom",
            Icon::Snap => "snap",
            Icon::Quantize => "quantize",
            Icon::Ghost => "ghost",
            Icon::Keys => "keys",
            Icon::Detach => "detach",
            Icon::Maximize => "maximize",
            Icon::Restore => "restore",
            Icon::Close => "close",
            Icon::Options => "options",
            Icon::Chevron => "chevron",
            Icon::Folder => "folder",
            Icon::Sample => "sample",
            Icon::Preset => "preset",
            Icon::Plugin => "plugin",
            Icon::Bypass => "bypass",
            Icon::Solo => "solo",
            Icon::Agent => "agent",
            Icon::Gear => "gear",
            Icon::Help => "help",
            Icon::Menu => "menu",
            Icon::Pattern => "pattern",
            Icon::Left => "left",
            Icon::Right => "right",
            Icon::Up => "up",
            Icon::Down => "down",
            Icon::Minus => "minus",
        }
    }

    /// The strokes, in 16 x 16 grid units.
    pub fn ops(self) -> &'static [Op] {
        use Op::*;
        match self {
            Icon::Play => &[F(&[(5.0, 3.0), (13.0, 8.0), (5.0, 13.0)])],
            Icon::Stop => &[F(&[(4.0, 4.0), (12.0, 4.0), (12.0, 12.0), (4.0, 12.0)])],
            Icon::Pause => &[L(5.5, 3.5, 5.5, 12.5), L(10.5, 3.5, 10.5, 12.5)],
            Icon::Record => &[D(8.0, 8.0, 4.6)],
            Icon::Loop => &[
                P(&[(3.0, 8.0), (3.0, 6.5), (5.0, 4.5), (12.0, 4.5)]),
                P(&[(10.0, 2.5), (12.0, 4.5), (10.0, 6.5)]),
                P(&[(13.0, 8.0), (13.0, 9.5), (11.0, 11.5), (4.0, 11.5)]),
                P(&[(6.0, 9.5), (4.0, 11.5), (6.0, 13.5)]),
            ],
            Icon::Metronome => &[
                Pc(&[(5.0, 13.5), (11.0, 13.5), (9.5, 3.0), (6.5, 3.0)]),
                L(8.0, 10.5, 11.0, 5.0),
                D(11.0, 5.0, 0.9),
            ],
            Icon::Undo => &[
                P(&[(5.5, 3.0), (2.5, 6.0), (5.5, 9.0)]),
                P(&[
                    (3.0, 6.0),
                    (9.0, 6.0),
                    (12.5, 8.0),
                    (12.5, 10.5),
                    (10.0, 12.5),
                    (6.0, 12.5),
                ]),
            ],
            Icon::Redo => &[
                P(&[(10.5, 3.0), (13.5, 6.0), (10.5, 9.0)]),
                P(&[
                    (13.0, 6.0),
                    (7.0, 6.0),
                    (3.5, 8.0),
                    (3.5, 10.5),
                    (6.0, 12.5),
                    (10.0, 12.5),
                ]),
            ],
            Icon::Save => &[
                Pc(&[
                    (3.0, 3.0),
                    (11.0, 3.0),
                    (13.0, 5.0),
                    (13.0, 13.0),
                    (3.0, 13.0),
                ]),
                R(5.0, 3.0, 5.0, 3.5, 0.5),
                R(5.0, 8.5, 6.0, 4.5, 0.5),
            ],
            Icon::Export => &[
                P(&[(3.0, 9.0), (3.0, 13.0), (13.0, 13.0), (13.0, 9.0)]),
                L(8.0, 10.0, 8.0, 2.5),
                P(&[(5.0, 5.0), (8.0, 2.5), (11.0, 5.0)]),
            ],
            Icon::Clone => &[R(2.5, 5.0, 8.0, 8.5, 1.0), R(5.5, 2.5, 8.0, 8.5, 1.0)],
            Icon::Rename => &[
                Pc(&[
                    (3.0, 10.0),
                    (10.0, 3.0),
                    (12.0, 5.0),
                    (5.0, 12.0),
                    (3.0, 12.0),
                ]),
                L(2.5, 14.0, 13.5, 14.0),
            ],
            Icon::Plus => &[L(8.0, 3.0, 8.0, 13.0), L(3.0, 8.0, 13.0, 8.0)],
            Icon::Search => &[C(6.8, 6.8, 4.0), L(9.8, 9.8, 13.5, 13.5)],
            Icon::Draw => &[
                Pc(&[
                    (3.0, 13.0),
                    (3.8, 9.5),
                    (11.0, 2.5),
                    (13.5, 5.0),
                    (6.5, 12.2),
                ]),
                L(9.5, 4.0, 12.0, 6.5),
            ],
            Icon::Paint => &[
                L(12.5, 3.0, 8.0, 8.0),
                Pc(&[
                    (8.0, 8.0),
                    (9.8, 9.8),
                    (6.5, 13.0),
                    (3.0, 13.0),
                    (3.0, 10.5),
                ]),
            ],
            Icon::Delete => &[
                Pc(&[
                    (2.5, 10.5),
                    (9.0, 3.5),
                    (13.5, 7.5),
                    (9.0, 12.5),
                    (5.0, 12.5),
                ]),
                L(5.5, 7.0, 10.5, 11.5),
                L(9.0, 12.5, 14.0, 12.5),
            ],
            Icon::Mute => &[
                Pc(&[
                    (2.0, 6.0),
                    (5.0, 6.0),
                    (8.5, 3.5),
                    (8.5, 12.5),
                    (5.0, 10.0),
                    (2.0, 10.0),
                ]),
                L(10.5, 6.0, 14.0, 10.0),
                L(14.0, 6.0, 10.5, 10.0),
            ],
            Icon::Slice => &[
                C(4.5, 11.5, 1.8),
                C(4.5, 4.5, 1.8),
                L(6.0, 10.5, 13.5, 4.0),
                L(6.0, 5.5, 13.5, 12.0),
            ],
            Icon::Select => &[
                P(&[(3.0, 6.0), (3.0, 3.0), (6.0, 3.0)]),
                P(&[(10.0, 3.0), (13.0, 3.0), (13.0, 6.0)]),
                P(&[(13.0, 10.0), (13.0, 13.0), (10.0, 13.0)]),
                P(&[(6.0, 13.0), (3.0, 13.0), (3.0, 10.0)]),
            ],
            Icon::Zoom => &[
                C(7.0, 7.0, 4.2),
                L(10.2, 10.2, 13.5, 13.5),
                L(5.0, 7.0, 9.0, 7.0),
                L(7.0, 5.0, 7.0, 9.0),
            ],
            Icon::Snap => &[
                P(&[
                    (4.0, 2.5),
                    (4.0, 8.0),
                    (5.2, 11.0),
                    (8.0, 12.5),
                    (10.8, 11.0),
                    (12.0, 8.0),
                    (12.0, 2.5),
                ]),
                L(4.0, 5.0, 6.8, 5.0),
                L(9.2, 5.0, 12.0, 5.0),
            ],
            Icon::Quantize => &[P(&[
                (2.5, 12.5),
                (2.5, 9.5),
                (6.0, 9.5),
                (6.0, 6.5),
                (9.5, 6.5),
                (9.5, 3.5),
                (13.5, 3.5),
            ])],
            Icon::Ghost => &[
                Pc(&[
                    (3.5, 13.5),
                    (3.5, 7.0),
                    (5.0, 4.0),
                    (8.0, 2.5),
                    (11.0, 4.0),
                    (12.5, 7.0),
                    (12.5, 13.5),
                    (10.3, 11.8),
                    (8.0, 13.5),
                    (5.7, 11.8),
                ]),
                D(6.3, 7.5, 0.8),
                D(9.7, 7.5, 0.8),
            ],
            Icon::Keys => &[
                R(2.5, 3.5, 11.0, 9.0, 1.0),
                L(5.2, 8.0, 5.2, 12.5),
                L(8.0, 8.0, 8.0, 12.5),
                L(10.8, 8.0, 10.8, 12.5),
                F(&[(4.4, 3.5), (6.0, 3.5), (6.0, 8.0), (4.4, 8.0)]),
                F(&[(7.2, 3.5), (8.8, 3.5), (8.8, 8.0), (7.2, 8.0)]),
                F(&[(10.0, 3.5), (11.6, 3.5), (11.6, 8.0), (10.0, 8.0)]),
            ],
            Icon::Detach => &[
                R(2.5, 5.5, 8.0, 8.0, 1.0),
                P(&[(7.0, 2.5), (13.5, 2.5), (13.5, 9.0)]),
                L(13.5, 2.5, 8.0, 8.0),
            ],
            Icon::Maximize => &[R(3.0, 3.0, 10.0, 10.0, 1.0)],
            Icon::Restore => &[
                R(2.5, 5.5, 8.0, 8.0, 1.0),
                P(&[
                    (5.5, 5.5),
                    (5.5, 2.5),
                    (13.5, 2.5),
                    (13.5, 10.5),
                    (10.5, 10.5),
                ]),
            ],
            Icon::Close => &[L(3.5, 3.5, 12.5, 12.5), L(12.5, 3.5, 3.5, 12.5)],
            Icon::Options => &[D(3.5, 8.0, 1.1), D(8.0, 8.0, 1.1), D(12.5, 8.0, 1.1)],
            Icon::Chevron => &[P(&[(3.5, 6.0), (8.0, 10.5), (12.5, 6.0)])],
            Icon::Folder => &[Pc(&[
                (2.0, 4.0),
                (6.5, 4.0),
                (8.0, 5.8),
                (14.0, 5.8),
                (14.0, 12.5),
                (2.0, 12.5),
            ])],
            Icon::Sample => &[
                L(2.5, 6.5, 2.5, 9.5),
                L(5.2, 4.0, 5.2, 12.0),
                L(8.0, 2.5, 8.0, 13.5),
                L(10.8, 5.0, 10.8, 11.0),
                L(13.5, 6.8, 13.5, 9.2),
            ],
            Icon::Preset => &[
                L(2.5, 4.0, 13.5, 4.0),
                L(2.5, 8.0, 13.5, 8.0),
                L(2.5, 12.0, 13.5, 12.0),
                D(6.0, 4.0, 1.5),
                D(10.5, 8.0, 1.5),
                D(5.0, 12.0, 1.5),
            ],
            Icon::Plugin => &[
                R(4.0, 5.0, 8.0, 5.5, 1.0),
                L(6.5, 2.5, 6.5, 5.0),
                L(9.5, 2.5, 9.5, 5.0),
                P(&[(8.0, 10.5), (8.0, 13.5)]),
            ],
            Icon::Bypass => &[A(8.0, 8.5, 5.0, -55.0, 235.0), L(8.0, 2.5, 8.0, 8.0)],
            Icon::Solo => &[
                A(8.0, 9.0, 5.0, 180.0, 360.0),
                R(2.5, 9.0, 2.6, 4.2, 1.0),
                R(10.9, 9.0, 2.6, 4.2, 1.0),
            ],
            Icon::Agent => &[Pc(&[
                (8.0, 2.0),
                (9.5, 6.5),
                (14.0, 8.0),
                (9.5, 9.5),
                (8.0, 14.0),
                (6.5, 9.5),
                (2.0, 8.0),
                (6.5, 6.5),
            ])],
            Icon::Gear => &[
                C(8.0, 8.0, 2.2),
                C(8.0, 8.0, 5.0),
                L(8.0, 1.8, 8.0, 3.0),
                L(8.0, 13.0, 8.0, 14.2),
                L(1.8, 8.0, 3.0, 8.0),
                L(13.0, 8.0, 14.2, 8.0),
                L(3.6, 3.6, 4.5, 4.5),
                L(11.5, 11.5, 12.4, 12.4),
                L(12.4, 3.6, 11.5, 4.5),
                L(4.5, 11.5, 3.6, 12.4),
            ],
            Icon::Help => &[C(8.0, 8.0, 5.8), L(8.0, 7.0, 8.0, 11.2), D(8.0, 5.0, 0.8)],
            Icon::Menu => &[
                L(3.0, 4.0, 13.0, 4.0),
                L(3.0, 8.0, 13.0, 8.0),
                L(3.0, 12.0, 13.0, 12.0),
            ],
            Icon::Left => &[P(&[(10.0, 3.0), (5.0, 8.0), (10.0, 13.0)])],
            Icon::Right => &[P(&[(6.0, 3.0), (11.0, 8.0), (6.0, 13.0)])],
            Icon::Minus => &[L(3.0, 8.0, 13.0, 8.0)],
            Icon::Up => &[P(&[(3.0, 10.0), (8.0, 5.0), (13.0, 10.0)])],
            Icon::Down => &[P(&[(3.0, 6.0), (8.0, 11.0), (13.0, 6.0)])],
            Icon::Pattern => &[
                R(2.5, 2.5, 11.0, 11.0, 1.0),
                L(2.5, 8.0, 13.5, 8.0),
                L(8.0, 2.5, 8.0, 13.5),
            ],
        }
    }

    /// Paints the icon centred in `rect` (scaled from the 16 px grid; stroke 1.5 px at 16).
    pub fn paint(self, p: &Painter, rect: Rect, colour: Color32) {
        p.extend(self.shapes(rect, colour));
    }

    /// The icon as shapes: what [`Self::paint`] draws.
    pub fn shapes(self, rect: Rect, colour: Color32) -> Vec<Shape> {
        shapes_of(rect, self.ops(), colour)
    }
}

/// The stroke width, in grid units.
pub const STROKE: f32 = 1.5;

fn shapes_of(rect: Rect, ops: &[Op], colour: Color32) -> Vec<Shape> {
    let mut out: Vec<Shape> = Vec::new();
    let side = rect.width().min(rect.height());
    let k = side / 16.0;
    let origin = rect.center() - Vec2::splat(side / 2.0);
    let at = |x: f32, y: f32| Pos2::new(origin.x + x * k, origin.y + y * k);
    let stroke = Stroke::new((STROKE * k).max(1.0), colour);
    for op in ops {
        match *op {
            Op::L(x0, y0, x1, y1) => {
                out.push(Shape::line_segment([at(x0, y0), at(x1, y1)], stroke));
            }
            Op::P(pts) => {
                out.push(Shape::line(
                    pts.iter().map(|&(x, y)| at(x, y)).collect(),
                    stroke,
                ));
            }
            Op::Pc(pts) => {
                out.push(Shape::closed_line(
                    pts.iter().map(|&(x, y)| at(x, y)).collect(),
                    stroke,
                ));
            }
            Op::F(pts) => {
                out.push(Shape::convex_polygon(
                    pts.iter().map(|&(x, y)| at(x, y)).collect(),
                    colour,
                    Stroke::new(1.0 * k, colour),
                ));
            }
            Op::R(x, y, w, h, r) => {
                out.push(Shape::rect_stroke(
                    Rect::from_min_max(at(x, y), at(x + w, y + h)),
                    egui::CornerRadius::same((r * k).round() as u8),
                    stroke,
                    StrokeKind::Middle,
                ));
            }
            Op::C(x, y, r) => {
                out.push(Shape::circle_stroke(at(x, y), r * k, stroke));
            }
            Op::D(x, y, r) => {
                out.push(Shape::circle_filled(at(x, y), r * k, colour));
            }
            Op::A(cx, cy, r, a0, a1) => {
                let n = (((a1 - a0).abs() / 12.0).ceil() as usize).max(4);
                let pts = (0..=n)
                    .map(|i| {
                        let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                        at(cx + r * a.cos(), cy + r * a.sin())
                    })
                    .collect();
                out.push(Shape::line(pts, stroke));
            }
        }
    }
    out
}

/// Every coordinate an op uses (for the bounds test).
pub fn op_points(op: &Op) -> Vec<(f32, f32)> {
    match *op {
        Op::L(a, b, c, d) => vec![(a, b), (c, d)],
        Op::P(p) | Op::Pc(p) | Op::F(p) => p.to_vec(),
        Op::R(x, y, w, h, _) => vec![(x, y), (x + w, y + h)],
        Op::C(x, y, r) | Op::D(x, y, r) => vec![(x - r, y - r), (x + r, y + r)],
        Op::A(x, y, r, ..) => vec![(x - r, y - r), (x + r, y + r)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_fits_the_16_px_grid() {
        for icon in Icon::ALL {
            assert!(!icon.ops().is_empty(), "{icon:?} is empty");
            for op in icon.ops() {
                for (x, y) in op_points(op) {
                    assert!(
                        (0.0..=16.0).contains(&x) && (0.0..=16.0).contains(&y),
                        "{icon:?} leaves the grid at ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_set_covers_the_brief() {
        use Icon::*;
        for needed in [
            Play, Stop, Draw, Paint, Delete, Mute, Slice, Select, Zoom, Snap, Loop, Metronome,
            Undo, Redo, Save, Export, Detach, Maximize, Close, Folder, Sample, Preset, Plugin,
        ] {
            assert!(Icon::ALL.contains(&needed), "{needed:?}");
        }
        let mut names: Vec<_> = Icon::ALL.iter().map(|i| i.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Icon::ALL.len(), "names are unique");
    }

    #[test]
    fn icons_paint_something() {
        for icon in Icon::ALL {
            let shapes = icon.shapes(
                Rect::from_min_size(Pos2::ZERO, Vec2::splat(16.0)),
                Color32::WHITE,
            );
            assert!(!shapes.is_empty(), "{icon:?} drew nothing");
        }
    }
}
