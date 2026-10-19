//! Audio clips in the playlist (M2/T6): geometry, gestures, painting and the right-click menu.
//!
//! Kept apart from the pattern-clip code. A gesture here is a [`AudioDrag`]: it previews while
//! the pointer moves and turns into **one** command (or one batch) on release, so it is one
//! undo step and an agent sees the same diff. Source positions are frames of the file and
//! playlist positions are ticks; [`AudioView`] converts between them the way the engine does.
//!
//! | Gesture | Command |
//! |---|---|
//! | drag the body | `playlist.move_clip` (the ordinary clip move) |
//! | drag a left or right edge | `audio.set_range` (start, length, source offset) |
//! | Alt+drag the body (slip) | `audio.set_range` (source offset only) |
//! | drag a fade handle | `audio.set_fade` |
//! | slice tool click | `audio.split` |
//! | Ctrl+B | `audio.duplicate` |
//! | right-click menu | `audio.set_gain`, `set_pitch`, `set_reverse`, `set_stretch`, `fit_to_tempo`, `sample.set_bpm` ... |

use super::*;
use crate::mapping_audio as ma;
use crate::waveform::{ColumnCache, SharedWaves, key_of};
use egui::{Color32, Id, Mesh, Shape};
use ph_core::{
    AudioClip, Fade, FadeCurve, SampleEntry, StretchMode, seconds_to_ticks, ticks_to_seconds,
};

/// Width of the edge band that trims, in points.
pub const EDGE_PX: f32 = 7.0;
/// The label strip on top of a clip.
pub const HEADER_H: f32 = 14.0;
/// Hit radius of a fade handle.
pub const HANDLE_R: f32 = 6.0;
/// Fade lengths are multiples of this many ticks.
const FADE_STEP: i64 = 10;

/// State the audio side of the playlist keeps between frames.
#[derive(Default)]
pub struct AudioUi {
    /// The slice tool is on: a click on a clip splits it.
    pub slice_tool: bool,
    /// The open right-click menu: the clip and where it was opened.
    pub menu: Option<(ClipId, Pos2)>,
    menu_fresh: bool,
    /// Finished waveform columns by clip.
    pub cols: ColumnCache,
    /// What the pointer is over, from the last frame (for the cursor and the handles).
    pub hover: Option<(ClipId, AudioZone)>,
    /// Waveform columns drawn last frame, for tests.
    pub drawn_waveforms: usize,
}

/// Where on an audio clip a point is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioZone {
    Body,
    TrimLeft,
    TrimRight,
    FadeIn,
    FadeOut,
}

/// An audio clip's timing, in the units the engine uses.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioView {
    pub start: Tick,
    pub length: Tick,
    /// Source frame at the clip start (the start of the played range).
    pub offset: u64,
    pub reverse: bool,
    pub file_frames: u64,
    /// The file's sample rate.
    pub rate: f64,
    /// Source seconds per output second ([`AudioClip::time_ratio`]).
    pub ratio: f64,
    pub bpm: f64,
}

/// A new playlist position and source offset for a trimmed clip.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Trim {
    pub start: Tick,
    pub length: Tick,
    pub offset: u64,
}

impl AudioView {
    pub fn of(p: &Project, c: &Clip) -> Option<AudioView> {
        let a = c.audio()?;
        let e = p.sample(a.sample).ok()?;
        Some(Self::new(c, a, e, p.settings.bpm))
    }

    pub fn new(c: &Clip, a: &AudioClip, e: &SampleEntry, bpm: f64) -> AudioView {
        AudioView {
            start: c.start,
            length: c.length,
            offset: a.source_offset,
            reverse: a.reverse,
            file_frames: e.frames,
            rate: e.sample_rate.max(1) as f64,
            ratio: a.time_ratio(e, bpm).max(1e-6),
            bpm,
        }
    }

    /// Source frames that `ticks` of the playlist play (signed).
    pub fn frames(&self, ticks: i64) -> i64 {
        let s = ticks_to_seconds(ticks.unsigned_abs(), self.bpm) * self.ratio * self.rate;
        (s.round() as i64) * ticks.signum()
    }

    /// Playlist ticks that play `frames` source frames (signed).
    pub fn ticks(&self, frames: i64) -> i64 {
        let t = seconds_to_ticks(
            frames.unsigned_abs() as f64 / self.rate / self.ratio,
            self.bpm,
        );
        (t.round() as i64) * frames.signum()
    }

    /// Frames of the file the clip plays.
    pub fn span(&self) -> u64 {
        self.frames(self.length as i64).max(0) as u64
    }

    /// The longest the clip may be made on its right edge without running out of file.
    pub fn max_length(&self) -> Tick {
        if self.reverse {
            self.ticks((self.offset + self.span()) as i64).max(1) as Tick
        } else {
            self.ticks(self.file_frames.saturating_sub(self.offset) as i64)
                .max(1) as Tick
        }
    }

    /// Drag the left edge to `new_start` (ticks). Trimming in keeps the sound where it is on
    /// the timeline; pulling out is limited by the file.
    pub fn trim_left(&self, new_start: i64) -> Trim {
        let end = (self.start + self.length) as i64;
        let ns = new_start.clamp(0, end - 1);
        let delta = ns - self.start as i64;
        if !self.reverse {
            let off =
                (self.offset as i64 + self.frames(delta)).clamp(0, self.file_frames as i64 - 1);
            let d = self
                .ticks(off - self.offset as i64)
                .clamp(-(self.start as i64), self.length as i64 - 1);
            Trim {
                start: (self.start as i64 + d) as Tick,
                length: (self.length as i64 - d) as Tick,
                offset: off as u64,
            }
        } else {
            // the left edge plays the end of the range: pulling it out plays more of it
            let span = self.span() as i64;
            let max_span = self.file_frames as i64 - self.offset as i64;
            let new_span = (span - self.frames(delta)).clamp(1, max_span.max(1));
            let d = self
                .ticks(span - new_span)
                .clamp(-(self.start as i64), self.length as i64 - 1);
            Trim {
                start: (self.start as i64 + d) as Tick,
                length: (self.length as i64 - d) as Tick,
                offset: self.offset,
            }
        }
    }

    /// Drag the right edge to `new_end` (ticks).
    pub fn trim_right(&self, new_end: i64) -> Trim {
        let want = (new_end - self.start as i64).max(1);
        if !self.reverse {
            Trim {
                start: self.start,
                length: want.min(self.max_length() as i64).max(1) as Tick,
                offset: self.offset,
            }
        } else {
            // the right edge plays the start of the range: pulling it out moves the offset down
            let df = self.frames(want - self.length as i64);
            let off = (self.offset as i64 - df).clamp(0, self.file_frames as i64 - 1);
            let len = (self.length as i64 + self.ticks(self.offset as i64 - off)).max(1);
            Trim {
                start: self.start,
                length: len as Tick,
                offset: off as u64,
            }
        }
    }

    /// Slip: move the sound `dt` ticks inside the clip (positive = later).
    pub fn slip(&self, dt: i64) -> u64 {
        let df = self.frames(dt);
        if !self.reverse {
            (self.offset as i64 - df).clamp(0, self.file_frames as i64 - 1) as u64
        } else {
            let top = (self.file_frames as i64 - self.span() as i64).max(0);
            (self.offset as i64 + df).clamp(0, top) as u64
        }
    }
}

// ---- gestures ----------------------------------------------------------------------------

/// A gesture in progress on an audio clip.
#[derive(Clone, Debug)]
pub enum AudioDrag {
    Trim {
        id: ClipId,
        left: bool,
        view: AudioView,
        new: Trim,
    },
    Slip {
        id: ClipId,
        view: AudioView,
        grab: f64,
        offset: u64,
    },
    Fade {
        id: ClipId,
        fade_in: bool,
        start: Tick,
        length: Tick,
        /// The other fade's length (they never overlap).
        other: Tick,
        curve: FadeCurve,
        new: Tick,
    },
    Slice {
        id: ClipId,
        start: Tick,
        end: Tick,
        at: Tick,
    },
}

impl AudioDrag {
    pub fn id(&self) -> ClipId {
        match self {
            AudioDrag::Trim { id, .. }
            | AudioDrag::Slip { id, .. }
            | AudioDrag::Fade { id, .. }
            | AudioDrag::Slice { id, .. } => *id,
        }
    }

    /// The clip's (start, length) while this gesture previews it.
    pub fn geom(&self) -> Option<(Tick, Tick)> {
        match self {
            AudioDrag::Trim { new, .. } => Some((new.start, new.length)),
            _ => None,
        }
    }

    /// The fade lengths while this gesture previews them: (fade in, fade out).
    pub fn fades(&self) -> (Option<Tick>, Option<Tick>) {
        match self {
            AudioDrag::Fade { fade_in, new, .. } => {
                if *fade_in {
                    (Some(*new), None)
                } else {
                    (None, Some(*new))
                }
            }
            _ => (None, None),
        }
    }

    pub fn update(&mut self, st: &PlaylistState, pos: Vec2, mods: Mods) {
        let t = st.tick_at(pos.x);
        match self {
            AudioDrag::Trim {
                left, view, new, ..
            } => {
                let grid = st.grid_for(mods);
                let snapped = snap_tick(t.max(0.0), grid) as i64;
                *new = if *left {
                    view.trim_left(snapped)
                } else {
                    view.trim_right(snapped)
                };
            }
            AudioDrag::Slip {
                view, grab, offset, ..
            } => {
                // fine movement: slipping is not snapped to the grid unless Ctrl is held
                let dt = (t - *grab).round() as i64;
                let dt = if mods.ctrl {
                    let g = st.grid().max(1) as i64;
                    (dt as f64 / g as f64).round() as i64 * g
                } else {
                    dt
                };
                *offset = view.slip(dt);
            }
            AudioDrag::Fade {
                start,
                length,
                other,
                new,
                fade_in,
                ..
            } => {
                let from_start = if *fade_in {
                    t - *start as f64
                } else {
                    (*start + *length) as f64 - t
                };
                let v = (from_start / FADE_STEP as f64).round() as i64 * FADE_STEP;
                *new = v.clamp(0, (*length as i64 - *other as i64).max(0)) as Tick;
            }
            AudioDrag::Slice { start, end, at, .. } => {
                let snapped = snap_tick(t.max(0.0), st.grid_for(mods));
                *at = snapped.clamp(*start + 1, end.saturating_sub(1).max(*start + 1));
            }
        }
    }

    /// What the gesture does when released.
    pub fn finish(&self, base: &Clip, a: &AudioClip) -> Vec<PlOut> {
        match self {
            AudioDrag::Trim { id, new, view, .. } => {
                if (new.start, new.length, new.offset) == (view.start, view.length, view.offset) {
                    return vec![];
                }
                vec![PlOut::Cmd(ma::set_range(
                    *id,
                    (new.start != base.start).then_some(new.start),
                    (new.length != base.length).then_some(new.length),
                    (new.offset != a.source_offset).then_some(new.offset),
                ))]
            }
            AudioDrag::Slip { id, offset, .. } => {
                if *offset == a.source_offset {
                    vec![]
                } else {
                    vec![PlOut::Cmd(ma::set_range(*id, None, None, Some(*offset)))]
                }
            }
            AudioDrag::Fade {
                id,
                fade_in,
                new,
                curve,
                ..
            } => {
                let cur = if *fade_in { a.fade_in } else { a.fade_out };
                if cur.length == *new {
                    return vec![];
                }
                let f = Fade {
                    length: *new,
                    curve: *curve,
                };
                vec![PlOut::Cmd(if *fade_in {
                    ma::set_fade_in(*id, f)
                } else {
                    ma::set_fade_out(*id, f)
                })]
            }
            AudioDrag::Slice { id, at, .. } => vec![PlOut::Cmd(ma::split(*id, *at))],
        }
    }
}

/// The zone a running gesture belongs to (for the cursor and the lit handle).
pub fn audio_zone_of(d: &AudioDrag) -> AudioZone {
    match d {
        AudioDrag::Trim { left: true, .. } => AudioZone::TrimLeft,
        AudioDrag::Trim { left: false, .. } => AudioZone::TrimRight,
        AudioDrag::Fade { fade_in: true, .. } => AudioZone::FadeIn,
        AudioDrag::Fade { fade_in: false, .. } => AudioZone::FadeOut,
        _ => AudioZone::Body,
    }
}

/// The mouse cursor for a zone.
pub fn cursor_for(z: AudioZone) -> egui::CursorIcon {
    match z {
        AudioZone::TrimLeft | AudioZone::TrimRight => egui::CursorIcon::ResizeHorizontal,
        AudioZone::FadeIn | AudioZone::FadeOut => egui::CursorIcon::PointingHand,
        AudioZone::Body => egui::CursorIcon::Default,
    }
}

impl PlaylistState {
    /// Where on audio clip `c` the grid-relative point `pos` is.
    pub fn audio_zone(&self, c: &Clip, a: &AudioClip, pos: Vec2) -> Option<AudioZone> {
        let r = self.clip_rect(c.track as i32, c.start, c.length);
        let p = Pos2::new(pos.x, pos.y);
        if !r.contains(p) {
            return None;
        }
        let fin = r.left() + a.fade_in.length as f32 * self.px_per_tick as f32;
        let fout = r.right() - a.fade_out.length as f32 * self.px_per_tick as f32;
        let handle_y = r.top() + HEADER_H * 0.5;
        // a handle wins over an edge, the one nearer the pointer first
        let hin = (p - Pos2::new(fin, handle_y)).length();
        let hout = (p - Pos2::new(fout, handle_y)).length();
        let wide = r.width() > 3.0 * HANDLE_R;
        if wide && hin <= HANDLE_R && hin <= hout {
            return Some(AudioZone::FadeIn);
        }
        if wide && hout <= HANDLE_R {
            return Some(AudioZone::FadeOut);
        }
        let edge = (r.width() * 0.25).clamp(2.0, EDGE_PX);
        Some(if p.x < r.left() + edge {
            AudioZone::TrimLeft
        } else if p.x > r.right() - edge {
            AudioZone::TrimRight
        } else {
            AudioZone::Body
        })
    }

    /// Primary press on audio clip `id`. Returns true when it started an audio gesture (the
    /// caller then skips the pattern-clip handling); selection is handled here.
    pub(super) fn audio_press(&mut self, id: ClipId, pos: Vec2, mods: Mods) -> bool {
        let Some(c) = self.clip(id).cloned() else {
            return false;
        };
        let Some(a) = c.audio().cloned() else {
            return false;
        };
        let Some(view) = self.audio_views.get(&id).cloned() else {
            return false;
        };
        let Some(zone) = self.audio_zone(&c, &a, pos) else {
            return false;
        };
        if !self.selection.contains(&id) {
            if !mods.shift {
                self.selection.clear();
            }
            self.selection.insert(id);
        }
        let drag = if self.audio.slice_tool {
            let at = snap_tick(self.tick_at(pos.x).max(0.0), self.grid_for(mods)).clamp(
                c.start + 1,
                (c.start + c.length).saturating_sub(1).max(c.start + 1),
            );
            AudioDrag::Slice {
                id,
                start: c.start,
                end: c.start + c.length,
                at,
            }
        } else {
            match zone {
                AudioZone::TrimLeft | AudioZone::TrimRight => AudioDrag::Trim {
                    id,
                    left: zone == AudioZone::TrimLeft,
                    new: Trim {
                        start: c.start,
                        length: c.length,
                        offset: a.source_offset,
                    },
                    view,
                },
                AudioZone::FadeIn | AudioZone::FadeOut => {
                    let fade_in = zone == AudioZone::FadeIn;
                    let (cur, other) = if fade_in {
                        (a.fade_in, a.fade_out)
                    } else {
                        (a.fade_out, a.fade_in)
                    };
                    AudioDrag::Fade {
                        id,
                        fade_in,
                        start: c.start,
                        length: c.length,
                        other: other.length,
                        curve: cur.curve,
                        new: cur.length,
                    }
                }
                AudioZone::Body if mods.alt => AudioDrag::Slip {
                    id,
                    grab: self.tick_at(pos.x),
                    offset: a.source_offset,
                    view,
                },
                // the ordinary move
                AudioZone::Body => return false,
            }
        };
        self.drag = Drag::Audio(drag);
        true
    }

    /// The clip with in-flight audio edits applied: (start, length), plus preview fades.
    pub(super) fn audio_preview(&self, c: &Clip) -> Option<(Tick, Tick)> {
        match &self.drag {
            Drag::Audio(d) if d.id() == c.id => d.geom(),
            _ => None,
        }
    }

    /// Ctrl+B for audio clips: a copy of each selected one, right after itself.
    pub(super) fn audio_duplicates(&self) -> Vec<Request> {
        self.selection
            .iter()
            .filter_map(|id| self.clip(*id))
            .filter(|c| c.audio().is_some())
            .map(|c| ma::duplicate(c.id))
            .collect()
    }
}

// ---- painting ----------------------------------------------------------------------------

/// What the painter needs of the project and the caches.
pub struct PaintCx<'a> {
    pub project: &'a Project,
    pub theme: &'a Theme,
    pub waves: &'a SharedWaves,
    pub grid: Rect,
}

/// Points of a fade's gain curve over `n` steps: (t, gain) with t in 0..=1 from the clip edge.
fn fade_points(curve: FadeCurve, n: usize) -> Vec<(f32, f32)> {
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            (t as f32, curve.gain(t) as f32)
        })
        .collect()
}

/// The base colour of audio clips.
pub fn clip_colour(theme: &Theme) -> Color32 {
    theme.colors.pattern[3]
}

/// Paints one audio clip into `r` (screen rectangle, already inset), clipped by `gp`.
#[allow(clippy::too_many_arguments)]
pub fn paint_clip(
    gp: &egui::Painter,
    ui_state: &mut AudioUi,
    cx: &PaintCx<'_>,
    c: &Clip,
    a: &AudioClip,
    r: Rect,
    selected: bool,
    fades: (Option<Tick>, Option<Tick>),
    hot: Option<AudioZone>,
    ppt: f64,
) {
    let col = &cx.theme.colors;
    let base = clip_colour(cx.theme);
    let corner = CornerRadius::same(3);
    gp.rect_filled(r, corner, lerp(col.bg2, base, 0.30));
    let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEADER_H.min(r.height())));
    gp.rect_filled(
        head,
        CornerRadius {
            nw: 3,
            ne: 3,
            sw: 0,
            se: 0,
        },
        base,
    );
    let entry = cx.project.sample(a.sample).ok();
    // ---- waveform
    let wave = Rect::from_min_max(
        Pos2::new(r.left(), r.top() + HEADER_H),
        Pos2::new(r.right(), r.bottom() - 1.0),
    );
    let view = entry.map(|e| AudioView::new(c, a, e, cx.project.settings.bpm));
    let mut loading = false;
    if let (Some(e), Some(v)) = (entry, view.as_ref())
        && wave.height() > 4.0
    {
        let loaded = cx.waves.lock().ok().and_then(|mut w| w.get(&e.path));
        match loaded {
            Some(l) => {
                let generation = cx.waves.lock().map_or(0, |w| w.generation);
                paint_wave(
                    gp, ui_state, c, a, v, &l.peaks, wave, base, cx, ppt, generation,
                );
            }
            None => loading = true,
        }
    }
    if loading {
        gp.line_segment(
            [wave.left_center(), wave.right_center()],
            Stroke::new(1.0, lerp(col.bg2, base, 0.6)),
        );
    }
    // ---- fades: the area the fade removes, and the curve
    let (fin, fout) = (
        fades.0.unwrap_or(a.fade_in.length),
        fades.1.unwrap_or(a.fade_out.length),
    );
    let shade = col.bg0.gamma_multiply(0.55);
    let line = Stroke::new(1.0, lerp(base, col.text, 0.7));
    for (len, curve, is_in) in [
        (fin, a.fade_in.curve, true),
        (fout, a.fade_out.curve, false),
    ] {
        let w = (len as f64 * ppt) as f32;
        if w < 1.0 {
            continue;
        }
        let w = w.min(r.width());
        let pts = fade_points(curve, 16);
        let mut mesh = Mesh::default();
        let mut curve_pts = Vec::with_capacity(pts.len());
        let y_of = |g: f32| wave.bottom() - g * (wave.height());
        let x_of = |t: f32| {
            if is_in {
                r.left() + t * w
            } else {
                r.right() - t * w
            }
        };
        // t runs from the clip edge inward; gain is 0 at the edge and 1 inside
        for (t, g) in &pts {
            curve_pts.push(Pos2::new(x_of(*t), y_of(*g)));
        }
        for pair in pts.windows(2) {
            let (t0, g0) = pair[0];
            let (t1, g1) = pair[1];
            let (xa, xb) = (x_of(t0), x_of(t1));
            let i = mesh.vertices.len() as u32;
            for (x, y) in [
                (xa, wave.top()),
                (xb, wave.top()),
                (xb, y_of(g1)),
                (xa, y_of(g0)),
            ] {
                mesh.colored_vertex(Pos2::new(x, y), shade);
            }
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
        gp.add(Shape::mesh(mesh));
        gp.add(Shape::line(curve_pts, line));
    }
    // ---- handles
    let handle_y = r.top() + HEADER_H * 0.5;
    if r.width() > 3.0 * HANDLE_R {
        for (x, zone, len) in [
            (r.left() + (fin as f64 * ppt) as f32, AudioZone::FadeIn, fin),
            (
                r.right() - (fout as f64 * ppt) as f32,
                AudioZone::FadeOut,
                fout,
            ),
        ] {
            let lit = hot == Some(zone) || len > 0;
            let c0 = if hot == Some(zone) {
                col.accent_hot
            } else if lit {
                col.accent_ink
            } else {
                col.accent_ink.gamma_multiply(0.55)
            };
            gp.rect_filled(
                Rect::from_center_size(Pos2::new(x, handle_y), Vec2::splat(5.0)),
                CornerRadius::same(1),
                c0,
            );
        }
    }
    if matches!(hot, Some(AudioZone::TrimLeft)) {
        gp.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(EDGE_PX.min(r.width() / 4.0), r.height())),
            CornerRadius::same(3),
            col.accent_ink.gamma_multiply(0.25),
        );
    }
    if matches!(hot, Some(AudioZone::TrimRight)) {
        let w = EDGE_PX.min(r.width() / 4.0);
        gp.rect_filled(
            Rect::from_min_size(Pos2::new(r.right() - w, r.top()), Vec2::new(w, r.height())),
            CornerRadius::same(3),
            col.accent_ink.gamma_multiply(0.25),
        );
    }
    // ---- label and badges
    let badges = badges_of(a, entry);
    let label = cx.project.clip_label(c);
    let inner_w = (r.width() - 8.0 - 2.0 * HANDLE_R).max(0.0);
    let badge_w = if inner_w > 150.0 && !badges.is_empty() {
        (badges.chars().count() as f32 * 5.6).min(inner_w * 0.45)
    } else {
        0.0
    };
    crate::paint::text_fit(
        &gp.with_clip_rect(r.intersect(gp.clip_rect())),
        r.left_top() + Vec2::new(4.0 + HANDLE_R, 1.0),
        Align2::LEFT_TOP,
        &label,
        FontId::new(11.0, crate::fonts::body_medium()),
        col.accent_ink,
        inner_w - badge_w,
    );
    if badge_w > 0.0 {
        crate::paint::text_fit(
            &gp.with_clip_rect(r.intersect(gp.clip_rect())),
            r.right_top() + Vec2::new(-4.0 - HANDLE_R, 2.0),
            Align2::RIGHT_TOP,
            &badges,
            FontId::new(9.0, crate::fonts::cond()),
            col.accent_ink.gamma_multiply(0.85),
            badge_w,
        );
    }
    if selected {
        gp.rect_stroke(
            r,
            corner,
            Stroke::new(1.5, col.selection),
            StrokeKind::Inside,
        );
    }
}

/// Short settings badges shown on a wide clip: `REV`, pitch, gain, sync.
pub fn badges_of(a: &AudioClip, e: Option<&SampleEntry>) -> String {
    let mut v: Vec<String> = Vec::new();
    if a.reverse {
        v.push("REV".into());
    }
    if a.pitch_semitones != 0 || a.fine_cents != 0.0 {
        v.push(format!("{:+.1}st", a.pitch()));
    }
    if a.gain_db.abs() >= 0.05 {
        v.push(format!("{:+.1}dB", a.gain_db));
    }
    if a.tempo_sync && e.is_some_and(|e| e.bpm.is_some()) {
        v.push(match a.stretch {
            StretchMode::Resample => "SYNC".into(),
            StretchMode::Stretch => "STRETCH".into(),
            StretchMode::Slice => "SLICE".into(),
        });
    }
    v.join(" ")
}

/// Draws the waveform of one clip as bars, from the cached pyramid and cached columns.
#[allow(clippy::too_many_arguments)]
fn paint_wave(
    gp: &egui::Painter,
    ui_state: &mut AudioUi,
    c: &Clip,
    a: &AudioClip,
    v: &AudioView,
    peaks: &crate::waveform::Peaks,
    wave: Rect,
    base: Color32,
    cx: &PaintCx<'_>,
    ppt: f64,
    generation: u64,
) {
    let clip = gp.clip_rect().intersect(wave);
    if clip.width() < 1.0 || clip.height() < 1.0 {
        return;
    }
    let x_first = clip.left().floor();
    let x_last = clip.right().ceil();
    let n = (x_last - x_first).max(0.0) as usize;
    let span = v.span() as i64;
    let fpp = v
        .frames((1.0 / ppt).round().max(1.0) as i64)
        .unsigned_abs()
        .max(1) as f64;
    let level = peaks.level_for(fpp);
    let key = key_of(&[
        generation,
        ppt.to_bits(),
        (wave.left() as f64).to_bits(),
        (x_first as f64).to_bits(),
        n as u64,
        v.offset,
        v.length,
        v.reverse as u64,
        v.ratio.to_bits(),
        v.rate.to_bits(),
        a.sample.0 as u64,
        level as u64,
    ]);
    let cols = ui_state.cols.get(c.id.0 as u64, key, || {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let x = x_first + i as f32;
            // output ticks since the clip start at the column's two edges
            let t0 = ((x - wave.left()) as f64 / ppt).round() as i64;
            let t1 = ((x + 1.0 - wave.left()) as f64 / ppt).round() as i64;
            let (mut f0, mut f1) = (v.frames(t0), v.frames(t1.max(t0 + 1)));
            if v.reverse {
                let (r0, r1) = (span - f1, span - f0);
                f0 = r0;
                f1 = r1;
            }
            let (f0, f1) = (v.offset as f64 + f0 as f64, v.offset as f64 + f1 as f64);
            out.push(
                peaks
                    .range(level, f0, f1.max(f0 + 1.0), None)
                    .unwrap_or((1.0, -1.0)),
            );
        }
        out
    });
    let mid = wave.center().y;
    let half = wave.height() * 0.5 - 1.0;
    let gain = 10f32.powf(a.gain_db / 20.0);
    let ink = lerp(base, cx.theme.colors.text, 0.6);
    let mut mesh = Mesh::default();
    let mut drawn = 0usize;
    for (i, (mn, mx)) in cols.iter().enumerate() {
        if mn > mx {
            continue;
        }
        let x = x_first + i as f32;
        let top = (mid - (mx * gain).clamp(-1.0, 1.0) * half).min(mid - 0.5);
        let bot = (mid - (mn * gain).clamp(-1.0, 1.0) * half).max(mid + 0.5);
        mesh.add_colored_rect(
            Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + 1.0, bot)),
            ink,
        );
        drawn += 1;
    }
    ui_state.drawn_waveforms += drawn;
    gp.add(Shape::mesh(mesh));
}

// ---- the right-click menu ------------------------------------------------------------------

const MENU_ID: &str = "ph_audio_clip_menu";

impl PlaylistState {
    /// Opens the clip menu at screen position `at`.
    pub fn open_audio_menu(&mut self, id: ClipId, at: Pos2) {
        self.audio.menu = Some((id, at));
        self.audio.menu_fresh = true;
        self.selection.clear();
        self.selection.insert(id);
    }
}

/// Shows the clip menu if one is open; its commands are appended to `out`.
pub fn show_menu(
    anchor: &egui::Response,
    st: &mut PlaylistState,
    cx: &super::DrawCtx<'_>,
    out: &mut Vec<PlOut>,
) {
    let Some((id, at)) = st.audio.menu else {
        return;
    };
    let Some(c) = cx.project.clips.get(&id).cloned() else {
        st.audio.menu = None;
        return;
    };
    let Some(a) = c.audio().cloned() else {
        st.audio.menu = None;
        return;
    };
    let entry = cx.project.sample(a.sample).ok().cloned();
    let popup_id = Id::new(MENU_ID);
    let open = if st.audio.menu_fresh {
        st.audio.menu_fresh = false;
        Some(egui::SetOpenCommand::Bool(true))
    } else {
        None
    };
    let shown = egui::Popup::menu(anchor)
        .id(popup_id)
        .at_position(at)
        .open_memory(open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(250.0)
        .show(|ui| menu_body(ui, cx, &c, &a, entry.as_ref(), out));
    if shown.is_none() {
        st.audio.menu = None;
    }
}

fn menu_body(
    ui: &mut egui::Ui,
    cx: &super::DrawCtx<'_>,
    c: &Clip,
    a: &AudioClip,
    entry: Option<&SampleEntry>,
    out: &mut Vec<PlOut>,
) {
    let theme = cx.theme;
    ui.spacing_mut().item_spacing.y = 5.0;
    let name = cx.project.clip_label(c);
    ui.add(
        egui::Label::new(
            egui::RichText::new(name)
                .font(FontId::new(12.5, crate::fonts::body_medium()))
                .color(theme.colors.text),
        )
        .truncate(),
    );
    ui.separator();
    // gain
    ui.horizontal(|ui| {
        ui.label(crate::widgets::caption(theme, "Gain"));
        let mut g = a.gain_db;
        let r = ui.add(
            egui::DragValue::new(&mut g)
                .range(-60.0..=24.0)
                .speed(0.1)
                .suffix(" dB")
                .fixed_decimals(1),
        );
        if (r.drag_stopped() || r.lost_focus()) && (g - a.gain_db).abs() > f32::EPSILON {
            out.push(PlOut::Cmd(ma::set_gain(c.id, g)));
        }
    });
    // pitch
    ui.horizontal(|ui| {
        ui.label(crate::widgets::caption(theme, "Pitch"));
        let mut st_ = a.pitch_semitones;
        let r = ui.add(egui::DragValue::new(&mut st_).range(-48..=48).suffix(" st"));
        if (r.drag_stopped() || r.lost_focus()) && st_ != a.pitch_semitones {
            out.push(PlOut::Cmd(ma::set_pitch(c.id, Some(st_), None)));
        }
        let mut cents = a.fine_cents;
        let r = ui.add(
            egui::DragValue::new(&mut cents)
                .range(-100.0..=100.0)
                .speed(0.5)
                .suffix(" ct")
                .fixed_decimals(0),
        );
        if (r.drag_stopped() || r.lost_focus()) && (cents - a.fine_cents).abs() > f32::EPSILON {
            out.push(PlOut::Cmd(ma::set_pitch(c.id, None, Some(cents))));
        }
    });
    let mut rev = a.reverse;
    if ui.checkbox(&mut rev, "Reverse").changed() {
        out.push(PlOut::Cmd(ma::set_reverse(c.id, rev)));
    }
    ui.separator();
    // stretch
    ui.horizontal(|ui| {
        ui.label(crate::widgets::caption(theme, "Mode"));
        for (m, label) in [
            (StretchMode::Resample, "Resample"),
            (StretchMode::Stretch, "Stretch"),
            (StretchMode::Slice, "Slice"),
        ] {
            if ui.selectable_label(a.stretch == m, label).clicked() && a.stretch != m {
                out.push(PlOut::Cmd(ma::set_stretch(c.id, Some(m), None)));
            }
        }
    });
    let mut sync = a.tempo_sync;
    if ui.checkbox(&mut sync, "Follow project tempo").changed() {
        out.push(PlOut::Cmd(ma::set_stretch(c.id, None, Some(sync))));
    }
    if let Some(e) = entry {
        ui.horizontal(|ui| {
            ui.label(crate::widgets::caption(theme, "BPM"));
            let mut bpm = e.bpm().unwrap_or(0.0);
            let r = ui.add(
                egui::DragValue::new(&mut bpm)
                    .range(0.0..=999.0)
                    .speed(0.1)
                    .fixed_decimals(2)
                    .custom_formatter(|v, _| {
                        if v < 10.0 {
                            "none".to_string()
                        } else {
                            format!("{v:.2}")
                        }
                    }),
            );
            if (r.drag_stopped() || r.lost_focus()) && (bpm - e.bpm().unwrap_or(0.0)).abs() > 1e-9 {
                out.push(PlOut::Cmd(ma::set_bpm(e.id, (bpm >= 10.0).then_some(bpm))));
            }
            if ui.button("Detect").clicked() {
                out.push(PlOut::Cmd(ma::detect_bpm(e.id)));
            }
            if let Some(b) = e.bpm {
                let src = match b.source {
                    ph_core::BpmSource::Detected => "detected",
                    ph_core::BpmSource::User => "set",
                };
                ui.label(egui::RichText::new(src).size(10.5).color(theme.colors.dim));
            }
        });
    }
    if ui
        .button("Fit to tempo")
        .on_hover_text("Follow the project tempo using the sample's BPM, sized to the whole loop")
        .clicked()
    {
        out.push(PlOut::Cmd(ma::fit_to_tempo(c.id)));
        ui.close();
    }
    ui.separator();
    // fades
    for (is_in, label, cur) in [
        (true, "Fade in", a.fade_in),
        (false, "Fade out", a.fade_out),
    ] {
        ui.menu_button(format!("{label}: {}", curve_name(cur.curve)), |ui| {
            for curve in [
                FadeCurve::Linear,
                FadeCurve::Exp,
                FadeCurve::Log,
                FadeCurve::SCurve,
            ] {
                if ui
                    .selectable_label(cur.curve == curve, curve_name(curve))
                    .clicked()
                {
                    // a curve for a fade of no length gets a short one so it shows
                    let length = if cur.length == 0 {
                        (c.length / 8).max(1)
                    } else {
                        cur.length
                    };
                    let f = Fade { length, curve };
                    out.push(PlOut::Cmd(if is_in {
                        ma::set_fade_in(c.id, f)
                    } else {
                        ma::set_fade_out(c.id, f)
                    }));
                    ui.close();
                }
            }
            if cur.length > 0 && ui.button("No fade").clicked() {
                let f = Fade {
                    length: 0,
                    curve: cur.curve,
                };
                out.push(PlOut::Cmd(if is_in {
                    ma::set_fade_in(c.id, f)
                } else {
                    ma::set_fade_out(c.id, f)
                }));
                ui.close();
            }
        });
    }
    ui.separator();
    let inside = cx.position > c.start && cx.position < c.start + c.length;
    if ui
        .add_enabled(inside, egui::Button::new("Split at the playhead"))
        .clicked()
    {
        out.push(PlOut::Cmd(ma::split(c.id, cx.position)));
        ui.close();
    }
    if ui.button("Duplicate").clicked() {
        out.push(PlOut::Cmd(ma::duplicate(c.id)));
        ui.close();
    }
    if let Some(e) = entry {
        if ui.button("Open in audio editor").clicked() {
            out.push(PlOut::Cmd(ma::editor_open(e.id)));
            ui.close();
        }
        if ui
            .button("Make sliced instrument")
            .on_hover_text("Detect the transients and build a Sampler with one slice per key")
            .clicked()
        {
            out.push(PlOut::Cmd(Request::new(
                "slicer.make_instrument",
                serde_json::json!({ "sample": e.id }),
            )));
            ui.close();
        }
    }
    ui.separator();
    if crate::widgets::menu_item_tinted(ui, "Delete clip", theme.colors.error).clicked() {
        out.push(PlOut::Cmd(mapping::clip_remove(c.id)));
        ui.close();
    }
}

pub fn curve_name(c: FadeCurve) -> &'static str {
    match c {
        FadeCurve::Linear => "Linear",
        FadeCurve::Exp => "Exponential",
        FadeCurve::Log => "Logarithmic",
        FadeCurve::SCurve => "S-curve",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM: 1920 ticks per second. A 48 kHz file of 4 s, a clip of one second at tick
    /// 3840 playing from frame 24 000.
    fn view(reverse: bool) -> AudioView {
        AudioView {
            start: 3840,
            length: 1920,
            offset: 24_000,
            reverse,
            file_frames: 192_000,
            rate: 48_000.0,
            ratio: 1.0,
            bpm: 120.0,
        }
    }

    #[test]
    fn conversions_follow_the_tempo() {
        let v = view(false);
        assert_eq!(v.frames(1920), 48_000);
        assert_eq!(v.ticks(48_000), 1920);
        assert_eq!(v.frames(-960), -24_000);
        assert_eq!(v.span(), 48_000);
        // a clip that plays at twice the speed covers twice the frames
        let fast = AudioView { ratio: 2.0, ..v };
        assert_eq!(fast.frames(1920), 96_000);
    }

    #[test]
    fn trimming_in_keeps_the_sound_where_it_is() {
        let v = view(false);
        // left edge 480 ticks later: starts later, shorter, offset moves on by 12 000 frames
        let t = v.trim_left(3840 + 480);
        assert_eq!(
            t,
            Trim {
                start: 4320,
                length: 1440,
                offset: 36_000
            }
        );
        // right edge in: only the length changes
        let t = v.trim_right(3840 + 960);
        assert_eq!(
            t,
            Trim {
                start: 3840,
                length: 960,
                offset: 24_000
            }
        );
        // an edge cannot cross the other one
        assert_eq!(v.trim_left(99_999).length, 1);
        assert_eq!(v.trim_right(0).length, 1);
    }

    #[test]
    fn pulling_out_is_limited_by_the_file() {
        let v = view(false);
        // left edge 1920 ticks earlier would need frames before the start of the file:
        // 24 000 frames are available = 960 ticks
        let t = v.trim_left(3840 - 1920);
        assert_eq!(t.offset, 0);
        assert_eq!(t.start, 3840 - 960);
        assert_eq!(t.length, 1920 + 960);
        // right edge: the file has 168 000 frames left = 6720 ticks
        let t = v.trim_right(3840 + 99_999);
        assert_eq!(t.length, 6720);
        assert_eq!(v.max_length(), 6720);
    }

    #[test]
    fn a_reversed_clip_trims_from_the_other_side() {
        let v = view(true);
        // the left edge plays the end of the range: shrinking it leaves the offset alone
        let t = v.trim_left(3840 + 480);
        assert_eq!((t.start, t.length, t.offset), (4320, 1440, 24_000));
        // the right edge plays the start: shrinking it moves the offset up
        let t = v.trim_right(3840 + 960);
        assert_eq!((t.start, t.length, t.offset), (3840, 960, 24_000 + 24_000));
        // pulling it out moves the offset down, down to the start of the file
        let t = v.trim_right(3840 + 1920 + 480);
        assert_eq!((t.length, t.offset), (2400, 12_000));
        let t = v.trim_right(3840 + 99_999);
        assert_eq!(t.offset, 0);
        assert_eq!(t.length, 1920 + 960);
        // pulling the left edge out plays more of the range end, until the file ends
        let t = v.trim_left(0);
        assert_eq!(t.offset, 24_000);
        assert!(t.length > 1920 && t.start < 3840);
        assert!(v.frames(t.length as i64) <= 192_000 - 24_000);
    }

    #[test]
    fn slip_moves_the_sound_inside_the_clip() {
        let v = view(false);
        // drag the sound 480 ticks later: it now starts 12 000 frames earlier in the file
        assert_eq!(v.slip(480), 12_000);
        assert_eq!(v.slip(-480), 36_000);
        assert_eq!(v.slip(99_999), 0, "clamped at the file start");
        assert_eq!(v.slip(-99_999), 191_999, "clamped inside the file");
        let r = view(true);
        assert_eq!(r.slip(480), 36_000, "a reversed clip slips the other way");
        assert_eq!(r.slip(-99_999), 0);
        assert_eq!(
            r.slip(99_999),
            192_000 - 48_000,
            "the range stays inside the file"
        );
    }

    #[test]
    fn fade_curve_points_are_exact_at_the_ends() {
        for curve in [
            FadeCurve::Linear,
            FadeCurve::Exp,
            FadeCurve::Log,
            FadeCurve::SCurve,
        ] {
            let p = fade_points(curve, 16);
            assert_eq!(p.first().unwrap(), &(0.0, 0.0));
            assert_eq!(p.last().unwrap(), &(1.0, 1.0));
            assert!(p.windows(2).all(|w| w[1].1 >= w[0].1));
        }
    }
}
