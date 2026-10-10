//! Custom controls: knob, fader, meter, toggle, and the generic parameter grid built from
//! `ph_node::NodeDescriptor`s. All vector-drawn from theme tokens.

use crate::theme::Theme;
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use ph_node::{NodeDescriptor, ParamInfo, ParamUnit};

/// What a continuous control did this frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Edit {
    None,
    /// Being dragged: show this value, do not send a command yet.
    Live(f64),
    /// Released (or double-clicked to reset): send a command with this value.
    Commit(f64),
}

impl Edit {
    pub fn commit(self) -> Option<f64> {
        match self {
            Edit::Commit(v) => Some(v),
            _ => None,
        }
    }
    pub fn shown(self, current: f64) -> f64 {
        match self {
            Edit::Live(v) | Edit::Commit(v) => v,
            Edit::None => current,
        }
    }
}

#[derive(Copy, Clone)]
struct DragState {
    value: f64,
    start: f64,
}

/// Rotary knob. Vertical drag (Shift = fine), double-click resets to `default`.
#[allow(clippy::too_many_arguments)]
pub fn knob(
    ui: &mut Ui,
    id: Id,
    value: f64,
    min: f64,
    max: f64,
    default: f64,
    diameter: f32,
    theme: &Theme,
) -> (Response, Edit) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
    let (edit, shown) = drag_logic(ui, &resp, id, value, min, max, default, 160.0);
    let c = &theme.colors;
    let p = ui.painter();
    let centre = rect.center();
    let r = diameter * 0.5 - 2.0;
    let (a0, a1) = (std::f32::consts::PI * 0.75, std::f32::consts::PI * 2.25);
    let arc = |from: f32, to: f32, stroke: Stroke| {
        let n = 24;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                centre + Vec2::new(a.cos(), a.sin()) * r
            })
            .collect();
        Shape::line(pts, stroke)
    };
    p.circle_filled(
        centre,
        r - 2.0,
        if resp.hovered() {
            c.widget_hover
        } else {
            c.widget
        },
    );
    p.add(arc(a0, a1, Stroke::new(2.5, c.border)));
    let t = if max > min {
        ((shown - min) / (max - min)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let bipolar = min < 0.0 && max > 0.0;
    let (from, to) = if bipolar {
        let mid = a0 + (a1 - a0) * ((0.0 - min) / (max - min)) as f32;
        let cur = a0 + (a1 - a0) * t;
        (mid.min(cur), mid.max(cur))
    } else {
        (a0, a0 + (a1 - a0) * t)
    };
    if to > from + 0.001 {
        p.add(arc(from, to, Stroke::new(2.5, c.accent)));
    }
    let a = a0 + (a1 - a0) * t;
    p.line_segment(
        [
            centre + Vec2::new(a.cos(), a.sin()) * (r * 0.3),
            centre + Vec2::new(a.cos(), a.sin()) * (r - 3.0),
        ],
        Stroke::new(2.0, c.text),
    );
    (resp, edit)
}

#[allow(clippy::too_many_arguments)]
fn drag_logic(
    ui: &Ui,
    resp: &Response,
    id: Id,
    value: f64,
    min: f64,
    max: f64,
    default: f64,
    pixels_for_full_range: f64,
) -> (Edit, f64) {
    if resp.double_clicked() {
        ui.data_mut(|d| d.remove::<DragState>(id));
        return (Edit::Commit(default.clamp(min, max)), default);
    }
    if resp.drag_started() {
        ui.data_mut(|d| {
            d.insert_temp(
                id,
                DragState {
                    value,
                    start: value,
                },
            )
        });
    }
    let mut state: Option<DragState> = ui.data(|d| d.get_temp(id));
    if resp.dragged()
        && let Some(s) = state.as_mut()
    {
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.1
        } else {
            1.0
        };
        let dy = -resp.drag_delta().y as f64;
        s.value = (s.value + dy / pixels_for_full_range * (max - min) * fine).clamp(min, max);
        ui.data_mut(|d| d.insert_temp(id, *s));
        return (Edit::Live(s.value), s.value);
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<DragState>(id));
        if let Some(s) = state
            && (s.value - s.start).abs() > f64::EPSILON
        {
            return (Edit::Commit(s.value), s.value);
        }
    }
    (Edit::None, state.map_or(value, |s| s.value))
}

/// Vertical fader. Same editing semantics as [`knob`].
#[allow(clippy::too_many_arguments)]
pub fn fader(
    ui: &mut Ui,
    id: Id,
    value: f64,
    min: f64,
    max: f64,
    default: f64,
    size: Vec2,
    theme: &Theme,
) -> (Response, Edit) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let (edit, shown) = drag_logic(
        ui,
        &resp,
        id,
        value,
        min,
        max,
        default,
        size.y as f64 - 12.0,
    );
    let c = &theme.colors;
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(4.0, rect.height() - 8.0));
    p.rect_filled(track, CornerRadius::same(2), c.border);
    let t = ((shown - min) / (max - min)).clamp(0.0, 1.0) as f32;
    let y = track.bottom() - track.height() * t;
    p.rect_filled(
        Rect::from_min_max(Pos2::new(track.left(), y), track.right_bottom()),
        CornerRadius::same(2),
        c.accent_dim,
    );
    let handle = Rect::from_center_size(
        Pos2::new(rect.center().x, y),
        Vec2::new(rect.width().min(26.0), 10.0),
    );
    p.rect_filled(
        handle,
        CornerRadius::same(3),
        if resp.hovered() || resp.dragged() {
            c.widget_active
        } else {
            c.widget_hover
        },
    );
    p.rect_stroke(
        handle,
        CornerRadius::same(3),
        Stroke::new(1.0, c.text_dim),
        StrokeKind::Inside,
    );
    p.line_segment(
        [
            Pos2::new(handle.left() + 3.0, y),
            Pos2::new(handle.right() - 3.0, y),
        ],
        Stroke::new(1.5, c.accent),
    );
    (resp, edit)
}

pub fn db_from_linear(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}

/// Level meter: `peak` (with hold) and `rms` per channel, drawn on a -60..+6 dB scale.
pub fn meter(ui: &mut Ui, size: Vec2, peak: [f32; 2], rms: [f32; 2], theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    paint_meter(ui, rect, peak, rms, theme);
}

pub fn paint_meter(ui: &Ui, rect: Rect, peak: [f32; 2], rms: [f32; 2], theme: &Theme) {
    let c = &theme.colors;
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), c.bg);
    let vertical = rect.height() >= rect.width();
    for ch in 0..2 {
        let lane = if vertical {
            let w = rect.width() / 2.0;
            Rect::from_min_size(
                rect.min + Vec2::new(w * ch as f32, 0.0),
                Vec2::new(w - 1.0, rect.height()),
            )
        } else {
            let h = rect.height() / 2.0;
            Rect::from_min_size(
                rect.min + Vec2::new(0.0, h * ch as f32),
                Vec2::new(rect.width(), h - 1.0),
            )
        };
        let norm = |lin: f32| ((db_from_linear(lin) + 60.0) / 66.0).clamp(0.0, 1.0);
        for (level, alpha) in [(norm(rms[ch]), 1.0f32), (norm(peak[ch]), 0.55)] {
            let fill = if vertical {
                Rect::from_min_max(
                    Pos2::new(lane.left(), lane.bottom() - lane.height() * level),
                    lane.right_bottom(),
                )
            } else {
                Rect::from_min_max(
                    lane.left_top(),
                    Pos2::new(lane.left() + lane.width() * level, lane.bottom()),
                )
            };
            let colour = if level > 0.91 {
                c.meter_hi
            } else if level > 0.76 {
                c.meter_mid
            } else {
                c.meter_lo
            };
            p.rect_filled(fill, CornerRadius::ZERO, colour.gamma_multiply(alpha));
        }
    }
}

/// A flat toggle button (mute, solo, bypass, ...). Returns true when clicked.
pub fn toggle(ui: &mut Ui, on: bool, label: &str, on_colour: Color32, theme: &Theme) -> Response {
    let c = &theme.colors;
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), FontId::proportional(11.0), c.text);
    let size = Vec2::new((galley.size().x + 12.0).max(22.0), 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if on {
        on_colour
    } else if resp.hovered() {
        c.widget_hover
    } else {
        c.widget
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(theme.metrics.rounding as u8), fill);
    let text_colour = if on { Color32::BLACK } else { c.text };
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, text_colour);
    resp
}

/// Number with its unit, the way the control shows it.
pub fn format_param(info: &ParamInfo, v: f64) -> String {
    let label_of = |v: f64| {
        let i = (v - info.min).round() as usize;
        info.choices.get(i).cloned()
    };
    match info.unit {
        ParamUnit::Decibels => format!("{v:.1} dB"),
        ParamUnit::Hertz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        ParamUnit::Hertz => format!("{v:.0} Hz"),
        ParamUnit::Seconds if v < 1.0 => format!("{:.0} ms", v * 1000.0),
        ParamUnit::Seconds => format!("{v:.2} s"),
        ParamUnit::Milliseconds => format!("{v:.0} ms"),
        ParamUnit::Percent => format!("{:.0}%", v * 100.0),
        ParamUnit::Semitones => format!("{v:+.0} st"),
        ParamUnit::Cents => format!("{v:+.0} ct"),
        ParamUnit::Ratio => format!("{v:.2}"),
        ParamUnit::Pan => {
            if v.abs() < 0.005 {
                "C".into()
            } else if v < 0.0 {
                format!("L{:.0}", -v * 100.0)
            } else {
                format!("R{:.0}", v * 100.0)
            }
        }
        ParamUnit::Choice => label_of(v).unwrap_or_else(|| format!("{v:.0}")),
        ParamUnit::Switch => (if v >= 0.5 { "On" } else { "Off" }).into(),
        ParamUnit::None if info.stepped => format!("{v:.0}"),
        ParamUnit::None => format!("{v:.2}"),
    }
}

/// Knobs and switches for the parameters of `desc` whose ids pass `filter`, in a wrapped grid.
/// `value_of` gives the current value (falling back to the default is the caller's job);
/// returns the committed `(param id, value)` changes.
pub fn param_grid(
    ui: &mut Ui,
    salt: Id,
    desc: &NodeDescriptor,
    filter: &dyn Fn(&ParamInfo) -> bool,
    value_of: &dyn Fn(&ParamInfo) -> f64,
    theme: &Theme,
) -> Vec<(String, f64)> {
    let mut changes = Vec::new();
    // Each cell is a vertical block, which egui's wrapping layout cannot wrap on its own, so
    // the rows are cut here from the width that is available.
    let cell = 66.0 + ui.spacing().item_spacing.x;
    let avail = ui.available_width();
    let per_row = if avail.is_finite() {
        (((avail + ui.spacing().item_spacing.x) / cell).floor() as usize).max(1)
    } else {
        8
    };
    let params: Vec<&ParamInfo> = desc.params.iter().filter(|p| filter(p)).collect();
    for row in params.chunks(per_row) {
        ui.horizontal_top(|ui| {
            for info in row.iter().copied() {
                let id = salt.with(&info.id);
                let cur = value_of(info);
                ui.vertical(|ui| {
                    ui.set_width(66.0);
                    ui.vertical_centered(|ui| {
                        if info.stepped
                            && (info.unit == ParamUnit::Switch
                                || info.max - info.min == 1.0 && info.choices.is_empty())
                        {
                            let on = cur >= 0.5;
                            let r = toggle(
                                ui,
                                on,
                                if on { "On" } else { "Off" },
                                theme.colors.accent,
                                theme,
                            );
                            if r.clicked() {
                                changes
                                    .push((info.id.clone(), if on { info.min } else { info.max }));
                            }
                        } else if info.stepped && !info.choices.is_empty()
                            || info.unit == ParamUnit::Choice
                        {
                            let mut sel = (cur - info.min).round() as i64;
                            let n = if info.choices.is_empty() {
                                (info.max - info.min) as i64 + 1
                            } else {
                                info.choices.len() as i64
                            };
                            let before = sel;
                            egui::ComboBox::from_id_salt(id)
                                .width(58.0)
                                .selected_text(format_param(info, cur))
                                .show_ui(ui, |ui| {
                                    for i in 0..n {
                                        let label = info
                                            .choices
                                            .get(i as usize)
                                            .cloned()
                                            .unwrap_or_else(|| format!("{i}"));
                                        ui.selectable_value(&mut sel, i, label);
                                    }
                                });
                            if sel != before {
                                changes.push((info.id.clone(), info.min + sel as f64));
                            }
                        } else if info.stepped {
                            let mut v = cur as i64;
                            let r = ui.add(
                                egui::DragValue::new(&mut v)
                                    .range(info.min as i64..=info.max as i64),
                            );
                            if r.double_clicked() {
                                changes.push((info.id.clone(), info.default));
                            } else if r.drag_stopped() || r.lost_focus() {
                                changes.push((info.id.clone(), v as f64));
                            }
                        } else {
                            let (_, edit) =
                                knob(ui, id, cur, info.min, info.max, info.default, 34.0, theme);
                            if let Some(v) = edit.commit() {
                                changes.push((info.id.clone(), v));
                            }
                            let _ = edit.shown(cur);
                        }
                        let shown = match (info.stepped, info.unit) {
                            (false, _) => format_param(
                                info,
                                ui.data(|d| d.get_temp::<DragState>(id))
                                    .map_or(cur, |s| s.value),
                            ),
                            _ => String::new(),
                        };
                        ui.label(
                            egui::RichText::new(&info.name)
                                .size(10.0)
                                .color(theme.colors.text_dim),
                        );
                        if !shown.is_empty() {
                            ui.label(egui::RichText::new(shown).size(10.0));
                        }
                    });
                });
            }
        });
    }
    changes
}

/// Small coloured badge with text (author tags, counters).
pub fn badge(ui: &mut Ui, text: &str, fill: Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.into(), FontId::proportional(10.0), Color32::BLACK);
    let (rect, _) = ui.allocate_exact_size(galley.size() + Vec2::new(8.0, 3.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), fill);
    ui.painter()
        .galley(rect.min + Vec2::new(4.0, 1.5), galley, Color32::BLACK);
}

pub fn section_label(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .strong()
            .color(theme.colors.text_dim),
    );
}

/// Draws centred text in a rect (cheap labels inside custom widgets).
pub fn text_in(ui: &Ui, rect: Rect, text: &str, size: f32, colour: Color32) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(size),
        colour,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(unit: ParamUnit, min: f64, max: f64) -> ParamInfo {
        ParamInfo {
            id: "x".into(),
            name: "X".into(),
            min,
            max,
            default: min,
            unit,
            stepped: false,
            choices: vec![],
            automatable: true,
        }
    }

    #[test]
    fn units_are_formatted() {
        assert_eq!(
            format_param(&info(ParamUnit::Decibels, -24.0, 24.0), -3.04),
            "-3.0 dB"
        );
        assert_eq!(
            format_param(&info(ParamUnit::Hertz, 20.0, 20000.0), 440.0),
            "440 Hz"
        );
        assert_eq!(
            format_param(&info(ParamUnit::Hertz, 20.0, 20000.0), 6000.0),
            "6.00 kHz"
        );
        assert_eq!(
            format_param(&info(ParamUnit::Seconds, 0.0, 20.0), 0.3),
            "300 ms"
        );
        assert_eq!(
            format_param(&info(ParamUnit::Seconds, 0.0, 20.0), 2.0),
            "2.00 s"
        );
        assert_eq!(
            format_param(&info(ParamUnit::Percent, 0.0, 1.0), 0.5),
            "50%"
        );
        assert_eq!(format_param(&info(ParamUnit::Pan, -1.0, 1.0), -0.25), "L25");
        assert_eq!(format_param(&info(ParamUnit::Pan, -1.0, 1.0), 0.0), "C");
        let mut choice = info(ParamUnit::Choice, 0.0, 2.0);
        choice.choices = vec!["Off".into(), "Forward".into(), "Ping-Pong".into()];
        assert_eq!(format_param(&choice, 1.0), "Forward");
        assert_eq!(format_param(&info(ParamUnit::Switch, 0.0, 1.0), 1.0), "On");
    }

    #[test]
    fn db_conversion() {
        assert!((db_from_linear(1.0)).abs() < 1e-4);
        assert!((db_from_linear(0.5) + 6.02).abs() < 0.01);
    }
}
