//! State and arithmetic of the audio editor window (M2/T6), apart from drawing.
//!
//! The editor works on one registered sample and never changes a file: each operation is an
//! `editor.apply` command whose result is a new file (see `ph_commands`' `editor` module). So
//! what lives here is only the *view*: zoom and scroll in frames, the selection, the caret,
//! the slice markers the person is placing, and the sample ids this window showed before (so
//! an undo walks the window back along with the project).

use ph_core::{FadeCurve, SampleId};

/// Smallest zoom: frames per pixel (a sample is 16 px wide).
pub const MIN_FPP: f64 = 1.0 / 16.0;

/// A pointer drag on the canvas.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EDrag {
    None,
    /// Selecting from this frame.
    Select(u64),
    /// Moving slice marker `i`.
    Marker(usize),
}

/// The view and the pending work of one audio editor window.
#[derive(Clone, Debug)]
pub struct EditorState {
    /// Frame at the left edge of the waveform.
    pub x0: f64,
    /// Frames per pixel.
    pub fpp: f64,
    /// The selection, `start..end` in frames (end exclusive).
    pub sel: Option<(u64, u64)>,
    /// The caret: where a paste goes and an audition starts when nothing is selected.
    pub caret: u64,
    /// Slice start points after the implicit one at frame 0, ascending.
    pub markers: Vec<u64>,
    /// The slicer controls are shown.
    pub slicer: bool,
    pub sensitivity: f32,
    pub write_pattern: bool,
    /// Key of the first slice (60 = C4).
    pub root_key: u8,
    /// Normalise target in dBFS, and gain in dB.
    pub norm_db: f64,
    pub gain_db: f64,
    pub fade_curve: FadeCurve,
    /// Earlier samples this window showed, most recent last.
    pub prev: Vec<SampleId>,
    /// An audition in progress: (app time at start, first frame, end frame).
    pub playing: Option<(f64, u64, u64)>,
    pub drag: EDrag,
    /// The view has been fitted to the window once.
    pub fitted: bool,
    /// The canvas as last drawn (screen coordinates), for tests.
    pub canvas: egui::Rect,
    /// The marker lane as last drawn.
    pub marker_lane: egui::Rect,
    /// Length of the sample in frames, as last drawn.
    pub frames: u64,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            x0: 0.0,
            fpp: 100.0,
            sel: None,
            caret: 0,
            markers: Vec::new(),
            slicer: false,
            sensitivity: 0.5,
            write_pattern: true,
            root_key: 36,
            norm_db: 0.0,
            gain_db: 0.0,
            fade_curve: FadeCurve::Linear,
            prev: Vec::new(),
            playing: None,
            drag: EDrag::None,
            fitted: false,
            canvas: egui::Rect::NOTHING,
            marker_lane: egui::Rect::NOTHING,
            frames: 0,
        }
    }
}

impl EditorState {
    /// Screen x (relative to the canvas) of a frame.
    pub fn x_of(&self, frame: f64) -> f32 {
        ((frame - self.x0) / self.fpp) as f32
    }

    /// The frame at canvas x.
    pub fn frame_at(&self, x: f32) -> f64 {
        self.x0 + x as f64 * self.fpp
    }

    /// Fits the whole sample into `width` pixels.
    pub fn fit(&mut self, width: f32, frames: u64) {
        self.fpp = (frames.max(1) as f64 / width.max(1.0) as f64).max(MIN_FPP);
        self.x0 = 0.0;
        self.fitted = true;
        self.frames = frames;
    }

    /// Fits the selection (or the whole sample when nothing is selected).
    pub fn fit_selection(&mut self, width: f32, frames: u64) {
        match self.sel {
            Some((a, b)) if b > a => {
                self.fpp = ((b - a) as f64 / width.max(1.0) as f64).max(MIN_FPP);
                self.x0 = a as f64;
            }
            _ => self.fit(width, frames),
        }
    }

    /// Zooms by `factor` (above 1 = in) keeping the frame under `anchor_x` still.
    pub fn zoom(&mut self, factor: f64, anchor_x: f32, width: f32, frames: u64) {
        let at = self.frame_at(anchor_x);
        let max_fpp = (frames.max(1) as f64 / width.max(1.0) as f64).max(MIN_FPP);
        self.fpp = (self.fpp / factor).clamp(MIN_FPP, max_fpp);
        self.x0 = at - anchor_x as f64 * self.fpp;
        self.clamp(width, frames);
    }

    /// Scrolls by `px` pixels.
    pub fn pan(&mut self, px: f32, width: f32, frames: u64) {
        self.x0 += px as f64 * self.fpp;
        self.clamp(width, frames);
    }

    /// Keeps the view over the sample.
    pub fn clamp(&mut self, width: f32, frames: u64) {
        let visible = width as f64 * self.fpp;
        let max_x0 = (frames as f64 - visible).max(0.0);
        self.x0 = self.x0.clamp(0.0, max_x0);
    }

    /// Selects `a..b` (either order), clamped to the sample; a selection of no length clears it.
    pub fn select(&mut self, a: u64, b: u64, frames: u64) {
        let (lo, hi) = (a.min(b).min(frames), a.max(b).min(frames));
        self.sel = (hi > lo).then_some((lo, hi));
        self.caret = lo;
    }

    /// The selection, or the whole sample.
    pub fn range_or_all(&self, frames: u64) -> (u64, u64) {
        self.sel.unwrap_or((0, frames))
    }

    // ---- slice markers ---------------------------------------------------------------------

    /// Adds a marker at `frame` (not 0, which is the implicit first slice start). Returns its
    /// index, or the index of the marker already within `min_gap` frames.
    pub fn add_marker(&mut self, frame: u64, frames: u64, min_gap: u64) -> Option<usize> {
        if frame == 0 || frame >= frames {
            return None;
        }
        if let Some(i) = self
            .markers
            .iter()
            .position(|m| m.abs_diff(frame) <= min_gap)
        {
            return Some(i);
        }
        let i = self.markers.partition_point(|m| *m < frame);
        self.markers.insert(i, frame);
        Some(i)
    }

    /// Moves marker `i` to `frame`, never past its neighbours or the ends. Returns whether it
    /// moved.
    pub fn move_marker(&mut self, i: usize, frame: u64, frames: u64) -> bool {
        let Some(&cur) = self.markers.get(i) else {
            return false;
        };
        let lo = if i == 0 { 1 } else { self.markers[i - 1] + 1 };
        let hi = self
            .markers
            .get(i + 1)
            .map_or(frames.saturating_sub(1), |n| n.saturating_sub(1));
        if lo > hi {
            return false;
        }
        let to = frame.clamp(lo, hi);
        self.markers[i] = to;
        to != cur
    }

    pub fn delete_marker(&mut self, i: usize) {
        if i < self.markers.len() {
            self.markers.remove(i);
        }
    }

    /// The marker nearest `frame` within `reach` frames.
    pub fn marker_near(&self, frame: f64, reach: f64) -> Option<usize> {
        self.markers
            .iter()
            .enumerate()
            .map(|(i, m)| (i, (*m as f64 - frame).abs()))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Replaces the markers with detected slice points (the implicit 0 is dropped).
    pub fn set_detected(&mut self, slices: &[u64], frames: u64) {
        self.markers = slices
            .iter()
            .copied()
            .filter(|f| *f > 0 && *f < frames)
            .collect();
        self.markers.sort_unstable();
        self.markers.dedup();
    }

    /// Slice start points for `slicer.make_instrument`: 0 first, then the markers.
    pub fn slices(&self) -> Vec<u64> {
        let mut v = vec![0];
        v.extend(self.markers.iter().copied());
        v
    }

    /// After an edit that changed the length, keep markers meaningful: the selection and the
    /// markers are dropped when the sample's length changed (their frames mean something
    /// else now); a length-preserving edit keeps them.
    pub fn after_edit(&mut self, old_frames: u64, new_frames: u64) {
        if old_frames != new_frames {
            self.markers.clear();
            self.sel = None;
            self.caret = self.caret.min(new_frames);
        }
    }
}

/// A frame as `m:ss.mmm`.
pub fn time_text(frame: u64, rate: u32) -> String {
    let s = frame as f64 / rate.max(1) as f64;
    let m = (s / 60.0).floor();
    format!("{}:{:06.3}", m as u64, s - m * 60.0)
}

/// A short label for a pitch (`C2`, `F#3`): MIDI 60 is C4.
pub fn key_name(key: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(key % 12) as usize], (key / 12) as i32 - 1)
}

// ---- what the app does when a command the editor sent comes back ------------------------

impl crate::core::AppCore {
    /// An `editor.apply` made a new sample: the window that showed the old one follows it (same
    /// place, same zoom), remembering the old one so an undo can walk back. A `slicer.detect`
    /// sets that sample's markers; `slicer.make_instrument` says what it built.
    pub fn on_editor_reply(&mut self, req: &ph_commands::Request, ex: &ph_commands::Executed) {
        use crate::workspace::WindowId;
        let sample = |v: &serde_json::Value| v.as_u64().map(|n| SampleId(n as u32));
        match req.command.as_str() {
            "editor.apply" => {
                let (Some(from), Some(to)) =
                    (sample(&ex.result["from"]), sample(&ex.result["sample"]))
                else {
                    return;
                };
                let Some(mut st) = self.audio_editors.remove(&from) else {
                    return;
                };
                st.prev.push(from);
                let new_frames = ex.result["frames"].as_u64().unwrap_or(st.frames);
                st.after_edit(st.frames, new_frames);
                st.frames = new_frames;
                let p = &req.params;
                match p["operation"].as_str() {
                    // the pasted piece is selected, like the editors people know
                    Some("paste") => {
                        let at = p["at"].as_u64().unwrap_or(st.caret);
                        let (a, b) = (
                            p["from_start"].as_u64().unwrap_or(0),
                            p["from_end"].as_u64().unwrap_or(0),
                        );
                        st.select(at, at + b.saturating_sub(a), new_frames);
                    }
                    Some("cut") => {
                        st.caret = p["start"].as_u64().unwrap_or(0).min(new_frames);
                    }
                    _ => {}
                }
                self.audio_editors.insert(to, st);
                self.editor_replaced
                    .push((WindowId::AudioEditor(from), WindowId::AudioEditor(to)));
            }
            "slicer.detect" => {
                if let Some(s) = sample(&ex.result["sample"])
                    && let Some(st) = self.audio_editors.get_mut(&s)
                {
                    let slices: Vec<u64> = ex.result["slices"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                        .unwrap_or_default();
                    let frames = st.frames;
                    st.set_detected(&slices, frames);
                    st.slicer = true;
                }
            }
            "slicer.make_instrument" => {
                if let Some(s) = &ex.summary {
                    self.toast(crate::core::ToastKind::Info, s.clone());
                }
                if let Some(c) = ex.result["channel"].as_u64() {
                    self.sel.channel = Some(ph_core::ChannelId(c as u32));
                }
                if let Some(p) = ex.result["pattern"].as_u64() {
                    self.sel.pattern = Some(ph_core::PatternId(p as u32));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_the_frame_under_the_pointer() {
        let mut s = EditorState::default();
        s.fit(1000.0, 100_000);
        assert_eq!(s.fpp, 100.0);
        let under = s.frame_at(400.0);
        s.zoom(4.0, 400.0, 1000.0, 100_000);
        assert!((s.frame_at(400.0) - under).abs() < 1e-6);
        assert_eq!(s.fpp, 25.0);
        // zooming out past the whole sample stops at the fit
        s.zoom(1e-6, 400.0, 1000.0, 100_000);
        assert_eq!(s.fpp, 100.0);
        assert_eq!(s.x0, 0.0);
        // and in past a sample stops at 16 px per frame
        for _ in 0..40 {
            s.zoom(2.0, 10.0, 1000.0, 100_000);
        }
        assert_eq!(s.fpp, MIN_FPP);
    }

    #[test]
    fn panning_stays_over_the_sample() {
        let mut s = EditorState::default();
        s.fit(1000.0, 100_000);
        s.zoom(10.0, 0.0, 1000.0, 100_000);
        s.pan(-5000.0, 1000.0, 100_000);
        assert_eq!(s.x0, 0.0);
        s.pan(1e9, 1000.0, 100_000);
        assert_eq!(s.x0, 100_000.0 - 1000.0 * s.fpp);
    }

    #[test]
    fn selection_is_ordered_clamped_and_clears_when_empty() {
        let mut s = EditorState::default();
        s.select(500, 200, 1000);
        assert_eq!(s.sel, Some((200, 500)));
        assert_eq!(s.caret, 200);
        s.select(900, 5000, 1000);
        assert_eq!(s.sel, Some((900, 1000)));
        s.select(300, 300, 1000);
        assert_eq!(s.sel, None);
        assert_eq!(s.range_or_all(1000), (0, 1000));
    }

    #[test]
    fn markers_stay_sorted_and_cannot_cross() {
        let mut s = EditorState::default();
        assert_eq!(
            s.add_marker(0, 1000, 2),
            None,
            "0 is the implicit first slice"
        );
        assert_eq!(s.add_marker(500, 1000, 2), Some(0));
        assert_eq!(s.add_marker(200, 1000, 2), Some(0));
        assert_eq!(s.add_marker(800, 1000, 2), Some(2));
        assert_eq!(s.markers, vec![200, 500, 800]);
        // a click near an existing marker picks it instead of adding another
        assert_eq!(s.add_marker(501, 1000, 2), Some(1));
        assert_eq!(s.markers.len(), 3);
        // moving cannot pass a neighbour
        assert!(s.move_marker(1, 900, 1000));
        assert_eq!(s.markers[1], 799);
        assert!(s.move_marker(1, 0, 1000));
        assert_eq!(s.markers[1], 201);
        assert!(!s.move_marker(7, 1, 1000));
        assert_eq!(s.marker_near(795.0, 10.0), Some(2));
        assert_eq!(s.marker_near(600.0, 10.0), None);
        s.delete_marker(0);
        assert_eq!(s.slices(), vec![0, 201, 800]);
    }

    #[test]
    fn detected_points_become_markers() {
        let mut s = EditorState::default();
        s.set_detected(&[0, 250, 500, 500, 999_999], 1000);
        assert_eq!(s.markers, vec![250, 500]);
        assert_eq!(s.slices(), vec![0, 250, 500]);
    }

    #[test]
    fn a_length_changing_edit_drops_frame_bound_state() {
        let mut s = EditorState::default();
        s.select(10, 20, 100);
        s.markers = vec![50];
        s.after_edit(100, 100);
        assert!(s.sel.is_some() && !s.markers.is_empty());
        s.after_edit(100, 80);
        assert!(s.sel.is_none() && s.markers.is_empty());
    }

    #[test]
    fn labels() {
        assert_eq!(time_text(48_000 * 75 + 24_000, 48_000), "1:15.500");
        assert_eq!(key_name(60), "C4");
        assert_eq!(key_name(36), "C2");
        assert_eq!(key_name(66), "F#4");
    }
}
