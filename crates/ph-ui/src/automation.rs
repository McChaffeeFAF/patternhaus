//! Automation in the UI (M2/T7): what a control needs to know about its automation, how a
//! clip's curve is drawn, and the recorder that turns knob drags into samples.
//!
//! Controls live deep in the widget tree and cannot reach `AppCore`, so the two sides talk
//! through the egui context: once a frame the app publishes an [`AutoView`] (which targets
//! have clips, where the playhead is, whether recording is armed); a control reads it with
//! [`knob_state`], and leaves requests (create a clip, a recorded sample) that the core
//! drains the same frame.

use crate::theme::Theme;
use egui::{Id, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use ph_core::{Clip, ParamPath, Project, Tick};
use std::collections::BTreeSet;
use std::sync::Arc;

const VIEW: &str = "ph_automation_view";
const CREATE: &str = "ph_automation_create";
const SAMPLES: &str = "ph_automation_samples";

/// What the controls may know about automation this frame.
#[derive(Clone, Debug)]
pub struct AutoView {
    pub project: Arc<Project>,
    /// Every target that has an automation clip (on an unmuted track).
    pub targets: BTreeSet<ParamPath>,
    /// Playhead, in ticks.
    pub tick: f64,
    pub playing: bool,
    /// Recording armed: knob drags are written as automation while the song plays.
    pub armed: bool,
}

impl AutoView {
    pub fn new(project: Arc<Project>, tick: f64, playing: bool, armed: bool) -> Self {
        let targets = project
            .clips
            .values()
            .filter_map(|c| c.automation().map(|a| a.target.clone()))
            .collect();
        Self {
            project,
            targets,
            tick,
            playing,
            armed,
        }
    }
}

/// Publishes this frame's view for the controls.
pub fn publish(ctx: &egui::Context, view: AutoView) {
    ctx.data_mut(|d| d.insert_temp(Id::new(VIEW), Arc::new(view)));
}

/// The published view, if any.
pub fn view(ctx: &egui::Context) -> Option<Arc<AutoView>> {
    ctx.data(|d| d.get_temp::<Arc<AutoView>>(Id::new(VIEW)))
}

/// What a control shows for its automation.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct KnobAuto {
    /// A clip drives this control (somewhere in the song).
    pub automated: bool,
    /// While playing, the value automation gives at the playhead (the knob follows it).
    pub value: Option<f64>,
}

pub fn knob_state(ctx: &egui::Context, target: &ParamPath) -> KnobAuto {
    let Some(v) = view(ctx) else {
        return KnobAuto::default();
    };
    if !v.targets.contains(target) {
        return KnobAuto::default();
    }
    KnobAuto {
        automated: true,
        value: if v.playing {
            v.project.automated_value(target, v.tick)
        } else {
            None
        },
    }
}

/// "Create automation clip" was chosen on a control.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateRequest {
    pub target: ParamPath,
    /// The control's value now: the new clip starts there.
    pub value: f64,
}

pub fn request_create(ctx: &egui::Context, target: ParamPath, value: f64) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<CreateRequest>>(Id::new(CREATE))
            .push(CreateRequest { target, value })
    });
}

pub fn take_create_requests(ctx: &egui::Context) -> Vec<CreateRequest> {
    ctx.data_mut(|d| d.remove_temp::<Vec<CreateRequest>>(Id::new(CREATE)))
        .unwrap_or_default()
}

/// A knob moved to `value`: when recording is armed and the song plays, remember it with the
/// playhead position.
pub fn note_value(ctx: &egui::Context, target: &ParamPath, value: f64) {
    let Some(v) = view(ctx) else { return };
    if !(v.armed && v.playing) {
        return;
    }
    let sample = (target.clone(), v.tick.max(0.0) as Tick, value);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(ParamPath, Tick, f64)>>(Id::new(SAMPLES))
            .push(sample)
    });
}

pub fn take_samples(ctx: &egui::Context) -> Vec<(ParamPath, Tick, f64)> {
    ctx.data_mut(|d| d.remove_temp::<Vec<(ParamPath, Tick, f64)>>(Id::new(SAMPLES)))
        .unwrap_or_default()
}

// ---- value ranges and text ---------------------------------------------------------------

/// The range a clip's curve is drawn over: the target's own range, or what the points span.
pub fn display_range(project: &Project, clip: &Clip) -> (f64, f64) {
    let Some(a) = clip.automation() else {
        return (0.0, 1.0);
    };
    let (lo, hi) = a.points.values().fold((f64::MAX, f64::MIN), |m, p| {
        (m.0.min(p.value), m.1.max(p.value))
    });
    let pts = if lo <= hi { Some((lo, hi)) } else { None };
    let pad = |lo: f64, hi: f64| {
        let span = (hi - lo).max(1e-6);
        if hi - lo < 1e-9 {
            (lo - 0.5, hi + 0.5)
        } else {
            (lo - span * 0.1, hi + span * 0.1)
        }
    };
    match &a.target {
        ParamPath::Tempo => {
            let (l, h) = pts.unwrap_or((120.0, 120.0));
            ((l - 10.0).clamp(10.0, 60.0), (h + 10.0).max(200.0))
        }
        t => {
            if let Some(r) = t.fixed_range() {
                return r;
            }
            if let Some(info) = param_info(project, t)
                && info.max > info.min
            {
                return (info.min, info.max);
            }
            match pts {
                Some((l, h)) => pad(l, h),
                None => (0.0, 1.0),
            }
        }
    }
}

/// The descriptor entry of a processor parameter target, if the processor is built in.
pub fn param_info(project: &Project, target: &ParamPath) -> Option<ph_node::ParamInfo> {
    let (desc, id) = match target {
        ParamPath::ChannelParam(c, p) => (
            ph_engine::catalog::instrument_descriptor(&project.channels.get(c)?.instrument).ok()?,
            p,
        ),
        ParamPath::EffectParam {
            insert,
            slot,
            param,
        } => (
            ph_engine::catalog::effect_descriptor(
                &project
                    .insert(*insert)
                    .ok()?
                    .effects
                    .get(*slot as usize)?
                    .effect,
            )
            .ok()?,
            param,
        ),
        _ => return None,
    };
    desc.params.iter().find(|i| i.id == *id).cloned()
}

/// A value of `target` as the control shows it.
pub fn format_value(project: &Project, target: &ParamPath, v: f64) -> String {
    match target {
        ParamPath::Tempo => format!("{v:.1} BPM"),
        ParamPath::ChannelVolume(_) => crate::widgets::fmt_percent(v),
        ParamPath::InsertVolume(_) | ParamPath::InsertSend { .. } => crate::widgets::fmt_gain_db(v),
        ParamPath::ChannelPan(_) | ParamPath::InsertPan(_) => crate::widgets::fmt_pan(v),
        ParamPath::InsertMute(_) => (if v >= 0.5 { "Muted" } else { "On" }).into(),
        _ => match param_info(project, target) {
            Some(info) => crate::widgets::format_param(&info, v),
            None => format!("{v:.3}"),
        },
    }
}

/// What the target is called, for window titles and clip labels.
pub fn target_name(project: &Project, target: &ParamPath) -> String {
    let channel = |c: &ph_core::ChannelId| {
        project
            .channels
            .get(c)
            .map_or_else(|| format!("Channel {}", c.0), |c| c.name.clone())
    };
    let insert = |i: &ph_core::InsertId| match project.insert(*i) {
        Ok(m) if !m.name.trim().is_empty() => m.name.clone(),
        _ if i.0 == 0 => "Master".to_string(),
        _ => format!("Insert {}", i.0),
    };
    let param =
        |t: &ParamPath, id: &str| param_info(project, t).map_or_else(|| id.to_string(), |i| i.name);
    match target {
        ParamPath::Tempo => "Tempo".into(),
        ParamPath::ChannelVolume(c) => format!("{} volume", channel(c)),
        ParamPath::ChannelPan(c) => format!("{} pan", channel(c)),
        ParamPath::ChannelParam(c, p) => format!("{} {}", channel(c), param(target, p)),
        ParamPath::InsertVolume(i) => format!("{} volume", insert(i)),
        ParamPath::InsertPan(i) => format!("{} pan", insert(i)),
        ParamPath::InsertMute(i) => format!("{} mute", insert(i)),
        ParamPath::InsertSend { insert: i, to } => {
            format!("{} send to {}", insert(i), insert(to))
        }
        ParamPath::EffectParam {
            insert: i,
            slot,
            param: p,
        } => format!("{} fx {} {}", insert(i), slot + 1, param(target, p)),
    }
}

// ---- drawing ------------------------------------------------------------------------------

/// Fills the area between a curve (left to right) and the line `bottom`, as one trapezoid per
/// step (the curve is not convex, so a polygon fill would fold over itself).
pub fn fill_under(p: &Painter, line: &[Pos2], bottom: f32, color: egui::Color32) {
    let mut mesh = egui::epaint::Mesh::default();
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let i = mesh.vertices.len() as u32;
        let v = |pos: Pos2| egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        };
        mesh.vertices.extend([
            v(a),
            v(b),
            v(Pos2::new(b.x, bottom.max(b.y))),
            v(Pos2::new(a.x, bottom.max(a.y))),
        ]);
        mesh.indices.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
    }
    p.add(Shape::mesh(mesh));
}

/// The curve of an automation clip inside its rectangle on the playlist (`r` is the clip's
/// full rectangle; the header strip at the top is left clear).
pub fn paint_clip_curve(
    p: &Painter,
    r: Rect,
    project: &Project,
    clip: &Clip,
    px_per_tick: f64,
    theme: &Theme,
) {
    let Some(a) = clip.automation() else { return };
    if a.points.is_empty() || r.width() < 3.0 {
        return;
    }
    let c = &theme.colors;
    let body = Rect::from_min_max(
        Pos2::new(r.left(), r.top() + 14.0f32.min(r.height() * 0.5)),
        r.max,
    )
    .shrink2(Vec2::new(0.0, 2.0));
    if body.height() < 4.0 {
        return;
    }
    let (lo, hi) = display_range(project, clip);
    let y_of = |v: f64| {
        let t = if hi > lo {
            ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        body.bottom() - body.height() * t as f32
    };
    let n = ((r.width() / 2.0) as usize).clamp(2, 600);
    let step = clip.length as f64 / n as f64;
    let pts: Vec<Pos2> = (0..=n)
        .filter_map(|i| {
            let t = i as f64 * step;
            let v = a.value_at(t + clip.offset as f64)?;
            let x = r.left() + (t * px_per_tick) as f32;
            Some(Pos2::new(x.min(r.right()), y_of(v)))
        })
        .collect();
    if pts.len() < 2 {
        return;
    }
    // a faint fill under the line, then the line
    fill_under(p, &pts, body.bottom(), c.accent.gamma_multiply(0.10));
    p.add(Shape::line(pts, Stroke::new(1.5, c.accent)));
    // the points, when there is room to see them
    if px_per_tick * 240.0 > 3.0 {
        for pt in a.points.values() {
            let x = r.left() + ((pt.tick as f64 - clip.offset as f64) * px_per_tick) as f32;
            if x >= r.left() && x <= r.right() {
                p.circle_filled(Pos2::new(x, y_of(pt.value)), 2.0, c.accent_hot);
            }
        }
    }
}
