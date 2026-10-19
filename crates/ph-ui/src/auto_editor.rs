//! The automation-clip editor (M2/T7): a floating workspace window with one clip's curve.
//!
//! Click empty space to add a point (drag before releasing to place it), drag a point to
//! move it, right-click a point or a segment to choose the segment's curve (linear, hold,
//! smooth, S-curve) or delete the point, Delete removes the selected point. Positions snap to
//! the grid (Alt = free). Like the playlist, the view model ([`AutoEditorState`]) is plain
//! data and functions of positions, tested without a window; every change is a command.

use crate::automation as au;
use crate::hint::HintExt;
use crate::mapping;
use crate::piano_roll::{Btn, Mods, SNAPS, paint_time_grid, snap_tick};
use crate::theme::{Theme, caps};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use ph_commands::Request;
use ph_core::{AutomationPoint, Clip, ClipId, CurveKind, ParamPath, PointId, Project, Tick};

/// Width of the value labels at the left.
pub const GUTTER_W: f32 = 58.0;
pub const RULER_H: f32 = 20.0;
/// Vertical padding inside the canvas, so points at the range ends stay grabbable.
const PAD: f32 = 10.0;
/// How close, in pixels, a press must be to grab a point.
const HIT_R: f32 = 9.0;

/// What the editor asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum AeOut {
    Cmd(Request),
    Batch(Vec<Request>),
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    None,
    /// A point being moved: where it would land.
    Move {
        id: PointId,
        start: (Tick, f64),
        tick: Tick,
        value: f64,
    },
    /// A point being placed on empty space.
    Add {
        tick: Tick,
        value: f64,
    },
}

pub struct AutoEditorState {
    /// Source tick at the left edge of the canvas.
    pub x0: f64,
    pub px_per_tick: f64,
    /// Snap grid in ticks (1 = off).
    pub snap: Tick,
    pub selected: Option<PointId>,
    drag: Drag,
    /// The point (or segment's left point) a right-click landed on.
    pub menu_point: Option<PointId>,
    /// The first frame fits the whole clip into the window.
    pub fitted: bool,
    /// Canvas size and value range of the last frame (screen units / plain values).
    pub size: Vec2,
    pub range: (f64, f64),
    /// Where the canvas was drawn last frame (screen coordinates), for tests.
    pub grid: Rect,
}

impl Default for AutoEditorState {
    fn default() -> Self {
        Self {
            x0: 0.0,
            px_per_tick: 0.1,
            snap: 240,
            selected: None,
            drag: Drag::None,
            menu_point: None,
            fitted: false,
            size: Vec2::new(600.0, 300.0),
            range: (0.0, 1.0),
            grid: Rect::NOTHING,
        }
    }
}

impl AutoEditorState {
    pub fn x_of(&self, tick: f64) -> f32 {
        ((tick - self.x0) * self.px_per_tick) as f32
    }
    pub fn tick_at(&self, x: f32) -> f64 {
        self.x0 + x as f64 / self.px_per_tick
    }
    pub fn y_of(&self, v: f64) -> f32 {
        let (lo, hi) = self.range;
        let t = if hi > lo {
            ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        PAD + (self.size.y - 2.0 * PAD).max(1.0) * (1.0 - t as f32)
    }
    pub fn value_at_y(&self, y: f32) -> f64 {
        let (lo, hi) = self.range;
        let t = 1.0 - ((y - PAD) / (self.size.y - 2.0 * PAD).max(1.0)) as f64;
        lo + (hi - lo) * t.clamp(0.0, 1.0)
    }

    /// Shows all of the clip.
    pub fn fit(&mut self, clip: &Clip) {
        let span = clip.length.max(1) as f64;
        self.px_per_tick = ((self.size.x as f64 - 16.0) / span).clamp(0.003, 2.0);
        self.x0 = clip.offset as f64 - 8.0 / self.px_per_tick;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag != Drag::None
    }

    fn grid_for(&self, mods: Mods) -> Tick {
        if mods.alt { 1 } else { self.snap.max(1) }
    }

    /// The point nearest to `pos`, if one is within reach.
    pub fn point_at(&self, clip: &Clip, pos: Vec2) -> Option<PointId> {
        let a = clip.automation()?;
        a.points
            .values()
            .map(|p| {
                let d = (Vec2::new(self.x_of(p.tick as f64), self.y_of(p.value)) - pos).length();
                (d, p.id)
            })
            .filter(|(d, _)| *d <= HIT_R)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// The point that starts the segment under `x`.
    pub fn segment_at(&self, clip: &Clip, x: f32) -> Option<PointId> {
        let a = clip.automation()?;
        let t = self.tick_at(x);
        a.points_in_order()
            .into_iter()
            .rev()
            .find(|p| p.tick as f64 <= t)
            .map(|p| p.id)
    }

    /// A value the target can hold: mute is on or off, stepped parameters are whole.
    pub fn quantize(project: &Project, target: &ParamPath, v: f64) -> f64 {
        if matches!(target, ParamPath::InsertMute(_)) {
            return v.round().clamp(0.0, 1.0);
        }
        if let Some(info) = au::param_info(project, target)
            && info.stepped
        {
            return v.round().clamp(info.min, info.max);
        }
        match target.fixed_range() {
            Some((lo, hi)) => v.clamp(lo, hi),
            None => v,
        }
    }

    pub fn press(&mut self, pos: Vec2, btn: Btn, mods: Mods, clip: &Clip, project: &Project) {
        let Some(a) = clip.automation() else { return };
        match btn {
            Btn::Middle => {}
            Btn::Secondary => {
                self.menu_point = self
                    .point_at(clip, pos)
                    .or_else(|| self.segment_at(clip, pos.x));
                if let Some(p) = self.menu_point {
                    self.selected = Some(p);
                }
            }
            Btn::Primary => match self.point_at(clip, pos) {
                Some(id) => {
                    self.selected = Some(id);
                    if let Some(p) = a.points.get(&id) {
                        self.drag = Drag::Move {
                            id,
                            start: (p.tick, p.value),
                            tick: p.tick,
                            value: p.value,
                        };
                    }
                }
                None => {
                    self.selected = None;
                    self.drag = Drag::Add {
                        tick: snap_tick(self.tick_at(pos.x), self.grid_for(mods)),
                        value: Self::quantize(project, &a.target, self.value_at_y(pos.y)),
                    };
                }
            },
        }
    }

    pub fn drag_to(&mut self, pos: Vec2, mods: Mods, clip: &Clip, project: &Project) {
        let Some(a) = clip.automation() else { return };
        let tick = snap_tick(self.tick_at(pos.x), self.grid_for(mods));
        let value = Self::quantize(project, &a.target, self.value_at_y(pos.y));
        match &mut self.drag {
            Drag::Move {
                tick: t, value: v, ..
            }
            | Drag::Add {
                tick: t, value: v, ..
            } => {
                *t = tick;
                *v = value;
            }
            Drag::None => {}
        }
    }

    pub fn release(&mut self, clip_id: ClipId) -> Vec<AeOut> {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Move {
                id,
                start,
                tick,
                value,
            } if (tick, value) != start => {
                vec![AeOut::Cmd(mapping::automation_move_point(
                    clip_id, id, tick, value,
                ))]
            }
            Drag::Add { tick, value } => vec![AeOut::Cmd(mapping::automation_add_point(
                clip_id, tick, value, "linear",
            ))],
            _ => Vec::new(),
        }
    }

    /// Delete / Backspace: removes the selected point.
    pub fn delete_selected(&mut self, clip_id: ClipId) -> Vec<AeOut> {
        match self.selected.take() {
            Some(p) => vec![AeOut::Cmd(mapping::automation_remove_points(clip_id, &[p]))],
            None => Vec::new(),
        }
    }

    /// Where a point is drawn: its place, or where an in-flight drag would put it.
    pub fn display_point(&self, p: &AutomationPoint) -> (Tick, f64) {
        match &self.drag {
            Drag::Move {
                id, tick, value, ..
            } if *id == p.id => (*tick, *value),
            _ => (p.tick, p.value),
        }
    }
}

/// The curve choices of the right-click menu.
pub const CURVES: [(CurveKind, &str, &str); 4] = [
    (CurveKind::Linear, "Linear", "linear"),
    (CurveKind::Hold, "Hold", "hold"),
    (CurveKind::Smooth, "Smooth", "smooth"),
    (CurveKind::SCurve, "S-curve", "s_curve"),
];

pub struct EditCtx<'a> {
    pub project: &'a Project,
    pub theme: &'a Theme,
    pub clip: ClipId,
    /// Song position in ticks, to show the playhead inside the clip.
    pub position: Tick,
}

/// Shows the editor for one clip; returns the commands it wants sent.
pub fn show(ui: &mut egui::Ui, st: &mut AutoEditorState, cx: &EditCtx<'_>) -> Vec<AeOut> {
    let mut out = Vec::new();
    let Some(clip) = cx.project.clips.get(&cx.clip) else {
        ui.label("This automation clip no longer exists.");
        return out;
    };
    let Some(a) = clip.automation() else {
        ui.label("This clip is not an automation clip.");
        return out;
    };
    let col = &cx.theme.colors;
    let target_name = au::target_name(cx.project, &a.target);

    // ---- header: what is automated, and the tools
    ui.add(
        egui::Label::new(
            egui::RichText::new(&target_name)
                .font(FontId::new(14.0, crate::fonts::cond_bold()))
                .color(col.text),
        )
        .truncate(),
    );
    let mut hover_text = String::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(crate::widgets::caption(cx.theme, "Snap"));
        let cur = SNAPS
            .iter()
            .find(|(_, t)| *t == st.snap)
            .map_or("custom", |(n, _)| *n);
        egui::ComboBox::from_id_salt(("auto_snap", cx.clip))
            .selected_text(caps(cur))
            .width(78.0)
            .show_ui(ui, |ui| {
                for (n, t) in SNAPS {
                    let ticks = if *t == 0 {
                        cx.project.settings.time_signature.ticks_per_bar()
                    } else {
                        *t
                    };
                    if ui.selectable_label(ticks == st.snap, caps(n)).clicked() {
                        st.snap = ticks;
                    }
                }
            })
            .response
            .hint_menu("Automation snap", cur, "chooses the grid points snap to");
        if crate::widgets::btn(ui, "Fit").clicked() {
            st.fit(clip);
        }
        ui.label(
            egui::RichText::new("click: add \u{b7} drag: move \u{b7} right-click: curve, delete")
                .size(11.0)
                .color(col.faint),
        );
    });

    // ---- canvas
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        Vec2::new(avail.x.max(80.0), avail.y.max(60.0)),
        Sense::click_and_drag(),
    );
    let resp = resp.hint(
        &target_name,
        "",
        "click to add a point, drag a point to move it, right-click for the curve or to delete",
    );
    let whole = resp.rect;
    let grid = Rect::from_min_max(
        Pos2::new(whole.left() + GUTTER_W, whole.top() + RULER_H),
        whole.max,
    );
    st.size = grid.size();
    st.grid = grid;
    st.range = au::display_range(cx.project, clip);
    if !st.fitted {
        st.fitted = true;
        st.fit(clip);
    }
    painter.rect_filled(whole, CornerRadius::ZERO, col.bg0);
    let gp = painter.with_clip_rect(grid);
    gp.rect_filled(grid, CornerRadius::ZERO, col.key_white);
    // the part of the timeline the clip plays
    let play = Rect::from_min_max(
        Pos2::new(grid.left() + st.x_of(clip.offset as f64), grid.top()),
        Pos2::new(
            grid.left() + st.x_of((clip.offset + clip.length) as f64),
            grid.bottom(),
        ),
    );
    gp.rect_filled(play.intersect(grid), CornerRadius::ZERO, col.row_alt);
    paint_time_grid(
        &gp,
        grid,
        st.x0,
        st.px_per_tick,
        cx.project.settings.time_signature.ticks_per_bar(),
        cx.theme,
    );
    // value lines and labels
    let (lo, hi) = st.range;
    for k in 0..=4 {
        let v = lo + (hi - lo) * k as f64 / 4.0;
        let y = grid.top() + st.y_of(v);
        gp.line_segment(
            [Pos2::new(grid.left(), y), Pos2::new(grid.right(), y)],
            Stroke::new(1.0, col.grid_minor),
        );
        crate::paint::text_fit(
            &painter,
            Pos2::new(whole.left() + GUTTER_W - 4.0, y),
            Align2::RIGHT_CENTER,
            &au::format_value(cx.project, &a.target, v),
            FontId::new(10.0, crate::fonts::mono()),
            col.dim,
            GUTTER_W - 6.0,
        );
    }
    // ruler: beats from the clip's start
    let rp = painter.with_clip_rect(Rect::from_min_max(
        whole.min,
        Pos2::new(whole.max.x, grid.top()),
    ));
    rp.rect_filled(
        Rect::from_min_max(
            Pos2::new(grid.left(), whole.top()),
            Pos2::new(grid.right(), grid.top()),
        ),
        CornerRadius::ZERO,
        col.bg3,
    );
    let bar = cx.project.settings.time_signature.ticks_per_bar().max(1);
    let mut step_bars = 1u64;
    while (bar * step_bars) as f64 * st.px_per_tick < 40.0 {
        step_bars *= 2;
    }
    let first = (st.x0.max(0.0) / bar as f64).floor() as u64;
    let mut b = first - first % step_bars;
    while ((b * bar) as f64) <= st.tick_at(grid.width()) {
        let x = grid.left() + st.x_of((b * bar) as f64);
        if x >= grid.left() {
            rp.line_segment(
                [Pos2::new(x, whole.top() + 10.0), Pos2::new(x, grid.top())],
                Stroke::new(1.0, col.grid_bar),
            );
            rp.text(
                Pos2::new(x + 3.0, whole.top() + 2.0),
                Align2::LEFT_TOP,
                format!("{}", b + 1),
                FontId::new(10.0, crate::fonts::mono()),
                col.dim,
            );
        }
        b += step_bars;
    }

    // ---- the curve
    let order = a.points_in_order();
    let shown: Vec<(PointId, Tick, f64)> = order
        .iter()
        .map(|p| {
            let (t, v) = st.display_point(p);
            (p.id, t, v)
        })
        .collect();
    // a preview clip with the in-flight point, so the line follows the drag
    let mut preview = a.clone();
    for (id, t, v) in &shown {
        if let Some(p) = preview.points.get_mut(id) {
            p.tick = *t;
            p.value = *v;
        }
    }
    if let Drag::Add { tick, value } = &st.drag {
        let id = PointId(u32::MAX);
        preview.points.insert(
            id,
            AutomationPoint {
                id,
                tick: *tick,
                value: *value,
                curve: CurveKind::Linear,
                tension: 0.0,
            },
        );
    }
    let n = (grid.width() / 2.0) as usize;
    let line: Vec<Pos2> = (0..=n.max(2))
        .filter_map(|i| {
            let x = grid.width() * i as f32 / n.max(2) as f32;
            let v = preview.value_at(st.tick_at(x))?;
            Some(Pos2::new(grid.left() + x, grid.top() + st.y_of(v)))
        })
        .collect();
    if line.len() > 1 {
        au::fill_under(&gp, &line, grid.bottom(), col.accent.gamma_multiply(0.12));
        gp.add(Shape::line(line, Stroke::new(2.0, col.accent)));
    }
    for (id, t, v) in &shown {
        let c = Pos2::new(grid.left() + st.x_of(*t as f64), grid.top() + st.y_of(*v));
        let sel = st.selected == Some(*id);
        if sel {
            gp.circle_stroke(c, 7.0, Stroke::new(1.5, col.selection));
        }
        gp.circle_filled(c, 4.0, if sel { col.accent_hot } else { col.accent });
        gp.circle_stroke(c, 4.0, Stroke::new(1.0, col.bg0));
    }
    if let Drag::Add { tick, value } = &st.drag {
        let c = Pos2::new(
            grid.left() + st.x_of(*tick as f64),
            grid.top() + st.y_of(*value),
        );
        gp.circle_stroke(c, 5.0, Stroke::new(1.5, col.accent_hot));
        hover_text = format!(
            "{}  {}",
            position_text(*tick, cx.project),
            au::format_value(cx.project, &a.target, *value)
        );
    }
    // playhead, when it is inside the clip
    if cx.position >= clip.start && cx.position < clip.start + clip.length {
        let src = cx.position - clip.start + clip.offset;
        let x = grid.left() + st.x_of(src as f64);
        if x >= grid.left() && x <= grid.right() {
            crate::paint::playhead(
                &painter.with_clip_rect(grid.expand2(Vec2::new(0.0, RULER_H))),
                x,
                whole.top(),
                grid.bottom(),
                cx.theme,
                crate::motion::reduce(ui.ctx()),
            );
        }
    }
    painter.rect_stroke(
        grid,
        CornerRadius::ZERO,
        Stroke::new(1.0, col.line),
        egui::StrokeKind::Inside,
    );

    // ---- input
    let (pointer, mods, pressed, released, down, scroll) = ui.input(|i| {
        let m = i.modifiers;
        (
            i.pointer.latest_pos(),
            Mods {
                ctrl: m.command,
                shift: m.shift,
                alt: m.alt,
            },
            [
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_pressed(egui::PointerButton::Secondary),
            ],
            [
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Secondary),
            ],
            i.pointer.any_down(),
            i.smooth_scroll_delta(),
        )
    });
    let hovered = resp.hovered() || st.is_dragging();
    if let Some(pp) = pointer
        && hovered
    {
        let rel = pp - grid.min;
        if resp.hovered() && (scroll.x != 0.0 || scroll.y != 0.0) && grid.contains(pp) {
            if mods.ctrl {
                let t = st.tick_at(rel.x);
                st.px_per_tick =
                    (st.px_per_tick * (1.0 + scroll.y as f64 * 0.002)).clamp(0.003, 2.0);
                st.x0 = t - rel.x as f64 / st.px_per_tick;
            } else {
                st.x0 -= (scroll.y + scroll.x) as f64 / st.px_per_tick;
            }
        }
        if grid.contains(pp) && !st.is_dragging() {
            if pressed[0] {
                st.press(rel, Btn::Primary, mods, clip, cx.project);
            } else if pressed[1] {
                st.press(rel, Btn::Secondary, mods, clip, cx.project);
            }
            if hover_text.is_empty() {
                hover_text = format!(
                    "{}  {}",
                    position_text(snap_tick(st.tick_at(rel.x), st.grid_for(mods)), cx.project),
                    au::format_value(cx.project, &a.target, st.value_at_y(rel.y))
                );
            }
        }
        if down && st.is_dragging() {
            st.drag_to(rel, mods, clip, cx.project);
        }
        if released[0] && st.is_dragging() {
            out.extend(st.release(cx.clip));
        }
    }
    if resp.hovered()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        out.extend(st.delete_selected(cx.clip));
    }
    // the right-click menu: the segment after the point, or the point itself
    let menu_point = st.menu_point;
    let mut chosen: Option<AeOut> = None;
    resp.context_menu(|ui| {
        let Some(pid) = menu_point.filter(|p| a.points.contains_key(p)) else {
            ui.close();
            return;
        };
        let cur = a.points[&pid].curve;
        ui.label(crate::widgets::caption(cx.theme, "Curve after this point"));
        for (kind, label, id) in CURVES {
            let r = ui.add(egui::Button::selectable(cur == kind, caps(label)));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            if r.clicked() {
                chosen = Some(AeOut::Cmd(mapping::automation_set_curve(cx.clip, pid, id)));
                ui.close();
            }
        }
        ui.separator();
        if crate::widgets::menu_item(ui, "Delete point", "Del").clicked() {
            chosen = Some(AeOut::Cmd(mapping::automation_remove_points(
                cx.clip,
                &[pid],
            )));
            st.selected = None;
            ui.close();
        }
    });
    out.extend(chosen);
    // the readout, right-aligned over the ruler gutter
    if !hover_text.is_empty() {
        crate::paint::text_fit(
            &painter,
            Pos2::new(whole.right() - 6.0, whole.top() + RULER_H * 0.5),
            Align2::RIGHT_CENTER,
            &hover_text,
            FontId::new(11.0, crate::fonts::mono()),
            col.accent,
            (grid.width() - 8.0).max(0.0),
        );
    }
    out
}

/// "bar.beat.tick" of a tick relative to the clip's start (1-based bars and beats).
pub fn position_text(tick: Tick, project: &Project) -> String {
    let ts = project.settings.time_signature;
    let bar = ts.ticks_per_bar().max(1);
    let beat = (ph_core::PPQ * 4 / ts.denominator.max(1) as Tick).max(1);
    format!(
        "{}.{}.{:03}",
        tick / bar + 1,
        tick % bar / beat + 1,
        tick % beat
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, NullHost, Registry, Session};
    use serde_json::json;
    use std::sync::Arc;

    /// A project with a volume clip (two points: 0.2 at 0, 0.8 at 1920, linear).
    fn setup() -> (AutoEditorState, Project, ClipId, [PointId; 2]) {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("t"));
        let mut h = NullHost::default();
        let who = Caller::human();
        let r = s
            .execute(
                &who,
                Request::new(
                    "automation.add_clip",
                    json!({"target": "insert/1/volume", "track": 0, "start": 0, "length": 3840,
                           "points": [{"tick": 0, "value": 0.2}, {"tick": 1920, "value": 0.8}]}),
                ),
                &mut h,
            )
            .unwrap()
            .result;
        let clip = ClipId(r["clip"].as_u64().unwrap() as u32);
        let pts = [
            PointId(r["points"][0].as_u64().unwrap() as u32),
            PointId(r["points"][1].as_u64().unwrap() as u32),
        ];
        let p = s.project().as_ref().clone();
        let mut st = AutoEditorState {
            size: Vec2::new(800.0, 300.0),
            px_per_tick: 0.2,
            ..Default::default()
        };
        st.range = au::display_range(&p, p.clip(clip).unwrap());
        (st, p, clip, pts)
    }

    fn at(st: &AutoEditorState, tick: f64, v: f64) -> Vec2 {
        Vec2::new(st.x_of(tick), st.y_of(v))
    }

    #[test]
    fn a_click_on_empty_space_adds_a_snapped_point() {
        let (mut st, p, id, _) = setup();
        let clip = p.clip(id).unwrap().clone();
        let pos = at(&st, 960.0 + 50.0, 0.5);
        st.press(pos, Btn::Primary, Mods::default(), &clip, &p);
        assert!(st.is_dragging());
        let out = st.release(id);
        let [AeOut::Cmd(r)] = &out[..] else {
            panic!("{out:?}")
        };
        assert_eq!(r.command, "automation.add_point");
        assert_eq!(r.params["tick"], 960); // 1010 snaps to 960
        assert!((r.params["value"].as_f64().unwrap() - 0.5).abs() < 0.02);
        // Alt: no snap
        st.press(
            pos,
            Btn::Primary,
            Mods {
                alt: true,
                ..Default::default()
            },
            &clip,
            &p,
        );
        let out = st.release(id);
        let [AeOut::Cmd(r)] = &out[..] else { panic!() };
        assert_eq!(r.params["tick"], 1010);
    }

    #[test]
    fn dragging_a_point_moves_it_and_a_still_click_does_nothing() {
        let (mut st, p, id, pts) = setup();
        let clip = p.clip(id).unwrap().clone();
        let from = at(&st, 1920.0, 0.8);
        assert_eq!(st.point_at(&clip, from), Some(pts[1]));
        st.press(from, Btn::Primary, Mods::default(), &clip, &p);
        assert_eq!(st.selected, Some(pts[1]));
        assert!(st.release(id).is_empty(), "no move, no command");
        st.press(from, Btn::Primary, Mods::default(), &clip, &p);
        st.drag_to(at(&st, 2400.0, 0.4), Mods::default(), &clip, &p);
        let shown = st.display_point(clip.automation().unwrap().points.get(&pts[1]).unwrap());
        assert_eq!(shown.0, 2400);
        let out = st.release(id);
        let [AeOut::Cmd(r)] = &out[..] else { panic!() };
        assert_eq!(r.command, "automation.move_point");
        assert_eq!(r.params["point"], pts[1].0);
        assert_eq!(r.params["tick"], 2400);
        assert!((r.params["value"].as_f64().unwrap() - 0.4).abs() < 0.02);
    }

    #[test]
    fn right_click_picks_the_point_or_the_segment_and_delete_removes_the_selection() {
        let (mut st, p, id, pts) = setup();
        let clip = p.clip(id).unwrap().clone();
        st.press(
            at(&st, 0.0, 0.2) + Vec2::new(2.0, 1.0),
            Btn::Secondary,
            Mods::default(),
            &clip,
            &p,
        );
        assert_eq!(st.menu_point, Some(pts[0]));
        // between the points: the segment's left point
        st.press(
            at(&st, 800.0, 0.9),
            Btn::Secondary,
            Mods::default(),
            &clip,
            &p,
        );
        assert_eq!(st.menu_point, Some(pts[0]));
        // past the last point: that point's (last) segment
        st.press(
            at(&st, 3000.0, 0.9),
            Btn::Secondary,
            Mods::default(),
            &clip,
            &p,
        );
        assert_eq!(st.menu_point, Some(pts[1]));
        let out = st.delete_selected(id);
        let [AeOut::Cmd(r)] = &out[..] else { panic!() };
        assert_eq!(r.command, "automation.remove_points");
        assert_eq!(r.params["points"], json!([pts[1].0]));
        assert!(st.delete_selected(id).is_empty());
    }

    #[test]
    fn values_snap_to_what_the_target_can_hold() {
        let (_, p, ..) = setup();
        let mute: ParamPath = "insert/1/mute".parse().unwrap();
        assert_eq!(AutoEditorState::quantize(&p, &mute, 0.62), 1.0);
        let vol: ParamPath = "insert/1/volume".parse().unwrap();
        assert_eq!(AutoEditorState::quantize(&p, &vol, 3.0), 2.0);
        assert_eq!(AutoEditorState::quantize(&p, &vol, 0.123), 0.123);
    }

    #[test]
    fn positions_read_as_bar_beat_tick() {
        let p = Project::empty("t");
        assert_eq!(position_text(0, &p), "1.1.000");
        assert_eq!(position_text(3840 + 960 + 5, &p), "2.2.005");
    }
}
