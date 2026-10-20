//! The audio editor window (M2/T6): an Edison-style view of one sample.
//!
//! Waveform with zoom and scroll, a selection and a caret, audition, and the operations as
//! buttons: cut, copy, paste, trim, silence, normalise, fades, reverse, gain, de-click. Every
//! operation is an `editor.apply` command; the editor never touches a file, so the original
//! stays and undo works. The slicer lives in a second row: transient detection with a
//! sensitivity, markers you place, move and delete, and `slicer.make_instrument`.

use crate::audio_editor::{EDrag, EditorState, key_name, time_text};
use crate::core::{AppCore, ToastKind};
use crate::hint::HintExt;
use crate::live::Audition;
use crate::mapping_audio as ma;
use crate::theme::lerp;
use crate::waveform::{Loaded, Peaks};
use crate::widgets::{btn, btn_enabled, caption, toggle};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use ph_core::{FadeCurve, SampleId};
use std::sync::Arc;

const RULER_H: f32 = 18.0;
const MARKER_LANE_H: f32 = 22.0;
const STATUS_H: f32 = 20.0;
/// Reach of a click on a marker, in points.
const MARKER_REACH: f32 = 6.0;

pub fn show(ui: &mut Ui, core: &mut AppCore, sample: SampleId) {
    let Some(entry) = core.project.samples.get(&sample).cloned() else {
        ui.label("This sample is no longer in the project.");
        return;
    };
    let theme = core.theme.clone();
    let loaded = core
        .waves
        .lock()
        .ok()
        .and_then(|mut w| w.request(&entry.path, true));
    let Some(loaded) = loaded.filter(|l| l.data.is_some()) else {
        let msg = core
            .waves
            .lock()
            .ok()
            .and_then(|w| w.failure(&entry.path).map(str::to_string));
        ui.add_space(8.0);
        match msg {
            Some(e) => ui.colored_label(
                theme.colors.error,
                format!("Could not open the sample: {e}"),
            ),
            None => ui.colored_label(theme.colors.dim, "Reading the sample..."),
        };
        return;
    };
    let frames = loaded.peaks.frames as u64;
    let rate = loaded.peaks.sample_rate.max(1);
    let mut st = core.audio_editors.remove(&sample).unwrap_or_default();
    st.frames = frames;
    if !st.fitted {
        st.fit(ui.available_width().max(200.0), frames);
    }
    if let Some((t0, a, b)) = st.playing {
        // the audition runs on the engine's voice; the playhead follows the clock
        let pos = a as f64 + (core.now - t0) * rate as f64;
        if pos >= b as f64 {
            st.playing = None;
        } else {
            ui.ctx().request_repaint();
        }
    }
    toolbar(ui, core, &mut st, sample, &loaded, rate);
    canvas(ui, core, &mut st, sample, &loaded, rate);
    status(ui, core, &st, &entry, rate);
    core.audio_editors.insert(sample, st);
}

// ---- toolbar ------------------------------------------------------------------------------

fn toolbar(
    ui: &mut Ui,
    core: &mut AppCore,
    st: &mut EditorState,
    sample: SampleId,
    loaded: &Arc<Loaded>,
    rate: u32,
) {
    let theme = core.theme.clone();
    let frames = st.frames;
    let width = (st.canvas.width()).max(200.0);
    ui.spacing_mut().item_spacing = Vec2::new(5.0, 4.0);
    ui.horizontal_wrapped(|ui| {
        // transport and view
        let playing = st.playing.is_some();
        if btn(ui, if playing { "Stop audition" } else { "Audition" })
            .tip("Audition the selection, or all of it (it plays outside the project)")
            .clicked()
        {
            if playing {
                stop(core, st);
            } else {
                play(core, st, loaded, rate);
            }
        }
        btn_enabled(ui, false, "Rec")
            .on_disabled_hover_text("Recording into the editor arrives with the recorder (M2/T8)");
        ui.separator();
        if btn(ui, "Zoom -").tip("Zoom out").clicked() {
            st.zoom(0.5, width * 0.5, width, frames);
        }
        if btn(ui, "Zoom +").tip("Zoom in (Ctrl+wheel)").clicked() {
            st.zoom(2.0, width * 0.5, width, frames);
        }
        if btn(ui, "Fit").tip("Show the whole sample").clicked() {
            st.fit(width, frames);
        }
        if btn_enabled(ui, st.sel.is_some(), "Zoom selection")
            .tip("Zoom to the selection")
            .clicked()
        {
            st.fit_selection(width, frames);
        }
        ui.separator();
        // edit
        let sel = st.sel;
        if btn_enabled(ui, sel.is_some(), "Cut")
            .tip("Cut the selection (Ctrl+X): copies it, then removes it")
            .clicked()
            && let Some(r) = sel
        {
            core.audio_clip = Some((sample, r));
            core.send(ma::editor_apply(sample, "cut", Some(r), None, None));
        }
        if btn_enabled(ui, sel.is_some(), "Copy")
            .tip("Copy the selection (Ctrl+C)")
            .clicked()
            && let Some(r) = sel
        {
            core.audio_clip = Some((sample, r));
            core.toast(ToastKind::Info, "Copied");
        }
        let clip = core.audio_clip;
        if btn_enabled(ui, clip.is_some(), "Paste")
            .tip("Paste at the caret, or at the start of the selection (Ctrl+V)")
            .clicked()
            && let Some((from, range)) = clip
        {
            let at = st.sel.map_or(st.caret, |s| s.0);
            core.send(ma::editor_paste(sample, from, range, at));
        }
        if btn_enabled(ui, sel.is_some(), "Trim")
            .tip("Keep only the selection")
            .clicked()
            && let Some(r) = sel
        {
            core.send(ma::editor_apply(sample, "trim", Some(r), None, None));
        }
        if btn_enabled(ui, sel.is_some(), "Silence")
            .tip("Replace the selection by silence")
            .clicked()
            && let Some(r) = sel
        {
            core.send(ma::editor_apply(sample, "silence", Some(r), None, None));
        }
    });
    ui.horizontal_wrapped(|ui| {
        let sel = st.sel;
        let scope = if sel.is_some() {
            "selection"
        } else {
            "whole sample"
        };
        let which = sel;
        if btn(ui, "Normalise")
            .tip(&format!("Scale the {scope} to the peak level on the right"))
            .clicked()
        {
            core.send(ma::editor_apply(
                sample,
                "normalize",
                which,
                Some(st.norm_db),
                None,
            ));
        }
        let r = ui
            .add(
                egui::DragValue::new(&mut st.norm_db)
                    .range(-60.0..=0.0)
                    .speed(0.1)
                    .suffix(" dBFS")
                    .fixed_decimals(1),
            )
            .hint_menu(
                "Normalise peak",
                &format!("{:.1} dBFS", st.norm_db),
                "drag or type",
            );
        let _ = r;
        ui.separator();
        egui::ComboBox::from_id_salt("editor_fade_curve")
            .selected_text(crate::playlist::audio::curve_name(st.fade_curve))
            .width(110.0)
            .show_ui(ui, |ui| {
                for c in [
                    FadeCurve::Linear,
                    FadeCurve::Exp,
                    FadeCurve::Log,
                    FadeCurve::SCurve,
                ] {
                    ui.selectable_value(
                        &mut st.fade_curve,
                        c,
                        crate::playlist::audio::curve_name(c),
                    );
                }
            })
            .response
            .hint_menu(
                "Fade shape",
                crate::playlist::audio::curve_name(st.fade_curve),
                "chooses the curve of the next fade",
            );
        if btn(ui, "Fade in")
            .tip(&format!("Fade the {scope} up from silence"))
            .clicked()
        {
            core.send(ma::editor_apply(
                sample,
                "fade_in",
                which,
                None,
                Some(st.fade_curve),
            ));
        }
        if btn(ui, "Fade out")
            .tip(&format!("Fade the {scope} down to silence"))
            .clicked()
        {
            core.send(ma::editor_apply(
                sample,
                "fade_out",
                which,
                None,
                Some(st.fade_curve),
            ));
        }
        ui.separator();
        if btn(ui, "Reverse")
            .tip(&format!("Play the {scope} backwards"))
            .clicked()
        {
            core.send(ma::editor_apply(sample, "reverse", which, None, None));
        }
        if btn(ui, "Gain")
            .tip(&format!(
                "Change the level of the {scope} by the amount on the right"
            ))
            .clicked()
        {
            core.send(ma::editor_apply(
                sample,
                "gain",
                which,
                Some(st.gain_db),
                None,
            ));
        }
        ui.add(
            egui::DragValue::new(&mut st.gain_db)
                .range(-60.0..=24.0)
                .speed(0.1)
                .suffix(" dB")
                .fixed_decimals(1),
        )
        .hint_menu("Gain", &format!("{:.1} dB", st.gain_db), "drag or type");
        if btn(ui, "De-click")
            .tip(&format!("Remove clicks from the {scope}"))
            .clicked()
        {
            core.send(ma::editor_apply(sample, "declick", which, None, None));
        }
        ui.separator();
        if toggle(ui, st.slicer, "Slicer", theme.colors.accent, &theme)
            .tip("Show the slice markers and the slicer controls")
            .clicked()
        {
            st.slicer = !st.slicer;
        }
    });
    if st.slicer {
        ui.horizontal_wrapped(|ui| {
            ui.label(caption(&theme, "Sensitivity"));
            let r = ui
                .add(egui::Slider::new(&mut st.sensitivity, 0.0..=1.0).show_value(false))
                .hint_menu(
                    "Slicer sensitivity",
                    &format!("{:.0}%", st.sensitivity * 100.0),
                    "higher finds more transients",
                );
            let moved = r.drag_stopped() || (r.lost_focus() && !r.dragged());
            if btn(ui, "Detect")
                .tip("Find the transients and place a marker on each")
                .clicked()
                || (moved && !st.markers.is_empty())
            {
                core.send(ma::slicer_detect(sample, st.sensitivity));
            }
            if btn(ui, "Add marker")
                .tip("Add a marker at the caret (or click in the lane below the waveform)")
                .clicked()
            {
                let gap = (st.fpp * 3.0) as u64;
                st.add_marker(st.caret, frames, gap);
            }
            if btn_enabled(ui, !st.markers.is_empty(), "Clear markers").clicked() {
                st.markers.clear();
            }
            ui.separator();
            ui.label(caption(&theme, "First key"));
            let mut k = st.root_key as i32;
            let r = ui
                .add(
                    egui::DragValue::new(&mut k)
                        .range(0..=127)
                        .custom_formatter(|v, _| key_name(v as u8)),
                )
                .hint_menu(
                    "First slice key",
                    &key_name(st.root_key),
                    "slice 1 plays on this key, slice 2 on the next one up",
                );
            let _ = r;
            st.root_key = k.clamp(0, 127) as u8;
            let mut wp = st.write_pattern;
            if ui
                .checkbox(&mut wp, "Pattern that replays the loop")
                .changed()
            {
                st.write_pattern = wp;
            }
            let n = st.markers.len() + 1;
            if btn(ui, "Make instrument")
                .tip(&format!(
                    "Build a Sampler channel with {n} slices, one per key"
                ))
                .clicked()
            {
                core.send(ma::make_instrument(
                    sample,
                    &st.slices(),
                    st.write_pattern,
                    Some(st.root_key),
                ));
            }
            ui.label(
                egui::RichText::new(format!("{n} slices"))
                    .size(11.0)
                    .color(theme.colors.dim),
            );
        });
    }
}

fn play(core: &mut AppCore, st: &mut EditorState, loaded: &Arc<Loaded>, rate: u32) {
    let Some(data) = loaded.data.as_ref() else {
        return;
    };
    let frames = st.frames;
    let (a, b) = st
        .sel
        .unwrap_or((st.caret.min(frames.saturating_sub(1)), frames));
    if b <= a {
        return;
    }
    let channels: Vec<Vec<f32>> = data
        .channels
        .iter()
        .map(|c| c[(a as usize).min(c.len())..(b as usize).min(c.len())].to_vec())
        .collect();
    let clip = ph_node::SampleData {
        sample_rate: rate as f64,
        channels,
        loop_range: None,
        root_key: None,
    };
    core.link.audition(Audition::Data(Arc::new(clip)));
    st.playing = Some((core.now, a, b));
}

fn stop(core: &mut AppCore, st: &mut EditorState) {
    // an empty voice replaces whatever is playing
    core.link
        .audition(Audition::Data(Arc::new(ph_node::SampleData {
            sample_rate: 48_000.0,
            channels: vec![Vec::new()],
            loop_range: None,
            root_key: None,
        })));
    st.playing = None;
}

// ---- canvas -------------------------------------------------------------------------------

/// One column of a lane: the min and max over frames `f0..f1`.
fn column(
    peaks: &Peaks,
    data: Option<&[f32]>,
    level: usize,
    chan: usize,
    f0: f64,
    f1: f64,
    fpp: f64,
) -> Option<(f32, f32)> {
    if f1 <= 0.0 || f0 >= peaks.frames as f64 {
        return None;
    }
    if fpp < crate::waveform::BASE_BLOCK as f64
        && let Some(d) = data
    {
        let a = f0.max(0.0) as usize;
        let b = (f1.ceil() as usize)
            .clamp(a + 1, d.len().max(a + 1))
            .min(d.len());
        if a >= b {
            return None;
        }
        return Some(
            d[a..b]
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x))),
        );
    }
    peaks.range(level, f0, f1, Some(chan))
}

fn nice_step(seconds_per_px: f64, min_px: f64) -> f64 {
    let want = seconds_per_px * min_px;
    let mut m = 1e-6;
    loop {
        for k in [1.0, 2.0, 5.0] {
            if m * k >= want {
                return m * k;
            }
        }
        m *= 10.0;
        if m > 1e6 {
            return m;
        }
    }
}

fn canvas(
    ui: &mut Ui,
    core: &mut AppCore,
    st: &mut EditorState,
    sample: SampleId,
    loaded: &Arc<Loaded>,
    rate: u32,
) {
    let theme = core.theme.clone();
    let col = &theme.colors;
    let frames = st.frames;
    let avail = ui.available_size();
    let size = Vec2::new(avail.x.max(100.0), (avail.y - STATUS_H).max(90.0));
    let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let whole = resp.rect;
    painter.rect_filled(whole, CornerRadius::ZERO, col.bg0);
    let lane_h = if st.slicer { MARKER_LANE_H } else { 0.0 };
    let ruler = Rect::from_min_size(whole.min, Vec2::new(whole.width(), RULER_H));
    let markers_r = Rect::from_min_max(Pos2::new(whole.left(), whole.bottom() - lane_h), whole.max);
    let wave = Rect::from_min_max(
        Pos2::new(whole.left(), ruler.bottom()),
        Pos2::new(whole.right(), whole.bottom() - lane_h),
    );
    st.canvas = wave;
    st.marker_lane = if st.slicer { markers_r } else { Rect::NOTHING };
    st.clamp(wave.width(), frames);
    let chans = loaded.peaks.channels.clamp(1, 2);

    // ---- input
    let (pointer, scroll, mods, press_origin) = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.smooth_scroll_delta(),
            i.modifiers,
            i.pointer.press_origin(),
        )
    });
    if let Some(pp) = pointer
        && resp.hovered()
        && (scroll.x != 0.0 || scroll.y != 0.0)
    {
        if mods.command {
            st.zoom(
                1.0 + scroll.y as f64 * 0.004,
                pp.x - wave.left(),
                wave.width(),
                frames,
            );
        } else {
            st.pan(-(scroll.x + scroll.y), wave.width(), frames);
        }
    }
    let slicer_on = st.slicer;
    let in_lane = move |p: Pos2| slicer_on && markers_r.contains(p);
    let reach = MARKER_REACH as f64 * st.fpp;
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(o) = press_origin
        && whole.contains(o)
    {
        let f = st.frame_at(o.x - wave.left());
        if st.slicer
            && (in_lane(o) || st.marker_near(f, reach).is_some())
            && let Some(i) = st.marker_near(f, reach)
        {
            st.drag = EDrag::Marker(i);
        } else if !in_lane(o) {
            st.drag = EDrag::Select(f.max(0.0).min(frames as f64) as u64);
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(pp) = pointer
    {
        let f = st.frame_at(pp.x - wave.left()).max(0.0).min(frames as f64) as u64;
        match st.drag {
            EDrag::Select(a) => st.select(a, f, frames),
            EDrag::Marker(i) => {
                st.move_marker(i, f, frames);
            }
            EDrag::None => {}
        }
    }
    if resp.drag_stopped() {
        st.drag = EDrag::None;
    }
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(pp) = resp.interact_pointer_pos()
    {
        let f = st.frame_at(pp.x - wave.left()).max(0.0).min(frames as f64);
        if in_lane(pp) {
            // a click in the marker lane places a marker
            st.add_marker(f as u64, frames, reach as u64);
        } else {
            st.sel = None;
            st.caret = f as u64;
        }
    }
    if resp.double_clicked() {
        st.select(0, frames, frames);
    }
    if resp.secondary_clicked()
        && let Some(pp) = resp.interact_pointer_pos()
        && st.slicer
    {
        let f = st.frame_at(pp.x - wave.left());
        if let Some(i) = st.marker_near(f, reach) {
            st.delete_marker(i);
        }
    }
    // keys, while the pointer is over the editor
    if resp.hovered() && !ui.ctx().egui_wants_keyboard_input() {
        let (cut, copy, paste, del, all) = ui.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::X),
                i.modifiers.command && i.key_pressed(egui::Key::C),
                i.modifiers.command && i.key_pressed(egui::Key::V),
                i.key_pressed(egui::Key::Delete),
                i.modifiers.command && i.key_pressed(egui::Key::A),
            )
        });
        if all {
            st.select(0, frames, frames);
        }
        if let Some(r) = st.sel {
            if copy || cut {
                core.audio_clip = Some((sample, r));
            }
            if cut || del {
                core.send(ma::editor_apply(sample, "cut", Some(r), None, None));
            }
        }
        if paste && let Some((from, range)) = core.audio_clip {
            let at = st.sel.map_or(st.caret, |s| s.0);
            core.send(ma::editor_paste(sample, from, range, at));
        }
    }
    // the marker under the pointer, for the cursor
    if let Some(pp) = pointer
        && resp.hovered()
        && st.slicer
        && st
            .marker_near(st.frame_at(pp.x - wave.left()), reach)
            .is_some()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    // ---- paint
    let p = painter.with_clip_rect(whole);
    // ruler
    p.rect_filled(ruler, CornerRadius::ZERO, col.bg3);
    let spp = st.fpp / rate as f64;
    let step = nice_step(spp, 70.0);
    let t0 = st.x0 / rate as f64;
    let t1 = (st.x0 + wave.width() as f64 * st.fpp) / rate as f64;
    let mut t = (t0 / step).floor() * step;
    while t <= t1 + step {
        let x = wave.left() + st.x_of(t * rate as f64);
        if x >= wave.left() - 1.0 && x <= wave.right() + 1.0 {
            p.line_segment(
                [
                    Pos2::new(x, ruler.top() + 10.0),
                    Pos2::new(x, ruler.bottom()),
                ],
                Stroke::new(1.0, col.grid_bar),
            );
            p.text(
                Pos2::new(x + 3.0, ruler.top() + 1.0),
                Align2::LEFT_TOP,
                if step >= 1.0 {
                    format!("{t:.0}s")
                } else if step >= 0.01 {
                    format!("{t:.2}s")
                } else if step >= 0.001 {
                    format!("{:.0}ms", t * 1000.0)
                } else {
                    format!("{t:.4}s")
                },
                FontId::new(9.5, crate::fonts::mono()),
                col.dim,
            );
            p.line_segment(
                [Pos2::new(x, wave.top()), Pos2::new(x, wave.bottom())],
                Stroke::new(1.0, col.grid_minor),
            );
        }
        t += step;
    }
    // slices, tinted in turn
    if st.slicer {
        let bounds = st.slices();
        for (i, f) in bounds.iter().enumerate() {
            let a = wave.left() + st.x_of(*f as f64);
            let b = wave.left() + st.x_of(bounds.get(i + 1).map_or(frames as f64, |n| *n as f64));
            if i % 2 == 0 {
                p.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(a.max(wave.left()), wave.top()),
                        Pos2::new(b.min(wave.right()), wave.bottom()),
                    ),
                    CornerRadius::ZERO,
                    col.accent.gamma_multiply(0.06),
                );
            }
        }
    }
    // lanes
    let lane_gap = 2.0;
    let lh = (wave.height() - lane_gap * (chans as f32 - 1.0)) / chans as f32;
    let data = loaded.data.as_ref();
    for c in 0..chans {
        let lane = Rect::from_min_size(
            Pos2::new(wave.left(), wave.top() + c as f32 * (lh + lane_gap)),
            Vec2::new(wave.width(), lh),
        );
        p.rect_filled(lane, CornerRadius::ZERO, col.key_white.gamma_multiply(0.5));
        let mid = lane.center().y;
        let half = lh * 0.5 - 2.0;
        p.line_segment(
            [Pos2::new(lane.left(), mid), Pos2::new(lane.right(), mid)],
            Stroke::new(1.0, col.grid_major),
        );
        let ink = lerp(col.accent, col.text, 0.35);
        let lp = p.with_clip_rect(lane.intersect(wave));
        let chan_data: Option<&[f32]> = data.and_then(|d| d.channels.get(c)).map(|v| v.as_slice());
        if st.fpp < 0.5
            && let Some(d) = chan_data
        {
            // closer than one pixel per frame: the samples themselves
            let first = st.x0.floor().max(0.0) as usize;
            let last = ((st.x0 + wave.width() as f64 * st.fpp).ceil() as usize + 1).min(d.len());
            let pts: Vec<Pos2> = (first..last)
                .map(|i| {
                    Pos2::new(
                        wave.left() + st.x_of(i as f64),
                        mid - d[i].clamp(-1.0, 1.0) * half,
                    )
                })
                .collect();
            lp.add(egui::Shape::line(pts.clone(), Stroke::new(1.2, ink)));
            if st.fpp < 1.0 / 6.0 {
                for q in pts {
                    lp.circle_filled(q, 2.0, col.accent_hot);
                }
            }
        } else {
            let level = loaded.peaks.level_for(st.fpp);
            let mut mesh = egui::Mesh::default();
            let w = wave.width() as usize;
            for x in 0..w {
                let f0 = st.x0 + x as f64 * st.fpp;
                let f1 = f0 + st.fpp;
                if let Some((mn, mx)) = column(&loaded.peaks, chan_data, level, c, f0, f1, st.fpp) {
                    let top = (mid - mx.clamp(-1.0, 1.0) * half).min(mid - 0.5);
                    let bot = (mid - mn.clamp(-1.0, 1.0) * half).max(mid + 0.5);
                    let clipped = mx >= 0.9999 || mn <= -0.9999;
                    mesh.add_colored_rect(
                        Rect::from_min_max(
                            Pos2::new(wave.left() + x as f32, top),
                            Pos2::new(wave.left() + x as f32 + 1.0, bot),
                        ),
                        if clipped { col.clip } else { ink },
                    );
                }
            }
            lp.add(egui::Shape::mesh(mesh));
        }
        if chans == 2 {
            p.text(
                lane.left_top() + Vec2::new(4.0, 2.0),
                Align2::LEFT_TOP,
                if c == 0 { "L" } else { "R" },
                FontId::new(10.0, crate::fonts::mono()),
                col.dim,
            );
        }
    }
    // selection and caret
    let wp = p.with_clip_rect(wave);
    if let Some((a, b)) = st.sel {
        let r = Rect::from_min_max(
            Pos2::new(wave.left() + st.x_of(a as f64), wave.top()),
            Pos2::new(wave.left() + st.x_of(b as f64), wave.bottom()),
        );
        wp.rect_filled(r, CornerRadius::ZERO, col.selection.gamma_multiply(0.22));
        for x in [r.left(), r.right()] {
            wp.line_segment(
                [Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
                Stroke::new(1.0, col.selection),
            );
        }
    } else {
        let x = wave.left() + st.x_of(st.caret as f64);
        wp.line_segment(
            [Pos2::new(x, wave.top()), Pos2::new(x, wave.bottom())],
            Stroke::new(1.0, col.accent_hot),
        );
    }
    // slice markers
    if st.slicer {
        p.rect_filled(markers_r, CornerRadius::ZERO, col.bg3);
        let all = st.slices();
        for (i, f) in all.iter().enumerate() {
            let x = wave.left() + st.x_of(*f as f64);
            if x < wave.left() - 20.0 || x > wave.right() + 1.0 {
                continue;
            }
            let hot = matches!(st.drag, EDrag::Marker(d) if d + 1 == i);
            let c = if hot { col.accent_hot } else { col.accent };
            let lane = p.with_clip_rect(whole);
            lane.line_segment(
                [Pos2::new(x, wave.top()), Pos2::new(x, markers_r.bottom())],
                Stroke::new(if i == 0 { 1.0 } else { 1.5 }, c),
            );
            lane.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(x - 4.0, markers_r.top() + 1.0),
                    Pos2::new(x + 4.0, markers_r.top() + 1.0),
                    Pos2::new(x, markers_r.top() + 8.0),
                ],
                c,
                Stroke::NONE,
            ));
            let key = st.root_key as usize + i;
            if key < 128 {
                crate::paint::text_fit(
                    &lane.with_clip_rect(markers_r),
                    Pos2::new(x + 4.0, markers_r.top() + 5.0),
                    Align2::LEFT_TOP,
                    &key_name(key as u8),
                    FontId::new(9.5, crate::fonts::mono()),
                    col.text,
                    36.0,
                );
            }
        }
    }
    // the audition playhead
    if let Some((t0, a, b)) = st.playing {
        let pos = (a as f64 + (core.now - t0) * rate as f64).min(b as f64);
        let x = wave.left() + st.x_of(pos);
        if x >= wave.left() && x <= wave.right() {
            crate::paint::playhead(
                &p.with_clip_rect(wave),
                x,
                wave.top(),
                wave.bottom(),
                &theme,
                crate::motion::reduce(ui.ctx()),
            );
        }
    }
}

// ---- status line --------------------------------------------------------------------------

fn status(ui: &mut Ui, core: &AppCore, st: &EditorState, entry: &ph_core::SampleEntry, rate: u32) {
    let col = &core.theme.colors;
    let (r, p) = ui.allocate_painter(Vec2::new(ui.available_width(), STATUS_H), Sense::hover());
    let sel = st.sel.map_or_else(
        || format!("caret {}", time_text(st.caret, rate)),
        |(a, b)| {
            format!(
                "selection {} to {} ({} frames, {:.3} s)",
                time_text(a, rate),
                time_text(b, rate),
                b - a,
                (b - a) as f64 / rate as f64
            )
        },
    );
    let text = format!(
        "{}  \u{b7}  {} Hz  \u{b7}  {}  \u{b7}  {}  \u{b7}  {}",
        entry.name,
        rate,
        if entry.channels == 1 {
            "mono"
        } else {
            "stereo"
        },
        time_text(entry.frames, rate),
        sel
    );
    crate::paint::text_fit(
        &p,
        r.rect.left_center() + Vec2::new(6.0, 0.0),
        Align2::LEFT_CENTER,
        &text,
        FontId::new(11.0, crate::fonts::mono()),
        col.dim,
        r.rect.width() - 12.0,
    );
}
