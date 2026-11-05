//! The Haus Sampler window (M3/T5): the instrument window of a channel that plays a
//! [`HausInstrument`], in the FL window style (one floating window per channel).
//!
//! - **Zone map:** keys across, velocity up, one rectangle per zone coloured by its group.
//!   Click selects, Shift-click and a rubber band select several; dragging a zone moves it,
//!   dragging an edge resizes it (one `sampler.zone_move` per drag, sent as one batch when
//!   several zones change: one undo step). Ctrl+wheel zooms, the wheel scrolls.
//! - **Keyboard strip** under the map: which keys play a zone, the keyswitches in their
//!   articulation's colour, the articulation last switched to; click to audition.
//! - **Tabs:** the zone inspector, the group list, the articulations, the modulation matrix,
//!   the selected group's envelopes, filter and LFOs, the instrument's global settings (the
//!   generic panel; skins are M3b) and the import report.
//!
//! Nothing here changes the model: every edit is a request from [`mapping_sampler`], sent as
//! the human, so undo, co-editing and the agent's `get_state { view: zones }` stay in step.

use crate::core::{AppCore, Dialog, PanelAction, ToastKind};
use crate::hint::HintExt;
use crate::mapping_sampler::{self as ms, ArticulationEdit, Place};
use crate::sampler_map::{self as map, Grab, MapView, ZoneShape};
use crate::theme::{Theme, rgb};
use crate::widgets::{self, Ctl, btn};
use crate::workspace::WindowId;
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};
use ph_commands::Request;
use ph_core::{
    ArticulationId, ArticulationTrigger, ChannelId, Envelope, Filter, FilterKind, GroupId,
    HausInstrument, ImportNoteKind, InstrumentId, KeyRange, Lfo, LfoShape, ModCurve, ModRoute,
    ModSource, ModTarget, Polyphony, RoundRobinMode, StealMode, VelRange, Zone, ZoneId,
    ZoneTrigger,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The tabs under the map.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Zones,
    Groups,
    Articulations,
    Modulation,
    Sound,
    Instrument,
    Report,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Zones,
        Tab::Groups,
        Tab::Articulations,
        Tab::Modulation,
        Tab::Sound,
        Tab::Instrument,
        Tab::Report,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Zones => "Zone",
            Tab::Groups => "Groups",
            Tab::Articulations => "Articulations",
            Tab::Modulation => "Modulation",
            Tab::Sound => "Envelopes, filter, LFOs",
            Tab::Instrument => "Instrument",
            Tab::Report => "Import report",
        }
    }
}

/// A drag on the map in progress.
#[derive(Clone, Debug)]
struct MapDrag {
    grab: Grab,
    origin: Pos2,
    zones: Vec<Zone>,
    /// (keys, velocity steps) so far.
    delta: (i32, i32),
}

/// Where the window drew its parts this frame (for hit tests, tests and screenshots).
#[derive(Clone, Debug)]
pub struct Drawn {
    pub map: Option<MapView>,
    pub strip: Rect,
    pub zones: Vec<ZoneShape>,
}

impl Default for Drawn {
    fn default() -> Self {
        Self {
            map: None,
            strip: Rect::NOTHING,
            zones: Vec::new(),
        }
    }
}

/// The view state of one instrument's window. Nothing in it is project data.
#[derive(Clone, Debug)]
pub struct SamplerView {
    pub selected: BTreeSet<ZoneId>,
    pub key0: f32,
    pub key_w: f32,
    fitted: bool,
    pub tab: Tab,
    /// The group the envelope/filter/LFO tab edits.
    pub group: Option<GroupId>,
    /// The articulation last switched to from the keyboard strip (what the engine plays
    /// after that keyswitch). `None` = the instrument's default.
    pub articulation: Option<ArticulationId>,
    held: Option<u8>,
    drag: Option<MapDrag>,
    band: Option<(Pos2, Pos2)>,
    pub drawn: Drawn,
}

impl Default for SamplerView {
    fn default() -> Self {
        Self {
            selected: BTreeSet::new(),
            key0: 24.0,
            key_w: 10.0,
            fitted: false,
            tab: Tab::Zones,
            group: None,
            articulation: None,
            held: None,
            drag: None,
            band: None,
            drawn: Drawn::default(),
        }
    }
}

impl SamplerView {
    /// Whether a drag on the map is in progress.
    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.band.is_some()
    }
}

fn to(v: impl serde::Serialize) -> Value {
    serde_json::to_value(v).unwrap_or_default()
}

/// The instrument a channel plays, if it is a Haus Sampler with one.
pub fn instrument_of(core: &AppCore, channel: ChannelId) -> Option<InstrumentId> {
    match core.project.channels.get(&channel)?.instrument {
        ph_core::InstrumentSpec::MultiSampler {
            instrument: Some(i),
            ..
        } => Some(i),
        _ => None,
    }
}

/// A group's colour on the map: its own, else the theme's object colour for its place.
pub fn group_colour(theme: &Theme, inst: &HausInstrument, g: GroupId) -> Color32 {
    match inst.groups.get(&g) {
        Some(gr) if gr.color != 0 => rgb(gr.color),
        _ => {
            let i = inst.groups.keys().position(|k| *k == g).unwrap_or(0);
            rgb(theme.object_color(i))
        }
    }
}

fn articulation_colour(theme: &Theme, inst: &HausInstrument, a: ArticulationId) -> Color32 {
    let i = inst
        .articulations
        .iter()
        .position(|x| x.id == a)
        .unwrap_or(0);
    match inst.articulations.get(i).and_then(|x| x.color) {
        Some(c) => rgb(c),
        None => rgb(theme.object_color(i + 2)),
    }
}

fn file_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

fn sample_label(z: &Zone) -> String {
    match &z.sample {
        ph_core::SampleRef::File(p) => file_name(p).to_string(),
        ph_core::SampleRef::Registry(s) => format!("sample {s}"),
    }
}

// ---- the window --------------------------------------------------------------------------

/// The instrument window of a Haus Sampler channel.
pub fn show(ui: &mut Ui, core: &mut AppCore, channel: ChannelId) {
    let theme = core.theme.clone();
    let project = core.project.clone();
    let Some(id) = instrument_of(core, channel) else {
        ui.label("This channel plays no Haus Sampler instrument yet.");
        if btn(ui, "Load Haus instrument…").clicked() {
            core.dialog = Some(Dialog::SamplerLoad(LoadForm::new(Some(channel))));
        }
        return;
    };
    let Ok(inst) = project.instrument(id) else {
        ui.label("The instrument this channel plays no longer exists.");
        return;
    };
    let mut view = core.sampler.remove(&id).unwrap_or_default();
    view.selected.retain(|z| inst.zones.contains_key(z));
    if view.group.is_none_or(|g| !inst.groups.contains_key(&g)) {
        view.group = inst.groups.keys().next().copied();
    }
    {
        header(ui, core, inst, channel, &theme);
        ui.add_space(4.0);
        map_toolbar(ui, core, inst, &mut view, &theme);
        let map_h = (ui.available_height() * 0.42).clamp(120.0, 340.0);
        zone_map(ui, core, inst, &mut view, map_h, &theme);
        keyboard_strip(ui, core, inst, channel, &mut view, &theme);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for t in Tab::ALL {
                if widgets::tab(ui, &theme, t.title(), view.tab == t).clicked() {
                    view.tab = t;
                }
            }
        });
        egui::ScrollArea::vertical()
            .id_salt(("sampler_tab", id))
            .auto_shrink(false)
            .show(ui, |ui| match view.tab {
                Tab::Zones => zone_inspector(ui, core, inst, &mut view, &theme),
                Tab::Groups => group_list(ui, core, inst, &mut view, &theme),
                Tab::Articulations => articulations(ui, core, inst, &mut view, &theme),
                Tab::Modulation => modulation(ui, core, inst, &theme),
                Tab::Sound => group_sound(ui, core, inst, &mut view, &theme),
                Tab::Instrument => generic_panel(ui, core, inst, &theme),
                Tab::Report => import_report(ui, inst, &theme),
            });
    }
    core.sampler.insert(id, view);
}

fn header(ui: &mut Ui, core: &mut AppCore, inst: &HausInstrument, ch: ChannelId, theme: &Theme) {
    let c = &theme.colors;
    ui.horizontal(|ui| {
        let nid = Id::new(("sampler_name", inst.id));
        let mut name: String = ui
            .data(|d| d.get_temp(nid))
            .unwrap_or_else(|| inst.name.clone());
        let r = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(180.0)
                .font(FontId::new(15.0, crate::fonts::body_medium())),
        );
        r.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Instrument name")
        });
        let r = r.hint(
            "Instrument name",
            &inst.name,
            "type to rename; Enter applies",
        );
        if r.has_focus() {
            ui.data_mut(|d| d.insert_temp(nid, name.clone()));
        } else {
            ui.data_mut(|d| d.remove::<String>(nid));
        }
        if r.lost_focus() && name.trim() != inst.name && !name.trim().is_empty() {
            core.send(ms::set(inst.id, Some(name.trim()), &[]));
        }
        let source = inst
            .source
            .as_ref()
            .map(|s| s.format.to_uppercase())
            .unwrap_or_else(|| "HAUS".into());
        widgets::badge(ui, &source, c.accent_dim, theme);
        let linked = inst
            .folder
            .as_deref()
            .is_some_and(|f| !f.starts_with("instruments/"));
        if linked {
            widgets::badge(ui, "linked", c.warn, theme);
        }
        ui.label(
            RichText::new(format!(
                "{} zones · {} groups",
                inst.zones.len(),
                inst.groups.len()
            ))
            .color(c.dim),
        );
    });
    ui.horizontal_wrapped(|ui| {
        if btn(ui, "Save as Haus instrument…")
            .hint(
                "Save as Haus instrument",
                "",
                "writes instrument.toml and its samples to a folder (sampler.save_native)",
            )
            .clicked()
        {
            core.dialog = Some(Dialog::SamplerSave(SaveForm::new(inst)));
        }
        let can_collect = inst
            .folder
            .as_deref()
            .is_some_and(|f| !f.starts_with("instruments/"));
        if widgets::btn_enabled(ui, can_collect, "Collect into project")
            .hint(
                "Collect into project",
                "",
                "copies the library samples into the project folder (sampler.collect)",
            )
            .clicked()
        {
            core.send(ms::collect(inst.id));
        }
        if btn(ui, "Load Haus instrument…")
            .hint(
                "Load Haus instrument",
                "",
                "replaces this channel's instrument with a saved one (sampler.load)",
            )
            .clicked()
        {
            core.dialog = Some(Dialog::SamplerLoad(LoadForm::new(Some(ch))));
        }
    });
}

fn map_toolbar(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    view: &mut SamplerView,
    theme: &Theme,
) {
    ui.horizontal(|ui| {
        widgets::section_label(ui, "Zone map", theme);
        if btn(ui, "Fit").clicked() {
            view.fitted = false;
        }
        if btn(ui, "Zoom in").clicked() {
            view.key_w = (view.key_w * 1.25).min(map::KEY_W_MAX);
        }
        if btn(ui, "Zoom out").clicked() {
            view.key_w = (view.key_w / 1.25).max(map::KEY_W_MIN);
        }
        if btn(ui, "Select all").clicked() {
            view.selected = inst.zones.keys().copied().collect();
        }
        let n = view.selected.len();
        if n > 0 {
            ui.label(RichText::new(format!("{n} selected")).color(theme.colors.accent));
            if widgets::btn_tinted(ui, "Remove zones", theme.colors.clip).clicked() {
                let ids: Vec<ZoneId> = view.selected.iter().copied().collect();
                core.send(ms::zone_remove(inst.id, &ids));
                view.selected.clear();
            }
        }
    });
}

fn zone_map(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    view: &mut SamplerView,
    height: f32,
    theme: &Theme,
) {
    let c = &theme.colors;
    let width = ui.available_width().max(120.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Zone map"));
    let mut mv = MapView {
        rect,
        key0: view.key0,
        key_w: view.key_w,
    };
    if !view.fitted {
        // the played keys and the keyswitches
        let (_, range) = map::coverage(inst);
        let (mut lo, mut hi) = range.unwrap_or((36, 84));
        for a in &inst.articulations {
            if let ArticulationTrigger::Key { key } = a.trigger {
                lo = lo.min(key);
                hi = hi.max(key);
            }
        }
        mv.fit(lo, hi);
        view.fitted = true;
    }
    mv.clamp();
    // wheel: Ctrl zooms, otherwise scrolls the keys
    if resp.hovered() {
        let (scroll, ctrl, at) = ui.input(|i| {
            (
                i.smooth_scroll_delta(),
                i.modifiers.command,
                i.pointer.hover_pos(),
            )
        });
        if scroll != Vec2::ZERO {
            if ctrl {
                mv.zoom(1.0 + scroll.y * 0.003, at.map_or(rect.center().x, |p| p.x));
            } else {
                mv.scroll(-(scroll.x + scroll.y));
            }
        }
    }
    let shapes = map::layout(inst, &mv);

    // gestures
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.drag_started()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
    {
        match map::hit_prefer(&shapes, o, |z| view.selected.contains(&z)) {
            Some((z, grab)) => {
                if !view.selected.contains(&z) {
                    if !shift {
                        view.selected.clear();
                    }
                    view.selected.insert(z);
                }
                // an edge resizes only the zone it belongs to... unless it is part of the
                // selection: then every selected zone moves that edge
                let zones: Vec<Zone> = view
                    .selected
                    .iter()
                    .filter_map(|id| inst.zones.get(id).cloned())
                    .collect();
                view.drag = Some(MapDrag {
                    grab,
                    origin: o,
                    zones,
                    delta: (0, 0),
                });
            }
            None => view.band = Some((o, o)),
        }
    }
    if let Some(p) = resp.interact_pointer_pos() {
        if let Some(d) = &mut view.drag {
            let dk = ((p.x - d.origin.x) / mv.key_w).round() as i32;
            let dv = (-(p.y - d.origin.y) / mv.vel_h()).round() as i32;
            d.delta = (dk, dv);
        }
        if let Some((a, _)) = view.band {
            view.band = Some((a, p));
        }
    }
    if resp.drag_stopped() {
        if let Some(d) = view.drag.take() {
            let reqs = map::drag_requests(inst.id, &d.zones, d.grab, d.delta.0, d.delta.1);
            send_all(core, reqs);
        }
        if let Some((a, b)) = view.band.take() {
            let hits = map::in_area(&shapes, Rect::from_two_pos(a, b));
            if !shift {
                view.selected.clear();
            }
            view.selected.extend(hits);
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match map::hit(&shapes, p) {
            Some((z, _)) if shift => {
                if !view.selected.remove(&z) {
                    view.selected.insert(z);
                }
            }
            Some((z, _)) => {
                view.selected.clear();
                view.selected.insert(z);
            }
            None if !shift => view.selected.clear(),
            None => {}
        }
        view.tab = Tab::Zones;
    }
    // the cursor tells what a press would grab
    if let Some(p) = resp.hover_pos()
        && view.drag.is_none()
    {
        match map::hit(&shapes, p) {
            Some((_, Grab::Left | Grab::Right)) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal)
            }
            Some((_, Grab::Top | Grab::Bottom)) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical)
            }
            Some((_, Grab::Move)) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
            None => {}
        }
    }
    let hover_text = resp
        .hover_pos()
        .map(|p| {
            let k = mv.key_at(p.x).floor().clamp(0.0, 127.0) as u8;
            let v = mv.vel_at(p.y).floor().clamp(0.0, 127.0) as u8;
            match map::hit(&shapes, p).and_then(|(z, _)| inst.zones.get(&z)) {
                Some(z) => format!(
                    "{}: {}-{} vel {}-{} root {}",
                    sample_label(z),
                    map::key_name(z.keys.lo),
                    map::key_name(z.keys.hi),
                    z.velocity.lo,
                    z.velocity.hi,
                    map::key_name(z.root_key)
                ),
                None => format!("{} vel {v}", map::key_name(k)),
            }
        })
        .unwrap_or_default();
    resp.hint(
        "Zone map",
        &hover_text,
        "click selects, Shift-click and a box add; drag to move, drag an edge to resize; Ctrl+wheel zooms",
    );

    // drawing
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(3), c.bg0);
    for v in [32.0, 64.0, 96.0] {
        let y = mv.y_of(v);
        p.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.0, c.grid_minor),
        );
    }
    let k_lo = mv.key0.floor().max(0.0) as i32;
    let k_hi = (mv.key_at(rect.right()).ceil() as i32).min(128);
    for k in k_lo..=k_hi {
        let x = mv.x_of(k as f32);
        if k % 12 == 0 {
            p.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0, c.grid_bar),
            );
            p.text(
                Pos2::new(x + 2.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                map::key_name(k as u8),
                FontId::new(theme.metrics.font_caption, crate::fonts::cond()),
                c.faint,
            );
        } else if mv.key_w >= 8.0 {
            p.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0, c.grid_minor.gamma_multiply(0.5)),
            );
        }
    }
    // zones (a drag shows the dragged ones where they would land)
    let dragged: Option<(Grab, (i32, i32), ((i32, i32), (i32, i32)))> = view
        .drag
        .as_ref()
        .map(|d| (d.grab, d.delta, map::move_limits(&d.zones)));
    let font = FontId::new(theme.metrics.font_caption, crate::fonts::body());
    for s in &shapes {
        let Some(z) = inst.zones.get(&s.zone) else {
            continue;
        };
        let col = group_colour(theme, inst, z.group);
        let selected = view.selected.contains(&s.zone);
        let fill = col.gamma_multiply(if selected { 0.55 } else { 0.32 });
        let band = s.band.shrink(0.5);
        if s.outlined {
            p.rect_stroke(
                band,
                CornerRadius::same(2),
                Stroke::new(if selected { 2.0 } else { 1.5 }, col),
                StrokeKind::Inside,
            );
        } else {
            p.rect_filled(band, CornerRadius::same(2), fill);
            p.rect_stroke(
                band,
                CornerRadius::same(2),
                Stroke::new(1.0, col.gamma_multiply(0.9)),
                StrokeKind::Inside,
            );
        }
        if selected {
            p.rect_stroke(
                band,
                CornerRadius::same(2),
                Stroke::new(2.0, c.selection),
                StrokeKind::Inside,
            );
        }
        if band.width() > 36.0 && band.height() > 12.0 {
            let mut label = sample_label(z);
            if !s.label.is_empty() {
                label = format!("{} {label}", s.label);
            }
            if s.outlined {
                label = format!("REL {label}");
            }
            let text = crate::paint::fit_str(&p, &label, font.clone(), band.width() - 6.0);
            // a release outline usually sits on its attack zone: its name goes at the bottom
            let (at, align) = if s.outlined {
                (
                    band.left_bottom() + Vec2::new(3.0, -2.0),
                    Align2::LEFT_BOTTOM,
                )
            } else {
                (band.left_top() + Vec2::new(3.0, 2.0), Align2::LEFT_TOP)
            };
            p.text(at, align, text, font.clone(), c.text);
        }
        if let Some((grab, (dk, dv), allowed)) = dragged
            && selected
        {
            let (k, v) = map::apply(z, grab, dk, dv, allowed);
            let ghost = mv.range_rect(k, v);
            p.rect_stroke(
                ghost.shrink(0.5),
                CornerRadius::same(2),
                Stroke::new(1.5, c.accent),
                StrokeKind::Inside,
            );
        }
    }
    if let Some((a, b)) = view.band {
        let r = Rect::from_two_pos(a, b);
        p.rect_filled(r, CornerRadius::ZERO, c.tint);
        p.rect_stroke(
            r,
            CornerRadius::ZERO,
            Stroke::new(1.0, c.accent),
            StrokeKind::Inside,
        );
    }
    if inst.zones.is_empty() {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No zones yet: drop samples or an instrument file on the channel rack",
            FontId::new(theme.metrics.font_body, crate::fonts::body()),
            c.dim,
        );
    }
    view.key0 = mv.key0;
    view.key_w = mv.key_w;
    view.drawn.map = Some(mv);
    view.drawn.zones = shapes;
}

fn send_all(core: &mut AppCore, mut reqs: Vec<Request>) {
    match reqs.len() {
        0 => {}
        1 => core.send(reqs.remove(0)),
        _ => core.send_batch(reqs),
    }
}

/// The articulation a keyswitch key selects.
fn keyswitch_at(inst: &HausInstrument, key: u8) -> Option<ArticulationId> {
    inst.articulations.iter().find_map(|a| match a.trigger {
        ArticulationTrigger::Key { key: k } if k == key => Some(a.id),
        _ => None,
    })
}

/// The articulation the instrument plays now as far as the window knows: the last one
/// switched to from the strip, else the default, else the first.
pub fn current_articulation(inst: &HausInstrument, view: &SamplerView) -> Option<ArticulationId> {
    view.articulation
        .filter(|a| inst.articulation(*a).is_some())
        .or(inst.settings.default_articulation)
        .or_else(|| inst.articulations.first().map(|a| a.id))
}

fn keyboard_strip(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    channel: ChannelId,
    view: &mut SamplerView,
    theme: &Theme,
) {
    let c = &theme.colors;
    let Some(mv) = view.drawn.map else {
        return;
    };
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(mv.rect.width(), 34.0), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Keyboard strip"));
    view.drawn.strip = rect;
    let strip = MapView { rect, ..mv };
    let (cover, _) = map::coverage(inst);
    let current = current_articulation(inst, view);
    let key_under = |x: f32| strip.key_at(x).floor().clamp(0.0, 127.0) as u8;
    // audition: press plays the key, release stops it
    let pressed = ui.input(|i| i.pointer.primary_pressed());
    let released = ui.input(|i| i.pointer.primary_released());
    if resp.is_pointer_button_down_on()
        && pressed
        && let Some(p) = resp.interact_pointer_pos()
    {
        let k = key_under(p.x);
        core.preview(channel, k, true);
        view.held = Some(k);
        if let Some(a) = keyswitch_at(inst, k) {
            view.articulation = Some(a);
        }
    }
    if (released || !resp.is_pointer_button_down_on())
        && let Some(k) = view.held.take()
    {
        core.preview(channel, k, false);
    }
    let hover = resp.hover_pos().map(|p| key_under(p.x));
    let hover_text = hover
        .map(
            |k| match keyswitch_at(inst, k).and_then(|a| inst.articulation(a)) {
                Some(a) => format!("{}: keyswitch {}", map::key_name(k), a.name),
                None if cover[k as usize] => format!("{}: plays", map::key_name(k)),
                None => format!("{}: no zone", map::key_name(k)),
            },
        )
        .unwrap_or_default();
    resp.hint(
        "Keyboard",
        &hover_text,
        "click a key to hear it; a coloured key switches articulation",
    );

    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::ZERO, c.bg1);
    let lo = strip.key0.floor().max(0.0) as i32;
    let hi = (strip.key_at(rect.right()).ceil() as i32).min(127);
    let cover_h = 4.0;
    for k in lo..=hi {
        let k8 = k as u8;
        let kr = Rect::from_min_max(
            Pos2::new(strip.x_of(k as f32), rect.top() + cover_h + 1.0),
            Pos2::new(strip.x_of(k as f32 + 1.0), rect.bottom()),
        )
        .shrink2(Vec2::new(0.5, 0.0));
        let ks = keyswitch_at(inst, k8);
        let fill = match ks {
            Some(a) => articulation_colour(theme, inst, a),
            None if map::is_black(k8) => c.bg0,
            None => crate::paint::over(c.bg4, c.text, 0.6),
        };
        p.rect_filled(kr, CornerRadius::same(1), fill);
        if view.held == Some(k8) {
            p.rect_filled(kr, CornerRadius::same(1), c.accent.gamma_multiply(0.5));
        }
        if let Some(a) = ks
            && Some(a) == current
        {
            crate::paint::glow_rect(&p, kr, 2, c.accent, 0.8);
            p.rect_stroke(
                kr,
                CornerRadius::same(1),
                Stroke::new(2.0, c.accent),
                StrokeKind::Inside,
            );
        }
        if cover[k as usize] {
            p.rect_filled(
                Rect::from_min_max(
                    Pos2::new(kr.left(), rect.top()),
                    Pos2::new(kr.right(), rect.top() + cover_h),
                ),
                CornerRadius::ZERO,
                c.accent_dim,
            );
        }
        if k % 12 == 0 && strip.key_w >= 6.0 {
            p.text(
                Pos2::new(kr.center().x, kr.bottom() - 1.0),
                Align2::CENTER_BOTTOM,
                format!("C{}", k / 12 - 1),
                FontId::new(9.0, crate::fonts::cond()),
                c.bg1,
            );
        }
    }
    if let Some(a) = current.and_then(|a| inst.articulation(a)) {
        ui.label(
            RichText::new(format!("Articulation: {}", a.name))
                .size(theme.metrics.font_caption + 1.0)
                .color(c.accent),
        );
    }
}

// ---- small editing helpers -----------------------------------------------------------------

/// A number field that shows edits as they happen and commits once, when the drag ends or
/// the typing is done. Returns the new value then.
#[allow(clippy::too_many_arguments)]
fn num(
    ui: &mut Ui,
    id: Id,
    label: &str,
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
    decimals: usize,
    suffix: &str,
) -> Option<f64> {
    let mut v: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(value);
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .range(range)
            .speed(speed)
            .max_decimals(decimals)
            .suffix(suffix),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, label));
    let r = r.hint(label, &format!("{v}{suffix}"), "drag or type a value");
    let done = r.drag_stopped() || r.lost_focus();
    if r.dragged() || r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, v));
    } else {
        ui.data_mut(|d| d.remove::<f64>(id));
    }
    if done {
        ui.data_mut(|d| d.remove::<f64>(id));
        if (v - value).abs() > 1e-9 {
            return Some(v);
        }
    }
    None
}

/// A key field showing note names.
fn key_field(ui: &mut Ui, id: Id, label: &str, value: u8) -> Option<u8> {
    let mut v: i32 = ui.data(|d| d.get_temp(id)).unwrap_or(value as i32);
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .range(0..=127)
            .speed(0.2)
            .custom_formatter(|v, _| map::key_name(v as u8))
            .custom_parser(|s| parse_key(s).map(|k| k as f64)),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, label));
    let r = r.hint(
        label,
        &map::key_name(v as u8),
        "drag, or type a note name (C4) or number",
    );
    let done = r.drag_stopped() || r.lost_focus();
    if r.dragged() || r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, v));
    } else {
        ui.data_mut(|d| d.remove::<i32>(id));
    }
    if done {
        ui.data_mut(|d| d.remove::<i32>(id));
        if v != value as i32 {
            return Some(v.clamp(0, 127) as u8);
        }
    }
    None
}

/// "C4", "c#3", "Eb2" or a number 0..=127.
pub fn parse_key(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i32>() {
        return (0..=127).contains(&n).then_some(n as u8);
    }
    ph_sampler_formats::automap::root_from_name(s)
}

/// A combo box over named choices; returns the new choice.
fn choice<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    let mut out = None;
    let shown = options
        .iter()
        .find(|(v, _)| *v == current)
        .map_or("?", |(_, n)| *n);
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(110.0)
        .show_ui(ui, |ui| {
            for (v, n) in options {
                if ui.selectable_label(*v == current, *n).clicked() && *v != current {
                    out = Some(*v);
                }
            }
        })
        .response
        .hint(label, shown, "click to choose");
    out
}

/// A knob for one value; returns the committed value.
#[allow(clippy::too_many_arguments)]
fn knob(
    ui: &mut Ui,
    id: Id,
    name: &str,
    value: f64,
    (min, max, default): (f64, f64, f64),
    fmt: &dyn Fn(f64) -> String,
    theme: &Theme,
) -> Option<f64> {
    let mut out = None;
    ui.vertical(|ui| {
        ui.set_width(58.0);
        let (_, edit) = widgets::knob(
            ui,
            id,
            value,
            min,
            max,
            default,
            34.0,
            theme,
            &Ctl::new(name, fmt),
        );
        ui.label(widgets::caption(theme, name));
        out = edit.commit();
    });
    out
}

/// Knob position (0..1) for a time of 0..`max` seconds, cube-root scaled.
fn time_to_knob(t: f32, max: f32) -> f64 {
    ((t / max).clamp(0.0, 1.0) as f64).cbrt()
}
fn knob_to_time(x: f64, max: f32) -> f32 {
    ((x.clamp(0.0, 1.0)).powi(3) * max as f64) as f32
}
fn fmt_time(max: f32) -> impl Fn(f64) -> String {
    move |x| {
        let t = knob_to_time(x, max);
        if t < 1.0 {
            format!("{:.0} ms", t * 1000.0)
        } else {
            format!("{t:.2} s")
        }
    }
}

// ---- tabs ----------------------------------------------------------------------------------

fn zone_inspector(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    view: &mut SamplerView,
    theme: &Theme,
) {
    let c = &theme.colors;
    let ids: Vec<ZoneId> = view.selected.iter().copied().collect();
    let Some(z) = ids.first().and_then(|id| inst.zones.get(id)) else {
        ui.label(
            RichText::new("Click a zone to edit it; Shift-click or drag a box to select several.")
                .color(c.dim),
        );
        return;
    };
    if ids.len() > 1 {
        ui.label(
            RichText::new(format!(
                "{} zones selected: a change applies to all of them (shown: {})",
                ids.len(),
                sample_label(z)
            ))
            .color(c.accent),
        );
    } else {
        ui.label(RichText::new(format!("Sample: {}", sample_label(z))).color(c.dim))
            .on_hover_text(match &z.sample {
                ph_core::SampleRef::File(p) => p.clone(),
                ph_core::SampleRef::Registry(s) => format!("registry sample {s}"),
            });
    }
    let i = inst.id;
    let base = Id::new(("zone_insp", i, z.id));
    let mut reqs: Vec<Request> = Vec::new();
    let set = |field: &str, v: Value| ms::zone_set(i, &ids, &[(field, v)]);
    egui::Grid::new(("zone_grid", i))
        .num_columns(4)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("Group");
            let groups: Vec<(GroupId, String)> = inst
                .groups
                .values()
                .map(|g| (g.id, g.name.clone()))
                .collect();
            let opts: Vec<(GroupId, &str)> = groups.iter().map(|(g, n)| (*g, n.as_str())).collect();
            if let Some(g) = choice(ui, ("zone_group", i), "Zone group", z.group, &opts) {
                reqs.push(ms::zone_move(
                    i,
                    &ids,
                    &Place {
                        group: Some(g),
                        root_follows: true,
                        ..Default::default()
                    },
                ));
            }
            ui.label("Root key");
            if let Some(k) = key_field(ui, base.with("root"), "Zone root key", z.root_key) {
                reqs.push(set("root_key", json!(k)));
            }
            ui.end_row();

            ui.label("Keys");
            ui.horizontal(|ui| {
                let lo = key_field(ui, base.with("klo"), "Zone lowest key", z.keys.lo);
                let hi = key_field(ui, base.with("khi"), "Zone highest key", z.keys.hi);
                if lo.is_some() || hi.is_some() {
                    let (a, b) = (lo.unwrap_or(z.keys.lo), hi.unwrap_or(z.keys.hi));
                    reqs.push(ms::zone_move(
                        i,
                        &ids,
                        &Place {
                            keys: Some(KeyRange {
                                lo: a.min(b),
                                hi: a.max(b),
                            }),
                            ..Default::default()
                        },
                    ));
                }
            });
            ui.label("Velocity");
            ui.horizontal(|ui| {
                let lo = num(
                    ui,
                    base.with("vlo"),
                    "Zone lowest velocity",
                    z.velocity.lo as f64,
                    0.0..=127.0,
                    0.3,
                    0,
                    "",
                );
                let hi = num(
                    ui,
                    base.with("vhi"),
                    "Zone highest velocity",
                    z.velocity.hi as f64,
                    0.0..=127.0,
                    0.3,
                    0,
                    "",
                );
                if lo.is_some() || hi.is_some() {
                    let a = lo.map_or(z.velocity.lo, |v| v as u8);
                    let b = hi.map_or(z.velocity.hi, |v| v as u8);
                    reqs.push(ms::zone_move(
                        i,
                        &ids,
                        &Place {
                            velocity: Some(VelRange {
                                lo: a.min(b),
                                hi: a.max(b),
                            }),
                            ..Default::default()
                        },
                    ));
                }
            });
            ui.end_row();

            ui.label("Volume");
            if let Some(v) = num(
                ui,
                base.with("vol"),
                "Zone volume",
                z.volume_db as f64,
                -60.0..=24.0,
                0.1,
                1,
                " dB",
            ) {
                reqs.push(set("volume_db", json!(v)));
            }
            ui.label("Pan");
            if let Some(v) = num(
                ui,
                base.with("pan"),
                "Zone pan",
                z.pan as f64,
                -1.0..=1.0,
                0.01,
                2,
                "",
            ) {
                reqs.push(set("pan", json!(v)));
            }
            ui.end_row();

            ui.label("Tune");
            if let Some(v) = num(
                ui,
                base.with("tune"),
                "Zone tune",
                z.tune as f64,
                -48.0..=48.0,
                0.1,
                0,
                " st",
            ) {
                reqs.push(set("tune", json!(v.round() as i32)));
            }
            ui.label("Fine");
            if let Some(v) = num(
                ui,
                base.with("fine"),
                "Zone fine tune",
                z.fine as f64,
                -100.0..=100.0,
                0.5,
                1,
                " ct",
            ) {
                reqs.push(set("fine", json!(v)));
            }
            ui.end_row();

            ui.label("Start");
            if let Some(v) = num(
                ui,
                base.with("start"),
                "Zone start",
                z.start as f64,
                0.0..=f64::MAX,
                50.0,
                0,
                "",
            ) {
                reqs.push(set("start", json!(v.round() as u64)));
            }
            ui.label("End");
            let end = z.end.unwrap_or(0) as f64;
            if let Some(v) = num(
                ui,
                base.with("end"),
                "Zone end (0 = file end)",
                end,
                0.0..=f64::MAX,
                50.0,
                0,
                "",
            ) {
                reqs.push(set(
                    "end",
                    if v < 1.0 {
                        Value::Null
                    } else {
                        json!(v.round() as u64)
                    },
                ));
            }
            ui.end_row();

            ui.label("Trigger");
            if let Some(t) = choice(
                ui,
                ("zone_trig", i),
                "Zone trigger",
                z.trigger,
                &[
                    (ZoneTrigger::Attack, "Attack"),
                    (ZoneTrigger::Release, "Release"),
                ],
            ) {
                reqs.push(set("trigger", serde_json::to_value(t).unwrap_or_default()));
            }
            ui.label("Round robin");
            let rr = z.round_robin.map_or(0.0, |r| r as f64 + 1.0);
            if let Some(v) = num(
                ui,
                base.with("rr"),
                "Zone round-robin position (0 = always)",
                rr,
                0.0..=64.0,
                0.1,
                0,
                "",
            ) {
                reqs.push(set(
                    "round_robin",
                    if v < 1.0 {
                        Value::Null
                    } else {
                        json!(v.round() as u32 - 1)
                    },
                ));
            }
            ui.end_row();

            ui.label("Release decay");
            if let Some(v) = num(
                ui,
                base.with("reldec"),
                "Zone release decay",
                z.release_decay_db as f64,
                0.0..=200.0,
                0.2,
                1,
                " dB/s",
            ) {
                reqs.push(set("release_decay_db", json!(v)));
            }
            ui.label("Options");
            ui.horizontal(|ui| {
                let mut rev = z.reverse;
                if ui.checkbox(&mut rev, "Reverse").changed() {
                    reqs.push(set("reverse", json!(rev)));
                }
                let mut one = z.one_shot;
                if ui.checkbox(&mut one, "One shot").changed() {
                    reqs.push(set("one_shot", json!(one)));
                }
            });
            ui.end_row();
        });
    if let Some(l) = &z.looping {
        ui.label(
            RichText::new(format!(
                "Loop {:?} {}..{} (crossfade {}), on release {:?}",
                l.mode, l.start, l.end, l.crossfade, l.on_release
            ))
            .color(c.dim),
        );
    }
    send_all(core, reqs);
}

fn group_list(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    view: &mut SamplerView,
    theme: &Theme,
) {
    let c = &theme.colors;
    let i = inst.id;
    let mut reqs: Vec<Request> = Vec::new();
    egui::Grid::new(("groups", i))
        .num_columns(9)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for h in [
                "",
                "Name",
                "Zones",
                "Voices",
                "Stealing",
                "Polyphony",
                "Round robin",
                "Output",
                "",
            ] {
                ui.label(widgets::caption(theme, h));
            }
            ui.end_row();
            for g in inst.groups.values() {
                let col = group_colour(theme, inst, g.id);
                let (r, p) = ui.allocate_painter(Vec2::splat(18.0), Sense::click());
                p.rect_filled(r.rect.shrink(2.0), CornerRadius::same(3), col);
                if view.group == Some(g.id) {
                    p.rect_stroke(
                        r.rect,
                        CornerRadius::same(3),
                        Stroke::new(1.5, c.accent),
                        StrokeKind::Inside,
                    );
                }
                let r = r.hint(
                    &g.name,
                    "",
                    "click to edit its envelopes and filter; right-click for a colour",
                );
                if r.clicked() {
                    view.group = Some(g.id);
                    view.selected = inst.zones_of(g.id).map(|z| z.id).collect();
                }
                r.context_menu(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.set_max_width(150.0);
                        for sw in theme.swatches() {
                            let (r, p) = ui.allocate_painter(Vec2::splat(22.0), Sense::click());
                            p.rect_filled(r.rect.shrink(1.0), CornerRadius::same(4), rgb(sw));
                            if r.clicked() {
                                reqs.push(ms::group_set(i, g.id, &[("color", json!(sw))]));
                                ui.close();
                            }
                        }
                    });
                });
                // name
                let nid = Id::new(("group_name", i, g.id));
                let mut name: String = ui
                    .data(|d| d.get_temp(nid))
                    .unwrap_or_else(|| g.name.clone());
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0));
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::TextEdit,
                        true,
                        format!("Group name {}", g.name),
                    )
                });
                if r.has_focus() {
                    ui.data_mut(|d| d.insert_temp(nid, name.clone()));
                } else {
                    ui.data_mut(|d| d.remove::<String>(nid));
                }
                if r.lost_focus() && name.trim() != g.name && !name.trim().is_empty() {
                    reqs.push(ms::group_set(i, g.id, &[("name", json!(name.trim()))]));
                }
                ui.label(inst.zones_of(g.id).count().to_string());
                let vl = g.voice_limit.unwrap_or(0) as f64;
                if let Some(v) = num(
                    ui,
                    nid.with("voices"),
                    &format!("{} voice limit (0 = none)", g.name),
                    vl,
                    0.0..=1024.0,
                    0.2,
                    0,
                    "",
                ) {
                    reqs.push(ms::group_set(
                        i,
                        g.id,
                        &[(
                            "voice_limit",
                            if v < 1.0 {
                                Value::Null
                            } else {
                                json!(v as u32)
                            },
                        )],
                    ));
                }
                if let Some(s) = choice(
                    ui,
                    ("steal", i, g.id),
                    "Voice stealing",
                    g.stealing,
                    &[
                        (StealMode::Oldest, "Oldest"),
                        (StealMode::Quietest, "Quietest"),
                        (StealMode::Newest, "Newest"),
                        (StealMode::None, "None"),
                    ],
                ) {
                    reqs.push(ms::group_set(
                        i,
                        g.id,
                        &[("stealing", serde_json::to_value(s).unwrap_or_default())],
                    ));
                }
                if let Some(s) = choice(
                    ui,
                    ("poly", i, g.id),
                    "Polyphony",
                    g.polyphony,
                    &[
                        (Polyphony::Poly, "Poly"),
                        (Polyphony::Mono, "Mono"),
                        (Polyphony::Legato, "Legato"),
                    ],
                ) {
                    reqs.push(ms::group_set(
                        i,
                        g.id,
                        &[("polyphony", serde_json::to_value(s).unwrap_or_default())],
                    ));
                }
                if let Some(s) = choice(
                    ui,
                    ("rr", i, g.id),
                    "Round-robin mode",
                    g.round_robin,
                    &[
                        (RoundRobinMode::Sequential, "Sequential"),
                        (RoundRobinMode::Random, "Random"),
                    ],
                ) {
                    reqs.push(ms::group_set(
                        i,
                        g.id,
                        &[("round_robin", serde_json::to_value(s).unwrap_or_default())],
                    ));
                }
                if let Some(v) = num(
                    ui,
                    nid.with("out"),
                    &format!("{} output", g.name),
                    g.output as f64,
                    0.0..=15.0,
                    0.05,
                    0,
                    "",
                ) {
                    reqs.push(ms::group_set(i, g.id, &[("output", json!(v as u32))]));
                }
                if widgets::btn_tinted(ui, "Remove", c.clip)
                    .hint("Remove group", &g.name, "removes the group and its zones")
                    .clicked()
                {
                    reqs.push(ms::group_remove(i, g.id));
                }
                ui.end_row();
            }
        });
    if btn(ui, "Add group").clicked() {
        reqs.push(ms::group_add(
            i,
            &format!("Group {}", inst.groups.len() + 1),
        ));
    }
    send_all(core, reqs);
}

/// A free key for a new keyswitch: below the played range, not used by another one.
fn free_keyswitch(inst: &HausInstrument) -> u8 {
    let (cover, _) = map::coverage(inst);
    let used = |k: u8| keyswitch_at(inst, k).is_some() || cover[k as usize];
    (0..=36u8)
        .rev()
        .find(|k| !used(*k))
        .or_else(|| (0..=127u8).find(|k| !used(*k)))
        .unwrap_or(0)
}

fn articulations(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    view: &mut SamplerView,
    theme: &Theme,
) {
    let c = &theme.colors;
    let i = inst.id;
    let mut reqs: Vec<Request> = Vec::new();
    if inst.articulations.is_empty() {
        ui.label(RichText::new("No articulations: every group plays every note.").color(c.dim));
    }
    let current = current_articulation(inst, view);
    for a in &inst.articulations {
        let edit = |e: ArticulationEdit| {
            ms::keyswitch_set(
                i,
                &ArticulationEdit {
                    id: Some(a.id),
                    ..e
                },
            )
        };
        ui.horizontal(|ui| {
            let col = articulation_colour(theme, inst, a.id);
            let (r, p) = ui.allocate_painter(Vec2::splat(18.0), Sense::click());
            p.rect_filled(r.rect.shrink(2.0), CornerRadius::same(3), col);
            if Some(a.id) == current {
                p.rect_stroke(
                    r.rect,
                    CornerRadius::same(3),
                    Stroke::new(1.5, c.accent),
                    StrokeKind::Inside,
                );
            }
            r.hint(&a.name, "", "right-click for a colour")
                .context_menu(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.set_max_width(150.0);
                        for sw in theme.swatches() {
                            let (r, p) = ui.allocate_painter(Vec2::splat(22.0), Sense::click());
                            p.rect_filled(r.rect.shrink(1.0), CornerRadius::same(4), rgb(sw));
                            if r.clicked() {
                                reqs.push(edit(ArticulationEdit {
                                    color: Some(sw),
                                    ..Default::default()
                                }));
                                ui.close();
                            }
                        }
                    });
                });
            let nid = Id::new(("art_name", i, a.id));
            let mut name: String = ui
                .data(|d| d.get_temp(nid))
                .unwrap_or_else(|| a.name.clone());
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0));
            r.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    true,
                    format!("Articulation name {}", a.name),
                )
            });
            if r.has_focus() {
                ui.data_mut(|d| d.insert_temp(nid, name.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(nid));
            }
            if r.lost_focus() && name.trim() != a.name && !name.trim().is_empty() {
                reqs.push(edit(ArticulationEdit {
                    name: Some(name.trim().into()),
                    ..Default::default()
                }));
            }
            let is_key = matches!(a.trigger, ArticulationTrigger::Key { .. });
            if let Some(k) = choice(
                ui,
                ("art_kind", i, a.id),
                "Articulation trigger",
                is_key,
                &[(true, "Keyswitch"), (false, "Controller")],
            ) {
                let t = if k {
                    ArticulationTrigger::Key {
                        key: free_keyswitch(inst),
                    }
                } else {
                    ArticulationTrigger::Cc {
                        controller: 32,
                        lo: 0.0,
                        hi: 1.0,
                    }
                };
                reqs.push(edit(ArticulationEdit {
                    trigger: Some(t),
                    ..Default::default()
                }));
            }
            match a.trigger {
                ArticulationTrigger::Key { key } => {
                    if let Some(k) =
                        key_field(ui, nid.with("key"), &format!("{} keyswitch", a.name), key)
                    {
                        reqs.push(edit(ArticulationEdit {
                            trigger: Some(ArticulationTrigger::Key { key: k }),
                            ..Default::default()
                        }));
                    }
                }
                ArticulationTrigger::Cc { controller, lo, hi } => {
                    let cc = num(
                        ui,
                        nid.with("cc"),
                        &format!("{} controller", a.name),
                        controller as f64,
                        0.0..=127.0,
                        0.2,
                        0,
                        "",
                    );
                    let l = num(
                        ui,
                        nid.with("lo"),
                        &format!("{} from", a.name),
                        lo as f64,
                        0.0..=1.0,
                        0.01,
                        2,
                        "",
                    );
                    let h = num(
                        ui,
                        nid.with("hi"),
                        &format!("{} to", a.name),
                        hi as f64,
                        0.0..=1.0,
                        0.01,
                        2,
                        "",
                    );
                    if cc.is_some() || l.is_some() || h.is_some() {
                        let (l, h) = (l.map_or(lo, |v| v as f32), h.map_or(hi, |v| v as f32));
                        reqs.push(edit(ArticulationEdit {
                            trigger: Some(ArticulationTrigger::Cc {
                                controller: cc.map_or(controller, |v| v as u8),
                                lo: l.min(h),
                                hi: l.max(h),
                            }),
                            ..Default::default()
                        }));
                    }
                }
            }
            let is_default = inst.settings.default_articulation == Some(a.id);
            if ui
                .selectable_label(is_default, crate::theme::caps("Default"))
                .hint(
                    "Default articulation",
                    if is_default { "yes" } else { "no" },
                    "click to play this one before any keyswitch",
                )
                .clicked()
                && !is_default
            {
                reqs.push(edit(ArticulationEdit {
                    default: true,
                    ..Default::default()
                }));
            }
            if widgets::btn_tinted(ui, "Remove", c.clip).clicked() {
                reqs.push(edit(ArticulationEdit {
                    remove: true,
                    ..Default::default()
                }));
            }
        });
        // which groups play it
        ui.horizontal_wrapped(|ui| {
            ui.add_space(22.0);
            ui.label(widgets::caption(theme, "Groups"));
            let members: Vec<GroupId> = inst
                .groups
                .values()
                .filter(|g| g.articulations.contains(&a.id))
                .map(|g| g.id)
                .collect();
            for g in inst.groups.values() {
                let mut on = members.contains(&g.id);
                if ui.checkbox(&mut on, g.name.as_str()).changed() {
                    let mut m = members.clone();
                    if on {
                        m.push(g.id);
                    } else {
                        m.retain(|x| *x != g.id);
                    }
                    reqs.push(edit(ArticulationEdit {
                        groups: Some(m),
                        ..Default::default()
                    }));
                }
            }
        });
    }
    if btn(ui, "Add articulation").clicked() {
        reqs.push(ms::keyswitch_set(
            i,
            &ArticulationEdit {
                name: Some(format!("Articulation {}", inst.articulations.len() + 1)),
                trigger: Some(ArticulationTrigger::Key {
                    key: free_keyswitch(inst),
                }),
                ..Default::default()
            },
        ));
    }
    send_all(core, reqs);
}

const SOURCES: [(&str, &str); 9] = [
    ("velocity", "Velocity"),
    ("key", "Key"),
    ("cc", "Controller"),
    ("pitch_bend", "Pitch bend"),
    ("aftertouch", "Aftertouch"),
    ("poly_pressure", "Poly pressure"),
    ("lfo", "LFO"),
    ("envelope", "Envelope"),
    ("random", "Random"),
];

fn source_kind(s: &ModSource) -> &'static str {
    match s {
        ModSource::Velocity => "velocity",
        ModSource::Key => "key",
        ModSource::Cc { .. } => "cc",
        ModSource::PitchBend => "pitch_bend",
        ModSource::Aftertouch => "aftertouch",
        ModSource::PolyPressure => "poly_pressure",
        ModSource::Lfo { .. } => "lfo",
        ModSource::Envelope { .. } => "envelope",
        ModSource::Random => "random",
    }
}

fn source_of(kind: &str, index: u8) -> ModSource {
    match kind {
        "key" => ModSource::Key,
        "cc" => ModSource::Cc { controller: index },
        "pitch_bend" => ModSource::PitchBend,
        "aftertouch" => ModSource::Aftertouch,
        "poly_pressure" => ModSource::PolyPressure,
        "lfo" => ModSource::Lfo {
            index: index.min(3),
        },
        "envelope" => ModSource::Envelope {
            index: index.min(1),
        },
        "random" => ModSource::Random,
        _ => ModSource::Velocity,
    }
}

fn source_index(s: &ModSource) -> Option<(u8, f64)> {
    match s {
        ModSource::Cc { controller } => Some((*controller, 127.0)),
        ModSource::Lfo { index } => Some((*index, 3.0)),
        ModSource::Envelope { index } => Some((*index, 1.0)),
        _ => None,
    }
}

fn target_unit(t: ModTarget) -> &'static str {
    match t {
        ModTarget::Pitch | ModTarget::Cutoff => " ct",
        ModTarget::Volume | ModTarget::Resonance => " dB",
        ModTarget::Pan => "",
        ModTarget::SampleStart => " fr",
    }
}

fn modulation(ui: &mut Ui, core: &mut AppCore, inst: &HausInstrument, theme: &Theme) {
    let c = &theme.colors;
    let i = inst.id;
    let routes = inst.modulation.clone();
    let mut new: Option<Vec<ModRoute>> = None;
    if routes.is_empty() {
        ui.label(RichText::new("No modulation routes.").color(c.dim));
    }
    let groups: Vec<(Option<GroupId>, String)> = std::iter::once((None, "All groups".to_string()))
        .chain(inst.groups.values().map(|g| (Some(g.id), g.name.clone())))
        .collect();
    egui::Grid::new(("mods", i))
        .num_columns(7)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for h in ["Source", "", "Target", "Depth", "Curve", "Scope", ""] {
                ui.label(widgets::caption(theme, h));
            }
            ui.end_row();
            for (n, r) in routes.iter().enumerate() {
                let mut edit = |f: &dyn Fn(&mut ModRoute)| {
                    let mut all = new.clone().unwrap_or_else(|| routes.clone());
                    f(&mut all[n]);
                    new = Some(all);
                };
                let kinds: Vec<(&str, &str)> = SOURCES.to_vec();
                let idx = source_index(&r.source).map_or(0, |x| x.0);
                if let Some(k) = choice(
                    ui,
                    ("mod_src", i, n),
                    "Modulation source",
                    source_kind(&r.source),
                    &kinds,
                ) {
                    let s = source_of(k, if k == "cc" { 1 } else { 0 });
                    edit(&|m| m.source = s.clone());
                }
                match source_index(&r.source) {
                    Some((_, max)) => {
                        if let Some(v) = num(
                            ui,
                            Id::new(("mod_idx", i, n)),
                            "Source number",
                            idx as f64,
                            0.0..=max,
                            0.1,
                            0,
                            "",
                        ) {
                            let s = source_of(source_kind(&r.source), v as u8);
                            edit(&|m| m.source = s.clone());
                        }
                    }
                    None => {
                        ui.label("");
                    }
                }
                if let Some(t) = choice(
                    ui,
                    ("mod_tgt", i, n),
                    "Modulation target",
                    r.target,
                    &[
                        (ModTarget::Pitch, "Pitch"),
                        (ModTarget::Volume, "Volume"),
                        (ModTarget::Pan, "Pan"),
                        (ModTarget::Cutoff, "Cutoff"),
                        (ModTarget::Resonance, "Resonance"),
                        (ModTarget::SampleStart, "Sample start"),
                    ],
                ) {
                    edit(&|m| m.target = t);
                }
                if let Some(v) = num(
                    ui,
                    Id::new(("mod_depth", i, n)),
                    "Modulation depth",
                    r.depth as f64,
                    -1.0e6..=1.0e6,
                    0.5,
                    2,
                    target_unit(r.target),
                ) {
                    edit(&|m| m.depth = v as f32);
                }
                if let Some(cv) = choice(
                    ui,
                    ("mod_curve", i, n),
                    "Modulation curve",
                    r.curve,
                    &[
                        (ModCurve::Linear, "Linear"),
                        (ModCurve::Convex, "Convex"),
                        (ModCurve::Concave, "Concave"),
                        (ModCurve::Switch, "Switch"),
                    ],
                ) {
                    edit(&|m| m.curve = cv);
                }
                let opts: Vec<(Option<GroupId>, &str)> =
                    groups.iter().map(|(g, s)| (*g, s.as_str())).collect();
                if let Some(g) = choice(ui, ("mod_scope", i, n), "Modulation scope", r.group, &opts)
                {
                    edit(&|m| m.group = g);
                }
                if widgets::btn_tinted(ui, "Remove", c.clip).clicked() {
                    let mut all = new.clone().unwrap_or_else(|| routes.clone());
                    all.remove(n);
                    new = Some(all);
                }
                ui.end_row();
            }
        });
    if btn(ui, "Add route").clicked() {
        let mut all = new.clone().unwrap_or_else(|| routes.clone());
        all.push(ModRoute {
            source: ModSource::Velocity,
            target: ModTarget::Volume,
            depth: 0.0,
            curve: ModCurve::Linear,
            group: None,
        });
        new = Some(all);
    }
    if let Some(all) = new {
        core.send(ms::modulation_set(i, &all));
    }
}

fn envelope_knobs(
    ui: &mut Ui,
    id: Id,
    what: &str,
    env: &Envelope,
    theme: &Theme,
) -> Option<Envelope> {
    let mut out: Option<Envelope> = None;
    ui.horizontal_wrapped(|ui| {
        let times: [(&str, f32); 5] = [
            ("Delay", env.delay),
            ("Attack", env.attack),
            ("Hold", env.hold),
            ("Decay", env.decay),
            ("Release", env.release),
        ];
        for (name, t) in times {
            let f = fmt_time(10.0);
            if let Some(x) = knob(
                ui,
                id.with(name),
                &format!("{what} {name}"),
                time_to_knob(t, 10.0),
                (0.0, 1.0, 0.0),
                &f,
                theme,
            ) {
                let mut e = out.clone().unwrap_or_else(|| env.clone());
                let v = knob_to_time(x, 10.0);
                match name {
                    "Delay" => e.delay = v,
                    "Attack" => e.attack = v,
                    "Hold" => e.hold = v,
                    "Decay" => e.decay = v,
                    _ => e.release = v,
                }
                out = Some(e);
            }
        }
        if let Some(x) = knob(
            ui,
            id.with("sus"),
            &format!("{what} Sustain"),
            env.sustain as f64,
            (0.0, 1.0, 1.0),
            &widgets::fmt_percent,
            theme,
        ) {
            let mut e = out.clone().unwrap_or_else(|| env.clone());
            e.sustain = x as f32;
            out = Some(e);
        }
    });
    out
}

fn group_sound(
    ui: &mut Ui,
    core: &mut AppCore,
    inst: &HausInstrument,
    view: &mut SamplerView,
    theme: &Theme,
) {
    let c = &theme.colors;
    let i = inst.id;
    let Some(g) = view.group.and_then(|g| inst.groups.get(&g)) else {
        ui.label(RichText::new("The instrument has no groups.").color(c.dim));
        return;
    };
    let mut reqs: Vec<Request> = Vec::new();
    ui.horizontal(|ui| {
        ui.label("Group");
        let opts: Vec<(GroupId, &str)> = inst
            .groups
            .values()
            .map(|g| (g.id, g.name.as_str()))
            .collect();
        if let Some(n) = choice(ui, ("sound_group", i), "Group to edit", g.id, &opts) {
            view.group = Some(n);
        }
    });
    let gid = g.id;
    let id = Id::new(("snd", i, gid));
    let set = |field: &str, v: Value| ms::group_set(i, gid, &[(field, v)]);

    widgets::section_label(ui, "Amp envelope", theme);
    if let Some(e) = envelope_knobs(ui, id.with("amp"), "Amp", &g.amp_env, theme) {
        reqs.push(set("amp_env", to(e)));
    }

    ui.horizontal(|ui| {
        widgets::section_label(ui, "Filter", theme);
        let on = g.filter.is_some();
        if widgets::toggle(ui, on, "Filter on", c.accent, theme).clicked() {
            reqs.push(set(
                "filter",
                if on {
                    Value::Null
                } else {
                    to(Filter {
                        kind: FilterKind::LowPass24,
                        cutoff: 8000.0,
                        resonance: 0.0,
                        key_track: 0.0,
                        env_depth: 0.0,
                    })
                },
            ));
        }
    });
    if let Some(f) = &g.filter {
        ui.horizontal_wrapped(|ui| {
            if let Some(k) = choice(
                ui,
                ("fkind", i, gid),
                "Filter type",
                f.kind,
                &[
                    (FilterKind::LowPass12, "Low pass 12"),
                    (FilterKind::LowPass24, "Low pass 24"),
                    (FilterKind::HighPass12, "High pass 12"),
                    (FilterKind::HighPass24, "High pass 24"),
                    (FilterKind::BandPass, "Band pass"),
                    (FilterKind::Notch, "Notch"),
                ],
            ) {
                reqs.push(set(
                    "filter",
                    to(Filter {
                        kind: k,
                        ..f.clone()
                    }),
                ));
            }
            let hz = |x: f64| {
                let v = 2f64.powf(x);
                if v >= 1000.0 {
                    format!("{:.1} kHz", v / 1000.0)
                } else {
                    format!("{v:.0} Hz")
                }
            };
            if let Some(x) = knob(
                ui,
                id.with("cut"),
                "Cutoff",
                (f.cutoff as f64).log2(),
                (10f64.log2(), 22_000f64.log2(), 8000f64.log2()),
                &hz,
                theme,
            ) {
                reqs.push(set(
                    "filter",
                    to(Filter {
                        cutoff: 2f32.powf(x as f32).clamp(10.0, 22_000.0),
                        ..f.clone()
                    }),
                ));
            }
            let db = |v: f64| format!("{v:.1} dB");
            if let Some(x) = knob(
                ui,
                id.with("res"),
                "Resonance",
                f.resonance as f64,
                (0.0, 40.0, 0.0),
                &db,
                theme,
            ) {
                reqs.push(set(
                    "filter",
                    to(Filter {
                        resonance: x as f32,
                        ..f.clone()
                    }),
                ));
            }
            let pct = |v: f64| format!("{v:.0}%");
            if let Some(x) = knob(
                ui,
                id.with("kt"),
                "Key track",
                f.key_track as f64,
                (0.0, 1200.0, 0.0),
                &pct,
                theme,
            ) {
                reqs.push(set(
                    "filter",
                    to(Filter {
                        key_track: x as f32,
                        ..f.clone()
                    }),
                ));
            }
            let ct = |v: f64| format!("{v:.0} ct");
            if let Some(x) = knob(
                ui,
                id.with("ed"),
                "Env depth",
                f.env_depth as f64,
                (-9600.0, 9600.0, 0.0),
                &ct,
                theme,
            ) {
                reqs.push(set(
                    "filter",
                    to(Filter {
                        env_depth: x as f32,
                        ..f.clone()
                    }),
                ));
            }
        });
        ui.horizontal(|ui| {
            widgets::section_label(ui, "Filter envelope", theme);
            let on = g.filter_env.is_some();
            if widgets::toggle(ui, on, "Filter envelope on", c.accent, theme).clicked() {
                reqs.push(set(
                    "filter_env",
                    if on {
                        Value::Null
                    } else {
                        to(Envelope::default())
                    },
                ));
            }
        });
        if let Some(fe) = &g.filter_env
            && let Some(e) = envelope_knobs(ui, id.with("fenv"), "Filter", fe, theme)
        {
            reqs.push(set("filter_env", to(e)));
        }
    }

    ui.horizontal(|ui| {
        widgets::section_label(ui, "LFOs", theme);
        if g.lfos.len() < 4 && btn(ui, "Add LFO").clicked() {
            let mut l = g.lfos.clone();
            l.push(Lfo {
                shape: LfoShape::Sine,
                rate: 5.0,
                delay: 0.0,
                fade: 0.0,
            });
            reqs.push(set("lfos", to(l)));
        }
    });
    for (n, l) in g.lfos.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("LFO {}", n + 1));
            let mut with = |f: &dyn Fn(&mut Lfo)| {
                let mut all = g.lfos.clone();
                f(&mut all[n]);
                reqs.push(set("lfos", to(all)));
            };
            if let Some(s) = choice(
                ui,
                ("lfo_shape", i, gid, n),
                "LFO shape",
                l.shape,
                &[
                    (LfoShape::Sine, "Sine"),
                    (LfoShape::Triangle, "Triangle"),
                    (LfoShape::Square, "Square"),
                    (LfoShape::SawUp, "Saw up"),
                    (LfoShape::SawDown, "Saw down"),
                    (LfoShape::Random, "Random"),
                ],
            ) {
                with(&|x| x.shape = s);
            }
            let hz = |x: f64| format!("{:.2} Hz", 2f64.powf(x));
            if let Some(x) = knob(
                ui,
                id.with(("lrate", n)),
                "LFO rate",
                (l.rate as f64).log2(),
                (0.01f64.log2(), 100f64.log2(), 5f64.log2()),
                &hz,
                theme,
            ) {
                with(&|l| l.rate = 2f32.powf(x as f32).clamp(0.01, 100.0));
            }
            let f = fmt_time(10.0);
            if let Some(x) = knob(
                ui,
                id.with(("ldelay", n)),
                "LFO delay",
                time_to_knob(l.delay, 10.0),
                (0.0, 1.0, 0.0),
                &f,
                theme,
            ) {
                with(&|l| l.delay = knob_to_time(x, 10.0));
            }
            if let Some(x) = knob(
                ui,
                id.with(("lfade", n)),
                "LFO fade",
                time_to_knob(l.fade, 10.0),
                (0.0, 1.0, 0.0),
                &f,
                theme,
            ) {
                with(&|l| l.fade = knob_to_time(x, 10.0));
            }
            if widgets::btn_tinted(ui, "Remove", c.clip).clicked() {
                let mut all = g.lfos.clone();
                all.remove(n);
                reqs.push(set("lfos", to(all)));
            }
        });
    }

    widgets::section_label(ui, "Voice", theme);
    ui.horizontal_wrapped(|ui| {
        let db = |v: f64| format!("{v:.1} dB");
        if let Some(x) = knob(
            ui,
            id.with("gvol"),
            "Group volume",
            g.volume_db as f64,
            (-60.0, 24.0, 0.0),
            &db,
            theme,
        ) {
            reqs.push(set("volume_db", json!(x)));
        }
        let f = fmt_time(10.0);
        if let Some(x) = knob(
            ui,
            id.with("porta"),
            "Portamento",
            time_to_knob(g.portamento, 10.0),
            (0.0, 1.0, 0.0),
            &f,
            theme,
        ) {
            reqs.push(set("portamento", json!(knob_to_time(x, 10.0))));
        }
    });
    if !g.effects.is_empty() {
        let names: Vec<&str> = g.effects.iter().map(|e| e.name.as_str()).collect();
        ui.label(RichText::new(format!("Effects: {}", names.join(" → "))).color(c.dim));
    }
    send_all(core, reqs);
}

/// The generic panel: the instrument's global settings (no skins until M3b).
fn generic_panel(ui: &mut Ui, core: &mut AppCore, inst: &HausInstrument, theme: &Theme) {
    let c = &theme.colors;
    let s = &inst.settings;
    let i = inst.id;
    let id = Id::new(("inst_settings", i));
    let mut reqs: Vec<Request> = Vec::new();
    ui.horizontal_wrapped(|ui| {
        let db = |v: f64| format!("{v:.1} dB");
        if let Some(x) = knob(
            ui,
            id.with("vol"),
            "Volume",
            s.volume_db as f64,
            (-60.0, 12.0, 0.0),
            &db,
            theme,
        ) {
            reqs.push(ms::set(i, None, &[("volume_db", json!(x))]));
        }
        if let Some(x) = knob(
            ui,
            id.with("pan"),
            "Pan",
            s.pan as f64,
            (-1.0, 1.0, 0.0),
            &widgets::fmt_pan,
            theme,
        ) {
            reqs.push(ms::set(i, None, &[("pan", json!(x))]));
        }
        let st = |v: f64| format!("{:+.0} st", v.round());
        if let Some(x) = knob(
            ui,
            id.with("tr"),
            "Transpose",
            s.transpose as f64,
            (-48.0, 48.0, 0.0),
            &st,
            theme,
        ) {
            reqs.push(ms::set(i, None, &[("transpose", json!(x.round() as i32))]));
        }
        let ct = |v: f64| format!("{v:+.0} ct");
        if let Some(x) = knob(
            ui,
            id.with("tune"),
            "Tune",
            s.tune_cents as f64,
            (-100.0, 100.0, 0.0),
            &ct,
            theme,
        ) {
            reqs.push(ms::set(i, None, &[("tune_cents", json!(x))]));
        }
        let semis = |v: f64| format!("{v:.0} st");
        if let Some(x) = knob(
            ui,
            id.with("bu"),
            "Bend up",
            s.bend_up as f64,
            (0.0, 48.0, 2.0),
            &semis,
            theme,
        ) {
            reqs.push(ms::set(i, None, &[("bend_up", json!(x.round()))]));
        }
        if let Some(x) = knob(
            ui,
            id.with("bd"),
            "Bend down",
            s.bend_down as f64,
            (0.0, 48.0, 2.0),
            &semis,
            theme,
        ) {
            reqs.push(ms::set(i, None, &[("bend_down", json!(x.round()))]));
        }
    });
    egui::Grid::new(("inst_grid", i))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Voice limit");
            if let Some(v) = num(
                ui,
                id.with("vl"),
                "Instrument voice limit",
                s.voice_limit as f64,
                1.0..=1024.0,
                0.5,
                0,
                "",
            ) {
                reqs.push(ms::set(i, None, &[("voice_limit", json!(v as u32))]));
            }
            ui.end_row();
            ui.label("Preload");
            let pre = s.preload_frames.unwrap_or(0) as f64;
            if let Some(v) = num(
                ui,
                id.with("pre"),
                "Preload frames (0 = engine default)",
                pre,
                0.0..=10_000_000.0,
                256.0,
                0,
                " frames",
            ) {
                reqs.push(ms::set(
                    i,
                    None,
                    &[(
                        "preload_frames",
                        if v < 1.0 {
                            Value::Null
                        } else {
                            json!(v as u32)
                        },
                    )],
                ));
            }
            ui.end_row();
            ui.label("Samples");
            ui.label(
                RichText::new(match inst.folder.as_deref() {
                    Some(f) if f.starts_with("instruments/") => format!("in the project: {f}"),
                    Some(f) => format!("linked from the library: {f}"),
                    None => "in the project folder".into(),
                })
                .color(c.dim),
            );
            ui.end_row();
        });
    send_all(core, reqs);
}

/// What the importer approximated, skipped or kept for later.
fn import_report(ui: &mut Ui, inst: &HausInstrument, theme: &Theme) {
    let c = &theme.colors;
    match &inst.source {
        Some(s) => {
            ui.label(format!(
                "Imported from {} ({}, importer version {})",
                s.file,
                s.format.to_uppercase(),
                s.importer_version
            ));
        }
        None => {
            ui.label("Made in Patternhaus.");
        }
    }
    if inst.import_notes.is_empty() && inst.extra.is_empty() {
        ui.label(RichText::new("Nothing was approximated or skipped.").color(c.ok));
        return;
    }
    for (kind, title, col) in [
        (ImportNoteKind::Approximated, "Approximated", c.warn),
        (ImportNoteKind::Skipped, "Skipped", c.error),
        (ImportNoteKind::Kept, "Kept for later (not used yet)", c.dim),
    ] {
        let notes: Vec<_> = inst
            .import_notes
            .iter()
            .filter(|n| n.kind == kind)
            .collect();
        if notes.is_empty() {
            continue;
        }
        ui.label(
            RichText::new(format!("{title} ({})", notes.len()))
                .color(col)
                .strong(),
        );
        for n in notes {
            let count = if n.count > 1 {
                format!(" ×{}", n.count)
            } else {
                String::new()
            };
            ui.label(format!("  {}{count}: {}", n.what, n.detail));
        }
    }
    if !inst.extra.is_empty() {
        let keys: Vec<&str> = inst.extra.keys().map(String::as_str).collect();
        ui.label(
            RichText::new(format!(
                "Kept as data for M3b (scripts, skins): {}",
                keys.join(", ")
            ))
            .color(c.dim),
        );
    }
}

// ---- files: import, save, load ---------------------------------------------------------------

/// One row of the auto-map preview.
#[derive(Clone, Debug, PartialEq)]
pub struct MapRow {
    pub file: String,
    pub root: u8,
    pub keys: KeyRange,
    pub velocity: VelRange,
}

/// "Drop a folder of samples": what the auto-mapper will do, shown before the import.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderForm {
    pub path: String,
    pub name: String,
    pub rows: Vec<MapRow>,
    /// Files it cannot map, with the reason.
    pub skipped: Vec<String>,
    pub error: Option<String>,
}

/// The auto-map preview of a folder (`ph_sampler_formats::automap`), read here only to
/// show it: the import itself is `sampler.import` on the same folder.
pub fn folder_preview(dir: &Path) -> FolderForm {
    let path = dir.to_string_lossy().into_owned();
    match ph_sampler_formats::automap::map_folder(dir) {
        Ok(imp) => {
            let mut rows: Vec<MapRow> = imp
                .instrument
                .zones
                .values()
                .map(|z| MapRow {
                    file: sample_label(z),
                    root: z.root_key,
                    keys: z.keys,
                    velocity: z.velocity,
                })
                .collect();
            rows.sort_by_key(|r| (r.root, r.velocity.lo));
            FolderForm {
                path,
                name: imp.instrument.name.clone(),
                rows,
                skipped: imp
                    .report
                    .notes
                    .iter()
                    .map(|n| format!("{}: {}", n.what.trim_start_matches("file:"), n.detail))
                    .collect(),
                error: None,
            }
        }
        Err(e) => FolderForm {
            path,
            name: dir
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            rows: Vec::new(),
            skipped: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

/// "This SoundFont holds several presets: which one?"
#[derive(Clone, Debug, PartialEq)]
pub struct PresetForm {
    pub path: String,
    /// (bank, program, name)
    pub presets: Vec<(u16, u16, String)>,
    pub choice: usize,
}

impl PresetForm {
    /// The `preset` parameter of the chosen one ("bank:program").
    pub fn chosen(&self) -> Option<String> {
        self.presets
            .get(self.choice)
            .map(|(b, p, _)| format!("{b}:{p}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveForm {
    pub instrument: InstrumentId,
    pub path: String,
}

impl SaveForm {
    pub fn new(inst: &HausInstrument) -> Self {
        let safe: String = inst
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe = safe.trim();
        Self {
            instrument: inst.id,
            path: format!(
                "saved instruments/{}",
                if safe.is_empty() { "Instrument" } else { safe }
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadForm {
    pub channel: Option<ChannelId>,
    pub path: String,
}

impl LoadForm {
    pub fn new(channel: Option<ChannelId>) -> Self {
        Self {
            channel,
            path: String::new(),
        }
    }
}

/// Whether a path is something the sampler imports: an instrument file (or a `.dsbundle`
/// folder) by its extension.
pub fn is_instrument_file(path: &Path) -> bool {
    ph_sampler_formats::importer_for(path).is_some()
}

/// Whether a folder directly holds audio files the auto-mapper picks up.
pub fn has_samples(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.filter_map(Result::ok).any(|e| {
            let p = e.path();
            p.is_file()
                && p.extension().is_some_and(|x| {
                    ph_sampler_formats::automap::AUDIO_EXTENSIONS
                        .contains(&x.to_string_lossy().to_lowercase().as_str())
                })
        })
    })
}

/// Starts importing `path` as a Haus Sampler channel: an instrument file goes straight to
/// `sampler.import`, an SF2/SF3 with several presets asks which one first, and a folder of
/// samples shows the auto-map preview first.
pub fn import_path(core: &mut AppCore, path: &Path, channel: Option<ChannelId>) {
    let s = path.to_string_lossy().into_owned();
    if path.is_dir() && !is_instrument_file(path) {
        core.dialog = Some(Dialog::SamplerFolder(folder_preview(path)));
        return;
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "sf2" || ext == "sf3" {
        match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|b| ph_sampler_formats::sf2::list_presets(&b))
        {
            Ok(presets) if presets.len() > 1 => {
                let mut presets = presets;
                presets.sort_by_key(|(b, p, _)| (*b, *p));
                core.dialog = Some(Dialog::SamplerPreset(PresetForm {
                    path: s,
                    presets,
                    choice: 0,
                }));
                return;
            }
            Ok(_) => {}
            // the command reports a damaged file in its own words
            Err(_) => {}
        }
    }
    core.send(ms::import(&s, None, channel));
}

/// The dialogs of this module; each returns whether it stays open.
pub fn folder_dialog(ctx: &egui::Context, core: &mut AppCore, f: &mut FolderForm) -> bool {
    let mut keep = true;
    let theme = core.theme.clone();
    crate::app::modal(ctx, "Map a folder of samples", |ui| {
        ui.label(format!(
            "{}: {} samples mapped by the note names in their file names.",
            f.name,
            f.rows.len()
        ));
        if let Some(e) = &f.error {
            ui.label(RichText::new(e).color(theme.colors.error));
        }
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                egui::Grid::new("automap_preview")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        for h in ["File", "Root", "Keys", "Velocity"] {
                            ui.label(widgets::caption(&theme, h));
                        }
                        ui.end_row();
                        for r in &f.rows {
                            ui.label(&r.file);
                            ui.label(map::key_name(r.root));
                            ui.label(format!(
                                "{}-{}",
                                map::key_name(r.keys.lo),
                                map::key_name(r.keys.hi)
                            ));
                            ui.label(format!("{}-{}", r.velocity.lo, r.velocity.hi));
                            ui.end_row();
                        }
                    });
                for s in &f.skipped {
                    ui.label(RichText::new(format!("Not mapped: {s}")).color(theme.colors.warn));
                }
            });
        ui.horizontal(|ui| {
            if widgets::btn_enabled(ui, !f.rows.is_empty(), "Import").clicked() {
                core.send(ms::import(&f.path, None, None));
                keep = false;
            }
            if btn(ui, "Cancel").clicked() {
                keep = false;
            }
        });
    });
    keep
}

pub fn preset_dialog(ctx: &egui::Context, core: &mut AppCore, f: &mut PresetForm) -> bool {
    let mut keep = true;
    crate::app::modal(ctx, "Choose a SoundFont preset", |ui| {
        ui.label(format!(
            "{} holds {} presets. Import which one?",
            file_name(&f.path),
            f.presets.len()
        ));
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for (n, (b, p, name)) in f.presets.iter().enumerate() {
                    let label = format!("{b}:{p}  {name}");
                    if ui.selectable_label(f.choice == n, label).clicked() {
                        f.choice = n;
                    }
                }
            });
        ui.horizontal(|ui| {
            if btn(ui, "Import preset").clicked() {
                core.send(ms::import(&f.path, f.chosen().as_deref(), None));
                keep = false;
            }
            if btn(ui, "Cancel").clicked() {
                keep = false;
            }
        });
    });
    keep
}

pub fn save_dialog(ctx: &egui::Context, core: &mut AppCore, f: &mut SaveForm) -> bool {
    let mut keep = true;
    let dim = core.theme.colors.dim;
    crate::app::modal(ctx, "Save as Haus instrument", |ui| {
        ui.label("Folder to write (instrument.toml and samples/), relative to the project:");
        ui.add(egui::TextEdit::singleline(&mut f.path).desired_width(420.0));
        ui.label(
            RichText::new(
                "An instrument.toml already there is replaced. Library folders are never written.",
            )
            .color(dim),
        );
        ui.horizontal(|ui| {
            if btn(ui, "Save instrument").clicked() && !f.path.trim().is_empty() {
                core.send(ms::save_native(f.instrument, f.path.trim()));
                keep = false;
            }
            if btn(ui, "Cancel").clicked() {
                keep = false;
            }
        });
    });
    keep
}

pub fn load_dialog(ctx: &egui::Context, core: &mut AppCore, f: &mut LoadForm) -> bool {
    let mut keep = true;
    let dim = core.theme.colors.dim;
    crate::app::modal(ctx, "Load Haus instrument", |ui| {
        ui.label("Instrument folder (holds instrument.toml), or its instrument.toml:");
        ui.add(egui::TextEdit::singleline(&mut f.path).desired_width(420.0));
        ui.label(
            RichText::new(if f.channel.is_some() {
                "It replaces what this channel plays."
            } else {
                "It goes on a new channel."
            })
            .color(dim),
        );
        ui.horizontal(|ui| {
            if btn(ui, "Load instrument").clicked() && !f.path.trim().is_empty() {
                core.send(ms::load(f.path.trim(), f.channel));
                keep = false;
            }
            if btn(ui, "Cancel").clicked() {
                keep = false;
            }
        });
    });
    keep
}

/// Follows up a `sampler.*` reply: opens the new instrument's window, says what happened.
pub fn on_reply(core: &mut AppCore, command: &str, result: &Value, summary: Option<&str>) {
    match command {
        "sampler.import" | "sampler.load" => {
            if let Some(ch) = result["channel"].as_u64() {
                let ch = ChannelId(ch as u32);
                core.sel.channel = Some(ch);
                core.window_requests
                    .push((WindowId::Instrument(ch), PanelAction::Open));
            }
            if let Some(s) = summary {
                core.toast(ToastKind::Info, s.to_string());
            }
            let notes = result["report"]["notes"].as_array().map_or(0, Vec::len)
                + result["report"]["unknown"].as_array().map_or(0, Vec::len);
            if notes > 0 {
                core.toast(
                    ToastKind::Warn,
                    format!(
                        "The import report lists {notes} approximated or skipped items: see the sampler window's Import report tab"
                    ),
                );
            }
        }
        "sampler.save_native" => {
            let path = result["path"].as_str().unwrap_or("");
            core.toast(ToastKind::Info, format!("Saved the instrument to {path}"));
        }
        "sampler.collect" => {
            let text = result["note"]
                .as_str()
                .map(str::to_string)
                .or_else(|| summary.map(str::to_string))
                .unwrap_or_else(|| "Collected".into());
            core.toast(ToastKind::Info, text);
        }
        _ => {}
    }
}

// ---- browser preview ------------------------------------------------------------------------

/// The keys a preview plays: up to three roots spread over the instrument, else C4 E4 G4.
pub fn preview_keys(inst: &HausInstrument) -> Vec<u8> {
    let mut roots: Vec<u8> = inst
        .zones
        .values()
        .filter(|z| z.trigger == ZoneTrigger::Attack)
        .map(|z| z.root_key.clamp(z.keys.lo, z.keys.hi))
        .collect();
    roots.sort();
    roots.dedup();
    let (_, range) = map::coverage(inst);
    match (range, roots.len()) {
        (Some((lo, hi)), _) if lo <= 60 && 67 <= hi => vec![60, 64, 67],
        (_, 0) => vec![60, 64, 67],
        (_, n) if n <= 3 => roots,
        (_, n) => vec![roots[0], roots[n / 2], roots[n - 1]],
    }
}

/// Renders a few notes of an instrument file for the browser's preview, offline and outside
/// the project: the file is imported into a throw-away project (samples inside the file are
/// written to a temporary folder first) and rendered.
pub fn preview_audio(path: &Path) -> Result<ph_node::SampleData, String> {
    let imported = if path.is_dir() && !is_instrument_file(path) {
        ph_sampler_formats::automap::map_folder(path)
    } else {
        ph_sampler_formats::import_path(path)
    }
    .map_err(|e| e.to_string())?;
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "patternhaus-preview-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = render_preview(&imported, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn render_preview(
    imported: &ph_sampler_formats::Imported,
    tmp: &Path,
) -> Result<ph_node::SampleData, String> {
    use ph_sampler_formats::SampleSource;
    let mut resolver = ph_engine::assets::FileResolver::new().allow_root(tmp);
    let mut files: HashMap<String, PathBuf> = HashMap::new();
    for s in &imported.samples {
        let file = match &s.source {
            SampleSource::File(p) => p.clone(),
            SampleSource::Audio(a) => {
                let p = tmp.join(format!("{}.wav", files.len()));
                std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
                std::fs::write(&p, ph_sampler_formats::audio_wav(a)?).map_err(|e| e.to_string())?;
                p
            }
            SampleSource::Generated(g) => {
                let p = tmp.join(format!("{}.wav", files.len()));
                std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
                let bytes = ph_formats::encode::encode_wav(
                    &[g.render()],
                    ph_sampler_formats::Generated::RATE,
                    ph_formats::encode::WavFormat::Float32,
                    ph_formats::encode::DITHER_SEED,
                )
                .map_err(|e| e.to_string())?;
                std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
                p
            }
            SampleSource::Chunk { .. } => continue,
        };
        if let Some(dir) = file.parent() {
            resolver = resolver.allow_root(dir);
        }
        files.insert(s.name.clone(), file);
    }
    let mut inst = imported.instrument.clone();
    inst.folder = None;
    for z in inst.zones.values_mut() {
        if let ph_core::SampleRef::File(n) = &z.sample
            && let Some(f) = files.get(n)
        {
            z.sample = ph_core::SampleRef::File(f.to_string_lossy().replace('\\', "/"));
        }
    }
    let keys = preview_keys(&inst);
    let base = ph_core::Project::empty("preview");
    let mut e = ph_core::Edit::new(&base);
    let bad = |err: ph_core::ModelError| err.to_string();
    let id = e.add_instrument(inst).map_err(bad)?;
    let ch = e
        .add_channel(
            "preview",
            ph_core::InstrumentSpec::MultiSampler {
                library: None,
                instrument: Some(id),
                params: Default::default(),
            },
        )
        .map_err(bad)?;
    e.update_channel(ch, |c| c.volume = 1.0).map_err(bad)?;
    let pat = e.add_pattern("preview").map_err(bad)?;
    let step = ph_core::PPQ / 2;
    for (n, k) in keys.iter().enumerate() {
        e.add_note(
            pat,
            ch,
            ph_core::NoteSpec::new(*k, n as u64 * step, step * 2),
        )
        .map_err(bad)?;
    }
    let len = step * (keys.len() as u64 + 2);
    let (project, _) = e.finish();
    let settings = ph_engine::RenderSettings {
        mode: ph_engine::PlayMode::Pattern(pat),
        range: Some((0, len)),
        sample_rate: 48_000,
        quality: ph_node::Quality::Live,
        include_tail: true,
        block_frames: 512,
    };
    let mut audio =
        ph_engine::render_offline(&project, &settings, &resolver).map_err(|e| e.to_string())?;
    for c in &mut audio.channels {
        c.truncate(48_000 * 4);
    }
    Ok(ph_node::SampleData {
        sample_rate: 48_000.0,
        channels: audio.channels,
        loop_range: None,
        root_key: None,
    })
}
