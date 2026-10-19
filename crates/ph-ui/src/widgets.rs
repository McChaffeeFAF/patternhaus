//! Custom controls in the approved style (DESIGN section 9): knob, fader, meter, buttons,
//! tabs, LEDs, LCD readouts, and the generic parameter grid built from `ph_node`
//! descriptors. All vector-drawn from theme tokens; every control registers a hint and carries
//! the shared right-click menu.

use crate::hint::{self, HintExt, MenuChoice};
use crate::icons::Icon;
use crate::motion::{self, Ballistics};
use crate::paint;
use crate::theme::{Theme, caps, caps_sized};
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};
use ph_control::UiHint;
use ph_core::ParamPath;
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

/// What a continuous control is called and how it shows its value (for the hover readout and
/// the hint bar).
pub struct Ctl<'a> {
    pub name: &'a str,
    pub fmt: &'a dyn Fn(f64) -> String,
    /// What automation drives through this control, if anything (M2/T7): the control shows an
    /// accent ring when a clip automates it, follows the automated value while the song
    /// plays, offers "Create automation clip", and records its drags when armed.
    pub target: Option<ParamPath>,
}

impl<'a> Ctl<'a> {
    pub fn new(name: &'a str, fmt: &'a dyn Fn(f64) -> String) -> Self {
        Self {
            name,
            fmt,
            target: None,
        }
    }

    pub fn target(mut self, target: ParamPath) -> Self {
        self.target = Some(target);
        self
    }

    pub fn target_opt(mut self, target: Option<ParamPath>) -> Self {
        self.target = target;
        self
    }
}

/// What a control does about its automation this frame: the value to show (the automated one
/// while it plays), and whether a clip drives it.
fn auto_state(ui: &Ui, ctl: &Ctl, dragging: bool) -> (Option<f64>, bool) {
    let Some(t) = ctl.target.as_ref() else {
        return (None, false);
    };
    let st = crate::automation::knob_state(ui.ctx(), t);
    (if dragging { None } else { st.value }, st.automated)
}

/// Remembers an edit for automation recording (armed and playing only).
fn record_edit(ui: &Ui, ctl: &Ctl, edit: Edit) {
    if let (Some(t), Edit::Live(v) | Edit::Commit(v)) = (ctl.target.as_ref(), edit) {
        crate::automation::note_value(ui.ctx(), t, v);
    }
}

/// Formats a 0..1 amount as a percentage.
pub fn fmt_percent(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}
/// Formats a linear gain as dB.
pub fn fmt_gain_db(v: f64) -> String {
    if v <= 0.0005 {
        "-inf dB".into()
    } else {
        format!("{:.1} dB", 20.0 * v.log10())
    }
}
/// Formats -1..1 as L/C/R.
pub fn fmt_pan(v: f64) -> String {
    if v.abs() < 0.005 {
        "C".into()
    } else if v < 0.0 {
        format!("L{:.0}", -v * 100.0)
    } else {
        format!("R{:.0}", v * 100.0)
    }
}

const KNOB_GESTURES: &str = "drag up/down, Shift = fine, double-click resets";

/// Rotary knob: a 270 degree track with a value arc (bipolar knobs fill from the centre), a
/// soft glow, a pointer line and a value readout while hovered or dragged. Vertical drag
/// (Shift = fine), double-click resets to `default`, right-click opens the shared menu.
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
    ctl: &Ctl,
) -> (Response, Edit) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
    let (mut edit, shown) = drag_logic(ui, &resp, id, value, min, max, default, 160.0);
    let (followed, automated) = auto_state(ui, ctl, resp.dragged());
    let shown = followed.map_or(shown, |v| v.clamp(min, max));
    let uh = UiHint {
        name: ctl.name.into(),
        value: (ctl.fmt)(shown),
        gestures: KNOB_GESTURES.into(),
    };
    resp.widget_info(|| egui::WidgetInfo::slider(true, shown, ctl.name));
    let resp = resp.hint(&uh.name, &uh.value, &uh.gestures);
    let menu_target = ctl.target.as_ref().map(|t| (t, shown));
    if hint::standard_menu_auto(&resp, true, &uh, menu_target) == MenuChoice::Reset {
        edit = Edit::Commit(default.clamp(min, max));
    }
    record_edit(ui, ctl, edit);
    let shown = edit.shown(shown);
    let active = resp.dragged();
    let c = &theme.colors;
    let ctx = ui.ctx().clone();
    let t_target = if max > min {
        ((shown - min) / (max - min)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    // the arc eases for ~75 ms; a drag follows the pointer
    let t = motion::ease(
        &ctx,
        id.with("arc"),
        t_target,
        if active { 0.03 } else { 0.075 },
    );
    paint_knob(
        ui.painter(),
        rect,
        t,
        min < 0.0 && max > 0.0,
        -min / (max - min),
        resp.hovered(),
        active,
        automated,
        theme,
    );
    if resp.hovered() || active {
        readout(&ctx, id, rect, &(ctl.fmt)(shown), theme);
    }
    let _ = c;
    (resp, edit)
}

/// The knob's picture: track, value arc with glow, cap, pointer.
#[allow(clippy::too_many_arguments)]
fn paint_knob(
    p: &egui::Painter,
    rect: Rect,
    t: f32,
    bipolar: bool,
    zero_at: f64,
    hovered: bool,
    active: bool,
    automated: bool,
    theme: &Theme,
) {
    let c = &theme.colors;
    let centre = rect.center();
    if automated {
        // the automation ring: a clip drives this control
        p.circle_stroke(
            centre,
            rect.width() * 0.5 - 0.4,
            Stroke::new(1.2, c.accent_hot),
        );
    }
    let d = rect.width();
    let r = d * 0.5 - 1.5;
    let width = (d * 0.11).clamp(2.0, 3.5);
    let (a0, a1) = (std::f32::consts::PI * 0.75, std::f32::consts::PI * 2.25);
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = (((to - from).abs() * 9.0) as usize).clamp(4, 40);
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                centre + Vec2::new(a.cos(), a.sin()) * (r - width * 0.5)
            })
            .collect()
    };
    p.add(Shape::line(arc(a0, a1), Stroke::new(width, c.knob_track)));
    let cur = a0 + (a1 - a0) * t;
    let (from, to) = if bipolar {
        let mid = a0 + (a1 - a0) * zero_at as f32;
        (mid.min(cur), mid.max(cur))
    } else {
        (a0, cur)
    };
    if to > from + 0.01 {
        let k = if active {
            1.0
        } else if hovered {
            0.8
        } else {
            0.55
        };
        paint::glow_path(p, arc(from, to), width, c.accent, k);
    }
    let cap = (r - width - 1.5).max(2.0);
    p.circle_filled(centre, cap, if hovered { c.bg4 } else { c.bg3 });
    p.circle_stroke(
        centre,
        cap,
        Stroke::new(1.0, if active { c.accent } else { c.line }),
    );
    let dir = Vec2::new(cur.cos(), cur.sin());
    p.line_segment(
        [centre + dir * (cap * 0.25), centre + dir * (cap * 0.92)],
        Stroke::new(
            1.8,
            if active || hovered {
                c.accent_hot
            } else {
                c.text
            },
        ),
    );
}

/// The small value readout above a control (on the tooltip layer, so nothing covers it).
fn readout(ctx: &egui::Context, id: Id, anchor: Rect, text: &str, theme: &Theme) {
    let c = &theme.colors;
    let layer = egui::LayerId::new(egui::Order::Tooltip, id.with("readout"));
    let p = ctx.layer_painter(layer);
    let galley = p.layout_no_wrap(
        text.to_string(),
        FontId::new(theme.metrics.font_readout, crate::fonts::mono()),
        c.accent,
    );
    let size = galley.size() + Vec2::new(10.0, 4.0);
    let screen = ctx.content_rect();
    let mut pos = Pos2::new(
        anchor.center().x - size.x / 2.0,
        anchor.top() - size.y - 3.0,
    );
    if pos.y < screen.top() {
        pos.y = anchor.bottom() + 3.0;
    }
    pos.x = pos.x.clamp(
        screen.left() + 2.0,
        (screen.right() - size.x - 2.0).max(screen.left()),
    );
    let r = Rect::from_min_size(pos, size);
    p.rect_filled(r, CornerRadius::same(3), c.lcd);
    paint::glow_rect(&p, r, 3, c.glow_soft, 0.5);
    p.rect_stroke(
        r,
        CornerRadius::same(3),
        Stroke::new(1.0, c.accent_dim),
        StrokeKind::Inside,
    );
    p.galley(r.min + Vec2::new(5.0, 2.0), galley, c.accent);
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

/// Tick marks for a fader: the value and its label.
pub type Ticks<'a> = &'a [(f64, &'a str)];

/// Vertical fader: a recessed track, a cap with a grip, and dB scale ticks. Same editing
/// semantics as [`knob`].
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
    ctl: &Ctl,
    ticks: Ticks,
) -> (Response, Edit) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let (mut edit, shown) = drag_logic(
        ui,
        &resp,
        id,
        value,
        min,
        max,
        default,
        size.y as f64 - 16.0,
    );
    let (followed, automated) = auto_state(ui, ctl, resp.dragged());
    let shown = followed.map_or(shown, |v| v.clamp(min, max));
    let uh = UiHint {
        name: ctl.name.into(),
        value: (ctl.fmt)(shown),
        gestures: KNOB_GESTURES.into(),
    };
    resp.widget_info(|| egui::WidgetInfo::slider(true, shown, ctl.name));
    let resp = resp.hint(&uh.name, &uh.value, &uh.gestures);
    let menu_target = ctl.target.as_ref().map(|t| (t, shown));
    if hint::standard_menu_auto(&resp, true, &uh, menu_target) == MenuChoice::Reset {
        edit = Edit::Commit(default.clamp(min, max));
    }
    record_edit(ui, ctl, edit);
    let shown = edit.shown(shown);
    let c = &theme.colors;
    let p = ui.painter();
    let top = rect.top() + 8.0;
    let bottom = rect.bottom() - 8.0;
    let cx = rect.center().x;
    let y_of = |v: f64| bottom - (bottom - top) * ((v - min) / (max - min)).clamp(0.0, 1.0) as f32;
    // recessed track
    let track = Rect::from_min_max(
        Pos2::new(cx - 3.0, top - 2.0),
        Pos2::new(cx + 3.0, bottom + 2.0),
    );
    paint::well(p, track, 3, c.bg0, theme);
    // scale ticks and labels
    for (v, label) in ticks {
        let y = y_of(*v);
        let major = !label.is_empty();
        let len = if major { 5.0 } else { 3.0 };
        let col = if major { c.dim } else { c.line };
        p.line_segment(
            [Pos2::new(cx + 6.0, y), Pos2::new(cx + 6.0 + len, y)],
            Stroke::new(1.0, col),
        );
        p.line_segment(
            [Pos2::new(cx - 6.0, y), Pos2::new(cx - 6.0 - len, y)],
            Stroke::new(1.0, col),
        );
        if major && rect.width() >= 40.0 {
            p.text(
                Pos2::new(cx - 13.0, y),
                Align2::RIGHT_CENTER,
                label,
                FontId::new(8.5, crate::fonts::mono()),
                c.faint,
            );
        }
    }
    let y = y_of(shown);
    // level fill under the cap
    let fill = Rect::from_min_max(Pos2::new(cx - 2.0, y), Pos2::new(cx + 2.0, bottom + 1.0));
    if fill.height() > 0.5 {
        p.rect_filled(fill, CornerRadius::same(2), c.accent_dim);
        p.rect_filled(
            Rect::from_min_max(Pos2::new(cx - 1.0, y), Pos2::new(cx + 1.0, bottom + 1.0)),
            CornerRadius::same(1),
            c.accent.gamma_multiply(0.7),
        );
    }
    // cap with a grip
    let active = resp.dragged();
    let cap = Rect::from_center_size(Pos2::new(cx, y), Vec2::new(rect.width().min(26.0), 14.0));
    if active || resp.hovered() {
        paint::glow_rect(p, cap, 3, c.glow_soft, if active { 1.0 } else { 0.6 });
    }
    p.rect_filled(
        cap.translate(Vec2::new(0.0, 1.5)),
        CornerRadius::same(3),
        c.bg0,
    );
    p.rect_filled(
        cap,
        CornerRadius::same(3),
        if active { c.bg4 } else { c.bg3 },
    );
    p.rect_stroke(
        cap,
        CornerRadius::same(3),
        Stroke::new(
            1.0,
            if active {
                c.accent
            } else if automated {
                c.accent_hot
            } else {
                c.line
            },
        ),
        StrokeKind::Inside,
    );
    if automated {
        p.rect_stroke(
            cap.expand(2.0),
            CornerRadius::same(4),
            Stroke::new(1.0, c.accent_hot),
            StrokeKind::Outside,
        );
    }
    for dy in [-3.5f32, 3.5] {
        p.line_segment(
            [
                Pos2::new(cap.left() + 4.0, y + dy),
                Pos2::new(cap.right() - 4.0, y + dy),
            ],
            Stroke::new(1.0, c.dim),
        );
    }
    p.line_segment(
        [
            Pos2::new(cap.left() + 3.0, y),
            Pos2::new(cap.right() - 3.0, y),
        ],
        Stroke::new(1.5, if active { c.accent_hot } else { c.accent }),
    );
    if active {
        readout(ui.ctx(), id, cap, &uh.value, theme);
    }
    (resp, edit)
}

pub fn db_from_linear(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}

/// The meter's scale: -60 dB at the bottom, +6 dB at the top.
pub const METER_DB_LO: f32 = -60.0;
pub const METER_DB_HI: f32 = 6.0;

/// Position of a linear level on the meter scale, 0..1.
pub fn meter_norm(lin: f32) -> f32 {
    ((db_from_linear(lin) - METER_DB_LO) / (METER_DB_HI - METER_DB_LO)).clamp(0.0, 1.0)
}

#[derive(Copy, Clone, Default)]
struct MeterState {
    ch: [Ballistics; 2],
    rms: [f32; 2],
}

/// Level meter: accent to warn to clip gradient, a held peak line, a clip LED (click to reset)
/// and a smooth fall-off. The fall-off and the hold live in the UI only; the audio path is
/// untouched. Allocates `size`.
pub fn meter(
    ui: &mut Ui,
    id: Id,
    size: Vec2,
    peak: [f32; 2],
    rms: [f32; 2],
    theme: &Theme,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    paint_meter(ui, id, rect, peak, rms, theme).hint(
        "Level meter",
        "",
        "click to clear the clip LED",
    )
}

/// Paints a meter into `rect` (see [`meter`]); the response is for the hint and the click that
/// clears the clip LEDs.
pub fn paint_meter(
    ui: &Ui,
    id: Id,
    rect: Rect,
    peak: [f32; 2],
    rms: [f32; 2],
    theme: &Theme,
) -> Response {
    let c = &theme.colors;
    let ctx = ui.ctx();
    let now = ctx.input(|i| i.time);
    let smooth = !motion::reduce(ctx);
    let mut st: MeterState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let resp = ui.interact(rect, id.with("clear"), Sense::click());
    if resp.clicked() {
        for b in &mut st.ch {
            b.clip = false;
        }
    }
    let mut moving = false;
    for ch in 0..2 {
        moving |= st.ch[ch].update(peak[ch], now, smooth);
        st.rms[ch] = rms[ch];
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
    if moving {
        // keep falling until the meter settles, then the UI can go idle
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    let p = ui.painter();
    paint::well(p, rect, 2, c.lcd, theme);
    let vertical = rect.height() >= rect.width();
    let inner = rect.shrink(1.0);
    let stops = [
        (0.0, c.accent),
        (0.70, c.accent),
        (0.83, c.warn),
        (0.909, c.clip),
        (1.0, c.clip),
    ];
    for ch in 0..2 {
        let lane = if vertical {
            let w = inner.width() / 2.0;
            Rect::from_min_size(
                inner.min + Vec2::new(w * ch as f32, 0.0),
                Vec2::new(w - 1.0, inner.height()),
            )
        } else {
            let h = inner.height() / 2.0;
            Rect::from_min_size(
                inner.min + Vec2::new(0.0, h * ch as f32),
                Vec2::new(inner.width(), h - 1.0),
            )
        };
        let pos = |norm: f32| {
            if vertical {
                lane.bottom() - lane.height() * norm
            } else {
                lane.left() + lane.width() * norm
            }
        };
        // two layers: the (smoothed) peak dimmer, the rms core solid
        for (norm, alpha) in [
            (meter_norm(st.ch[ch].level), 0.5f32),
            (
                meter_norm(st.rms[ch].min(st.ch[ch].level.max(st.rms[ch]))),
                1.0,
            ),
        ] {
            if norm <= 0.0 {
                continue;
            }
            // the gradient is fixed to the scale, cut at the level
            let mut lo = 0.0f32;
            for w in stops.windows(2) {
                let (s0, s1): (f32, f32) = (w[0].0, w[1].0);
                if s1 <= s0 || lo >= norm {
                    continue;
                }
                let hi = s1.min(norm);
                if hi > s0 {
                    let col0 = paint::sample_stops(&stops, s0.max(lo));
                    let col1 = paint::sample_stops(&stops, hi);
                    let (a, b) = (s0.max(lo), hi);
                    let seg = if vertical {
                        Rect::from_min_max(
                            Pos2::new(lane.left(), pos(b)),
                            Pos2::new(lane.right(), pos(a)),
                        )
                    } else {
                        Rect::from_min_max(
                            Pos2::new(pos(a), lane.top()),
                            Pos2::new(pos(b), lane.bottom()),
                        )
                    };
                    let (cb, ca) = (col1.gamma_multiply(alpha), col0.gamma_multiply(alpha));
                    if vertical {
                        paint::vgradient(p, seg, cb, ca);
                    } else {
                        // horizontal: approximate with the mid colour
                        p.rect_filled(
                            seg,
                            CornerRadius::ZERO,
                            paint::over(c.lcd, ca, 0.5).gamma_multiply(1.0),
                        );
                        p.rect_filled(seg, CornerRadius::ZERO, cb.gamma_multiply(0.5));
                    }
                    lo = hi;
                }
            }
        }
        // held peak line
        let pk = meter_norm(st.ch[ch].peak);
        if pk > 0.0 {
            let line_col = if st.ch[ch].peak > 1.0 {
                c.clip
            } else {
                c.accent_hot
            };
            if vertical {
                let y = pos(pk);
                p.line_segment(
                    [Pos2::new(lane.left(), y), Pos2::new(lane.right(), y)],
                    Stroke::new(1.5, line_col),
                );
            } else {
                let x = pos(pk);
                p.line_segment(
                    [Pos2::new(x, lane.top()), Pos2::new(x, lane.bottom())],
                    Stroke::new(1.5, line_col),
                );
            }
        }
        // clip LED at the end of the lane
        let led_c = if vertical {
            Pos2::new(lane.center().x, rect.top() - 4.0)
        } else {
            Pos2::new(rect.right() + 5.0, lane.center().y)
        };
        led_at(
            p,
            led_c,
            2.0,
            if st.ch[ch].clip { Led::Clip } else { Led::Off },
            theme,
        );
    }
    resp
}

/// LED states.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Led {
    On,
    Off,
    Warn,
    Clip,
}

/// Paints one LED.
pub fn led_at(p: &egui::Painter, centre: Pos2, r: f32, kind: Led, theme: &Theme) {
    let c = &theme.colors;
    match kind {
        Led::Off => {
            p.circle_filled(centre, r, c.bg0);
            p.circle_stroke(centre, r, Stroke::new(1.0, c.line));
        }
        Led::On => paint::glow_dot(p, centre, r, c.accent, 1.0),
        Led::Warn => paint::glow_dot(p, centre, r, c.warn, 1.0),
        Led::Clip => paint::glow_dot(p, centre, r, c.clip, 1.0),
    }
}

/// A standalone LED with a hint.
pub fn led(ui: &mut Ui, kind: Led, name: &str, theme: &Theme) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    led_at(ui.painter(), rect.center(), 3.0, kind, theme);
    resp.hint(
        name,
        match kind {
            Led::Off => "off",
            Led::On => "on",
            Led::Warn => "warning",
            Led::Clip => "clipping",
        },
        "",
    )
}

/// LCD readout: mono accent text with a glow on the `lcd` background. Returns the response.
pub fn lcd(
    ui: &mut Ui,
    theme: &Theme,
    text: &str,
    size: f32,
    min_width: f32,
    name: &str,
) -> Response {
    let c = &theme.colors;
    let font = FontId::new(size, crate::fonts::mono());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), c.accent);
    let w = (galley.size().x + 16.0).max(min_width);
    let h = galley.size().y + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let p = ui.painter();
    paint::well(p, rect, 3, c.lcd, theme);
    paint::lcd_text(
        p,
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font,
        c.accent,
        c.glow_soft,
    );
    resp.hint(name, text, "")
}

/// A flat toggle button (mute, solo, play, ...) in the approved states: default, hover,
/// pressed, toggled-on (accent fill, accent-ink text) and disabled. Uppercased label.
pub fn toggle(ui: &mut Ui, on: bool, label: &str, on_colour: Color32, theme: &Theme) -> Response {
    toggle_enabled(ui, on, true, label, on_colour, theme)
}

pub fn toggle_enabled(
    ui: &mut Ui,
    on: bool,
    enabled: bool,
    label: &str,
    on_colour: Color32,
    theme: &Theme,
) -> Response {
    let c = &theme.colors;
    let galley = ui.painter().layout_no_wrap(
        label.to_uppercase(),
        FontId::new(theme.metrics.font_label, crate::fonts::cond()),
        c.text,
    );
    let size = Vec2::new((galley.size().x + 14.0).max(24.0), 20.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, label));
    let resp = resp.hint(label, if on { "on" } else { "off" }, "click to toggle");
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let (fill, stroke, ink) = state_colours(theme, on, on_colour, hovered, pressed, enabled);
    let p = ui.painter();
    let round = CornerRadius::same(theme.metrics.radius_md as u8);
    if on && enabled {
        paint::glow_rect(
            p,
            rect,
            theme.metrics.radius_md as u8,
            on_colour.gamma_multiply(0.45),
            0.6,
        );
    }
    p.rect_filled(rect, round, fill);
    p.rect_stroke(rect, round, Stroke::new(1.0, stroke), StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, ink);
    let uh = UiHint {
        name: label.into(),
        value: if on { "on".into() } else { "off".into() },
        gestures: "click to toggle".into(),
    };
    hint::standard_menu(&resp, false, &uh);
    resp
}

/// (fill, outline, ink) of a button in each state.
fn state_colours(
    theme: &Theme,
    on: bool,
    on_colour: Color32,
    hovered: bool,
    pressed: bool,
    enabled: bool,
) -> (Color32, Color32, Color32) {
    let c = &theme.colors;
    if !enabled {
        return (c.bg2, c.line_soft, c.faint);
    }
    if on {
        let fill = if pressed {
            paint::over(on_colour, c.bg0, 0.2)
        } else if hovered {
            paint::over(on_colour, c.accent_hot, 0.25)
        } else {
            on_colour
        };
        return (fill, on_colour, c.accent_ink);
    }
    if pressed {
        (c.bg2, c.accent, c.accent_hot)
    } else if hovered {
        (c.bg4, c.accent_dim, c.text)
    } else {
        (c.bg3, c.line, c.text)
    }
}

/// A push button with an uppercased label, the shared right-click menu and a hint. The
/// replacement for `ui.button` / `ui.small_button` in the panels.
pub fn btn(ui: &mut Ui, label: &str) -> Response {
    let r = ui.add(egui::Button::new(caps(label)).min_size(Vec2::new(0.0, 20.0)));
    // the face is uppercase; the accessible name stays as typed
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let r = r.hint(label, "", "click");
    hint::standard_menu(
        &r,
        false,
        &UiHint {
            name: label.into(),
            value: String::new(),
            gestures: "click".into(),
        },
    );
    r
}

/// A menu button with an uppercase label: the hint and the accessible name are as typed.
pub fn menu_button<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> Response {
    menu_button_rich(ui, label, caps(label), add)
}

/// As [`menu_button`] with the label's look (size, tracking) given.
pub fn menu_button_rich<R>(
    ui: &mut Ui,
    label: &str,
    face: egui::RichText,
    add: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let r = ui.menu_button(face, add).response;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    r.hint(label, "", "click to open")
}

/// Widest a menu of user-named entries grows before its entries are shortened with "…".
pub const MENU_W: f32 = 280.0;

/// Runs `add` in a column that leaves `reserve` px of the row free for the controls that follow
/// it, so a long name inside it (`Label::truncate`, `Button::truncate`) is shortened instead of
/// pushing them out of the row or under them.
pub fn leave_room<R>(ui: &mut Ui, reserve: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        // a sideways-scrolling parent offers unbounded room: stop at the visible edge
        let room = ui
            .available_width()
            .min(ui.clip_rect().right() - ui.cursor().left());
        let w = (room - reserve).max(48.0);
        ui.set_max_width(w);
        add(ui)
    })
    .inner
}

/// A menu entry: uppercase face, accessible name as typed, optional shortcut text on the
/// right. No right-click menu of its own (menus are already menus).
pub fn menu_item(ui: &mut Ui, label: &str, shortcut: &str) -> Response {
    // entries named by the user (insert, pattern, preset names) must not stretch the menu
    ui.set_max_width(ui.available_width().min(MENU_W));
    let mut b = egui::Button::new(caps(label)).truncate();
    if !shortcut.is_empty() {
        b = b.shortcut_text(
            egui::RichText::new(shortcut)
                .font(FontId::new(11.0, crate::fonts::mono()))
                .color(ui.visuals().weak_text_color()),
        );
    }
    let r = ui.add(b);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    r.hint(
        label,
        "",
        if shortcut.is_empty() {
            "click"
        } else {
            shortcut
        },
    )
}

/// A menu entry in a colour (destructive entries).
pub fn menu_item_tinted(ui: &mut Ui, label: &str, colour: Color32) -> Response {
    let r = ui.add(egui::Button::new(caps(label).color(colour)));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    r.hint(label, "", "click")
}

/// A push button in a colour (a warning, a destructive choice).
pub fn btn_tinted(ui: &mut Ui, label: &str, colour: Color32) -> Response {
    let r = ui.add(egui::Button::new(caps(label).color(colour)).min_size(Vec2::new(0.0, 20.0)));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    r.hint(label, "", "click")
}

/// As [`btn`], but disabled when `enabled` is false.
pub fn btn_enabled(ui: &mut Ui, enabled: bool, label: &str) -> Response {
    let r = ui.add_enabled(
        enabled,
        egui::Button::new(caps(label)).min_size(Vec2::new(0.0, 20.0)),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    r.hint(label, if enabled { "" } else { "unavailable" }, "click")
}

/// An icon button (26 x 24 px): tools, transport, undo/redo. `on` draws the toggled-on state.
/// `name` is what the control is called (its accessible label, the tooltip's first words and
/// the hint's name); `gestures` says how to use it.
pub fn icon_button(
    ui: &mut Ui,
    theme: &Theme,
    icon: Icon,
    on: bool,
    name: &str,
    gestures: &str,
) -> Response {
    icon_button_coloured(
        ui,
        theme,
        icon,
        on,
        true,
        name,
        gestures,
        theme.colors.accent,
    )
}

pub fn icon_button_enabled(
    ui: &mut Ui,
    theme: &Theme,
    icon: Icon,
    on: bool,
    enabled: bool,
    name: &str,
    gestures: &str,
) -> Response {
    icon_button_coloured(
        ui,
        theme,
        icon,
        on,
        enabled,
        name,
        gestures,
        theme.colors.accent,
    )
}

/// As [`icon_button_enabled`], with the colour the toggled-on state uses.
#[allow(clippy::too_many_arguments)]
pub fn icon_button_coloured(
    ui: &mut Ui,
    theme: &Theme,
    icon: Icon,
    on: bool,
    enabled: bool,
    name: &str,
    gestures: &str,
    on_colour: Color32,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(26.0, 24.0), sense);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, name));
    let tip = if gestures.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {gestures}")
    };
    let resp = resp
        .on_hover_text(tip)
        .hint(name, if on { "on" } else { "" }, gestures);
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let (fill, stroke, ink) = state_colours(theme, on, on_colour, hovered, pressed, enabled);
    let p = ui.painter();
    let round = CornerRadius::same(theme.metrics.radius_md as u8);
    if on && enabled {
        paint::glow_rect(
            p,
            rect,
            theme.metrics.radius_md as u8,
            on_colour.gamma_multiply(0.45),
            0.6,
        );
    }
    // quiet until hovered: icon buttons are chrome, not decoration
    if on || hovered || pressed || !enabled {
        p.rect_filled(rect, round, fill);
        p.rect_stroke(rect, round, Stroke::new(1.0, stroke), StrokeKind::Inside);
    } else {
        p.rect_filled(rect, round, theme.colors.bg3);
        p.rect_stroke(
            rect,
            round,
            Stroke::new(1.0, theme.colors.line_soft),
            StrokeKind::Inside,
        );
    }
    icon.paint(
        p,
        Rect::from_center_size(rect.center(), Vec2::splat(16.0)),
        ink,
    );
    hint::standard_menu(
        &resp,
        false,
        &UiHint {
            name: name.into(),
            value: String::new(),
            gestures: gestures.into(),
        },
    );
    resp
}

/// A tab: uppercased label, accent underline on the active one.
pub fn tab(ui: &mut Ui, theme: &Theme, label: &str, active: bool) -> Response {
    let c = &theme.colors;
    let galley = ui.painter().layout_no_wrap(
        label.to_uppercase(),
        FontId::new(theme.metrics.font_label, crate::fonts::cond()),
        c.text,
    );
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 16.0, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label));
    let resp = resp.hint(label, if active { "shown" } else { "" }, "click to show");
    let p = ui.painter();
    if resp.hovered() && !active {
        p.rect_filled(rect, CornerRadius::same(2), c.bg3);
    }
    let ink = if active {
        c.accent_hot
    } else if resp.hovered() {
        c.text
    } else {
        c.dim
    };
    p.galley(
        rect.center() - galley.size() / 2.0 - Vec2::new(0.0, 1.0),
        galley,
        ink,
    );
    if active {
        let y = rect.bottom() - 1.0;
        paint::glow_line(
            p,
            Pos2::new(rect.left() + 4.0, y),
            Pos2::new(rect.right() - 4.0, y),
            2.0,
            c.accent,
            0.7,
        );
    }
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
    param_grid_with(ui, salt, desc, filter, value_of, &|_| None, theme)
}

/// [`param_grid`] whose knobs are automatable: `target_of` gives the automation target of a
/// parameter (`None` for ones that cannot be automated).
pub fn param_grid_with(
    ui: &mut Ui,
    salt: Id,
    desc: &NodeDescriptor,
    filter: &dyn Fn(&ParamInfo) -> bool,
    value_of: &dyn Fn(&ParamInfo) -> f64,
    target_of: &dyn Fn(&ParamInfo) -> Option<ParamPath>,
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
                            )
                            .hint(
                                &info.name,
                                if on { "on" } else { "off" },
                                "click to toggle",
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
                                })
                                .response
                                .hint_menu(&info.name, &format_param(info, cur), "click to choose");
                            if sel != before {
                                changes.push((info.id.clone(), info.min + sel as f64));
                            }
                        } else if info.stepped {
                            let mut v = cur as i64;
                            let r = ui
                                .add(
                                    egui::DragValue::new(&mut v)
                                        .range(info.min as i64..=info.max as i64),
                                )
                                .hint_menu(
                                    &info.name,
                                    &format!("{v}"),
                                    "drag or type; double-click resets",
                                );
                            if r.double_clicked() {
                                changes.push((info.id.clone(), info.default));
                            } else if r.drag_stopped() || r.lost_focus() {
                                changes.push((info.id.clone(), v as f64));
                            }
                        } else {
                            let fmt = |v: f64| format_param(info, v);
                            let (_, edit) = knob(
                                ui,
                                id,
                                cur,
                                info.min,
                                info.max,
                                info.default,
                                34.0,
                                theme,
                                &Ctl::new(&info.name, &fmt).target_opt(target_of(info)),
                            );
                            if let Some(v) = edit.commit() {
                                changes.push((info.id.clone(), v));
                            }
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
                            caps_sized(
                                &info.name,
                                theme.metrics.font_caption,
                                crate::theme::TRACK_CAPTION,
                            )
                            .color(theme.colors.dim),
                        );
                        if !shown.is_empty() {
                            ui.label(
                                egui::RichText::new(shown)
                                    .font(FontId::new(10.0, crate::fonts::mono()))
                                    .color(theme.colors.text),
                            );
                        }
                    });
                });
            }
        });
    }
    changes
}

/// Longest badge text, in characters.
pub const BADGE_MAX_CHARS: usize = 28;

/// Small coloured badge with text (author tags, counters).
pub fn badge(ui: &mut Ui, text: &str, fill: Color32, theme: &Theme) {
    let ink = theme.colors.accent_ink;
    // a badge measures its text whole: a long client or author name is cut to 28 characters
    let shown = crate::paint::ellipsize(text, BADGE_MAX_CHARS);
    let galley = ui.painter().layout_no_wrap(
        shown.to_uppercase(),
        FontId::new(theme.metrics.font_caption, crate::fonts::cond()),
        ink,
    );
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(8.0, 3.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if shown != text {
        let _ = resp.on_hover_text(text);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(3), fill);
    ui.painter()
        .galley(rect.min + Vec2::new(4.0, 1.5), galley, ink);
}

/// A caption: tiny, uppercase, dim, the widest tracking.
pub fn caption(theme: &Theme, text: &str) -> egui::RichText {
    caps_sized(
        text,
        theme.metrics.font_caption,
        crate::theme::TRACK_CAPTION,
    )
    .color(theme.colors.dim)
}

/// A section name: uppercase, dim, wide tracking.
pub fn section_label(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(
        caps_sized(text, theme.metrics.font_label, crate::theme::TRACK_LABEL)
            .color(theme.colors.dim),
    );
}

/// Draws centred text in a rect (cheap labels inside custom widgets).
pub fn text_in(ui: &Ui, rect: Rect, text: &str, size: f32, colour: Color32) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::new(size, crate::fonts::body()),
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

    #[test]
    fn meter_scale_puts_zero_db_near_the_top() {
        assert_eq!(meter_norm(0.0), 0.0);
        assert!((meter_norm(1.0) - 60.0 / 66.0).abs() < 1e-3);
        assert_eq!(meter_norm(10.0), 1.0);
    }

    #[test]
    fn value_formats() {
        assert_eq!(fmt_percent(0.5), "50%");
        assert_eq!(fmt_gain_db(1.0), "0.0 dB");
        assert_eq!(fmt_gain_db(0.0), "-inf dB");
        assert_eq!(fmt_pan(-0.5), "L50");
    }
}
