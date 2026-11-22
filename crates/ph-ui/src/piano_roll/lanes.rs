//! The controller lane editor under the piano roll (M3/T11): FL's "event editor" for CC 0-119
//! and pitch bend, one lane at a time, sharing the note grid's horizontal zoom and scroll.
//!
//! Like the rest of the piano roll this is a view model (positions in, [`PrOut`] requests
//! out) plus a painter, and the view model is tested without a window. Every edit is a
//! `cc.*` command: there is no command that moves or re-curves a point, so a move, a curve
//! change and a tension drag are a `cc.remove_points` and a `cc.add_points` sent as one
//! batch (one undo step); a freehand stroke and a line are one `cc.set`; a click is one
//! `cc.add_points`. Edited points get new ids, so the selection follows them by tick.
//!
//! Gestures in a controller lane (positions snap to the roll's grid, Alt = free):
//!
//! | gesture | effect |
//! |---|---|
//! | click on empty space | add a point |
//! | drag on empty space | freehand curve, thinned, written as one `cc.set` |
//! | Shift + drag on empty space | straight ramp (line tool) |
//! | Ctrl + drag on empty space | rubber-band select (Shift too: add to the selection) |
//! | drag a point | move it (the whole selection if it is selected) in time and value |
//! | Shift / Ctrl + click a point | toggle it in the selection |
//! | right-click a point | delete it |
//! | right-click elsewhere (or Shift + right-click a point) | menu: curve of the segment, delete |
//! | drag the diamond on a segment | tension of a linear or S-curve segment |
//! | Delete, Ctrl + A | delete / select the lane's points (when the lane has focus) |

use super::{Btn, Mods, PrOut, View, snap_tick};
use crate::mapping;
use crate::theme::Theme;
use egui::{Align2, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use ph_core::{
    AutomationPoint, CcPoint, CcPointSpec, ChannelId, Controller, CurveKind, PatternId, PointId,
    Project, Tick,
};
use std::collections::{BTreeMap, BTreeSet};

/// Height of the strip with the lane selector and tabs.
pub const HEADER_H: f32 = 22.0;
/// The draggable strip between the note grid and the lane area.
pub const DIVIDER_H: f32 = 4.0;
pub const MIN_H: f32 = 44.0;
pub const MAX_H: f32 = 400.0;
pub const DEFAULT_H: f32 = 96.0;
/// Vertical padding inside the body, so values at the range ends stay grabbable.
const PAD: f32 = 8.0;
/// How close, in pixels, a press must be to grab a point.
const HIT_R: f32 = 8.0;
/// How close a press must be to a segment's tension diamond.
const HANDLE_R: f32 = 7.0;
/// A press that moves less than this is a click.
const SLOP: f32 = 3.0;
/// Freehand thinning: the largest distance, in pixels, the thinned line may stray.
pub const THIN_PX: f32 = 1.5;

/// The lanes offered by the selector after the ones that already have points.
pub const PRESETS: [Controller; 5] = [
    Controller::Cc(1),
    Controller::Cc(11),
    Controller::Cc(7),
    Controller::Cc(64),
    Controller::PitchBend,
];

/// The curve choices of the segment menu.
pub const CURVES: [(CurveKind, &str); 4] = [
    (CurveKind::Smooth, "Smooth"),
    (CurveKind::Linear, "Linear"),
    (CurveKind::SCurve, "S-curve"),
    (CurveKind::Hold, "Hold"),
];

/// What the lane area shows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LaneKind {
    /// The note velocities (the roll's original velocity editor).
    #[default]
    Velocity,
    Controller(Controller),
}

impl LaneKind {
    pub fn label(self) -> String {
        match self {
            LaneKind::Velocity => "Velocity".into(),
            LaneKind::Controller(c) => c.label(),
        }
    }
    /// Short tab text: `CC1`, `Bend`.
    pub fn short(self) -> String {
        match self {
            LaneKind::Velocity => "Velocity".into(),
            LaneKind::Controller(Controller::PitchBend) => "Bend".into(),
            LaneKind::Controller(Controller::Cc(n)) => format!("CC{n}"),
        }
    }
}

/// What a gesture needs to know about the roll around the lane.
#[derive(Copy, Clone, Debug)]
pub struct Env {
    pub view: View,
    /// The roll's snap grid in ticks (1 = off).
    pub snap: Tick,
    pub bar: Tick,
    /// Pattern length, for the end marker.
    pub pattern_len: Tick,
    pub pattern: PatternId,
    pub channel: ChannelId,
}

/// A value the controller can carry: CC in 1/127 steps, pitch bend in 1/8192 steps. Written
/// values are snapped like this, so what the lane shows, what it stores and what the engine
/// sends agree.
pub fn snap_value(c: Controller, v: f64) -> f64 {
    match c {
        Controller::Cc(_) => (v.clamp(0.0, 1.0) * 127.0).round() / 127.0,
        Controller::PitchBend => {
            (v.clamp(-1.0, 1.0) * 8192.0).round().clamp(-8192.0, 8191.0) / 8192.0
        }
    }
}

/// A value in the controller's units: `0..=127` for a CC, `-8192..=8191` (signed) for bend.
pub fn units(c: Controller, v: f64) -> String {
    match c {
        Controller::Cc(_) => format!("{}", c.quantize(v)),
        Controller::PitchBend => format!("{:+}", c.quantize(v)),
    }
}

/// The lane's value at `tick` as the engine plays it: the shared `value_at` evaluation, sent
/// as the controller's wire value.
pub fn evaluate(c: Controller, points: &[CcPoint], tick: f64) -> Option<f64> {
    let autos: Vec<AutomationPoint> = points.iter().map(CcPoint::as_auto).collect();
    ph_core::value_at(&autos, tick).map(|v| f64::from(c.wire(v)))
}

#[derive(Clone, Debug)]
enum LDrag {
    None,
    /// The selection being moved; `dt` / `dv` are the snapped offsets so far.
    Move {
        ids: Vec<PointId>,
        anchor_tick: Tick,
        anchor_value: f64,
        /// Pointer minus the anchor point at the press, so a grab off centre does not jump.
        grab_dt: f64,
        grab_dv: f64,
        min_tick: Tick,
        dt: i64,
        dv: f64,
        /// Where the press landed; a press that stays within [`SLOP`] is a click, not a move.
        start: Vec2,
        moved: bool,
    },
    /// A freehand stroke: one sample per grid tick, kept in time order whichever way the
    /// pointer travels (drawing back over the stroke replaces what is under it).
    Draw {
        samples: BTreeMap<Tick, f64>,
        /// The sample at the press: a still click adds just this point.
        first: (Tick, f64),
        /// The tick of the newest sample.
        last: Tick,
        start: Vec2,
        moved: bool,
    },
    Line {
        from: (Tick, f64),
        to: (Tick, f64),
    },
    Boxed {
        from: Vec2,
        to: Vec2,
        additive: bool,
    },
    /// The tension of the segment that starts at `id`.
    Tension {
        id: PointId,
        base: f32,
        tension: f32,
        kind: CurveKind,
        a: f64,
        b: f64,
        t_h: f64,
        /// Pointer value minus the diamond's, so a grab off centre does not jump.
        grab_dv: f64,
        start: Vec2,
        moved: bool,
    },
}

/// Points an edit is about to create: selected as soon as they show up in a snapshot.
#[derive(Clone, Debug)]
struct Want {
    ticks: BTreeSet<Tick>,
    exclude: BTreeSet<PointId>,
}

pub struct LaneState {
    pub kind: LaneKind,
    /// Collapsed: only the header strip shows.
    pub open: bool,
    /// Body height in pixels (the user drags the divider).
    pub height: f32,
    /// The lane was clicked last, so Ctrl+A and Delete are about its points.
    pub focused: bool,
    pub selection: BTreeSet<PointId>,
    /// The point a right-click picked for the segment menu.
    pub menu_point: Option<PointId>,
    /// The controller number of the selector's "Other CC…" entry.
    pub other_cc: u8,
    /// The shown lane's points in `(tick, id)` order.
    pub points: Vec<CcPoint>,
    /// Lanes with points on this channel, with their point counts.
    pub tabs: Vec<(Controller, usize)>,
    /// Body height of the last frame; positions are relative to the body's top-left.
    pub h: f32,
    /// Where the body was drawn last frame (screen coordinates), for tests.
    pub body: Rect,
    drag: LDrag,
    want: Option<Want>,
}

impl Default for LaneState {
    fn default() -> Self {
        Self {
            kind: LaneKind::Velocity,
            open: true,
            height: DEFAULT_H,
            focused: false,
            selection: BTreeSet::new(),
            menu_point: None,
            other_cc: 74,
            points: Vec::new(),
            tabs: Vec::new(),
            h: DEFAULT_H,
            body: Rect::NOTHING,
            drag: LDrag::None,
            want: None,
        }
    }
}

impl LaneState {
    /// The controller being edited, if the lane area shows one.
    pub fn controller(&self) -> Option<Controller> {
        match self.kind {
            LaneKind::Controller(c) => Some(c),
            LaneKind::Velocity => None,
        }
    }

    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, LDrag::None)
    }

    /// Forgets everything about the old pattern or channel.
    pub fn reset(&mut self) {
        self.selection.clear();
        self.drag = LDrag::None;
        self.want = None;
        self.menu_point = None;
        self.focused = false;
        self.points.clear();
        self.tabs.clear();
    }

    /// Switches lanes (a tab or the selector). `project` refreshes the points at once.
    pub fn set_kind(
        &mut self,
        kind: LaneKind,
        project: &Project,
        pattern: PatternId,
        channel: ChannelId,
    ) {
        if kind != self.kind {
            self.kind = kind;
            self.selection.clear();
            self.drag = LDrag::None;
            self.want = None;
            self.menu_point = None;
            self.refresh(project, pattern, channel);
        }
    }

    /// Re-reads the lane from a new snapshot: tabs, points, the selection (points that are
    /// gone are dropped) and edits waiting to be selected. A drag whose point another author
    /// removed is cancelled.
    pub fn refresh(&mut self, project: &Project, pattern: PatternId, channel: ChannelId) {
        let Ok(pat) = project.pattern(pattern) else {
            self.reset();
            return;
        };
        self.tabs = pat
            .lanes(channel)
            .iter()
            .map(|l| (l.controller, l.points.len()))
            .collect();
        self.points = match self.kind {
            LaneKind::Controller(c) => pat.lane(channel, c).points,
            LaneKind::Velocity => Vec::new(),
        };
        let have: BTreeSet<PointId> = self.points.iter().map(|p| p.id).collect();
        self.selection.retain(|id| have.contains(id));
        if self.menu_point.is_some_and(|p| !have.contains(&p)) {
            self.menu_point = None;
        }
        let gone = match &self.drag {
            LDrag::Move { ids, .. } => ids.iter().any(|i| !have.contains(i)),
            LDrag::Tension { id, .. } => !have.contains(id),
            _ => false,
        };
        if gone {
            self.drag = LDrag::None;
        }
        if let Some(w) = &self.want {
            let found: BTreeSet<PointId> = self
                .points
                .iter()
                .filter(|p| w.ticks.contains(&p.tick) && !w.exclude.contains(&p.id))
                .map(|p| p.id)
                .collect();
            if !found.is_empty() {
                self.selection = found;
                self.want = None;
            }
        }
    }

    /// The tabs in order: velocity first, then the lanes that have points, then the lane
    /// being edited if it has none yet.
    pub fn tab_kinds(&self) -> Vec<LaneKind> {
        let mut v = vec![LaneKind::Velocity];
        v.extend(self.tabs.iter().map(|(c, _)| LaneKind::Controller(*c)));
        if !v.contains(&self.kind) {
            v.push(self.kind);
        }
        v
    }

    /// The selector's entries: lanes with points first, then the usual controllers.
    pub fn selector_entries(&self) -> Vec<LaneKind> {
        let mut v: Vec<LaneKind> = self
            .tabs
            .iter()
            .map(|(c, _)| LaneKind::Controller(*c))
            .collect();
        v.push(LaneKind::Velocity);
        for c in PRESETS {
            let k = LaneKind::Controller(c);
            if !v.contains(&k) {
                v.push(k);
            }
        }
        v
    }

    // ---- geometry --------------------------------------------------------------------

    fn span(&self) -> f32 {
        (self.h - 2.0 * PAD).max(1.0)
    }

    /// Where `v` sits in the body (top of the body = the top of the controller's range).
    pub fn y_of(&self, c: Controller, v: f64) -> f32 {
        let (lo, hi) = c.range();
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        PAD + self.span() * (1.0 - t as f32)
    }

    pub fn value_at_y(&self, c: Controller, y: f32) -> f64 {
        let (lo, hi) = c.range();
        let t = 1.0 - f64::from((y - PAD) / self.span());
        lo + (hi - lo) * t.clamp(0.0, 1.0)
    }

    /// A point's place in the body.
    pub fn pos_of(&self, env: &Env, c: Controller, tick: Tick, value: f64) -> Vec2 {
        Vec2::new(env.view.x_of(tick as f64), self.y_of(c, value))
    }

    fn grid_for(env: &Env, mods: Mods) -> Tick {
        if mods.alt { 1 } else { env.snap.max(1) }
    }

    /// The point under `pos`, if one is within reach (nearest first).
    pub fn point_at(&self, env: &Env, pos: Vec2) -> Option<PointId> {
        let c = self.controller()?;
        self.points
            .iter()
            .map(|p| ((self.pos_of(env, c, p.tick, p.value) - pos).length(), p.id))
            .filter(|(d, _)| *d <= HIT_R)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// The point that starts the segment under `x` (the first point before the lane starts).
    pub fn segment_at(&self, env: &Env, x: f32) -> Option<PointId> {
        let t = env.view.tick_at(x);
        self.points
            .iter()
            .rev()
            .find(|p| p.tick as f64 <= t)
            .or_else(|| self.points.first())
            .map(|p| p.id)
    }

    /// The tension diamonds: segment start, and where the diamond sits.
    pub fn handles(&self, env: &Env) -> Vec<(PointId, Vec2)> {
        let Some(c) = self.controller() else {
            return Vec::new();
        };
        let pts = self.display_points();
        pts.windows(2)
            .filter_map(|w| {
                let (a, b) = (&w[0], &w[1]);
                let t_h = handle_t(a.curve)?;
                if b.tick <= a.tick || (b.value - a.value).abs() < 1e-9 {
                    return None;
                }
                let tick = a.tick as f64 + (b.tick - a.tick) as f64 * t_h;
                let v = a.value + (b.value - a.value) * a.curve.shape(a.tension, t_h);
                let p = Vec2::new(env.view.x_of(tick), self.y_of(c, v));
                Some((a.id, p))
            })
            .collect()
    }

    fn handle_at(&self, env: &Env, pos: Vec2) -> Option<PointId> {
        self.handles(env)
            .into_iter()
            .map(|(id, p)| ((p - pos).length(), id))
            .filter(|(d, _)| *d <= HANDLE_R)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// The points as drawn: an in-flight move or tension drag applied.
    pub fn display_points(&self) -> Vec<CcPoint> {
        let Some(c) = self.controller() else {
            return Vec::new();
        };
        let mut v = self.points.clone();
        match &self.drag {
            LDrag::Move { ids, dt, dv, .. } => {
                for p in v.iter_mut().filter(|p| ids.contains(&p.id)) {
                    p.tick = (p.tick as i64 + dt).max(0) as Tick;
                    p.value = snap_value(c, p.value + dv);
                }
            }
            LDrag::Tension { id, tension, .. } => {
                for p in v.iter_mut().filter(|p| p.id == *id) {
                    p.tension = *tension;
                }
            }
            _ => {}
        }
        v.sort_by_key(|p| (p.tick, p.id));
        v
    }

    // ---- gestures --------------------------------------------------------------------

    pub fn press(&mut self, env: &Env, pos: Vec2, btn: Btn, mods: Mods) -> Vec<PrOut> {
        let Some(c) = self.controller() else {
            return Vec::new();
        };
        self.focused = true;
        match btn {
            Btn::Middle => Vec::new(),
            Btn::Secondary => {
                if let Some(id) = self.point_at(env, pos) {
                    if mods.shift {
                        // the menu of this point's segment
                        self.menu_point = Some(id);
                        self.selection = BTreeSet::from([id]);
                        Vec::new()
                    } else {
                        self.menu_point = None;
                        self.selection.remove(&id);
                        vec![PrOut::Cmd(mapping::cc_remove_points(
                            env.pattern,
                            env.channel,
                            &[id],
                        ))]
                    }
                } else {
                    self.menu_point = self.segment_at(env, pos.x);
                    Vec::new()
                }
            }
            Btn::Primary => {
                if let Some(id) = self.point_at(env, pos) {
                    if mods.shift || mods.ctrl {
                        if !self.selection.remove(&id) {
                            self.selection.insert(id);
                        }
                        return Vec::new();
                    }
                    if !self.selection.contains(&id) {
                        self.selection = BTreeSet::from([id]);
                    }
                    let anchor = self.points.iter().find(|p| p.id == id).cloned();
                    if let Some(a) = anchor {
                        let ids: Vec<PointId> = self.selection.iter().copied().collect();
                        let min_tick = self
                            .points
                            .iter()
                            .filter(|p| self.selection.contains(&p.id))
                            .map(|p| p.tick)
                            .min()
                            .unwrap_or(0);
                        self.drag = LDrag::Move {
                            ids,
                            anchor_tick: a.tick,
                            anchor_value: a.value,
                            grab_dt: env.view.tick_at(pos.x) - a.tick as f64,
                            grab_dv: self.value_at_y(c, pos.y) - a.value,
                            min_tick,
                            dt: 0,
                            dv: 0.0,
                            start: pos,
                            moved: false,
                        };
                    }
                } else if let Some(id) = self.handle_at(env, pos) {
                    if let Some(a) = self.points.iter().position(|p| p.id == id) {
                        let (pa, pb) = (&self.points[a], self.points.get(a + 1));
                        if let (Some(t_h), Some(pb)) = (handle_t(pa.curve), pb) {
                            let at_handle =
                                pa.value + (pb.value - pa.value) * pa.curve.shape(pa.tension, t_h);
                            self.drag = LDrag::Tension {
                                id,
                                base: pa.tension,
                                tension: pa.tension,
                                kind: pa.curve,
                                a: pa.value,
                                b: pb.value,
                                t_h,
                                grab_dv: self.value_at_y(c, pos.y) - at_handle,
                                start: pos,
                                moved: false,
                            };
                        }
                    }
                } else if mods.ctrl {
                    self.drag = LDrag::Boxed {
                        from: pos,
                        to: pos,
                        additive: mods.shift,
                    };
                } else {
                    let s = self.sample_at(env, c, pos, mods);
                    if mods.shift {
                        self.drag = LDrag::Line { from: s, to: s };
                    } else {
                        self.selection.clear();
                        self.drag = LDrag::Draw {
                            samples: BTreeMap::from([s]),
                            first: s,
                            last: s.0,
                            start: pos,
                            moved: false,
                        };
                    }
                }
                Vec::new()
            }
        }
    }

    fn sample_at(&self, env: &Env, c: Controller, pos: Vec2, mods: Mods) -> (Tick, f64) {
        let t = snap_tick(env.view.tick_at(pos.x).max(0.0), Self::grid_for(env, mods));
        (t, snap_value(c, self.value_at_y(c, pos.y)))
    }

    pub fn drag_to(&mut self, env: &Env, pos: Vec2, mods: Mods) {
        let Some(c) = self.controller() else { return };
        let sample = self.sample_at(env, c, pos, mods);
        let value_here = self.value_at_y(c, pos.y);
        let tick_here = env.view.tick_at(pos.x);
        match &mut self.drag {
            LDrag::Move {
                anchor_tick,
                anchor_value,
                grab_dt,
                grab_dv,
                min_tick,
                dt,
                dv,
                start,
                moved,
                ..
            } => {
                // a press that barely moves is a click: it selects, it does not rewrite
                if !*moved && (pos - *start).length() <= SLOP {
                    return;
                }
                *moved = true;
                let grid = Self::grid_for(env, mods);
                let target = snap_tick((tick_here - *grab_dt).max(0.0), grid) as i64;
                let lowest = -(*min_tick as i64);
                *dt = (target - *anchor_tick as i64).max(lowest);
                *dv = snap_value(c, value_here - *grab_dv) - *anchor_value;
            }
            LDrag::Draw {
                samples,
                last,
                start,
                moved,
                ..
            } => {
                if (pos - *start).length() > SLOP {
                    *moved = true;
                }
                // whichever way the pointer travels, the stroke stays in time order: drawing
                // back over it replaces what is under the new stretch
                let (lo, hi) = (sample.0.min(*last), sample.0.max(*last));
                let under: Vec<Tick> = samples
                    .range(lo..=hi)
                    .map(|(t, _)| *t)
                    .filter(|t| t != last)
                    .collect();
                for t in under {
                    samples.remove(&t);
                }
                samples.insert(sample.0, sample.1);
                *last = sample.0;
            }
            LDrag::Line { to, .. } => *to = sample,
            LDrag::Boxed { to, .. } => *to = pos,
            LDrag::Tension {
                tension,
                kind,
                a,
                b,
                t_h,
                grab_dv,
                start,
                moved,
                ..
            } => {
                if !*moved && (pos - *start).length() <= SLOP {
                    return;
                }
                *moved = true;
                *tension = tension_for(*kind, *a, *b, *t_h, value_here - *grab_dv);
            }
            LDrag::None => {}
        }
    }

    pub fn release(&mut self, env: &Env, _pos: Vec2, _mods: Mods) -> Vec<PrOut> {
        let drag = std::mem::replace(&mut self.drag, LDrag::None);
        let Some(c) = self.controller() else {
            return Vec::new();
        };
        let (p, ch) = (env.pattern, env.channel);
        let spec = |tick: Tick, value: f64| CcPointSpec {
            controller: c,
            tick,
            value,
            curve: CurveKind::Linear,
            tension: 0.0,
        };
        match drag {
            LDrag::None => Vec::new(),
            LDrag::Move { ids, dt, dv, .. } => {
                if dt == 0 && dv == 0.0 {
                    return Vec::new();
                }
                let moved: Vec<CcPoint> = self
                    .points
                    .iter()
                    .filter(|x| ids.contains(&x.id))
                    .map(|x| CcPoint {
                        tick: (x.tick as i64 + dt).max(0) as Tick,
                        value: snap_value(c, x.value + dv),
                        ..x.clone()
                    })
                    .collect();
                vec![self.rewrite(env, c, &moved)]
            }
            LDrag::Draw {
                first,
                moved: false,
                ..
            } => {
                let (t, v) = first;
                vec![PrOut::Cmd(mapping::cc_add_points(p, ch, c, &[spec(t, v)]))]
            }
            LDrag::Draw { samples, .. } => {
                let samples: Vec<(Tick, f64)> = samples.into_iter().collect();
                let kept = self.thin(env, c, &samples);
                let (t0, t1) = (kept[0].0, kept[kept.len() - 1].0);
                let pts: Vec<CcPointSpec> = kept.iter().map(|(t, v)| spec(*t, *v)).collect();
                vec![PrOut::Cmd(mapping::cc_set(p, ch, c, t0, t1 + 1, &pts))]
            }
            LDrag::Line { from, to } => {
                if from.0 == to.0 {
                    return vec![PrOut::Cmd(mapping::cc_add_points(
                        p,
                        ch,
                        c,
                        &[spec(to.0, to.1)],
                    ))];
                }
                let (a, b) = if from.0 <= to.0 {
                    (from, to)
                } else {
                    (to, from)
                };
                vec![PrOut::Cmd(mapping::cc_set(
                    p,
                    ch,
                    c,
                    a.0,
                    b.0 + 1,
                    &[spec(a.0, a.1), spec(b.0, b.1)],
                ))]
            }
            LDrag::Boxed { from, to, additive } => {
                let r = Rect::from_two_pos(Pos2::new(from.x, from.y), Pos2::new(to.x, to.y));
                let inside: BTreeSet<PointId> = self
                    .points
                    .iter()
                    .filter(|x| {
                        let q = self.pos_of(env, c, x.tick, x.value);
                        r.contains(Pos2::new(q.x, q.y))
                    })
                    .map(|x| x.id)
                    .collect();
                if additive {
                    self.selection.extend(inside);
                } else {
                    self.selection = inside;
                }
                Vec::new()
            }
            LDrag::Tension {
                id, base, tension, ..
            } => {
                if (tension - base).abs() < 1e-6 {
                    return Vec::new();
                }
                let Some(x) = self.points.iter().find(|x| x.id == id) else {
                    return Vec::new();
                };
                let edited = CcPoint {
                    tension,
                    ..x.clone()
                };
                vec![self.rewrite(env, c, &[edited])]
            }
        }
    }

    /// Thins a stroke: the fewest points that keep the line within [`THIN_PX`] of the
    /// stroke, measured on screen, and always its two ends.
    pub fn thin(&self, env: &Env, c: Controller, samples: &[(Tick, f64)]) -> Vec<(Tick, f64)> {
        if samples.len() <= 2 {
            return samples.to_vec();
        }
        let px: Vec<Vec2> = samples
            .iter()
            .map(|(t, v)| self.pos_of(env, c, *t, *v))
            .collect();
        let mut keep = vec![false; samples.len()];
        keep[0] = true;
        *keep.last_mut().unwrap() = true;
        let mut stack = vec![(0usize, samples.len() - 1)];
        while let Some((lo, hi)) = stack.pop() {
            if hi <= lo + 1 {
                continue;
            }
            let (a, b) = (px[lo], px[hi]);
            let ab = b - a;
            let len2 = ab.length_sq().max(1e-6);
            let (mut worst, mut at) = (0.0f32, lo);
            for (i, q) in px.iter().enumerate().take(hi).skip(lo + 1) {
                let t = (((*q - a).x * ab.x + (*q - a).y * ab.y) / len2).clamp(0.0, 1.0);
                let d = (*q - (a + ab * t)).length();
                if d > worst {
                    (worst, at) = (d, i);
                }
            }
            if worst > THIN_PX {
                keep[at] = true;
                stack.push((lo, at));
                stack.push((at, hi));
            }
        }
        samples
            .iter()
            .zip(keep)
            .filter_map(|(s, k)| k.then_some(*s))
            .collect()
    }

    /// Edited points: the old ones go and the new ones come, one batch, one undo step.
    fn rewrite(&mut self, env: &Env, c: Controller, new: &[CcPoint]) -> PrOut {
        let ids: Vec<PointId> = new.iter().map(|p| p.id).collect();
        let specs: Vec<CcPointSpec> = new
            .iter()
            .map(|p| CcPointSpec {
                controller: c,
                tick: p.tick,
                value: p.value,
                curve: p.curve,
                tension: p.tension,
            })
            .collect();
        self.want = Some(Want {
            ticks: new.iter().map(|p| p.tick).collect(),
            exclude: self.points.iter().map(|p| p.id).collect(),
        });
        PrOut::Batch(vec![
            mapping::cc_remove_points(env.pattern, env.channel, &ids),
            mapping::cc_add_points(env.pattern, env.channel, c, &specs),
        ])
    }

    /// The segment menu's choice: the curve of the segment that starts at `id`.
    pub fn set_curve(&mut self, env: &Env, id: PointId, curve: CurveKind) -> Vec<PrOut> {
        let Some(c) = self.controller() else {
            return Vec::new();
        };
        let Some(x) = self.points.iter().find(|x| x.id == id) else {
            return Vec::new();
        };
        if x.curve == curve {
            return Vec::new();
        }
        let edited = CcPoint { curve, ..x.clone() };
        vec![self.rewrite(env, c, &[edited])]
    }

    /// Delete: removes the selected points.
    pub fn delete_selected(&mut self, env: &Env) -> Vec<PrOut> {
        if self.selection.is_empty() || self.controller().is_none() {
            return Vec::new();
        }
        let ids: Vec<PointId> = std::mem::take(&mut self.selection).into_iter().collect();
        vec![PrOut::Cmd(mapping::cc_remove_points(
            env.pattern,
            env.channel,
            &ids,
        ))]
    }

    pub fn select_all(&mut self) {
        self.selection = self.points.iter().map(|p| p.id).collect();
    }

    /// "bar.beat.tick  value" for the pointer at `pos` (or the point being dragged).
    pub fn readout(&self, env: &Env, pos: Vec2, mods: Mods, project: &Project) -> String {
        let Some(c) = self.controller() else {
            return String::new();
        };
        let (t, v) = match &self.drag {
            LDrag::Draw { samples, last, .. } => (*last, samples.get(last).copied().unwrap_or(0.0)),
            LDrag::Line { to, .. } => *to,
            LDrag::Move {
                anchor_tick,
                anchor_value,
                dt,
                dv,
                ..
            } => (
                (*anchor_tick as i64 + dt).max(0) as Tick,
                snap_value(c, anchor_value + dv),
            ),
            _ => self.sample_at(env, c, pos, mods),
        };
        format!(
            "{}  {}",
            crate::auto_editor::position_text(t, project),
            units(c, v)
        )
    }

    // ---- painting --------------------------------------------------------------------

    /// The evaluated curve across `width` pixels: a sample every two pixels, and the two
    /// sides of every jump, so a hold or a shared tick draws as a step.
    pub fn curve_line(&self, env: &Env, c: Controller, width: f32) -> Vec<Pos2> {
        let pts = self.display_points();
        if pts.is_empty() {
            return Vec::new();
        }
        let autos: Vec<AutomationPoint> = pts.iter().map(CcPoint::as_auto).collect();
        let n = ((width / 2.0) as usize).max(2);
        let mut xs: Vec<f32> = (0..=n).map(|i| width * i as f32 / n as f32).collect();
        for p in &pts {
            let x = env.view.x_of(p.tick as f64);
            if x > -2.0 && x < width + 2.0 {
                xs.push(x);
                xs.push(x - 0.05);
            }
        }
        xs.sort_by(f32::total_cmp);
        xs.into_iter()
            .filter_map(|x| {
                let t = env.view.tick_at(x);
                let v = ph_core::value_at(&autos, t)?;
                Some(Pos2::new(x, self.y_of(c, f64::from(c.wire(v)))))
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        p: &Painter,
        body: Rect,
        gutter: Rect,
        env: &Env,
        theme: &Theme,
        touched: &BTreeSet<PointId>,
        focused_frame: bool,
    ) {
        let col = &theme.colors;
        let Some(c) = self.controller() else {
            return;
        };
        let gp = p.with_clip_rect(body);
        gp.rect_filled(body, CornerRadius::ZERO, col.bg2);
        p.rect_filled(gutter, CornerRadius::ZERO, col.bg1);
        super::paint_time_grid(&gp, body, env.view.x0, env.view.px_per_tick, env.bar, theme);
        let o = body.min.to_vec2();
        // value lines: quarters of the range, the centre brighter (the bend's zero)
        let (lo, hi) = c.range();
        for k in 0..=4 {
            let v = lo + (hi - lo) * f64::from(k) / 4.0;
            let y = body.top() + self.y_of(c, v);
            let centre = k == 2 && c == Controller::PitchBend;
            gp.line_segment(
                [Pos2::new(body.left(), y), Pos2::new(body.right(), y)],
                Stroke::new(1.0, if centre { col.grid_bar } else { col.grid_minor }),
            );
        }
        for (v, a) in [
            (hi, Align2::LEFT_TOP),
            (lo + (hi - lo) / 2.0, Align2::LEFT_CENTER),
            (lo, Align2::LEFT_BOTTOM),
        ] {
            let y = body.top() + self.y_of(c, v);
            let txt = match c {
                Controller::Cc(_) => units(c, v),
                Controller::PitchBend => {
                    if v == 0.0 {
                        "0".into()
                    } else {
                        format!("{:+}", if v > 0.0 { 8191 } else { -8192 })
                    }
                }
            };
            p.text(
                Pos2::new(gutter.left() + 4.0, y),
                a,
                txt,
                FontId::new(10.0, crate::fonts::mono()),
                col.dim,
            );
        }
        // pattern end
        let end_x = body.left() + env.view.x_of(env.pattern_len as f64);
        gp.line_segment(
            [
                Pos2::new(end_x, body.top()),
                Pos2::new(end_x, body.bottom()),
            ],
            Stroke::new(2.0, col.accent_dim),
        );
        // the curve, as the engine plays it, filled toward the baseline
        let line: Vec<Pos2> = self
            .curve_line(env, c, body.width())
            .into_iter()
            .map(|q| q + o)
            .collect();
        if line.len() > 1 {
            let base = body.top() + self.y_of(c, if lo < 0.0 { 0.0 } else { lo });
            fill_toward(&gp, &line, base, col.accent.gamma_multiply(0.12));
            gp.add(Shape::line(line, Stroke::new(2.0, col.accent)));
        }
        // diamonds, then points
        for (id, q) in self.handles(env) {
            let q = Pos2::new(q.x, q.y) + o;
            let active = matches!(&self.drag, LDrag::Tension { id: d, .. } if *d == id);
            gp.add(Shape::convex_polygon(
                vec![
                    q + Vec2::new(0.0, -4.0),
                    q + Vec2::new(4.0, 0.0),
                    q + Vec2::new(0.0, 4.0),
                    q + Vec2::new(-4.0, 0.0),
                ],
                if active { col.accent_hot } else { col.bg3 },
                Stroke::new(1.0, col.accent_dim),
            ));
        }
        for pt in self.display_points() {
            let q = self.pos_of(env, c, pt.tick, pt.value);
            let q = Pos2::new(q.x, q.y) + o;
            if q.x < body.left() - 8.0 || q.x > body.right() + 8.0 {
                continue;
            }
            let sel = self.selection.contains(&pt.id);
            if touched.contains(&pt.id) {
                gp.circle_stroke(q, 9.0, Stroke::new(1.5, col.agent));
            }
            if sel {
                gp.circle_stroke(q, 7.0, Stroke::new(1.5, col.selection));
            }
            gp.circle_filled(q, 4.0, if sel { col.accent_hot } else { col.accent });
            gp.circle_stroke(q, 4.0, Stroke::new(1.0, col.bg0));
        }
        // gesture previews
        match &self.drag {
            LDrag::Draw { samples, moved, .. } if *moved => {
                let pts: Vec<Pos2> = samples
                    .iter()
                    .map(|(t, v)| {
                        let q = self.pos_of(env, c, *t, *v);
                        Pos2::new(q.x, q.y) + o
                    })
                    .collect();
                gp.add(Shape::line(pts, Stroke::new(1.5, col.accent_hot)));
            }
            LDrag::Line { from, to } => {
                let a = self.pos_of(env, c, from.0, from.1);
                let b = self.pos_of(env, c, to.0, to.1);
                gp.line_segment(
                    [Pos2::new(a.x, a.y) + o, Pos2::new(b.x, b.y) + o],
                    Stroke::new(1.5, col.accent_hot),
                );
            }
            LDrag::Boxed { from, to, .. } => {
                let r =
                    Rect::from_two_pos(Pos2::new(from.x, from.y) + o, Pos2::new(to.x, to.y) + o);
                gp.rect_filled(r, CornerRadius::ZERO, col.selection.gamma_multiply(0.15));
                gp.rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, col.selection),
                    egui::StrokeKind::Inside,
                );
            }
            _ => {}
        }
        if focused_frame || self.focused {
            gp.rect_stroke(
                body,
                CornerRadius::ZERO,
                Stroke::new(1.0, col.accent_dim),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// Where along a segment its tension diamond sits: the middle of a linear segment, a quarter
/// of an S-curve (whose middle never moves). Hold and smooth segments have none.
pub fn handle_t(kind: CurveKind) -> Option<f64> {
    match kind {
        CurveKind::Linear => Some(0.5),
        CurveKind::SCurve => Some(0.25),
        CurveKind::Hold | CurveKind::Smooth => None,
    }
}

/// The tension that puts the curve through `value` at the diamond's place `t_h` (the nearest
/// of a fine scan, so it is robust for every shape).
pub fn tension_for(kind: CurveKind, a: f64, b: f64, t_h: f64, value: f64) -> f32 {
    if (b - a).abs() < 1e-9 {
        return 0.0;
    }
    let want = ((value - a) / (b - a)).clamp(0.0, 1.0);
    let mut best = (f64::INFINITY, 0.0f32);
    for i in 0..=100 {
        let t = (i as f32 - 50.0) / 50.0;
        let d = (kind.shape(t, t_h) - want).abs();
        if d < best.0 {
            best = (d, t);
        }
    }
    (best.1 * 100.0).round() / 100.0
}

/// Fills between a curve and the horizontal line `base`, one trapezoid per step (the base may
/// sit in the middle of the curve's range, as the bend lane's zero does).
fn fill_toward(p: &Painter, line: &[Pos2], base: f32, color: egui::Color32) {
    let mut mesh = egui::epaint::Mesh::default();
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let i = mesh.vertices.len() as u32;
        let v = |pos: Pos2| egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        };
        mesh.vertices
            .extend([v(a), v(b), v(Pos2::new(b.x, base)), v(Pos2::new(a.x, base))]);
        mesh.indices.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
    }
    p.add(Shape::mesh(mesh));
}

// ---- the header: collapse button, selector, tabs ----------------------------------------

impl LaneState {
    /// Draws the header strip into `rect`; a lane the user picks becomes the shown one.
    #[allow(clippy::too_many_arguments)]
    pub fn show_header(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        project: &Project,
        pattern: PatternId,
        channel: ChannelId,
        theme: &Theme,
        readout: &str,
    ) {
        use crate::hint::HintExt;
        let col = &theme.colors;
        ui.painter().rect_filled(rect, CornerRadius::ZERO, col.bg3);
        let mut chosen: Option<LaneKind> = None;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(Vec2::new(4.0, 1.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.set_clip_rect(rect);
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 5.0;
        let toggle = if self.open { "Lanes -" } else { "Lanes +" };
        if crate::widgets::btn(ui, toggle)
            .tip("Show or hide the controller lane area")
            .clicked()
        {
            self.open = !self.open;
        }
        egui::ComboBox::from_id_salt("lane_selector")
            .selected_text(self.kind.label())
            .width(140.0)
            .show_ui(ui, |ui| {
                for k in self.selector_entries() {
                    let n = match k {
                        LaneKind::Controller(c) => {
                            self.tabs.iter().find(|(x, _)| *x == c).map(|(_, n)| *n)
                        }
                        LaneKind::Velocity => None,
                    };
                    let text = match n {
                        Some(n) => format!("{}  ({n})", k.label()),
                        None => k.label(),
                    };
                    if ui.selectable_label(self.kind == k, text).clicked() {
                        chosen = Some(k);
                        ui.close();
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Other CC\u{2026}");
                    ui.add(egui::DragValue::new(&mut self.other_cc).range(0..=ph_core::MAX_CC));
                    if ui.button("Open").clicked() {
                        chosen = Some(LaneKind::Controller(Controller::Cc(self.other_cc)));
                        ui.close();
                    }
                });
            })
            .response
            .hint(
                "Lane",
                &self.kind.label(),
                "chooses the controller lane under the notes",
            );
        for k in self.tab_kinds() {
            let r = ui
                .selectable_label(self.kind == k, k.short())
                .on_hover_text(k.label());
            let r = r.hint(&k.label(), "", "click to show this lane");
            if r.clicked() {
                chosen = Some(k);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(readout)
                    .font(FontId::new(11.0, crate::fonts::mono()))
                    .color(col.accent),
            );
        });
        if let Some(k) = chosen {
            self.open = true;
            self.set_kind(k, project, pattern, channel);
        }
    }

    /// The right-click menu of a segment: its curve, and deleting its point.
    pub fn menu(&mut self, ui: &mut egui::Ui, env: &Env, theme: &Theme) -> Vec<PrOut> {
        let mut out = Vec::new();
        let Some(pid) = self.menu_point else {
            ui.close();
            return out;
        };
        let Some(cur) = self.points.iter().find(|p| p.id == pid).map(|p| p.curve) else {
            ui.close();
            return out;
        };
        ui.label(crate::widgets::caption(theme, "Curve after this point"));
        for (kind, label) in CURVES {
            let r = ui.add(egui::Button::selectable(
                cur == kind,
                crate::theme::caps(label),
            ));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            if r.clicked() {
                out.extend(self.set_curve(env, pid, kind));
                ui.close();
            }
        }
        ui.separator();
        if crate::widgets::menu_item(ui, "Delete point", "Del").clicked() {
            self.selection = BTreeSet::from([pid]);
            out.extend(self.delete_selected(env));
            ui.close();
        }
        out
    }
}
