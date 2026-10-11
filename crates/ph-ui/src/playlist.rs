//! Playlist: view model (hit-testing, snapping, drags) and the egui widget.

use crate::mapping;
use crate::piano_roll::{Btn, Mods, snap_floor, snap_tick};
use crate::theme::{Theme, lerp, rgb};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ph_commands::Request;
use ph_core::{Clip, ClipId, ObjectPath, PatternId, Project, Tick};
use std::collections::{BTreeSet, HashMap};

pub const HEADER_W: f32 = 132.0;
pub const RULER_H: f32 = 22.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Zone {
    Body,
    ResizeRight,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlOut {
    Cmd(Request),
    Batch(Vec<Request>),
    Seek(Tick),
    /// Open the piano roll for the first channel of this pattern's clip (double-click).
    OpenPattern(PatternId),
}

#[derive(Clone, Debug)]
enum Drag {
    None,
    Move {
        ids: Vec<ClipId>,
        grab_tick: f64,
        grab_track: i32,
        grab_start: Tick,
        dt: i64,
        dtrack: i32,
    },
    Resize {
        id: ClipId,
        base_len: Tick,
        grab_end: f64,
        delta: i64,
    },
    /// Placing a new clip at the pointer; committed on release.
    Place {
        track: u32,
        start: Tick,
        pattern: PatternId,
    },
    Erase {
        ids: BTreeSet<ClipId>,
    },
    Loop {
        from: Tick,
        to: Tick,
    },
    Box {
        from: Vec2,
        to: Vec2,
    },
}

pub struct PlaylistState {
    pub x0: f64,
    pub px_per_tick: f64,
    pub y0: f64,
    pub track_h: f32,
    /// Snap grid in ticks (default one bar).
    pub snap: Tick,
    pub selection: BTreeSet<ClipId>,
    drag: Drag,
    pub clips: Vec<Clip>,
    pub bar: Tick,
    pub n_tracks: u32,
    /// Where the clip grid was drawn last frame (screen coordinates), for tests.
    pub last_grid: Rect,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            x0: 0.0,
            px_per_tick: 0.05,
            y0: 0.0,
            track_h: 34.0,
            snap: 3840,
            selection: BTreeSet::new(),
            drag: Drag::None,
            clips: Vec::new(),
            bar: 3840,
            n_tracks: ph_core::PLAYLIST_TRACKS as u32,
            last_grid: Rect::NOTHING,
        }
    }
}

impl PlaylistState {
    pub fn sync(&mut self, project: &Project) {
        self.clips = project.clips.values().cloned().collect();
        self.bar = project.settings.time_signature.ticks_per_bar();
        self.n_tracks = project.tracks.len() as u32;
        let clips = &project.clips;
        self.selection.retain(|id| clips.contains_key(id));
    }

    pub fn x_of(&self, tick: f64) -> f32 {
        ((tick - self.x0) * self.px_per_tick) as f32
    }
    pub fn tick_at(&self, x: f32) -> f64 {
        self.x0 + x as f64 / self.px_per_tick
    }
    pub fn track_at(&self, y: f32) -> i32 {
        (self.y0 + y as f64 / self.track_h as f64).floor() as i32
    }
    pub fn y_of(&self, track: i32) -> f32 {
        ((track as f64 - self.y0) * self.track_h as f64) as f32
    }

    pub fn clip_rect(&self, track: i32, start: Tick, len: Tick) -> Rect {
        Rect::from_min_size(
            Pos2::new(self.x_of(start as f64), self.y_of(track)),
            Vec2::new((len as f64 * self.px_per_tick) as f32, self.track_h),
        )
    }

    pub fn grid(&self) -> Tick {
        self.snap.max(1)
    }

    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// Clip under a grid-relative point (later clips win).
    pub fn clip_at(&self, pos: Vec2) -> Option<(ClipId, Zone)> {
        for c in self.clips.iter().rev() {
            let r = self.clip_rect(c.track as i32, c.start, c.length);
            if r.contains(Pos2::new(pos.x, pos.y)) {
                let edge = (r.width() * 0.25).clamp(2.0, 8.0);
                return Some((
                    c.id,
                    if pos.x > r.right() - edge {
                        Zone::ResizeRight
                    } else {
                        Zone::Body
                    },
                ));
            }
        }
        None
    }

    fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    pub fn press(&mut self, pos: Vec2, btn: Btn, mods: Mods, place: Option<PatternId>) {
        let hit = self.clip_at(pos);
        match btn {
            Btn::Middle => {}
            Btn::Secondary => {
                let mut ids = BTreeSet::new();
                if let Some((id, _)) = hit {
                    ids.insert(id);
                }
                self.drag = Drag::Erase { ids };
            }
            Btn::Primary => match hit {
                Some((id, zone)) => {
                    let c = self.clip(id).cloned().expect("hit clip exists");
                    if !self.selection.contains(&id) {
                        if !mods.shift {
                            self.selection.clear();
                        }
                        self.selection.insert(id);
                    }
                    self.drag = if zone == Zone::ResizeRight {
                        Drag::Resize {
                            id,
                            base_len: c.length,
                            grab_end: self.tick_at(pos.x),
                            delta: 0,
                        }
                    } else {
                        Drag::Move {
                            ids: self.selection.iter().copied().collect(),
                            grab_tick: self.tick_at(pos.x),
                            grab_track: self.track_at(pos.y),
                            grab_start: c.start,
                            dt: 0,
                            dtrack: 0,
                        }
                    };
                }
                None if mods.ctrl => self.drag = Drag::Box { from: pos, to: pos },
                None => {
                    self.selection.clear();
                    let track = self.track_at(pos.y).clamp(0, self.n_tracks as i32 - 1) as u32;
                    if let Some(pattern) = place {
                        self.drag = Drag::Place {
                            track,
                            start: snap_floor(self.tick_at(pos.x), self.grid_for(mods)),
                            pattern,
                        };
                    }
                }
            },
        }
    }

    fn grid_for(&self, mods: Mods) -> Tick {
        if mods.alt { 1 } else { self.grid() }
    }

    /// Ruler press: seek on click; shift-drag makes the loop region.
    pub fn ruler_press(&mut self, x: f32, mods: Mods) -> Vec<PlOut> {
        let t = snap_tick(self.tick_at(x), self.grid_for(mods));
        if mods.shift {
            self.drag = Drag::Loop { from: t, to: t };
            Vec::new()
        } else {
            vec![PlOut::Seek(t)]
        }
    }

    /// Right-click on the ruler clears the loop region.
    pub fn ruler_clear_loop(&self) -> Vec<PlOut> {
        vec![PlOut::Cmd(mapping::set_loop(None))]
    }

    pub fn drag_to(&mut self, pos: Vec2, mods: Mods) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.drag = match drag {
            Drag::Move {
                ids,
                grab_tick,
                grab_track,
                grab_start,
                ..
            } => {
                let raw = self.tick_at(pos.x) - grab_tick;
                let target =
                    snap_tick((grab_start as f64 + raw).max(0.0), self.grid_for(mods)) as i64;
                Drag::Move {
                    ids,
                    grab_tick,
                    grab_track,
                    grab_start,
                    dt: target - grab_start as i64,
                    dtrack: self.track_at(pos.y) - grab_track,
                }
            }
            Drag::Resize {
                id,
                base_len,
                grab_end,
                ..
            } => {
                let start = self.clip(id).map_or(0, |c| c.start);
                let raw = self.tick_at(pos.x) - grab_end;
                let end = snap_tick((start + base_len) as f64 + raw, self.grid_for(mods)) as i64;
                Drag::Resize {
                    id,
                    base_len,
                    grab_end,
                    delta: (end - (start + base_len) as i64).max(1 - base_len as i64),
                }
            }
            Drag::Place { pattern, .. } => Drag::Place {
                pattern,
                track: self.track_at(pos.y).clamp(0, self.n_tracks as i32 - 1) as u32,
                start: snap_floor(self.tick_at(pos.x), self.grid_for(mods)),
            },
            Drag::Erase { mut ids } => {
                if let Some((id, _)) = self.clip_at(pos) {
                    ids.insert(id);
                }
                Drag::Erase { ids }
            }
            Drag::Loop { from, .. } => Drag::Loop {
                from,
                to: snap_tick(self.tick_at(pos.x), self.grid_for(mods)),
            },
            Drag::Box { from, .. } => Drag::Box { from, to: pos },
            Drag::None => Drag::None,
        };
    }

    pub fn release(&mut self, pos: Vec2, project: &Project) -> Vec<PlOut> {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let mut out = Vec::new();
        match drag {
            Drag::Move {
                ids, dt, dtrack, ..
            } => {
                if dt != 0 || dtrack != 0 {
                    // one transaction: every selected clip moves together
                    let reqs: Vec<Request> = ids
                        .iter()
                        .filter_map(|id| self.clip(*id))
                        .map(|c| {
                            mapping::clip_move(
                                c.id,
                                (c.start as i64 + dt).max(0) as Tick,
                                (c.track as i32 + dtrack).clamp(0, self.n_tracks as i32 - 1) as u32,
                            )
                        })
                        .collect();
                    out.push(PlOut::Batch(reqs));
                }
            }
            Drag::Resize {
                id,
                base_len,
                delta,
                ..
            } => {
                if delta != 0 {
                    let new_len = (base_len as i64 + delta).max(1) as Tick;
                    let ids: Vec<ClipId> = if self.selection.contains(&id) {
                        self.selection.iter().copied().collect()
                    } else {
                        vec![id]
                    };
                    let reqs: Vec<Request> = ids
                        .iter()
                        .filter_map(|i| self.clip(*i))
                        .map(|c| {
                            mapping::clip_resize(c.id, (c.length as i64 + delta).max(1) as Tick)
                        })
                        .collect();
                    let _ = new_len;
                    out.push(PlOut::Batch(reqs));
                }
            }
            Drag::Place {
                track,
                start,
                pattern,
            } => {
                out.push(PlOut::Cmd(mapping::clip_add(track, pattern, start, None)));
            }
            Drag::Erase { ids } => {
                if !ids.is_empty() {
                    let reqs = ids.iter().map(|id| mapping::clip_remove(*id)).collect();
                    for id in &ids {
                        self.selection.remove(id);
                    }
                    out.push(PlOut::Batch(reqs));
                }
            }
            Drag::Loop { from, to } => {
                let (a, b) = (from.min(to), from.max(to));
                out.push(PlOut::Cmd(mapping::set_loop((b > a).then_some((a, b)))));
            }
            Drag::Box { from, .. } => {
                let r = Rect::from_two_pos(Pos2::new(from.x, from.y), Pos2::new(pos.x, pos.y));
                self.selection = self
                    .clips
                    .iter()
                    .filter(|c| r.intersects(self.clip_rect(c.track as i32, c.start, c.length)))
                    .map(|c| c.id)
                    .collect();
            }
            Drag::None => {}
        }
        let _ = project;
        out
    }

    /// Ctrl+B: copy each selected clip to just after itself, as one transaction.
    pub fn duplicate(&self) -> Vec<PlOut> {
        let reqs: Vec<Request> = self
            .selection
            .iter()
            .filter_map(|id| self.clip(*id))
            .map(|c| mapping::clip_add(c.track, c.pattern, c.start + c.length, Some(c.length)))
            .collect();
        if reqs.is_empty() {
            vec![]
        } else {
            vec![PlOut::Batch(reqs)]
        }
    }

    pub fn delete_selected(&mut self) -> Vec<PlOut> {
        let reqs: Vec<Request> = self
            .selection
            .iter()
            .map(|id| mapping::clip_remove(*id))
            .collect();
        self.selection.clear();
        if reqs.is_empty() {
            vec![]
        } else {
            vec![PlOut::Batch(reqs)]
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.clips.iter().map(|c| c.id).collect();
    }

    /// Clip geometry with an in-flight drag applied: (track, start, length).
    pub fn display_geom(&self, c: &Clip) -> (i32, Tick, Tick) {
        match &self.drag {
            Drag::Move {
                ids, dt, dtrack, ..
            } if ids.contains(&c.id) => (
                (c.track as i32 + dtrack).clamp(0, self.n_tracks as i32 - 1),
                (c.start as i64 + dt).max(0) as Tick,
                c.length,
            ),
            Drag::Resize { id, delta, .. } if *id == c.id || self.selection.contains(&c.id) => (
                c.track as i32,
                c.start,
                (c.length as i64 + delta).max(1) as Tick,
            ),
            _ => (c.track as i32, c.start, c.length),
        }
    }

    /// Applies a zoom preset to a grid `grid_w` wide over a song of `song_len` ticks.
    pub fn zoom_to(&mut self, preset: crate::piano_roll::ZoomPreset, grid_w: f32, song_len: Tick) {
        let mid = self.tick_at(grid_w * 0.5);
        let ppt = preset
            .px_per_tick(grid_w, song_len, self.bar)
            .clamp(0.003, crate::piano_roll::ZOOM_MAX);
        self.px_per_tick = ppt;
        self.x0 = if preset == crate::piano_roll::ZoomPreset::Fit {
            0.0
        } else {
            (mid - (grid_w as f64 * 0.5) / ppt).max(0.0)
        };
    }

    pub fn zoom_x(&mut self, factor: f64, anchor_x: f32) {
        let t = self.tick_at(anchor_x);
        self.px_per_tick = (self.px_per_tick * factor).clamp(0.003, crate::piano_roll::ZOOM_MAX);
        self.x0 = (t - anchor_x as f64 / self.px_per_tick).max(0.0);
    }
}

// ---- widget ------------------------------------------------------------------------------

pub struct DrawCtx<'a> {
    pub project: &'a Project,
    pub theme: &'a Theme,
    /// Song position in ticks while the transport plays or is parked.
    pub position: Tick,
    pub loop_range: Option<(Tick, Tick)>,
    /// Pattern new clips are made from.
    pub place_pattern: Option<PatternId>,
    pub touches: &'a HashMap<ObjectPath, (String, f64)>,
    pub agent_selected: &'a BTreeSet<ObjectPath>,
    pub now: f64,
}

/// Result of showing the playlist: commands plus header edits.
#[derive(Default)]
pub struct PlShow {
    pub out: Vec<PlOut>,
    /// Track header actions.
    pub mute_track: Option<(u32, bool)>,
    pub rename_track: Option<u32>,
}

pub fn show(ui: &mut egui::Ui, st: &mut PlaylistState, cx: &DrawCtx<'_>) -> PlShow {
    let mut res = PlShow::default();
    st.sync(cx.project);
    let col = &cx.theme.colors;
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let whole = resp.rect;
    painter.rect_filled(whole, CornerRadius::ZERO, col.bg0);
    let grid = Rect::from_min_max(
        Pos2::new(whole.left() + HEADER_W, whole.top() + RULER_H),
        whole.max,
    );
    st.last_grid = grid;
    let ruler = Rect::from_min_max(
        Pos2::new(grid.left(), whole.top()),
        Pos2::new(grid.right(), grid.top()),
    );
    let headers = Rect::from_min_max(
        Pos2::new(whole.left(), grid.top()),
        Pos2::new(grid.left(), grid.bottom()),
    );
    let visible_tracks = (grid.height() / st.track_h).ceil() as i32 + 1;
    st.y0 = st.y0.clamp(
        0.0,
        (st.n_tracks as f64 - (grid.height() / st.track_h) as f64).max(0.0),
    );

    // ---- input
    let (pointer, mods, scroll, pressed, released, down) = ui.input(|i| {
        let m = i.modifiers;
        (
            i.pointer.latest_pos(),
            Mods {
                ctrl: m.command,
                shift: m.shift,
                alt: m.alt,
            },
            i.smooth_scroll_delta(),
            [
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_pressed(egui::PointerButton::Secondary),
            ],
            [
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Secondary),
            ],
            i.pointer.any_down(),
        )
    });
    let hovered = resp.hovered() || st.is_dragging();
    if let Some(pp) = pointer {
        let rel = pp - grid.min;
        if resp.hovered() && whole.contains(pp) && (scroll.x != 0.0 || scroll.y != 0.0) {
            if mods.ctrl {
                st.zoom_x(1.0 + scroll.y as f64 * 0.002, rel.x.max(0.0));
            } else if mods.shift {
                st.x0 = (st.x0 - (scroll.y + scroll.x) as f64 / st.px_per_tick).max(0.0);
            } else {
                st.y0 = (st.y0 - scroll.y as f64 / st.track_h as f64).max(0.0);
                st.x0 = (st.x0 - scroll.x as f64 / st.px_per_tick).max(0.0);
            }
        }
        if hovered {
            for (k, btn) in [Btn::Primary, Btn::Secondary].into_iter().enumerate() {
                if pressed[k] {
                    if grid.contains(pp) && !st.is_dragging() {
                        st.press(rel, btn, mods, cx.place_pattern);
                    } else if ruler.contains(pp) {
                        if btn == Btn::Primary {
                            res.out.extend(st.ruler_press(pp.x - grid.left(), mods));
                        } else {
                            res.out.extend(st.ruler_clear_loop());
                        }
                    }
                }
            }
            if down && st.is_dragging() {
                st.drag_to(rel, mods);
            }
            if (released[0] || released[1]) && st.is_dragging() {
                res.out.extend(st.release(rel, cx.project));
            }
            if resp.double_clicked()
                && grid.contains(pp)
                && let Some((id, _)) = st.clip_at(rel)
                && let Some(c) = st.clip(id)
            {
                res.out.push(PlOut::OpenPattern(c.pattern));
            }
        }
    }

    // ---- headers
    let hp = painter.with_clip_rect(headers);
    let first_track = st.y0.floor() as i32;
    for t in first_track..(first_track + visible_tracks).min(st.n_tracks as i32) {
        let y = grid.top() + st.y_of(t);
        let r = Rect::from_min_size(
            Pos2::new(headers.left(), y),
            Vec2::new(HEADER_W, st.track_h),
        );
        let tr = &cx.project.tracks[t as usize];
        hp.rect_filled(
            r.shrink2(Vec2::new(0.0, 0.5)),
            CornerRadius::ZERO,
            if tr.muted { col.row_alt } else { col.bg3 },
        );
        hp.text(
            r.left_center() + Vec2::new(8.0, 0.0),
            Align2::LEFT_CENTER,
            &tr.name,
            FontId::proportional(12.0),
            if tr.muted { col.dim } else { col.text },
        );
        let m = Rect::from_center_size(
            Pos2::new(r.right() - 16.0, r.center().y),
            Vec2::new(22.0, 18.0),
        );
        let id = resp.id.with(("mute", t));
        let mresp = ui.interact(m, id, Sense::click());
        hp.rect_filled(
            m,
            CornerRadius::same(3),
            if tr.muted {
                col.warn
            } else if mresp.hovered() {
                col.bg4
            } else {
                col.bg3
            },
        );
        hp.text(
            m.center(),
            Align2::CENTER_CENTER,
            "M",
            FontId::proportional(10.0),
            if tr.muted { col.accent_ink } else { col.dim },
        );
        if mresp.clicked() {
            res.mute_track = Some((t as u32, !tr.muted));
        }
        let nresp = ui.interact(
            Rect::from_min_max(r.min, Pos2::new(m.left() - 2.0, r.max.y)),
            id.with("name"),
            Sense::click(),
        );
        if nresp.double_clicked() {
            res.rename_track = Some(t as u32);
        }
    }

    // ---- grid
    let gp = painter.with_clip_rect(grid);
    for t in first_track..(first_track + visible_tracks).min(st.n_tracks as i32) {
        let y = grid.top() + st.y_of(t);
        let r = Rect::from_min_size(
            Pos2::new(grid.left(), y),
            Vec2::new(grid.width(), st.track_h),
        );
        let muted = cx.project.tracks[t as usize].muted;
        gp.rect_filled(
            r,
            CornerRadius::ZERO,
            if t % 2 == 0 {
                col.key_white
            } else {
                col.row_alt
            },
        );
        if muted {
            gp.rect_filled(r, CornerRadius::ZERO, col.bg0.gamma_multiply(0.5));
        }
        gp.line_segment(
            [r.left_bottom(), r.right_bottom()],
            Stroke::new(1.0, col.grid_minor),
        );
    }
    let bar = st.bar.max(1);
    let t0 = st.x0;
    let t1 = st.tick_at(grid.width());
    crate::piano_roll::paint_time_grid(&gp, grid, st.x0, st.px_per_tick, bar, cx.theme);

    // loop region
    let loop_now = match &st.drag {
        Drag::Loop { from, to } => Some((*from.min(to), *from.max(to))),
        _ => cx.loop_range,
    };
    if let Some((a, b)) = loop_now {
        let r = Rect::from_min_max(
            Pos2::new(grid.left() + st.x_of(a as f64), whole.top()),
            Pos2::new(grid.left() + st.x_of(b as f64), grid.bottom()),
        );
        gp.rect_filled(
            r.intersect(grid),
            CornerRadius::ZERO,
            col.accent.gamma_multiply(0.07),
        );
        painter.with_clip_rect(ruler).rect_filled(
            Rect::from_min_max(r.min, Pos2::new(r.max.x, ruler.bottom())),
            CornerRadius::ZERO,
            col.accent.gamma_multiply(0.45),
        );
    }

    // clips
    for c in &st.clips {
        let (track, start, len) = st.display_geom(c);
        let mut r = st
            .clip_rect(track, start, len)
            .translate(grid.min.to_vec2());
        if !r.intersects(grid) {
            continue;
        }
        r = r.shrink2(Vec2::new(0.0, 1.0));
        let (name, base) = cx
            .project
            .patterns
            .get(&c.pattern)
            .map_or(("?".to_string(), col.ghost), |p| {
                (p.name.clone(), rgb(p.color))
            });
        let sel = st.selection.contains(&c.id);
        gp.rect_filled(r, CornerRadius::same(3), lerp(col.bg2, base, 0.85));
        gp.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(r.width(), 14.0f32.min(r.height()))),
            CornerRadius {
                nw: 3,
                ne: 3,
                sw: 0,
                se: 0,
            },
            base,
        );
        gp.with_clip_rect(r.intersect(grid)).text(
            r.left_top() + Vec2::new(4.0, 1.0),
            Align2::LEFT_TOP,
            name,
            FontId::new(11.0, crate::fonts::body_medium()),
            col.accent_ink,
        );
        if sel {
            gp.rect_stroke(
                r,
                CornerRadius::same(3),
                Stroke::new(1.5, col.selection),
                StrokeKind::Inside,
            );
        }
        if cx.agent_selected.contains(&ObjectPath::Clip(c.id)) {
            gp.rect_stroke(
                r.expand(1.0),
                CornerRadius::same(3),
                Stroke::new(2.0, col.agent),
                StrokeKind::Outside,
            );
        }
        if let Some((who, at)) = cx.touches.get(&ObjectPath::Clip(c.id)) {
            let k = (1.0 - ((cx.now - at) / crate::piano_roll::FLASH_SECS) as f32).clamp(0.0, 1.0);
            if k > 0.0 {
                gp.rect_stroke(
                    r.expand(2.0),
                    CornerRadius::same(3),
                    Stroke::new(2.0, col.agent.gamma_multiply(k)),
                    StrokeKind::Outside,
                );
                gp.text(
                    r.right_top() + Vec2::new(-3.0, 1.0),
                    Align2::RIGHT_TOP,
                    who.trim_start_matches("agent:"),
                    FontId::new(9.0, crate::fonts::cond()),
                    col.accent_ink.gamma_multiply(k.max(0.4)),
                );
            }
        }
    }
    match &st.drag {
        Drag::Place {
            track,
            start,
            pattern,
        } => {
            let len = cx.project.pattern_length(*pattern).unwrap_or(3840);
            let r = st
                .clip_rect(*track as i32, *start, len)
                .translate(grid.min.to_vec2());
            gp.rect_filled(
                r.shrink2(Vec2::new(0.0, 1.0)),
                CornerRadius::same(3),
                col.accent.gamma_multiply(0.35),
            );
            gp.rect_stroke(
                r.shrink2(Vec2::new(0.0, 1.0)),
                CornerRadius::same(3),
                Stroke::new(1.0, col.accent),
                StrokeKind::Inside,
            );
        }
        Drag::Box { from, to } => {
            let r = Rect::from_two_pos(grid.min + *from, grid.min + *to);
            gp.rect_filled(r, CornerRadius::ZERO, col.accent.gamma_multiply(0.15));
            gp.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.0, col.accent),
                StrokeKind::Inside,
            );
        }
        Drag::Erase { ids } => {
            for c in st.clips.iter().filter(|c| ids.contains(&c.id)) {
                let r = st
                    .clip_rect(c.track as i32, c.start, c.length)
                    .translate(grid.min.to_vec2());
                gp.rect_filled(r, CornerRadius::same(3), col.error.gamma_multiply(0.5));
            }
        }
        _ => {}
    }

    // ruler
    let rp = painter.with_clip_rect(ruler);
    rp.rect_filled(
        Rect::from_min_max(ruler.min, Pos2::new(ruler.max.x, ruler.max.y)),
        CornerRadius::ZERO,
        col.bg3,
    );
    if let Some((a, b)) = loop_now {
        rp.rect_filled(
            Rect::from_min_max(
                Pos2::new(grid.left() + st.x_of(a as f64), ruler.bottom() - 6.0),
                Pos2::new(grid.left() + st.x_of(b as f64), ruler.bottom()),
            ),
            CornerRadius::ZERO,
            col.accent,
        );
    }
    let mut bar_step = 1u64;
    while (bar as f64 * bar_step as f64 * st.px_per_tick) < 36.0 {
        bar_step *= 2;
    }
    let first_bar = (t0 / bar as f64).floor().max(0.0) as u64;
    let mut b = first_bar - first_bar % bar_step;
    while (b * bar) as f64 <= t1 {
        let x = grid.left() + st.x_of((b * bar) as f64);
        rp.line_segment(
            [
                Pos2::new(x, ruler.top() + 12.0),
                Pos2::new(x, ruler.bottom()),
            ],
            Stroke::new(1.0, col.grid_bar),
        );
        rp.text(
            Pos2::new(x + 3.0, ruler.top() + 2.0),
            Align2::LEFT_TOP,
            format!("{}", b + 1),
            FontId::new(10.0, crate::fonts::mono()),
            col.dim,
        );
        b += bar_step;
    }
    // playhead
    let x = grid.left() + st.x_of(cx.position as f64);
    if x >= grid.left() && x <= grid.right() {
        painter.line_segment(
            [Pos2::new(x, whole.top()), Pos2::new(x, grid.bottom())],
            Stroke::new(1.5, col.playhead),
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, NullHost, Registry, Session};
    use serde_json::json;
    use std::sync::Arc;

    fn setup() -> (PlaylistState, Project, PatternId) {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("t"));
        let mut h = NullHost::default();
        let who = Caller::human();
        let mut run =
            |c: &str, p: serde_json::Value| s.execute(&who, Request::new(c, p), &mut h).unwrap();
        let pat = run("pattern.add", json!({})).result["pattern"]
            .as_u64()
            .unwrap() as u32;
        run(
            "playlist.add_clip",
            json!({"track": 1, "pattern": pat, "start": 3840, "length": 3840}),
        );
        run(
            "playlist.add_clip",
            json!({"track": 2, "pattern": pat, "start": 0, "length": 3840}),
        );
        let p = s.project().as_ref().clone();
        let mut st = PlaylistState::default();
        st.sync(&p);
        (st, p, PatternId(pat))
    }

    fn centre_of(st: &PlaylistState, c: &Clip) -> Vec2 {
        let r = st.clip_rect(c.track as i32, c.start, c.length);
        Vec2::new(r.center().x, r.center().y)
    }

    #[test]
    fn clip_hit_testing() {
        let (st, ..) = setup();
        let c = st.clips[0].clone();
        assert_eq!(st.clip_at(centre_of(&st, &c)), Some((c.id, Zone::Body)));
        let r = st.clip_rect(c.track as i32, c.start, c.length);
        assert_eq!(
            st.clip_at(Vec2::new(r.right() - 1.0, r.center().y))
                .unwrap()
                .1,
            Zone::ResizeRight
        );
        assert_eq!(st.clip_at(Vec2::new(r.center().x, r.bottom() + 3.0)), None);
    }

    #[test]
    fn click_empty_places_the_pattern_on_the_snapped_bar() {
        let (mut st, p, pat) = setup();
        let pos = Vec2::new(st.x_of(3840.0 * 2.0 + 700.0), st.y_of(5) + 4.0);
        st.press(pos, Btn::Primary, Mods::default(), Some(pat));
        let out = st.release(pos, &p);
        let [PlOut::Cmd(r)] = &out[..] else {
            panic!("{out:?}")
        };
        assert_eq!(r.command, "playlist.add_clip");
        assert_eq!(r.params["track"], 5);
        assert_eq!(r.params["start"], 7680);
    }

    #[test]
    fn dragging_a_clip_moves_it_by_bars_and_tracks_in_one_batch() {
        let (mut st, p, _) = setup();
        let c = st.clips[0].clone(); // track 1, bar 2
        let from = centre_of(&st, &c);
        st.press(from, Btn::Primary, Mods::default(), None);
        let to = from + Vec2::new(st.px_per_tick as f32 * 3840.0 * 2.0, st.track_h);
        st.drag_to(to, Mods::default());
        assert_eq!(st.display_geom(&c), (2, 3840 * 3, 3840));
        let out = st.release(to, &p);
        let [PlOut::Batch(reqs)] = &out[..] else {
            panic!()
        };
        assert_eq!(reqs[0].command, "playlist.move_clip");
        assert_eq!(reqs[0].params["start"], 3840 * 3);
        assert_eq!(reqs[0].params["track"], 2);
    }

    #[test]
    fn resize_snaps_to_bars() {
        let (mut st, p, _) = setup();
        let c = st.clips[0].clone();
        let r = st.clip_rect(c.track as i32, c.start, c.length);
        let edge = Vec2::new(r.right() - 1.0, r.center().y);
        st.press(edge, Btn::Primary, Mods::default(), None);
        let to = edge + Vec2::new(st.px_per_tick as f32 * (3840.0 + 300.0), 0.0);
        st.drag_to(to, Mods::default());
        let out = st.release(to, &p);
        let [PlOut::Batch(reqs)] = &out[..] else {
            panic!()
        };
        assert_eq!(reqs[0].command, "playlist.resize_clip");
        assert_eq!(reqs[0].params["length"], 7680);
    }

    #[test]
    fn right_click_deletes_and_duplicate_copies_after() {
        let (mut st, p, _) = setup();
        let c = st.clips[0].clone();
        st.press(centre_of(&st, &c), Btn::Secondary, Mods::default(), None);
        let out = st.release(centre_of(&st, &c), &p);
        let [PlOut::Batch(reqs)] = &out[..] else {
            panic!()
        };
        assert_eq!(reqs[0].command, "playlist.remove_clip");
        st.select_all();
        let [PlOut::Batch(dups)] = &st.duplicate()[..] else {
            panic!()
        };
        assert_eq!(dups.len(), 2);
        assert_eq!(dups[0].params["start"], c.start + c.length);
    }

    #[test]
    fn ruler_click_seeks_and_shift_drag_loops() {
        let (mut st, p, _) = setup();
        let x = st.x_of(3840.0 + 100.0);
        assert_eq!(st.ruler_press(x, Mods::default()), vec![PlOut::Seek(3840)]);
        assert!(
            st.ruler_press(
                st.x_of(3840.0),
                Mods {
                    shift: true,
                    ..Default::default()
                }
            )
            .is_empty()
        );
        st.drag_to(Vec2::new(st.x_of(3840.0 * 3.0), 0.0), Mods::default());
        let out = st.release(Vec2::ZERO, &p);
        let [PlOut::Cmd(r)] = &out[..] else { panic!() };
        assert_eq!(r.command, "transport.set_loop");
        assert_eq!(r.params["range"], json!([3840, 11520]));
    }
}
