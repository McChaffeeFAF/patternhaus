//! The window manager: a pure model of FL-style floating windows (DESIGN §9).
//!
//! No egui types live here. The model owns every window's rectangle, visibility, maximized
//! state, stacking order and detach flag, and the geometry rules (snapping, clamping, minimum
//! sizes). The egui shell in [`crate::windows`] only renders this model and reports drags and
//! clicks back into it, so all of the behaviour below is unit-tested without a UI.
//!
//! Coordinates are workspace-local: `(0, 0)` is the top-left of the area the windows live in.

use ph_core::{ChannelId, ClipId, InsertId, SampleId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Magnetic snap distance in points while dragging or resizing.
pub const SNAP: f32 = 8.0;
/// Height of a window's title bar.
pub const TITLE_H: f32 = 24.0;
/// How much of a window must stay inside the workspace horizontally, so it can be grabbed.
pub const GRAB: f32 = 64.0;

#[derive(Copy, Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }
    fn overlaps_x(&self, o: &Rect, margin: f32) -> bool {
        self.x < o.right() + margin && o.x < self.right() + margin
    }
    fn overlaps_y(&self, o: &Rect, margin: f32) -> bool {
        self.y < o.bottom() + margin && o.y < self.bottom() + margin
    }
}

/// Which window edges a resize drag moves.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub const fn new(left: bool, right: bool, top: bool, bottom: bool) -> Self {
        Edges {
            left,
            right,
            top,
            bottom,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WindowId {
    Playlist,
    ChannelRack,
    PianoRoll,
    Mixer,
    History,
    Agents,
    /// One window per channel.
    Instrument(ChannelId),
    /// One window per effect slot of an insert.
    Effect(InsertId, u32),
    /// The audio editor on one registered sample (M2/T6).
    AudioEditor(SampleId),
    /// The editor of one automation clip (M2/T7).
    Automation(ClipId),
}

impl WindowId {
    /// Instrument and effect windows belong to a project object.
    pub fn is_object(self) -> bool {
        matches!(
            self,
            WindowId::Instrument(_)
                | WindowId::Effect(..)
                | WindowId::AudioEditor(_)
                | WindowId::Automation(_)
        )
    }

    /// Parses the object-window names `ui.open_panel` accepts: `instrument:<channel>` and
    /// `effect:<insert>:<slot>`.
    pub fn parse(name: &str) -> Option<WindowId> {
        if let Some(c) = name.strip_prefix("instrument:") {
            return c.parse().ok().map(|c| WindowId::Instrument(ChannelId(c)));
        }
        if let Some(s) = name.strip_prefix("audio_editor:") {
            return s.parse().ok().map(|s| WindowId::AudioEditor(SampleId(s)));
        }
        if let Some(c) = name.strip_prefix("automation:") {
            return c.parse().ok().map(|c| WindowId::Automation(ClipId(c)));
        }
        let (i, s) = name.strip_prefix("effect:")?.split_once(':')?;
        Some(WindowId::Effect(InsertId(i.parse().ok()?), s.parse().ok()?))
    }

    pub fn min_size(self) -> (f32, f32) {
        match self {
            WindowId::Playlist => (320.0, 180.0),
            WindowId::ChannelRack => (300.0, 120.0),
            WindowId::PianoRoll => (320.0, 200.0),
            WindowId::Mixer => (360.0, 220.0),
            WindowId::History | WindowId::Agents => (240.0, 140.0),
            WindowId::Instrument(_) => (280.0, 180.0),
            WindowId::Effect(..) => (240.0, 140.0),
            WindowId::AudioEditor(_) => (560.0, 320.0),
            WindowId::Automation(_) => (360.0, 220.0),
        }
    }

    fn default_size(self) -> (f32, f32) {
        match self {
            WindowId::Playlist => (900.0, 520.0),
            WindowId::ChannelRack => (640.0, 440.0),
            WindowId::PianoRoll => (880.0, 500.0),
            WindowId::Mixer => (900.0, 440.0),
            WindowId::History => (420.0, 420.0),
            WindowId::Agents => (420.0, 380.0),
            WindowId::Instrument(_) => (500.0, 480.0),
            WindowId::Effect(..) => (420.0, 360.0),
            WindowId::AudioEditor(_) => (900.0, 460.0),
            WindowId::Automation(_) => (680.0, 340.0),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    /// The floating rectangle. While maximized this is the rectangle to restore.
    pub rect: Rect,
    pub visible: bool,
    pub maximized: bool,
    /// Stacking order; a larger value is further in front.
    pub z: u64,
    /// Shown in its own OS window (multi-viewport) instead of the workspace.
    pub detached: bool,
}

/// The docked browser at the left edge.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserDock {
    pub visible: bool,
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    area: Rect,
    windows: BTreeMap<WindowId, WindowState>,
    next_z: u64,
    pub browser: BrowserDock,
}

impl Workspace {
    /// The layout a fresh install opens with: the playlist maximized as the background, the
    /// channel rack floating at the upper left, everything else hidden.
    pub fn new(area: Rect) -> Self {
        let mut ws = Workspace {
            area: Rect::new(0.0, 0.0, area.w, area.h),
            windows: BTreeMap::new(),
            next_z: 1,
            browser: BrowserDock {
                visible: true,
                width: 240.0,
            },
        };
        for id in [
            WindowId::Playlist,
            WindowId::ChannelRack,
            WindowId::PianoRoll,
            WindowId::Mixer,
            WindowId::History,
            WindowId::Agents,
        ] {
            let rect = ws.default_rect(id);
            let z = ws.bump();
            ws.windows.insert(
                id,
                WindowState {
                    rect,
                    visible: false,
                    maximized: false,
                    z,
                    detached: false,
                },
            );
        }
        let pl = ws.windows.get_mut(&WindowId::Playlist).unwrap();
        pl.visible = true;
        pl.maximized = true;
        ws.windows.get_mut(&WindowId::ChannelRack).unwrap().visible = true;
        ws.show(WindowId::ChannelRack);
        ws
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    fn bump(&mut self) -> u64 {
        let z = self.next_z;
        self.next_z += 1;
        z
    }

    // ---- queries ---------------------------------------------------------------------------

    pub fn get(&self, id: WindowId) -> Option<&WindowState> {
        self.windows.get(&id)
    }

    pub fn is_visible(&self, id: WindowId) -> bool {
        self.windows.get(&id).is_some_and(|w| w.visible)
    }

    /// Where the window is drawn: the whole workspace when maximized, else its own rectangle.
    pub fn effective_rect(&self, id: WindowId) -> Option<Rect> {
        self.windows
            .get(&id)
            .map(|w| if w.maximized { self.area } else { w.rect })
    }

    /// Visible windows from the back to the front.
    pub fn visible_in_z_order(&self) -> Vec<WindowId> {
        let mut v: Vec<_> = self
            .windows
            .iter()
            .filter(|(_, w)| w.visible)
            .map(|(id, w)| (w.z, *id))
            .collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// The window at the front.
    pub fn front(&self) -> Option<WindowId> {
        self.visible_in_z_order().pop()
    }

    pub fn is_front(&self, id: WindowId) -> bool {
        self.front() == Some(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.keys().copied()
    }

    // ---- show, hide, focus, toggle ----------------------------------------------------------

    /// Creates the window's state the first time it is wanted, at a cascaded default position
    /// (or its remembered one).
    fn ensure(&mut self, id: WindowId) {
        if self.windows.contains_key(&id) {
            return;
        }
        let rect = self.default_rect(id);
        let z = self.bump();
        self.windows.insert(
            id,
            WindowState {
                rect,
                visible: false,
                maximized: false,
                z,
                detached: false,
            },
        );
    }

    /// Show the window and bring it to the front.
    pub fn show(&mut self, id: WindowId) {
        self.ensure(id);
        let z = self.bump();
        let w = self.windows.get_mut(&id).unwrap();
        w.visible = true;
        w.z = z;
        self.clamp_one(id);
    }

    pub fn hide(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.visible = false;
        }
    }

    /// Bring a visible window to the front. Does nothing for hidden or unknown windows.
    pub fn focus(&mut self, id: WindowId) {
        if self.is_visible(id) && !self.is_front(id) {
            let z = self.bump();
            self.windows.get_mut(&id).unwrap().z = z;
        }
    }

    /// The F-key semantics: hidden -> show and front; visible but behind -> front; front ->
    /// hide. Returns whether the window is visible afterwards.
    pub fn toggle(&mut self, id: WindowId) -> bool {
        if !self.is_visible(id) {
            self.show(id);
            true
        } else if self.is_front(id) {
            self.hide(id);
            false
        } else {
            self.focus(id);
            true
        }
    }

    pub fn toggle_browser(&mut self) -> bool {
        self.browser.visible = !self.browser.visible;
        self.browser.visible
    }

    pub fn set_browser_width(&mut self, w: f32) {
        self.browser.width = w.clamp(140.0, 640.0);
    }

    pub fn maximize_toggle(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.maximized = !w.maximized;
        }
        self.focus(id);
        self.clamp_one(id);
    }

    /// Per-window options: put a window back at its default size and place, un-maximized.
    pub fn reset_window(&mut self, id: WindowId) {
        let r = self.default_rect(id);
        if let Some(w) = self.windows.get_mut(&id) {
            w.rect = r;
            w.maximized = false;
            w.detached = false;
        }
        self.focus(id);
        self.clamp_one(id);
    }

    pub fn set_detached(&mut self, id: WindowId, detached: bool) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.detached = detached;
        }
    }

    /// Escape: close the front-most instrument or effect window in z-order, even when a
    /// panel (the rack, say) has been clicked since and is in front of it. Returns the window
    /// that was closed.
    /// Moves a window's place, size and stacking to another id (the audio editor follows its
    /// sample to the file an edit wrote). Does nothing when `old` is unknown.
    pub fn replace(&mut self, old: WindowId, new: WindowId) {
        if old == new {
            return;
        }
        if let Some(w) = self.windows.remove(&old) {
            self.windows.insert(new, w);
        }
    }

    /// Forgets a window (its object is gone).
    pub fn remove(&mut self, id: WindowId) {
        self.windows.remove(&id);
    }

    pub fn close_front_object(&mut self) -> Option<WindowId> {
        let f = self
            .visible_in_z_order()
            .into_iter()
            .rev()
            .find(|id| id.is_object())?;
        self.hide(f);
        Some(f)
    }

    // ---- geometry --------------------------------------------------------------------------

    /// Move a window towards `(x, y)` (its desired top-left), snapping to the workspace edges
    /// and to other windows within [`SNAP`], then keeping it reachable. A maximized window
    /// does not move.
    pub fn drag_move(&mut self, id: WindowId, x: f32, y: f32) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        if w.maximized {
            return;
        }
        let mut r = Rect::new(x, y, w.rect.w, w.rect.h);
        let (xs, ys) = self.snap_lines(id, &r);
        if let Some(d) = nearest_shift(&[r.x, r.right()], &xs) {
            r.x += d;
        }
        if let Some(d) = nearest_shift(&[r.y, r.bottom()], &ys) {
            r.y += d;
        }
        self.windows.get_mut(&id).unwrap().rect = r;
        self.clamp_one(id);
    }

    /// Resize towards the desired rectangle `want`; only the edges in `edges` are meant to
    /// move. Those edges snap, then the minimum size is enforced against the opposite edge.
    pub fn drag_resize(&mut self, id: WindowId, edges: Edges, want: Rect) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        if w.maximized {
            return;
        }
        let (mnw, mnh) = id.min_size();
        let (xs, ys) = self.snap_lines(id, &want);
        let (mut l, mut r, mut t, mut b) = (want.x, want.right(), want.y, want.bottom());
        if edges.left
            && let Some(d) = nearest_shift(&[l], &xs)
        {
            l += d;
        }
        if edges.right
            && let Some(d) = nearest_shift(&[r], &xs)
        {
            r += d;
        }
        if edges.top
            && let Some(d) = nearest_shift(&[t], &ys)
        {
            t += d;
        }
        if edges.bottom
            && let Some(d) = nearest_shift(&[b], &ys)
        {
            b += d;
        }
        if r - l < mnw {
            if edges.left && !edges.right {
                l = r - mnw;
            } else {
                r = l + mnw;
            }
        }
        if b - t < mnh {
            if edges.top && !edges.bottom {
                t = b - mnh;
            } else {
                b = t + mnh;
            }
        }
        self.windows.get_mut(&id).unwrap().rect = Rect::new(l, t, r - l, b - t);
        self.clamp_one(id);
    }

    /// Lines other things offer to snap to: the workspace edges, and the edges of every other
    /// visible, non-maximized window that is near enough to `r` along the other axis.
    fn snap_lines(&self, id: WindowId, r: &Rect) -> (Vec<f32>, Vec<f32>) {
        let a = self.area;
        let mut xs = vec![a.x, a.right()];
        let mut ys = vec![a.y, a.bottom()];
        for (oid, o) in &self.windows {
            if *oid == id || !o.visible || o.maximized || o.detached {
                continue;
            }
            if r.overlaps_y(&o.rect, SNAP) {
                xs.push(o.rect.x);
                xs.push(o.rect.right());
            }
            if r.overlaps_x(&o.rect, SNAP) {
                ys.push(o.rect.y);
                ys.push(o.rect.bottom());
            }
        }
        (xs, ys)
    }

    /// Resize the workspace (the window was resized, the browser moved) and keep every window
    /// reachable.
    pub fn set_area(&mut self, area: Rect) {
        self.area = Rect::new(0.0, 0.0, area.w, area.h);
        let ids: Vec<_> = self.windows.keys().copied().collect();
        for id in ids {
            self.clamp_one(id);
        }
    }

    /// Keep every window's title bar inside the workspace (and at least [`GRAB`] wide).
    pub fn clamp_to_workspace(&mut self) {
        self.set_area(self.area);
    }

    fn clamp_one(&mut self, id: WindowId) {
        let a = self.area;
        let (mnw, mnh) = id.min_size();
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        let r = &mut w.rect;
        // never larger than the workspace (but never below the minimum either)
        r.w = r.w.min(a.w.max(mnw)).max(mnw);
        r.h = r.h.min(a.h.max(mnh)).max(mnh);
        let grab = GRAB.min(r.w);
        r.x =
            r.x.clamp(a.x + grab - r.w, (a.right() - grab).max(a.x + grab - r.w));
        r.y = r.y.clamp(a.y, (a.bottom() - TITLE_H).max(a.y));
    }

    fn default_rect(&self, id: WindowId) -> Rect {
        let a = self.area;
        let (dw, dh) = id.default_size();
        let (mnw, mnh) = id.min_size();
        let w = dw.min(a.w * 0.92).max(mnw);
        let h = dh.min(a.h * 0.88).max(mnh);
        let (x, y) = match id {
            WindowId::Playlist => (a.w * 0.04, a.h * 0.05),
            WindowId::ChannelRack => (24.0, 24.0),
            WindowId::PianoRoll => (a.w * 0.18, a.h * 0.16),
            WindowId::Mixer => (a.w * 0.04, a.h * 0.42),
            WindowId::History => (a.w * 0.58, a.h * 0.1),
            WindowId::Agents => (a.w * 0.5, a.h * 0.2),
            WindowId::AudioEditor(_) => {
                // centred over the workspace, then cascaded past the object windows
                let n = self.windows.keys().filter(|k| k.is_object()).count() as f32;
                (
                    ((a.w - w) * 0.5).max(0.0) + 28.0 * (n % 6.0),
                    a.h * 0.16 + 28.0 * (n % 6.0),
                )
            }
            WindowId::Instrument(_) | WindowId::Effect(..) | WindowId::Automation(_) => {
                // cascade past the object windows already known
                let n = self.windows.keys().filter(|k| k.is_object()).count() as f32;
                (a.w * 0.34 + 28.0 * (n % 8.0), 40.0 + 28.0 * (n % 8.0))
            }
        };
        Rect::new(x, y, w, h)
    }

    /// Where a window's title text should start (an offset from the window's left edge), so
    /// it is not hidden under a window in front of this one. Picks the first stretch of the
    /// title bar, left of `reserved_right` (the buttons), that no front window covers and
    /// that is `text_w` wide; failing that the widest stretch; failing that the default.
    pub fn title_start(&self, id: WindowId, text_w: f32, reserved_right: f32) -> f32 {
        const PAD: f32 = 10.0;
        let (Some(r), Some(me)) = (self.effective_rect(id), self.windows.get(&id)) else {
            return PAD;
        };
        let (lo, hi) = (r.x + PAD, r.right() - reserved_right);
        let mut covers: Vec<(f32, f32)> = self
            .windows
            .iter()
            .filter(|(oid, o)| **oid != id && o.visible && !o.detached && o.z > me.z)
            .filter_map(|(oid, _)| self.effective_rect(*oid))
            .filter(|o| o.y < r.y + TITLE_H && o.bottom() > r.y && o.x < hi && o.right() > lo)
            .map(|o| (o.x.max(lo), o.right().min(hi)))
            .collect();
        covers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut runs = Vec::new();
        let mut at = lo;
        for (a, b) in covers {
            if a > at {
                runs.push((at, a));
            }
            at = at.max(b);
        }
        if hi > at {
            runs.push((at, hi));
        }
        let pick = runs
            .iter()
            .find(|(a, b)| b - a >= text_w)
            .or_else(|| {
                runs.iter()
                    .filter(|(a, b)| b - a >= 24.0)
                    .max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0)))
            })
            .map_or(lo, |(a, _)| *a);
        pick - r.x
    }

    // ---- effect identity -------------------------------------------------------------------

    /// Carry the state of an insert's effect windows to the slots their effects moved to.
    /// `map` lists `(old slot, new slot)`, with `None` for an effect that is gone (its window
    /// is forgotten). The move is simultaneous, so swaps work, and a window keeps its
    /// rectangle, stacking and visibility: the window follows its effect.
    pub fn remap_effects(&mut self, insert: InsertId, map: &[(u32, Option<u32>)]) {
        let mut moved = Vec::new();
        for (old, new) in map {
            if let Some(st) = self.windows.remove(&WindowId::Effect(insert, *old))
                && let Some(n) = new
            {
                moved.push((WindowId::Effect(insert, *n), st));
            }
        }
        for (id, st) in moved {
            self.windows.insert(id, st);
        }
    }

    /// The slot map for moving the effect at `from` to `to` (the ones in between shift by one).
    pub fn move_map(from: u32, to: u32) -> Vec<(u32, Option<u32>)> {
        let (lo, hi) = (from.min(to), from.max(to));
        (lo..=hi)
            .map(|s| {
                let n = if s == from {
                    to
                } else if from < to {
                    s - 1
                } else {
                    s + 1
                };
                (s, Some(n))
            })
            .collect()
    }

    /// The slot map for removing the effect at `slot` from a list of `len` effects.
    pub fn remove_map(slot: u32, len: u32) -> Vec<(u32, Option<u32>)> {
        (slot..len)
            .map(|s| (s, if s == slot { None } else { Some(s - 1) }))
            .collect()
    }

    // ---- project snapshots -----------------------------------------------------------------

    /// Forget automation editors whose clip is gone. Returns the ids that were removed.
    pub fn prune_clips(&mut self, clip_exists: impl Fn(ClipId) -> bool) -> Vec<WindowId> {
        let gone: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| matches!(id, WindowId::Automation(c) if !clip_exists(*c)))
            .collect();
        for id in &gone {
            self.windows.remove(id);
        }
        gone
    }

    /// Forget instrument and effect windows whose object no longer exists (a deleted channel,
    /// a removed effect), closing them. Returns the ids that were removed.
    pub fn prune_objects(
        &mut self,
        channel_exists: impl Fn(ChannelId) -> bool,
        effect_exists: impl Fn(InsertId, u32) -> bool,
    ) -> Vec<WindowId> {
        let gone: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| match id {
                WindowId::Instrument(c) => !channel_exists(*c),
                WindowId::Effect(i, s) => !effect_exists(*i, *s),
                _ => false,
            })
            .collect();
        for id in &gone {
            self.windows.remove(id);
        }
        gone
    }
}

/// Where each effect of an insert went when its list changed from `old` to `new` (both are the
/// effects' kinds, in slot order). An effect is matched by insert and kind; the position is
/// used only to choose among several effects of the same kind (the same slot first, else the
/// nearest). `None` means the effect is gone. Returns only the slots that changed.
pub fn match_effects(old: &[String], new: &[String]) -> Vec<(u32, Option<u32>)> {
    let mut to: Vec<Option<usize>> = vec![None; old.len()];
    let mut taken = vec![false; new.len()];
    for i in 0..old.len().min(new.len()) {
        if old[i] == new[i] {
            to[i] = Some(i);
            taken[i] = true;
        }
    }
    let mut pairs: Vec<(usize, usize, usize)> = Vec::new();
    for (i, o) in old.iter().enumerate() {
        if to[i].is_some() {
            continue;
        }
        for (j, n) in new.iter().enumerate() {
            if !taken[j] && n == o {
                pairs.push((i.abs_diff(j), i, j));
            }
        }
    }
    pairs.sort();
    for (_, i, j) in pairs {
        if to[i].is_none() && !taken[j] {
            to[i] = Some(j);
            taken[j] = true;
        }
    }
    to.iter()
        .enumerate()
        .filter(|(i, t)| **t != Some(*i))
        .map(|(i, t)| (i as u32, t.map(|j| j as u32)))
        .collect()
}

/// The signed shift that brings the closest of `edges` onto the closest of `lines`, when that
/// is within [`SNAP`].
fn nearest_shift(edges: &[f32], lines: &[f32]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for e in edges {
        for l in lines {
            let d = l - e;
            if d.abs() <= SNAP && best.is_none_or(|b| d.abs() < b.abs()) {
                best = Some(d);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> Workspace {
        Workspace::new(Rect::new(0.0, 0.0, 1600.0, 900.0))
    }

    #[test]
    fn defaults_playlist_background_rack_floating_rest_hidden() {
        let w = ws();
        let pl = w.get(WindowId::Playlist).unwrap();
        assert!(pl.visible && pl.maximized);
        assert_eq!(w.effective_rect(WindowId::Playlist), Some(w.area()));
        assert!(w.is_visible(WindowId::ChannelRack));
        for id in [
            WindowId::PianoRoll,
            WindowId::Mixer,
            WindowId::History,
            WindowId::Agents,
        ] {
            assert!(!w.is_visible(id), "{id:?}");
        }
        assert!(w.is_front(WindowId::ChannelRack));
        assert!(w.browser.visible);
    }

    #[test]
    fn toggle_semantics() {
        let mut w = ws();
        let roll = WindowId::PianoRoll;
        assert!(w.toggle(roll), "hidden -> shown");
        assert!(w.is_front(roll));
        w.focus(WindowId::ChannelRack);
        assert!(!w.is_front(roll));
        assert!(w.toggle(roll), "visible but behind -> raised, not hidden");
        assert!(w.is_front(roll));
        assert!(!w.toggle(roll), "in front -> hidden");
        assert!(!w.is_visible(roll));
        assert!(w.toggle(roll), "and shown again");
    }

    #[test]
    fn z_order_follows_focus() {
        let mut w = ws();
        w.show(WindowId::Mixer);
        w.show(WindowId::PianoRoll);
        assert_eq!(
            w.visible_in_z_order(),
            vec![
                WindowId::Playlist,
                WindowId::ChannelRack,
                WindowId::Mixer,
                WindowId::PianoRoll
            ]
        );
        w.focus(WindowId::Playlist);
        assert!(w.is_front(WindowId::Playlist));
        w.focus(WindowId::Mixer);
        assert_eq!(w.front(), Some(WindowId::Mixer));
        // focusing a hidden window does nothing
        w.focus(WindowId::History);
        assert!(!w.is_visible(WindowId::History));
        // hiding a front window hands the front to the next one
        w.hide(WindowId::Mixer);
        assert_eq!(w.front(), Some(WindowId::Playlist));
    }

    #[test]
    fn move_snaps_to_workspace_edges_within_eight_points() {
        let mut w = ws();
        let id = WindowId::ChannelRack;
        w.drag_move(id, 5.0, 6.0);
        let r = w.get(id).unwrap().rect;
        assert_eq!((r.x, r.y), (0.0, 0.0));
        w.drag_move(id, 100.0, 100.0);
        assert_eq!(w.get(id).unwrap().rect.x, 100.0, "far from any edge: free");
        // right edge snaps to the workspace's right edge
        let width = w.get(id).unwrap().rect.w;
        w.drag_move(id, 1600.0 - width - 7.0, 100.0);
        assert_eq!(w.get(id).unwrap().rect.right(), 1600.0);
        w.drag_move(id, 1600.0 - width - 9.0, 100.0);
        assert_eq!(w.get(id).unwrap().rect.right(), 1600.0 - 9.0, "9 pt is out");
    }

    #[test]
    fn move_snaps_to_other_windows() {
        let mut w = ws();
        w.show(WindowId::Mixer);
        let m = w.get(WindowId::Mixer).unwrap().rect;
        // put the rack just right of the mixer, a bit above it
        w.drag_move(WindowId::ChannelRack, m.right() + 5.0, m.y - 200.0);
        let r = w.get(WindowId::ChannelRack).unwrap().rect;
        assert_eq!(r.x, m.right(), "left edge sticks to the mixer's right edge");
        // vertical: the rack's top edge sticks to the mixer's bottom edge
        w.drag_move(WindowId::ChannelRack, m.x + 30.0, m.bottom() + 6.0);
        let r = w.get(WindowId::ChannelRack).unwrap().rect;
        assert_eq!(r.y, m.bottom());
        // a window that is far away along the other axis offers no edge
        let mut w2 = ws();
        w2.show(WindowId::Mixer);
        w2.drag_move(WindowId::Mixer, 700.0, 700.0);
        w2.drag_move(WindowId::ChannelRack, 703.0, 20.0);
        assert_eq!(w2.get(WindowId::ChannelRack).unwrap().rect.x, 703.0);
    }

    #[test]
    fn resize_snaps_enforces_minimums_and_keeps_the_far_edge() {
        let mut w = ws();
        let id = WindowId::ChannelRack;
        w.drag_move(id, 100.0, 100.0);
        let r0 = w.get(id).unwrap().rect;
        // dragging the right edge to 7 pt short of the workspace edge snaps to it
        w.drag_resize(
            id,
            Edges::new(false, true, false, false),
            Rect::new(r0.x, r0.y, 1600.0 - 7.0 - r0.x, r0.h),
        );
        assert_eq!(w.get(id).unwrap().rect.right(), 1600.0);
        // too small: the left edge is dragged past the right, the right edge stays put
        let r1 = w.get(id).unwrap().rect;
        w.drag_resize(
            id,
            Edges::new(true, false, false, false),
            Rect::new(r1.right() - 10.0, r1.y, 10.0, r1.h),
        );
        let r2 = w.get(id).unwrap().rect;
        assert_eq!(r2.w, id.min_size().0);
        assert_eq!(r2.right(), r1.right());
        // top edge: height floor against the bottom edge
        w.drag_resize(
            id,
            Edges::new(false, false, true, false),
            Rect::new(r2.x, r2.bottom() - 5.0, r2.w, 5.0),
        );
        let r3 = w.get(id).unwrap().rect;
        assert_eq!(r3.h, id.min_size().1);
        assert_eq!(r3.bottom(), r2.bottom());
    }

    #[test]
    fn clamping_keeps_the_title_bar_reachable_after_the_workspace_shrinks() {
        let mut w = ws();
        w.show(WindowId::Mixer);
        w.drag_move(WindowId::Mixer, 1200.0, 800.0);
        w.set_area(Rect::new(0.0, 0.0, 800.0, 400.0));
        for id in w.ids().collect::<Vec<_>>() {
            let r = w.get(id).unwrap().rect;
            assert!(r.y >= 0.0 && r.y <= 400.0 - TITLE_H, "{id:?} {r:?}");
            assert!(r.x <= 800.0 - GRAB, "{id:?} {r:?}");
            assert!(r.right() >= GRAB, "{id:?} {r:?}");
            assert!(r.w <= 800.0 && r.h <= 400.0);
        }
        // dragging far off the top-left cannot lose the window either
        w.drag_move(WindowId::Mixer, -5000.0, -5000.0);
        let r = w.get(WindowId::Mixer).unwrap().rect;
        assert!(r.y >= 0.0 && r.right() >= GRAB);
        w.drag_move(WindowId::Mixer, 9000.0, 9000.0);
        let r = w.get(WindowId::Mixer).unwrap().rect;
        assert!(r.y <= 400.0 - TITLE_H && r.x <= 800.0 - GRAB);
    }

    #[test]
    fn maximize_and_restore() {
        let mut w = ws();
        let id = WindowId::ChannelRack;
        w.drag_move(id, 150.0, 90.0);
        let before = w.get(id).unwrap().rect;
        w.maximize_toggle(id);
        assert_eq!(w.effective_rect(id), Some(w.area()));
        assert!(w.is_front(id), "maximizing raises the window");
        // a maximized window does not drag
        w.drag_move(id, 400.0, 400.0);
        assert_eq!(w.get(id).unwrap().rect, before);
        // it follows the workspace while maximized
        w.set_area(Rect::new(0.0, 0.0, 1000.0, 700.0));
        assert_eq!(
            w.effective_rect(id),
            Some(Rect::new(0.0, 0.0, 1000.0, 700.0))
        );
        w.maximize_toggle(id);
        assert_eq!(w.effective_rect(id), Some(before));
    }

    #[test]
    fn object_windows_remember_their_rect_by_object() {
        let mut w = ws();
        let a = WindowId::Instrument(ChannelId(1));
        let b = WindowId::Instrument(ChannelId(2));
        w.show(a);
        w.show(b);
        assert_ne!(
            w.get(a).unwrap().rect,
            w.get(b).unwrap().rect,
            "cascaded, not stacked exactly"
        );
        w.drag_move(a, 333.0, 222.0);
        w.hide(a);
        w.show(a);
        let r = w.get(a).unwrap().rect;
        assert_eq!((r.x, r.y), (333.0, 222.0));
        // effect windows are keyed by (insert, slot)
        let fx = WindowId::Effect(InsertId(4), 1);
        w.show(fx);
        assert!(w.is_visible(fx) && !w.is_visible(WindowId::Effect(InsertId(4), 0)));
    }

    #[test]
    fn deleting_objects_closes_their_windows() {
        let mut w = ws();
        w.show(WindowId::Instrument(ChannelId(1)));
        w.show(WindowId::Instrument(ChannelId(2)));
        w.show(WindowId::Effect(InsertId(4), 0));
        w.show(WindowId::Effect(InsertId(4), 1));
        let gone = w.prune_objects(|c| c == ChannelId(2), |i, s| i == InsertId(4) && s == 0);
        assert_eq!(
            gone,
            vec![
                WindowId::Instrument(ChannelId(1)),
                WindowId::Effect(InsertId(4), 1)
            ]
        );
        assert!(w.get(WindowId::Instrument(ChannelId(1))).is_none());
        assert!(w.is_visible(WindowId::Instrument(ChannelId(2))));
        assert!(w.is_visible(WindowId::Effect(InsertId(4), 0)));
        // fixed windows are never pruned
        assert!(w.is_visible(WindowId::Playlist));
    }

    #[test]
    fn escape_closes_the_front_most_object_window_in_z_order() {
        let mut w = ws();
        assert_eq!(w.close_front_object(), None, "the rack is not an object");
        let a = WindowId::Instrument(ChannelId(1));
        let b = WindowId::Effect(InsertId(4), 0);
        w.show(a);
        w.show(b);
        // a panel in front of both does not shield them
        w.focus(WindowId::ChannelRack);
        assert_eq!(w.close_front_object(), Some(b));
        assert_eq!(w.close_front_object(), Some(a));
        assert_eq!(w.close_front_object(), None);
        assert!(w.is_visible(WindowId::ChannelRack));
        assert!(w.is_visible(WindowId::Playlist));
    }

    #[test]
    fn browser_dock_toggles_and_clamps_its_width() {
        let mut w = ws();
        assert!(!w.toggle_browser());
        assert!(w.toggle_browser());
        w.set_browser_width(10.0);
        assert_eq!(w.browser.width, 140.0);
        w.set_browser_width(5000.0);
        assert_eq!(w.browser.width, 640.0);
    }

    #[test]
    fn round_trips_through_ron() {
        let mut w = ws();
        w.show(WindowId::Mixer);
        w.show(WindowId::Instrument(ChannelId(3)));
        w.show(WindowId::Effect(InsertId(4), 2));
        w.drag_move(WindowId::Mixer, 321.0, 123.0);
        w.maximize_toggle(WindowId::Agents);
        w.set_detached(WindowId::History, true);
        w.browser.width = 301.0;
        let text = ron::ser::to_string_pretty(&w, ron::ser::PrettyConfig::default()).unwrap();
        let back: Workspace = ron::from_str(&text).unwrap();
        assert_eq!(back, w);
        assert_eq!(back.visible_in_z_order(), w.visible_in_z_order());
    }

    #[test]
    fn effect_windows_follow_their_effect() {
        let mut w = ws();
        let i = InsertId(4);
        let fx = |s| WindowId::Effect(i, s);
        w.show(fx(0));
        w.drag_move(fx(0), 300.0, 200.0);
        w.show(fx(1));
        w.drag_move(fx(1), 700.0, 300.0);
        // move slot 0 down to 1: the windows swap slots and keep their rectangles
        w.remap_effects(i, &Workspace::move_map(0, 1));
        assert_eq!(w.get(fx(1)).unwrap().rect.x, 300.0);
        assert_eq!(w.get(fx(0)).unwrap().rect.x, 700.0);
        // removing slot 0: its window goes, the other shifts down
        w.remap_effects(i, &Workspace::remove_map(0, 2));
        assert_eq!(w.get(fx(0)).unwrap().rect.x, 300.0);
        assert!(w.get(fx(1)).is_none());
    }

    #[test]
    fn move_and_remove_maps() {
        assert_eq!(
            Workspace::move_map(0, 2),
            vec![(0, Some(2)), (1, Some(0)), (2, Some(1))]
        );
        assert_eq!(Workspace::move_map(2, 1), vec![(1, Some(2)), (2, Some(1))]);
        assert_eq!(
            Workspace::remove_map(1, 4),
            vec![(1, None), (2, Some(1)), (3, Some(2))]
        );
    }

    #[test]
    fn title_text_avoids_windows_in_front() {
        let mut w = ws();
        let a = WindowId::Effect(InsertId(4), 0);
        let b = WindowId::Instrument(ChannelId(1));
        w.show(a);
        w.drag_move(a, 800.0, 100.0);
        assert_eq!(w.title_start(a, 120.0, 68.0), 10.0, "nothing in front");
        w.show(b);
        w.drag_move(b, 480.0, 90.0);
        let rb = w.get(b).unwrap().rect;
        let ra = w.get(a).unwrap().rect;
        assert!(
            rb.right() > ra.x + 10.0 && rb.right() < ra.right() - 200.0,
            "setup"
        );
        let x = w.title_start(a, 120.0, 68.0);
        assert!(
            ra.x + x >= rb.right(),
            "the title starts right of the cover: {x}"
        );
        // a window behind never matters
        assert_eq!(w.title_start(b, 120.0, 68.0), 10.0);
    }

    #[test]
    fn effects_are_matched_by_kind_then_position() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // unchanged
        assert!(match_effects(&v(&["eq", "comp"]), &v(&["eq", "comp"])).is_empty());
        // an agent reorders: matched by kind, not by slot
        assert_eq!(
            match_effects(&v(&["eq", "comp", "rev"]), &v(&["rev", "eq", "comp"])),
            vec![(0, Some(1)), (1, Some(2)), (2, Some(0))]
        );
        // an effect is removed: it is gone, the ones after it shift
        assert_eq!(
            match_effects(&v(&["eq", "comp", "rev"]), &v(&["eq", "rev"])),
            vec![(1, None), (2, Some(1))]
        );
        // one added at the front: everything shifts by one
        assert_eq!(
            match_effects(&v(&["eq"]), &v(&["rev", "eq"])),
            vec![(0, Some(1))]
        );
        // the same kind twice: position decides (the same slot wins, else the nearest)
        assert_eq!(
            match_effects(&v(&["eq", "comp", "eq"]), &v(&["comp", "eq", "eq"])),
            vec![(0, Some(1)), (1, Some(0))]
        );
        assert_eq!(
            match_effects(&v(&["eq", "eq"]), &v(&["eq"])),
            vec![(1, None)]
        );
    }
}
