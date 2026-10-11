//! Piano roll: view model (coordinates, hit-testing, snapping, drag state machine) and the
//! egui widget that draws it.
//!
//! The model half has no egui painting in it and is unit-tested without a window: pointer
//! events go in, [`PrOut`] requests come out. The widget half draws only the notes in view
//! (binary search over notes sorted by start), so 10 000 notes stay cheap.

use crate::mapping::{self, NewNote};
use crate::theme::{Theme, lerp, rgb};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2,
};
use ph_commands::Request;
use ph_core::{ChannelId, Note, NoteId, PatternId, Project, STEP, Tick};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub const KEYBOARD_W: f32 = 64.0;
pub const RULER_H: f32 = 20.0;
pub const VEL_LANE_H: f32 = 64.0;

/// Snap choices: label and grid in ticks (`0` = per-bar, resolved from the time signature).
pub const SNAPS: &[(&str, Tick)] = &[
    ("Off", 1),
    ("Bar", 0),
    ("1/2 bar", 1920),
    ("Beat", 960),
    ("1/2 beat", 480),
    ("Step", 240),
    ("1/2 step", 120),
    ("1/4 step", 60),
    ("Triplet beat", 640),
    ("Triplet step", 160),
];

/// Nearest grid line. A grid of 0 or 1 tick means "off".
pub fn snap_tick(t: f64, grid: Tick) -> Tick {
    let t = t.max(0.0);
    if grid <= 1 {
        return t.round() as Tick;
    }
    ((t / grid as f64).round() as Tick) * grid
}

/// Grid line at or before `t` (used to place new notes where the cursor is).
pub fn snap_floor(t: f64, grid: Tick) -> Tick {
    let t = t.max(0.0);
    if grid <= 1 {
        return t.floor() as Tick;
    }
    ((t / grid as f64).floor() as Tick) * grid
}

/// Quantize one start to the grid, pulled `strength` of the way (what `note.quantize` does).
pub fn quantize_tick(t: Tick, grid: Tick, strength: f32) -> Tick {
    let target = snap_tick(t as f64, grid) as f64;
    (t as f64 + (target - t as f64) * strength as f64).round() as Tick
}

/// FL-style note name: MIDI 60 is C5.
pub fn note_name(key: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(key % 12) as usize], key / 12)
}

pub fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

/// What the viewport shows. All coordinates are relative to the grid's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Tick at the left edge.
    pub x0: f64,
    pub px_per_tick: f64,
    /// Key (fractional) at the top edge: the top row shows the key below this value.
    pub y0: f64,
    pub key_h: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            x0: 0.0,
            px_per_tick: 0.12,
            y0: 84.0,
            key_h: 16.0,
        }
    }
}

impl View {
    pub fn x_of(&self, tick: f64) -> f32 {
        ((tick - self.x0) * self.px_per_tick) as f32
    }
    pub fn tick_at(&self, x: f32) -> f64 {
        self.x0 + x as f64 / self.px_per_tick
    }
    /// Top edge of a key's row.
    pub fn y_of(&self, key: u8) -> f32 {
        ((self.y0 - (key as f64 + 1.0)) * self.key_h as f64) as f32
    }
    pub fn key_at(&self, y: f32) -> i32 {
        (self.y0 - y as f64 / self.key_h as f64).floor() as i32
    }
    pub fn note_rect(&self, key: u8, start: Tick, len: Tick) -> Rect {
        let x = self.x_of(start as f64);
        Rect::from_min_size(
            Pos2::new(x, self.y_of(key)),
            Vec2::new((len as f64 * self.px_per_tick) as f32, self.key_h),
        )
    }
    /// Zoom horizontally keeping the tick under `anchor_x` fixed.
    pub fn zoom_x(&mut self, factor: f64, anchor_x: f32) {
        let t = self.tick_at(anchor_x);
        self.px_per_tick = (self.px_per_tick * factor).clamp(0.005, 2.0);
        self.x0 = (t - anchor_x as f64 / self.px_per_tick).max(0.0);
    }
    pub fn zoom_y(&mut self, factor: f32, anchor_y: f32) {
        let k = self.y0 - anchor_y as f64 / self.key_h as f64;
        self.key_h = (self.key_h * factor).clamp(6.0, 48.0);
        self.y0 = k + anchor_y as f64 / self.key_h as f64;
        self.clamp_y(0.0);
    }
    pub fn clamp_y(&mut self, grid_h: f32) {
        let rows = (grid_h / self.key_h) as f64;
        self.y0 = self.y0.clamp(rows.min(128.0), 128.0);
    }
}

/// The notes of one channel in one pattern, sorted by start, for culling and hit-testing.
#[derive(Default)]
pub struct NoteCache {
    src: Option<Arc<ph_core::Pattern>>,
    channel: Option<ChannelId>,
    pub notes: Vec<Note>,
    pub max_len: Tick,
    index: HashMap<NoteId, usize>,
}

impl NoteCache {
    /// Rebuilds only when the pattern's `Arc` or the channel changed.
    pub fn refresh(&mut self, project: &Project, pattern: PatternId, channel: ChannelId) {
        let Some(arc) = project.patterns.get(&pattern) else {
            *self = Self::default();
            return;
        };
        if self.channel == Some(channel) && self.src.as_ref().is_some_and(|s| Arc::ptr_eq(s, arc)) {
            return;
        }
        self.src = Some(arc.clone());
        self.channel = Some(channel);
        self.notes = arc
            .notes
            .get(&channel)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        self.notes.sort_by_key(|n| (n.start, n.key, n.id));
        self.max_len = self.notes.iter().map(|n| n.length).max().unwrap_or(0);
        self.index = self
            .notes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
    }

    pub fn get(&self, id: NoteId) -> Option<&Note> {
        self.index.get(&id).map(|&i| &self.notes[i])
    }

    /// Index range of notes that could overlap `[t0, t1)`.
    pub fn visible_range(&self, t0: f64, t1: f64) -> std::ops::Range<usize> {
        let lo_start = (t0 - self.max_len as f64).max(0.0) as Tick;
        let lo = self.notes.partition_point(|n| n.start < lo_start);
        let hi = self.notes.partition_point(|n| (n.start as f64) < t1);
        lo..hi.max(lo)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    Draw,
    Select,
    Delete,
    /// Click a note to split it at the (snapped) click position.
    Slice,
    /// Click or drag over notes to toggle whether they are muted.
    Mute,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Zone {
    Body,
    ResizeRight,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Btn {
    Primary,
    Secondary,
    Middle,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What the piano roll asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum PrOut {
    /// One command, one undo step.
    Cmd(Request),
    /// Several commands that are one undo step.
    Batch(Vec<Request>),
    /// Preview a note through the engine.
    Preview { key: u8, on: bool },
    /// Seek the transport (click in the ruler).
    Seek(Tick),
    /// After the next `note.add`/`note.copy` reply, select the notes it created.
    SelectCreated,
}

/// Keyboard actions the app routes to the focused piano roll (from the keymap).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PrAction {
    Duplicate,
    Quantize,
    TransposeUp,
    TransposeDown,
    OctaveUp,
    OctaveDown,
    SelectAll,
    DeleteSelected,
    Copy,
    Cut,
    Paste,
    SetTool(Tool),
}

#[derive(Clone, Debug)]
enum Drag {
    None,
    /// Moving the selection (or drawing a new note when `ids` is empty).
    Move {
        ids: Vec<NoteId>,
        grab_tick: f64,
        grab_key: i32,
        grab_start: Tick,
        dt: i64,
        dk: i32,
    },
    /// A note being drawn: placed at release.
    NewNote {
        start: Tick,
        key: u8,
        len: Tick,
        moved: bool,
    },
    Resize {
        grabbed: NoteId,
        base_len: Tick,
        grab_end: f64,
        delta: i64,
    },
    BoxSelect {
        from: Vec2,
        to: Vec2,
        additive: bool,
    },
    /// Alt+drag: a freehand loop; the notes inside it are selected on release.
    Lasso {
        points: Vec<Vec2>,
        additive: bool,
    },
    /// Mute tool: notes touched are all set to `to` (decided by the first note touched).
    Mute {
        ids: BTreeSet<NoteId>,
        to: Option<bool>,
    },
    /// Right-drag (or Delete tool) collecting notes to delete.
    Erase {
        ids: BTreeSet<NoteId>,
    },
    Velocity {
        ids: Vec<NoteId>,
        value: f32,
    },
    Pan {
        last: Vec2,
    },
}

/// Even-odd point-in-polygon test; the polygon is closed implicitly.
fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Per-roll state: view, tool, selection and the in-flight drag.
pub struct PianoRollState {
    pub view: View,
    pub tool: Tool,
    /// Snap grid in ticks; see [`SNAPS`].
    pub snap: Tick,
    pub selection: BTreeSet<NoteId>,
    pub last_len: Tick,
    pub ghosts: bool,
    pub pattern: Option<PatternId>,
    pub channel: Option<ChannelId>,
    pub clipboard: Vec<NewNote>,
    /// Notes a command just created; selected as soon as they show up in a snapshot.
    pub want: BTreeSet<NoteId>,
    pub cache: NoteCache,
    drag: Drag,
    ticks_per_bar: Tick,
    /// Pattern length in ticks, for drawing the end marker.
    pub pattern_len: Tick,
    lane_h: f32,
    needs_fit: bool,
    /// Where the note grid was drawn last frame (screen coordinates), for tests.
    pub last_grid: Rect,
}

impl Default for PianoRollState {
    fn default() -> Self {
        Self {
            view: View::default(),
            tool: Tool::Draw,
            snap: STEP,
            selection: BTreeSet::new(),
            last_len: STEP,
            ghosts: true,
            pattern: None,
            channel: None,
            clipboard: Vec::new(),
            want: BTreeSet::new(),
            cache: NoteCache::default(),
            drag: Drag::None,
            ticks_per_bar: 3840,
            pattern_len: 3840,
            lane_h: VEL_LANE_H,
            needs_fit: true,
            last_grid: Rect::NOTHING,
        }
    }
}

impl PianoRollState {
    /// Points the roll at a pattern and channel (clears the selection if it changed).
    pub fn focus(&mut self, pattern: PatternId, channel: ChannelId) {
        if self.pattern != Some(pattern) || self.channel != Some(channel) {
            self.selection.clear();
            self.drag = Drag::None;
            self.needs_fit = true;
        }
        self.pattern = Some(pattern);
        self.channel = Some(channel);
    }

    /// Refreshes caches from a new snapshot and drops selected ids that no longer exist.
    pub fn sync(&mut self, project: &Project) {
        let (Some(p), Some(c)) = (self.pattern, self.channel) else {
            return;
        };
        self.cache.refresh(project, p, c);
        self.ticks_per_bar = project.settings.time_signature.ticks_per_bar();
        self.pattern_len = project.pattern_length(p).unwrap_or(self.ticks_per_bar);
        let cache = &self.cache;
        self.selection.retain(|id| cache.get(*id).is_some());
        if !self.want.is_empty() {
            let found: Vec<NoteId> = self
                .want
                .iter()
                .copied()
                .filter(|i| cache.get(*i).is_some())
                .collect();
            if !found.is_empty() {
                self.selection.clear();
                for i in found {
                    self.want.remove(&i);
                    self.selection.insert(i);
                }
                self.want.clear();
            }
        }
    }

    /// Frames the pattern: whole length across the grid, notes centred vertically.
    pub fn fit(&mut self, grid: Vec2) {
        let len = self.pattern_len.max(self.ticks_per_bar).max(1) as f64;
        self.view.px_per_tick = ((grid.x as f64 - 24.0) / len).clamp(0.01, 0.5);
        self.view.x0 = 0.0;
        let (lo, hi) = self
            .cache
            .notes
            .iter()
            .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
        let centre = if self.cache.notes.is_empty() {
            60.0
        } else {
            (lo as f64 + hi as f64) / 2.0
        };
        let rows = (grid.y / self.view.key_h) as f64;
        self.view.y0 = centre + rows / 2.0 + 0.5;
        self.view.clamp_y(grid.y);
        self.needs_fit = false;
    }

    /// The effective grid in ticks.
    pub fn grid(&self) -> Tick {
        if self.snap == 0 {
            self.ticks_per_bar
        } else {
            self.snap
        }
    }

    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// Note under a grid-relative point, topmost (latest in time) first.
    pub fn note_at(&self, pos: Vec2) -> Option<(NoteId, Zone)> {
        let t = self.view.tick_at(pos.x);
        let range = self.cache.visible_range(t - 1.0, t + 1.0);
        for n in self.cache.notes[range].iter().rev() {
            let r = self.view.note_rect(n.key, n.start, n.length);
            let p = Pos2::new(pos.x, pos.y);
            if r.contains(p) {
                let edge = (r.width() * 0.3).clamp(2.0, 7.0);
                let zone = if pos.x > r.right() - edge {
                    Zone::ResizeRight
                } else {
                    Zone::Body
                };
                return Some((n.id, zone));
            }
        }
        None
    }

    /// Notes whose rectangle intersects the box (grid-relative corners).
    pub fn notes_in_box(&self, a: Vec2, b: Vec2) -> Vec<NoteId> {
        let r = Rect::from_two_pos(Pos2::new(a.x, a.y), Pos2::new(b.x, b.y));
        let range = self
            .cache
            .visible_range(self.view.tick_at(r.left()), self.view.tick_at(r.right()));
        self.cache.notes[range]
            .iter()
            .filter(|n| r.intersects(self.view.note_rect(n.key, n.start, n.length)))
            .map(|n| n.id)
            .collect()
    }

    /// Notes whose centre lies inside the loop (grid-relative points, closed automatically).
    pub fn notes_in_lasso(&self, points: &[Vec2]) -> Vec<NoteId> {
        if points.len() < 3 {
            return Vec::new();
        }
        let (mut lo, mut hi) = (points[0], points[0]);
        for p in points {
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let range = self
            .cache
            .visible_range(self.view.tick_at(lo.x), self.view.tick_at(hi.x));
        self.cache.notes[range]
            .iter()
            .filter(|n| {
                let c = self.view.note_rect(n.key, n.start, n.length).center();
                point_in_polygon(Vec2::new(c.x, c.y), points)
            })
            .map(|n| n.id)
            .collect()
    }

    /// The Slice tool: split the clicked note (and the other selected notes the cut passes
    /// through, when the clicked one is selected) at the snapped click position, as one
    /// undo step.
    fn slice(&mut self, id: NoteId, pos: Vec2, fine: bool) -> Option<PrOut> {
        let (p, c) = self.pc()?;
        let cut = snap_tick(self.view.tick_at(pos.x), if fine { 1 } else { self.grid() });
        let targets: Vec<NoteId> = if self.selection.contains(&id) {
            self.selection.iter().copied().collect()
        } else {
            vec![id]
        };
        let mut reqs = Vec::new();
        for t in targets {
            if let Some(n) = self.cache.get(t)
                && cut > n.start
                && cut < n.start + n.length
            {
                reqs.extend(mapping::note_slice(p, c, n, cut));
            }
        }
        (!reqs.is_empty()).then_some(PrOut::Batch(reqs))
    }

    fn pc(&self) -> Option<(PatternId, ChannelId)> {
        Some((self.pattern?, self.channel?))
    }

    fn snap_delta(&self, grab_start: Tick, raw_dt: f64, alt: bool) -> i64 {
        let grid = if alt { 1 } else { self.grid() };
        let target = snap_tick((grab_start as f64 + raw_dt).max(0.0), grid) as i64;
        target - grab_start as i64
    }

    /// Feeds one pointer event (positions relative to the grid's top-left) to the roll.
    pub fn pointer_press(&mut self, pos: Vec2, btn: Btn, mods: Mods) -> Vec<PrOut> {
        let mut out = Vec::new();
        let hit = self.note_at(pos);
        match (btn, self.tool) {
            (Btn::Middle, _) => self.drag = Drag::Pan { last: pos },
            (Btn::Secondary, _) | (Btn::Primary, Tool::Delete) => {
                let mut ids = BTreeSet::new();
                if let Some((id, _)) = hit {
                    ids.insert(id);
                }
                self.drag = Drag::Erase { ids };
            }
            (Btn::Primary, Tool::Slice) => {
                if let Some((id, _)) = hit {
                    out.extend(self.slice(id, pos, mods.alt));
                }
            }
            (Btn::Primary, Tool::Mute) => {
                let mut ids = BTreeSet::new();
                let mut to = None;
                if let Some((id, _)) = hit {
                    ids.insert(id);
                    to = self.cache.get(id).map(|n| !n.muted);
                }
                self.drag = Drag::Mute { ids, to };
            }
            (Btn::Primary, tool) => match hit {
                Some((id, zone)) => {
                    let n = self.cache.get(id).cloned().expect("hit note is cached");
                    if !self.selection.contains(&id) {
                        if !mods.shift {
                            self.selection.clear();
                        }
                        self.selection.insert(id);
                    } else if mods.shift {
                        self.selection.remove(&id);
                    }
                    out.push(PrOut::Preview {
                        key: n.key,
                        on: true,
                    });
                    if zone == Zone::ResizeRight {
                        self.drag = Drag::Resize {
                            grabbed: id,
                            base_len: n.length,
                            grab_end: self.view.tick_at(pos.x),
                            delta: 0,
                        };
                    } else {
                        self.drag = Drag::Move {
                            ids: self.selection.iter().copied().collect(),
                            grab_tick: self.view.tick_at(pos.x),
                            grab_key: self.view.key_at(pos.y),
                            grab_start: n.start,
                            dt: 0,
                            dk: 0,
                        };
                    }
                }
                None => {
                    if mods.alt {
                        self.drag = Drag::Lasso {
                            points: vec![pos],
                            additive: mods.shift,
                        };
                    } else if tool == Tool::Select || mods.ctrl {
                        self.drag = Drag::BoxSelect {
                            from: pos,
                            to: pos,
                            additive: mods.shift,
                        };
                    } else {
                        let key = self.view.key_at(pos.y).clamp(0, 127) as u8;
                        let start = snap_floor(
                            self.view.tick_at(pos.x),
                            if mods.alt { 1 } else { self.grid() },
                        );
                        self.selection.clear();
                        self.drag = Drag::NewNote {
                            start,
                            key,
                            len: self.last_len.max(1),
                            moved: false,
                        };
                        out.push(PrOut::Preview { key, on: true });
                    }
                }
            },
        }
        out
    }

    pub fn pointer_move(&mut self, pos: Vec2, mods: Mods) -> Vec<PrOut> {
        let mut out = Vec::new();
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.drag = match drag {
            Drag::Move {
                ids,
                grab_tick,
                grab_key,
                grab_start,
                ..
            } => {
                let raw = self.view.tick_at(pos.x) - grab_tick;
                let dt = self.snap_delta(grab_start, raw, mods.alt);
                let dk = self.view.key_at(pos.y) - grab_key;
                Drag::Move {
                    ids,
                    grab_tick,
                    grab_key,
                    grab_start,
                    dt,
                    dk,
                }
            }
            Drag::NewNote {
                start,
                key,
                len,
                moved,
            } => {
                let _ = (start, key, moved);
                let nk = self.view.key_at(pos.y).clamp(0, 127) as u8;
                let ns = snap_floor(
                    self.view.tick_at(pos.x),
                    if mods.alt { 1 } else { self.grid() },
                );
                if nk != key {
                    out.push(PrOut::Preview { key, on: false });
                    out.push(PrOut::Preview { key: nk, on: true });
                }
                Drag::NewNote {
                    start: ns,
                    key: nk,
                    len,
                    moved: true,
                }
            }
            Drag::Resize {
                grabbed,
                base_len,
                grab_end,
                ..
            } => {
                let raw = self.view.tick_at(pos.x) - grab_end;
                let grid = if mods.alt { 1 } else { self.grid() };
                let start = self.cache.get(grabbed).map(|n| n.start).unwrap_or(0);
                let new_end = snap_tick((start + base_len) as f64 + raw, grid) as i64;
                let delta = (new_end - (start + base_len) as i64).max(1 - base_len as i64);
                Drag::Resize {
                    grabbed,
                    base_len,
                    grab_end,
                    delta,
                }
            }
            Drag::BoxSelect { from, additive, .. } => Drag::BoxSelect {
                from,
                to: pos,
                additive,
            },
            Drag::Lasso {
                mut points,
                additive,
            } => {
                if points.last().is_none_or(|l| (*l - pos).length() >= 3.0) {
                    points.push(pos);
                }
                Drag::Lasso { points, additive }
            }
            Drag::Mute { mut ids, mut to } => {
                if let Some((id, _)) = self.note_at(pos) {
                    if to.is_none() {
                        to = self.cache.get(id).map(|n| !n.muted);
                    }
                    ids.insert(id);
                }
                Drag::Mute { ids, to }
            }
            Drag::Erase { mut ids } => {
                if let Some((id, _)) = self.note_at(pos) {
                    ids.insert(id);
                }
                Drag::Erase { ids }
            }
            Drag::Velocity { ids, .. } => Drag::Velocity {
                ids,
                value: self.vel_from_y(pos.y),
            },
            Drag::Pan { last } => {
                self.view.x0 =
                    (self.view.x0 - (pos.x - last.x) as f64 / self.view.px_per_tick).max(0.0);
                self.view.y0 += (pos.y - last.y) as f64 / self.view.key_h as f64;
                self.view.clamp_y(0.0);
                Drag::Pan { last: pos }
            }
            Drag::None => Drag::None,
        };
        out
    }

    pub fn pointer_release(&mut self, pos: Vec2, _mods: Mods) -> Vec<PrOut> {
        let mut out = Vec::new();
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let Some((p, c)) = self.pc() else {
            return out;
        };
        match drag {
            Drag::Move { ids, dt, dk, .. } => {
                if let Some(first) = ids.first().and_then(|i| self.cache.get(*i)) {
                    out.push(PrOut::Preview {
                        key: first.key,
                        on: false,
                    });
                }
                if dt != 0 || dk != 0 {
                    out.push(PrOut::Cmd(mapping::note_move(p, c, &ids, dt, dk)));
                }
            }
            Drag::NewNote {
                start, key, len, ..
            } => {
                out.push(PrOut::Preview { key, on: false });
                out.push(PrOut::Cmd(mapping::note_add(
                    p,
                    c,
                    &[NewNote {
                        key,
                        start,
                        length: len,
                        velocity: 0.78,
                        muted: false,
                    }],
                )));
                out.push(PrOut::SelectCreated);
            }
            Drag::Resize {
                grabbed,
                base_len,
                delta,
                ..
            } => {
                if let Some(n) = self.cache.get(grabbed) {
                    out.push(PrOut::Preview {
                        key: n.key,
                        on: false,
                    });
                }
                self.last_len = (base_len as i64 + delta).max(1) as Tick;
                if delta != 0 {
                    let ids: Vec<NoteId> = if self.selection.contains(&grabbed) {
                        self.selection.iter().copied().collect()
                    } else {
                        vec![grabbed]
                    };
                    out.push(PrOut::Cmd(mapping::note_resize_delta(p, c, &ids, delta)));
                }
            }
            Drag::BoxSelect { from, additive, .. } => {
                let ids = self.notes_in_box(from, pos);
                if !additive {
                    self.selection.clear();
                }
                self.selection.extend(ids);
            }
            Drag::Lasso {
                mut points,
                additive,
            } => {
                points.push(pos);
                let ids = self.notes_in_lasso(&points);
                if !additive {
                    self.selection.clear();
                }
                self.selection.extend(ids);
            }
            Drag::Mute { ids, to } => {
                if let Some(to) = to {
                    // all the notes touched go the same way, whichever way each started
                    let ids: Vec<NoteId> = ids
                        .into_iter()
                        .filter(|i| self.cache.get(*i).is_some_and(|n| n.muted != to))
                        .collect();
                    if !ids.is_empty() {
                        out.push(PrOut::Cmd(mapping::note_mute(p, c, &ids, to)));
                    }
                }
            }
            Drag::Erase { ids } => {
                if !ids.is_empty() {
                    let ids: Vec<NoteId> = ids.into_iter().collect();
                    for i in &ids {
                        self.selection.remove(i);
                    }
                    out.push(PrOut::Cmd(mapping::note_remove(p, c, &ids)));
                }
            }
            Drag::Velocity { ids, value } => {
                out.push(PrOut::Cmd(mapping::note_velocity(p, c, &ids, value)));
            }
            Drag::Pan { .. } | Drag::None => {}
        }
        out
    }

    /// Velocity lane: press near a note's bar to start dragging its velocity.
    pub fn velocity_press(&mut self, lane_pos: Vec2, lane_h: f32) -> bool {
        let t = self.view.tick_at(lane_pos.x);
        let tol = 5.0 / self.view.px_per_tick;
        let range = self.cache.visible_range(t - tol, t + tol);
        let best = self.cache.notes[range]
            .iter()
            .filter(|n| (n.start as f64 - t).abs() <= tol)
            .min_by(|a, b| {
                (a.start as f64 - t)
                    .abs()
                    .total_cmp(&(b.start as f64 - t).abs())
            });
        let Some(n) = best else {
            return false;
        };
        let ids = if self.selection.contains(&n.id) {
            self.selection.iter().copied().collect()
        } else {
            vec![n.id]
        };
        self.lane_h = lane_h;
        self.drag = Drag::Velocity {
            ids,
            value: (1.0 - lane_pos.y / lane_h).clamp(0.0, 1.0),
        };
        true
    }

    fn vel_from_y(&self, y_in_lane: f32) -> f32 {
        (1.0 - y_in_lane / self.lane_h.max(1.0)).clamp(0.0, 1.0)
    }

    /// Keyboard actions (from the keymap) routed to this roll.
    pub fn action(&mut self, a: PrAction) -> Vec<PrOut> {
        let mut out = Vec::new();
        let Some((p, c)) = self.pc() else {
            return out;
        };
        let ids: Vec<NoteId> = self.selection.iter().copied().collect();
        match a {
            PrAction::SetTool(t) => self.tool = t,
            PrAction::SelectAll => self.selection = self.cache.notes.iter().map(|n| n.id).collect(),
            PrAction::DeleteSelected if !ids.is_empty() => {
                out.push(PrOut::Cmd(mapping::note_remove(p, c, &ids)));
                self.selection.clear();
            }
            PrAction::Duplicate if !ids.is_empty() => {
                let (lo, hi) = self.selection_span();
                let grid = self.grid().max(1);
                let span = (hi - lo).div_ceil(grid).max(1) * grid;
                out.push(PrOut::Cmd(mapping::note_duplicate(p, c, &ids, span as i64)));
                out.push(PrOut::SelectCreated);
            }
            PrAction::Quantize => {
                let ids = if ids.is_empty() { Vec::new() } else { ids };
                out.push(PrOut::Cmd(if ids.is_empty() {
                    // whole channel: omit ids
                    let mut r = mapping::note_quantize(p, c, &[], self.grid());
                    r.params.as_object_mut().map(|o| o.remove("ids"));
                    r
                } else {
                    mapping::note_quantize(p, c, &ids, self.grid())
                }));
            }
            PrAction::TransposeUp
            | PrAction::TransposeDown
            | PrAction::OctaveUp
            | PrAction::OctaveDown
                if !ids.is_empty() =>
            {
                let st = match a {
                    PrAction::TransposeUp => 1,
                    PrAction::TransposeDown => -1,
                    PrAction::OctaveUp => 12,
                    _ => -12,
                };
                out.push(PrOut::Cmd(mapping::note_transpose(p, c, &ids, st)));
            }
            PrAction::Copy | PrAction::Cut if !ids.is_empty() => {
                let (lo, _) = self.selection_span();
                self.clipboard = ids
                    .iter()
                    .filter_map(|i| self.cache.get(*i))
                    .map(|n| NewNote {
                        key: n.key,
                        start: n.start - lo,
                        length: n.length,
                        velocity: n.velocity,
                        muted: n.muted,
                    })
                    .collect();
                if a == PrAction::Cut {
                    out.push(PrOut::Cmd(mapping::note_remove(p, c, &ids)));
                    self.selection.clear();
                }
            }
            _ => {}
        }
        out
    }

    /// Pastes the clipboard at `at` ticks (snapped).
    pub fn paste(&mut self, at: Tick) -> Vec<PrOut> {
        let Some((p, c)) = self.pc() else {
            return Vec::new();
        };
        if self.clipboard.is_empty() {
            return Vec::new();
        }
        let base = snap_floor(at as f64, self.grid());
        let notes: Vec<NewNote> = self
            .clipboard
            .iter()
            .map(|n| NewNote {
                start: n.start + base,
                ..*n
            })
            .collect();
        vec![
            PrOut::Cmd(mapping::note_add(p, c, &notes)),
            PrOut::SelectCreated,
        ]
    }

    fn selection_span(&self) -> (Tick, Tick) {
        let mut lo = Tick::MAX;
        let mut hi = 0;
        for n in self.selection.iter().filter_map(|i| self.cache.get(*i)) {
            lo = lo.min(n.start);
            hi = hi.max(n.start + n.length);
        }
        if lo == Tick::MAX { (0, 0) } else { (lo, hi) }
    }

    /// Display geometry of a note with any in-flight drag applied: (key, start, length).
    pub fn display_geom(&self, n: &Note) -> (u8, Tick, Tick) {
        match &self.drag {
            Drag::Move { ids, dt, dk, .. } if ids.contains(&n.id) => (
                (n.key as i32 + dk).clamp(0, 127) as u8,
                (n.start as i64 + dt).max(0) as Tick,
                n.length,
            ),
            Drag::Resize { grabbed, delta, .. }
                if *grabbed == n.id || self.selection.contains(&n.id) =>
            {
                (n.key, n.start, (n.length as i64 + delta).max(1) as Tick)
            }
            _ => (n.key, n.start, n.length),
        }
    }
}

// ---- drawing -------------------------------------------------------------------------------

/// Things an agent just touched, so the roll can flash them and badge the author.
#[derive(Clone, Debug)]
pub struct Flash {
    pub note: NoteId,
    pub author: String,
    /// Seconds since the app started (egui time) when it happened.
    pub at: f64,
}

pub struct DrawCtx<'a> {
    pub project: &'a Project,
    pub theme: &'a Theme,
    /// Playhead in pattern ticks, if the transport is playing this pattern.
    pub playhead: Option<Tick>,
    pub flashes: &'a [Flash],
    pub now: f64,
    /// Notes highlighted because an agent called `ui.select`.
    pub agent_selected: &'a BTreeSet<NoteId>,
}

pub const FLASH_SECS: f64 = 2.0;

/// Draws the piano roll into `ui` and returns what the user did.
pub fn show(ui: &mut egui::Ui, st: &mut PianoRollState, cx: &DrawCtx<'_>) -> Vec<PrOut> {
    let mut out = Vec::new();
    let (Some(pattern), Some(channel)) = (st.pattern, st.channel) else {
        ui.centered_and_justified(|ui| ui.label("Pick a channel to edit its notes."));
        return out;
    };
    st.sync(cx.project);
    let col = &cx.theme.colors;
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(avail, Sense::click_and_drag());
    let whole = resp.rect;
    painter.rect_filled(whole, CornerRadius::ZERO, col.bg0);

    let grid_rect = Rect::from_min_max(
        Pos2::new(whole.left() + KEYBOARD_W, whole.top() + RULER_H),
        Pos2::new(
            whole.right(),
            (whole.bottom() - VEL_LANE_H - 4.0).max(whole.top() + RULER_H + 20.0),
        ),
    );
    st.last_grid = grid_rect;
    let lane_rect = Rect::from_min_max(
        Pos2::new(grid_rect.left(), grid_rect.bottom() + 4.0),
        whole.max,
    );
    let key_rect = Rect::from_min_max(
        Pos2::new(whole.left(), grid_rect.top()),
        Pos2::new(grid_rect.left(), grid_rect.bottom()),
    );
    let ruler_rect = Rect::from_min_max(
        Pos2::new(grid_rect.left(), whole.top()),
        Pos2::new(grid_rect.right(), grid_rect.top()),
    );
    if st.needs_fit {
        st.fit(grid_rect.size());
    }
    st.view.clamp_y(grid_rect.height());

    // ---- input -------------------------------------------------------------------------
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
                i.pointer.button_pressed(egui::PointerButton::Middle),
            ],
            [
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Secondary),
                i.pointer.button_released(egui::PointerButton::Middle),
            ],
            i.pointer.any_down(),
        )
    });
    let hovered = resp.hovered() || st.is_dragging();
    if let Some(pp) = pointer {
        let rel = pp - grid_rect.min;
        if resp.hovered() && (scroll.x != 0.0 || scroll.y != 0.0) && whole.contains(pp) {
            if mods.ctrl {
                st.view
                    .zoom_x(1.0 + scroll.y as f64 * 0.002, rel.x.max(0.0));
            } else if mods.alt {
                st.view.zoom_y(1.0 + scroll.y * 0.002, rel.y.max(0.0));
            } else if mods.shift {
                st.view.x0 =
                    (st.view.x0 - (scroll.y + scroll.x) as f64 / st.view.px_per_tick).max(0.0);
            } else {
                st.view.y0 += scroll.y as f64 / st.view.key_h as f64;
                st.view.x0 = (st.view.x0 - scroll.x as f64 / st.view.px_per_tick).max(0.0);
                st.view.clamp_y(grid_rect.height());
            }
        }
        if hovered {
            let btns = [Btn::Primary, Btn::Secondary, Btn::Middle];
            for (k, b) in btns.iter().enumerate() {
                if pressed[k] {
                    if grid_rect.contains(pp) && !st.is_dragging() {
                        out.extend(st.pointer_press(rel, *b, mods));
                    } else if lane_rect.contains(pp) && *b == Btn::Primary {
                        st.velocity_press(pp - lane_rect.min, lane_rect.height());
                    } else if key_rect.contains(pp) && *b == Btn::Primary {
                        let key = st.view.key_at(pp.y - grid_rect.top()).clamp(0, 127) as u8;
                        out.push(PrOut::Preview { key, on: true });
                        ui.data_mut(|d| d.insert_temp(resp.id, key));
                    } else if ruler_rect.contains(pp) && *b == Btn::Primary {
                        let t = snap_tick(st.view.tick_at(pp.x - grid_rect.left()), st.grid());
                        out.push(PrOut::Seek(t));
                    }
                }
            }
            if down && st.is_dragging() {
                let lane_rel = pp - lane_rect.min;
                let moved = if matches!(st.drag, Drag::Velocity { .. }) {
                    st.pointer_move(Vec2::new(lane_rel.x, lane_rel.y), mods)
                } else {
                    st.pointer_move(rel, mods)
                };
                out.extend(moved);
            }
            for (k, _) in btns.iter().enumerate() {
                if released[k] && st.is_dragging() {
                    out.extend(st.pointer_release(rel, mods));
                }
            }
        }
        if released[0]
            && let Some(key) = ui.data_mut(|d| d.remove_temp::<u8>(resp.id))
        {
            out.push(PrOut::Preview { key, on: false });
        }
    }

    // ---- grid ----------------------------------------------------------------------------
    let view = st.view;
    let gp = painter.with_clip_rect(grid_rect);
    let origin = grid_rect.min;
    let rows_lo = (view.key_at(grid_rect.height()).max(0)) as u8;
    let rows_hi = view.key_at(0.0).clamp(0, 127) as u8;
    for key in rows_lo..=rows_hi {
        let y = origin.y + view.y_of(key);
        let r = Rect::from_min_size(
            Pos2::new(grid_rect.left(), y),
            Vec2::new(grid_rect.width(), view.key_h),
        );
        gp.rect_filled(
            r,
            CornerRadius::ZERO,
            if is_black(key) {
                col.key_black
            } else {
                col.key_white
            },
        );
        if key % 12 == 0 {
            gp.line_segment(
                [
                    Pos2::new(r.left(), r.bottom()),
                    Pos2::new(r.right(), r.bottom()),
                ],
                Stroke::new(1.0, col.grid_major),
            );
        }
    }
    // vertical lines: choose a subdivision that is at least 10 px wide
    let bar = st.ticks_per_bar.max(1);
    let mut sub = st.grid().max(1);
    while (sub as f64 * view.px_per_tick) < 10.0 && sub < bar * 64 {
        sub *= 2;
    }
    let t0 = view.x0;
    let t1 = view.tick_at(grid_rect.width());
    let mut shapes: Vec<Shape> = Vec::new();
    let first = (t0 / sub as f64).floor() as i64;
    let last = (t1 / sub as f64).ceil() as i64;
    for i in first.max(0)..=last {
        let t = i as Tick * sub;
        let x = origin.x + view.x_of(t as f64);
        let c = if t.is_multiple_of(bar) {
            col.grid_bar
        } else if t.is_multiple_of(960) {
            col.grid_major
        } else {
            col.grid_minor
        };
        shapes.push(Shape::line_segment(
            [
                Pos2::new(x, grid_rect.top()),
                Pos2::new(x, grid_rect.bottom()),
            ],
            Stroke::new(1.0, c),
        ));
    }
    gp.extend(shapes);
    // pattern end
    let end_x = origin.x + view.x_of(st.pattern_len as f64);
    gp.line_segment(
        [
            Pos2::new(end_x, grid_rect.top()),
            Pos2::new(end_x, grid_rect.bottom()),
        ],
        Stroke::new(2.0, col.accent_dim),
    );

    // ghost notes from other channels
    if st.ghosts
        && let Ok(pat) = cx.project.pattern(pattern)
    {
        let mut mesh = egui::Mesh::default();
        let ghost = col.ghost.gamma_multiply(0.45);
        for (ch, notes) in &pat.notes {
            if *ch == channel {
                continue;
            }
            for n in notes.values() {
                let r = view
                    .note_rect(n.key, n.start, n.length)
                    .translate(origin.to_vec2());
                if r.intersects(grid_rect) {
                    mesh.add_colored_rect(
                        Rect::from_min_max(
                            r.min,
                            Pos2::new(r.max.x.max(r.min.x + 1.0), r.max.y - 1.0),
                        ),
                        ghost,
                    );
                }
            }
        }
        gp.add(Shape::mesh(mesh));
    }

    // notes
    let ch_color = cx
        .project
        .channels
        .get(&channel)
        .map(|c| rgb(c.color))
        .unwrap_or(col.note);
    let flash_map: HashMap<NoteId, &Flash> = cx
        .flashes
        .iter()
        .filter(|f| cx.now - f.at < FLASH_SECS)
        .map(|f| (f.note, f))
        .collect();
    let range = st.cache.visible_range(t0, t1);
    let mut shapes = Vec::new();
    // Small notes (everything when zoomed out) go into one mesh of plain rects: far cheaper to
    // tessellate than 10 000 rounded-rect shapes. Large notes get rounding and outlines.
    let mut mesh = egui::Mesh::default();
    let mut badges: Vec<(Pos2, &str, f32)> = Vec::new();
    let big_enough = view.px_per_tick * 240.0 > 18.0 && view.key_h >= 12.0;
    let mut labels: Vec<(Pos2, String)> = Vec::new();
    for n in &st.cache.notes[range] {
        let (key, start, len) = st.display_geom(n);
        let mut r = view.note_rect(key, start, len).translate(origin.to_vec2());
        if !r.intersects(grid_rect) {
            continue;
        }
        r.max.y -= 1.0;
        r.max.x = r.max.x.max(r.min.x + 1.5);
        let base = lerp(ch_color.gamma_multiply(0.55), ch_color, n.velocity);
        // a muted note is drawn washed out, with a dashed-looking outline
        let base = if n.muted {
            base.gamma_multiply(0.28)
        } else {
            base
        };
        let sel = st.selection.contains(&n.id);
        let special = cx.agent_selected.contains(&n.id) || flash_map.contains_key(&n.id);
        if r.width() < 8.0 || r.height() < 9.0 {
            mesh.add_colored_rect(r, if sel { col.selection } else { base });
            if !special {
                continue;
            }
        } else {
            shapes.push(Shape::rect_filled(r, CornerRadius::same(2), base));
            if sel {
                shapes.push(Shape::rect_stroke(
                    r,
                    CornerRadius::same(2),
                    Stroke::new(1.5, col.selection),
                    StrokeKind::Inside,
                ));
            }
        }
        if cx.agent_selected.contains(&n.id) {
            shapes.push(Shape::rect_stroke(
                r.expand(1.5),
                CornerRadius::same(3),
                Stroke::new(2.0, col.agent),
                StrokeKind::Outside,
            ));
        }
        if let Some(f) = flash_map.get(&n.id) {
            let k = (1.0 - ((cx.now - f.at) / FLASH_SECS) as f32).clamp(0.0, 1.0);
            shapes.push(Shape::rect_stroke(
                r.expand(2.0),
                CornerRadius::same(3),
                Stroke::new(2.0, col.agent.gamma_multiply(k)),
                StrokeKind::Outside,
            ));
            badges.push((r.left_top(), &f.author, k));
        }
        if big_enough && r.width() > 40.0 {
            labels.push((r.left_center() + Vec2::new(3.0, 0.0), note_name(key)));
        }
    }
    gp.add(Shape::mesh(mesh));
    gp.extend(shapes);
    for (p, s) in labels {
        gp.text(
            p,
            Align2::LEFT_CENTER,
            s,
            FontId::proportional(10.0),
            Color32::from_black_alpha(190),
        );
    }
    for (p, who, k) in badges {
        let label = who.trim_start_matches("agent:");
        let g = gp.layout_no_wrap(
            label.to_string(),
            FontId::proportional(10.0),
            Color32::BLACK,
        );
        let r = Rect::from_min_size(
            p - Vec2::new(0.0, g.size().y + 2.0),
            g.size() + Vec2::new(6.0, 2.0),
        );
        gp.rect_filled(r, CornerRadius::same(3), col.agent.gamma_multiply(k));
        gp.galley(r.min + Vec2::new(3.0, 1.0), g, Color32::BLACK);
    }

    // in-flight new note and box
    match &st.drag {
        Drag::NewNote {
            start, key, len, ..
        } => {
            let r = view
                .note_rect(*key, *start, *len)
                .translate(origin.to_vec2());
            gp.rect_filled(r, CornerRadius::same(2), ch_color.gamma_multiply(0.7));
            gp.rect_stroke(
                r,
                CornerRadius::same(2),
                Stroke::new(1.0, col.selection),
                StrokeKind::Inside,
            );
        }
        Drag::BoxSelect { from, to, .. } => {
            let r = Rect::from_two_pos(origin + *from, origin + *to);
            gp.rect_filled(r, CornerRadius::ZERO, col.accent.gamma_multiply(0.15));
            gp.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.0, col.accent),
                StrokeKind::Inside,
            );
        }
        Drag::Lasso { points, .. } => {
            let pts: Vec<Pos2> = points.iter().map(|p| origin + *p).collect();
            if pts.len() > 2 {
                gp.add(Shape::convex_polygon(
                    pts.clone(),
                    col.accent.gamma_multiply(0.12),
                    Stroke::NONE,
                ));
            }
            let mut closed = pts;
            if let Some(first) = closed.first().copied() {
                closed.push(first);
            }
            gp.add(Shape::line(closed, Stroke::new(1.5, col.accent)));
        }
        Drag::Mute { ids, to } => {
            for id in ids {
                if let Some(n) = st.cache.get(*id) {
                    let r = view
                        .note_rect(n.key, n.start, n.length)
                        .translate(origin.to_vec2());
                    let c = if *to == Some(true) {
                        col.dim
                    } else {
                        col.accent
                    };
                    gp.rect_filled(r, CornerRadius::same(2), c.gamma_multiply(0.6));
                }
            }
        }
        Drag::Erase { ids } => {
            for id in ids {
                if let Some(n) = st.cache.get(*id) {
                    let r = view
                        .note_rect(n.key, n.start, n.length)
                        .translate(origin.to_vec2());
                    gp.rect_filled(r, CornerRadius::same(2), col.error.gamma_multiply(0.6));
                }
            }
        }
        _ => {}
    }

    // playhead
    if let Some(ph) = cx.playhead {
        let x = origin.x + view.x_of(ph as f64);
        gp.line_segment(
            [
                Pos2::new(x, grid_rect.top()),
                Pos2::new(x, grid_rect.bottom()),
            ],
            Stroke::new(1.5, col.playhead),
        );
    }

    // ---- keyboard column -----------------------------------------------------------------
    let kp = painter.with_clip_rect(key_rect);
    for key in rows_lo..=rows_hi {
        let y = grid_rect.top() + view.y_of(key);
        let r = Rect::from_min_size(
            Pos2::new(key_rect.left(), y),
            Vec2::new(KEYBOARD_W, view.key_h),
        );
        let black = is_black(key);
        kp.rect_filled(
            r.shrink2(Vec2::new(0.0, 0.5)),
            CornerRadius::ZERO,
            if black {
                col.key_black.gamma_multiply(1.0)
            } else {
                col.bg3
            },
        );
        if black {
            kp.rect_filled(
                Rect::from_min_size(r.min, Vec2::new(KEYBOARD_W * 0.62, r.height()))
                    .shrink2(Vec2::new(0.0, 0.5)),
                CornerRadius::ZERO,
                col.bg0,
            );
        }
        if key % 12 == 0 || view.key_h >= 14.0 {
            kp.text(
                Pos2::new(key_rect.right() - 4.0, r.center().y),
                Align2::RIGHT_CENTER,
                note_name(key),
                FontId::proportional(10.0),
                if key % 12 == 0 { col.text } else { col.dim },
            );
        }
    }

    // ---- ruler ---------------------------------------------------------------------------
    let rp = painter.with_clip_rect(ruler_rect);
    rp.rect_filled(ruler_rect, CornerRadius::ZERO, col.bg3);
    let mut bar_step = 1;
    while (bar as f64 * bar_step as f64 * view.px_per_tick) < 40.0 {
        bar_step *= 2;
    }
    let first_bar = (t0 / bar as f64).floor().max(0.0) as u64;
    let mut b = first_bar - first_bar % bar_step;
    while (b * bar) as f64 <= t1 {
        let x = origin.x + view.x_of((b * bar) as f64);
        rp.line_segment(
            [
                Pos2::new(x, ruler_rect.top() + 12.0),
                Pos2::new(x, ruler_rect.bottom()),
            ],
            Stroke::new(1.0, col.grid_bar),
        );
        rp.text(
            Pos2::new(x + 3.0, ruler_rect.top() + 2.0),
            Align2::LEFT_TOP,
            format!("{}", b + 1),
            FontId::proportional(10.0),
            col.dim,
        );
        b += bar_step;
    }
    if let Some(ph) = cx.playhead {
        let x = origin.x + view.x_of(ph as f64);
        rp.line_segment(
            [
                Pos2::new(x, ruler_rect.top()),
                Pos2::new(x, ruler_rect.bottom()),
            ],
            Stroke::new(1.5, col.playhead),
        );
    }

    // ---- velocity lane -------------------------------------------------------------------
    let lp = painter.with_clip_rect(lane_rect);
    lp.rect_filled(lane_rect, CornerRadius::ZERO, col.bg2);
    let vel_override: Option<(&Vec<NoteId>, f32)> = match &st.drag {
        Drag::Velocity { ids, value } => Some((ids, *value)),
        _ => None,
    };
    let mut mesh = egui::Mesh::default();
    let range = st.cache.visible_range(t0, t1);
    for n in &st.cache.notes[range] {
        let x = lane_rect.left() + view.x_of(n.start as f64);
        if x < lane_rect.left() - 2.0 || x > lane_rect.right() {
            continue;
        }
        let v = match vel_override {
            Some((ids, val)) if ids.contains(&n.id) => val,
            _ => n.velocity,
        };
        let h = v * (lane_rect.height() - 4.0);
        let sel = st.selection.contains(&n.id);
        mesh.add_colored_rect(
            Rect::from_min_max(
                Pos2::new(x, lane_rect.bottom() - h - 2.0),
                Pos2::new(x + 3.0, lane_rect.bottom() - 2.0),
            ),
            if sel {
                col.selection
            } else {
                lerp(ch_color.gamma_multiply(0.6), ch_color, v)
            },
        );
    }
    lp.add(Shape::mesh(mesh));
    lp.text(
        lane_rect.left_top() + Vec2::new(4.0, 2.0),
        Align2::LEFT_TOP,
        "Velocity",
        FontId::proportional(10.0),
        col.dim,
    );
    out
}

/// A pattern's notes for one channel as `BTreeMap` (used by tests and the step grid).
pub fn channel_notes(
    project: &Project,
    pattern: PatternId,
    channel: ChannelId,
) -> BTreeMap<NoteId, Note> {
    project
        .patterns
        .get(&pattern)
        .and_then(|p| p.notes.get(&channel))
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
