//! Plugins in the UI (M2/T3), kept minimal:
//!
//! - a **plugin picker** in the add-instrument ("+ Channel") and add-effect menus, from the scan
//!   cache, with "Scan for plugins" (all through `plugin.*` commands);
//! - the **generic parameter window**: a plugin channel's instrument window and a plugin
//!   effect's window show the plugin's parameters as the same knob grid the built-ins use
//!   (`plugin.set_param`), its latency, and an "Open plugin editor" button;
//! - **plugin editors**: `plugin.open_editor` (from the button or an agent) reaches the UI as a
//!   [`ph_commands::UiRequest::PluginEditor`]. On Windows the editor is embedded in the
//!   floating workspace window of its channel or effect: a native child frame
//!   (`ph_plugin_host::embed::EditorFrame`) is moved to the space the window reserves for it
//!   every frame. Elsewhere, or when a plugin cannot embed, it opens as the plugin's own
//!   floating window.
//!
//! [`PluginEditors`] holds the open editors behind two small traits, so its logic is tested
//! with fakes and no window is ever created in tests.

use crate::core::AppCore;
use crate::mapping;
use crate::theme::Theme;
use crate::widgets::{self, menu_item};
use egui::{Id, RichText, Ui};
use ph_commands::PluginTarget;
use ph_core::PluginFormat;
use ph_engine::plugins::{
    EditorSize, ListedPlugin, PluginEditor, PluginKind, PluginSlot, PluginStatus, RawWindowHandle,
};
use std::collections::HashMap;

/// How often the plugin list and live parameter values are re-read (seconds).
const REFRESH: f64 = 1.0;

/// A native rectangle an editor is embedded in.
pub trait EditorFrameApi {
    fn handle(&self) -> RawWindowHandle;
    /// Moves it (physical pixels in the app window's client area) and shows or hides it.
    fn place(&mut self, rect: [i32; 4], visible: bool);
}

/// Makes [`EditorFrameApi`]s inside the app window.
pub trait FrameFactory {
    fn create(&mut self, parent: RawWindowHandle) -> Result<Box<dyn EditorFrameApi>, String>;
}

/// The real frames: `ph_plugin_host::embed::EditorFrame` (Windows); elsewhere creation fails
/// and editors float.
pub struct NativeFrames;

impl EditorFrameApi for ph_engine::plugins::EditorFrame {
    fn handle(&self) -> RawWindowHandle {
        ph_engine::plugins::EditorFrame::handle(self)
    }
    fn place(&mut self, rect: [i32; 4], visible: bool) {
        ph_engine::plugins::EditorFrame::place(self, rect, visible)
    }
}

impl FrameFactory for NativeFrames {
    fn create(&mut self, parent: RawWindowHandle) -> Result<Box<dyn EditorFrameApi>, String> {
        ph_engine::plugins::EditorFrame::create(parent)
            .map(|f| Box::new(f) as Box<dyn EditorFrameApi>)
            .map_err(|e| e.to_string())
    }
}

/// One open editor.
pub struct OpenEditor {
    pub editor: Box<dyn PluginEditor>,
    pub frame: Option<Box<dyn EditorFrameApi>>,
    pub size: EditorSize,
    /// Where the frame was last placed.
    pub rect: Option<[i32; 4]>,
}

/// The open plugin editors, by where their plugin sits.
pub struct PluginEditors {
    pub open: HashMap<PluginSlot, OpenEditor>,
    factory: Box<dyn FrameFactory>,
}

impl PluginEditors {
    pub fn new(factory: Box<dyn FrameFactory>) -> Self {
        Self {
            open: HashMap::new(),
            factory,
        }
    }

    /// Opens `editor` for `slot`: embedded in a frame inside `parent` when the plugin can
    /// embed and the platform has frames, floating otherwise. Replaces an editor already
    /// open there.
    pub fn open(
        &mut self,
        slot: PluginSlot,
        mut editor: Box<dyn PluginEditor>,
        parent: Option<RawWindowHandle>,
    ) -> Result<EditorSize, String> {
        self.close(slot);
        let mut frame = None;
        if let Some(parent) = parent
            && editor.can_embed()
            && let Ok(f) = self.factory.create(parent)
        {
            frame = Some(f);
        }
        let size = match &frame {
            Some(f) => match editor.open(Some(f.handle())) {
                Ok(s) => s,
                // the plugin refused to embed: float it instead
                Err(_) => {
                    frame = None;
                    editor.open(None).map_err(|e| e.to_string())?
                }
            },
            None => editor.open(None).map_err(|e| e.to_string())?,
        };
        self.open.insert(
            slot,
            OpenEditor {
                editor,
                frame,
                size,
                rect: None,
            },
        );
        Ok(size)
    }

    pub fn close(&mut self, slot: PluginSlot) {
        if let Some(mut e) = self.open.remove(&slot) {
            // the plugin's window goes first, then the frame it lived in
            e.editor.close();
            drop(e.frame.take());
        }
    }

    pub fn close_all(&mut self) {
        let slots: Vec<PluginSlot> = self.open.keys().copied().collect();
        for s in slots {
            self.close(s);
        }
    }

    pub fn is_open(&self, slot: PluginSlot) -> bool {
        self.open.contains_key(&slot)
    }

    /// Size in physical pixels of an embedded editor (`None` when floating or closed).
    pub fn embedded_size(&self, slot: PluginSlot) -> Option<EditorSize> {
        self.open
            .get(&slot)
            .filter(|e| e.frame.is_some())
            .map(|e| e.size)
    }

    /// Moves every embedded editor to where its window drew it this frame; hides the ones
    /// whose window was not drawn (closed, collapsed, scrolled away).
    pub fn place(&mut self, drawn: &HashMap<PluginSlot, [i32; 4]>) {
        for (slot, e) in self.open.iter_mut() {
            let Some(f) = e.frame.as_mut() else { continue };
            match drawn.get(slot) {
                Some(r) => {
                    if e.rect != Some(*r) {
                        f.place(*r, true);
                        e.rect = Some(*r);
                    }
                }
                None => {
                    if e.rect.is_some() {
                        f.place(e.rect.unwrap_or_default(), false);
                        e.rect = None;
                    }
                }
            }
        }
    }
}

/// The UI's plugin state, in [`AppCore::plugins`].
pub struct PluginUi {
    list: Vec<ListedPlugin>,
    list_at: f64,
    values: HashMap<PluginSlot, (f64, Vec<f64>)>,
    /// `plugin.open_editor` / `close_editor` requests from the control side.
    pub requests: Vec<(PluginTarget, bool)>,
    pub editors: PluginEditors,
    /// Embedded editor rectangles drawn this frame.
    drawn: HashMap<PluginSlot, [i32; 4]>,
}

impl Default for PluginUi {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            list_at: f64::NEG_INFINITY,
            values: HashMap::new(),
            requests: Vec::new(),
            editors: PluginEditors::new(Box::new(NativeFrames)),
            drawn: HashMap::new(),
        }
    }
}

pub fn slot_of(t: PluginTarget) -> PluginSlot {
    match t {
        PluginTarget::Channel(c) => PluginSlot::Channel(c),
        PluginTarget::Effect { insert, slot } => PluginSlot::Effect { insert, slot },
    }
}

fn window_of(t: PluginTarget) -> crate::workspace::WindowId {
    match t {
        PluginTarget::Channel(c) => crate::workspace::WindowId::Instrument(c),
        PluginTarget::Effect { insert, slot } => {
            crate::workspace::WindowId::Effect(ph_core::InsertId(insert), slot)
        }
    }
}

/// The scanned plugins (re-read at most once a second).
pub fn plugin_list(core: &mut AppCore) -> Vec<ListedPlugin> {
    if core.now - core.plugins.list_at > REFRESH {
        core.plugins.list = core
            .studio
            .engine
            .plugin_env()
            .map(|e| e.host.list())
            .unwrap_or_default();
        core.plugins.list_at = core.now;
    }
    core.plugins.list.clone()
}

/// The plugin entries of the add-instrument (`instrument`) or add-effect menu. Clicking one
/// calls `on_pick(format, id)`.
pub fn plugin_menu(
    ui: &mut Ui,
    core: &mut AppCore,
    instrument: bool,
    mut on_pick: impl FnMut(&mut AppCore, PluginFormat, &str),
) {
    let list: Vec<ListedPlugin> = plugin_list(core)
        .into_iter()
        .filter(|p| (p.info.kind == PluginKind::Instrument) == instrument)
        .collect();
    widgets::menu_button(ui, "Plugins", |ui| {
        if list.is_empty() {
            ui.label(RichText::new("No plugins scanned yet").color(core.theme.colors.dim));
        }
        for p in &list {
            let label = format!(
                "{} ({}{})",
                p.info.name,
                match p.info.format {
                    PluginFormat::Clap => "CLAP",
                    PluginFormat::Vst3 => "VST3",
                },
                match p.status {
                    PluginStatus::Ok => "",
                    PluginStatus::Isolate => ", isolated",
                    PluginStatus::Blocked => ", blocked",
                }
            );
            let enabled = p.status == PluginStatus::Ok;
            if ui
                .add_enabled_ui(enabled, |ui| menu_item(ui, &label, ""))
                .inner
                .clicked()
            {
                on_pick(core, p.info.format, &p.info.id);
                ui.close();
            }
        }
        ui.separator();
        if menu_item(ui, "Scan for plugins", "").clicked() {
            core.send(mapping::plugin_scan());
            core.plugins.list_at = f64::NEG_INFINITY;
            ui.close();
        }
    });
}

fn live_values(core: &mut AppCore, slot: PluginSlot) -> Option<Vec<f64>> {
    let now = core.now;
    if let Some((at, v)) = core.plugins.values.get(&slot)
        && now - at < REFRESH
    {
        return Some(v.clone());
    }
    let v = core.studio.engine.plugin(slot)?.values()?;
    core.plugins.values.insert(slot, (now, v.clone()));
    Some(v)
}

/// The generic parameter window of a plugin instrument or effect.
pub fn plugin_panel(ui: &mut Ui, core: &mut AppCore, target: PluginTarget) {
    let theme: std::sync::Arc<Theme> = core.theme.clone();
    let slot = slot_of(target);
    let Some(live) = core.studio.engine.plugin(slot) else {
        ui.label(
            RichText::new(
                "The plugin is not loaded (see the warnings: not scanned, isolated or blocked).",
            )
            .color(theme.colors.dim),
        );
        return;
    };
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(&live.info.name)
                    .font(egui::FontId::new(15.0, crate::fonts::cond_bold()))
                    .color(theme.colors.text),
            )
            .truncate(),
        );
        ui.label(widgets::caption(&theme, &live.info.vendor));
        let latency = live.latency();
        if latency > 0 {
            ui.label(widgets::caption(
                &theme,
                &format!("latency {latency} samples"),
            ));
        }
    });
    if live.fault.is_dead() {
        ui.label(
            RichText::new("This plugin crashed: it plays silence and is isolated.")
                .color(theme.colors.dim),
        );
        return;
    }
    if live.info.has_editor {
        let open = core.plugins.editors.is_open(slot);
        let label = if open {
            "Close plugin editor"
        } else {
            "Open plugin editor"
        };
        if ui.button(crate::theme::caps(label)).clicked() {
            core.send(mapping::plugin_editor(target, !open));
        }
    }
    // an embedded editor gets its space here; the frame follows it every frame
    if let Some(size) = core.plugins.editors.embedded_size(slot) {
        let ppp = ui.ctx().pixels_per_point();
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(size.width as f32 / ppp, size.height as f32 / ppp),
            egui::Sense::hover(),
        );
        let visible = rect.intersect(ui.clip_rect());
        if visible.area() > 0.0 {
            core.plugins.drawn.insert(
                slot,
                [
                    (rect.min.x * ppp).round() as i32,
                    (rect.min.y * ppp).round() as i32,
                    size.width as i32,
                    size.height as i32,
                ],
            );
        }
    }
    ui.separator();
    let spec_params: std::collections::BTreeMap<String, f64> = match target {
        PluginTarget::Channel(c) => match core.project.channels.get(&c).map(|c| &c.instrument) {
            Some(ph_core::InstrumentSpec::Plugin { params, .. }) => params.clone(),
            _ => Default::default(),
        },
        PluginTarget::Effect { insert, slot } => match core
            .project
            .mixer
            .get(insert as usize)
            .and_then(|m| m.effects.get(slot as usize))
            .map(|e| &e.effect)
        {
            Some(ph_core::EffectSpec::Plugin { params, .. }) => params.clone(),
            _ => Default::default(),
        },
    };
    let values = live_values(core, slot);
    let desc = live.desc.clone();
    let changes = widgets::param_grid(
        ui,
        Id::new(("plugin", slot)),
        &desc,
        &|_| true,
        &|info| {
            spec_params.get(&info.id).copied().unwrap_or_else(|| {
                desc.param_id(&info.id)
                    .and_then(|i| values.as_ref().and_then(|v| v.get(i.0 as usize)).copied())
                    .unwrap_or(info.default)
            })
        },
        &theme,
    );
    for (param, value) in changes {
        core.send(mapping::plugin_param(target, &param, value));
        core.plugins.values.remove(&slot);
    }
}

/// End of a UI frame: open and close requested editors (with the app window as parent),
/// close editors whose plugin is gone, and move the embedded ones.
pub fn after_frame(core: &mut AppCore, parent: Option<RawWindowHandle>) {
    let requests = std::mem::take(&mut core.plugins.requests);
    for (target, open) in requests {
        let slot = slot_of(target);
        if !open {
            core.plugins.editors.close(slot);
            continue;
        }
        let editor = core
            .studio
            .engine
            .plugin(slot)
            .and_then(|p| p.control.clone())
            .and_then(|c| c.editor());
        match editor {
            Some(ed) => match core.plugins.editors.open(slot, ed, parent) {
                Ok(_) => core
                    .window_requests
                    .push((window_of(target), crate::core::PanelAction::Open)),
                Err(e) => core.toast(crate::core::ToastKind::Error, format!("plugin editor: {e}")),
            },
            None => core.toast(
                crate::core::ToastKind::Error,
                format!("the plugin at {target} has no editor"),
            ),
        }
    }
    // editors of plugins that were removed, replaced or crashed close
    let stale: Vec<PluginSlot> = core
        .plugins
        .editors
        .open
        .keys()
        .copied()
        .filter(|s| {
            core.studio
                .engine
                .plugin(*s)
                .is_none_or(|p| p.fault.is_dead())
        })
        .collect();
    for s in stale {
        core.plugins.editors.close(s);
    }
    let drawn = std::mem::take(&mut core.plugins.drawn);
    core.plugins.editors.place(&drawn);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_engine::plugins::PluginError;
    use std::sync::{Arc, Mutex};

    /// Records what the UI asked of an editor; opens no window.
    struct FakeEditor {
        embed: bool,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl PluginEditor for FakeEditor {
        fn can_embed(&self) -> bool {
            self.embed
        }
        fn open(&mut self, parent: Option<RawWindowHandle>) -> Result<EditorSize, PluginError> {
            self.log.lock().unwrap().push(match parent {
                Some(_) => "open embedded".into(),
                None => "open floating".into(),
            });
            Ok(EditorSize {
                width: 300,
                height: 200,
            })
        }
        fn close(&mut self) {
            self.log.lock().unwrap().push("close".into());
        }
        fn is_open(&self) -> bool {
            true
        }
        fn can_resize(&self) -> bool {
            false
        }
        fn set_size(&mut self, s: EditorSize) -> Result<EditorSize, PluginError> {
            Ok(s)
        }
        fn size(&self) -> Option<EditorSize> {
            None
        }
    }

    struct FakeFrame {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl EditorFrameApi for FakeFrame {
        fn handle(&self) -> RawWindowHandle {
            RawWindowHandle::Web(raw_window_handle::WebWindowHandle::new(7))
        }
        fn place(&mut self, r: [i32; 4], visible: bool) {
            self.log
                .lock()
                .unwrap()
                .push(format!("place {r:?} {visible}"));
        }
    }

    impl Drop for FakeFrame {
        fn drop(&mut self) {
            self.log.lock().unwrap().push("frame dropped".into());
        }
    }

    struct FakeFrames {
        log: Arc<Mutex<Vec<String>>>,
        works: bool,
    }

    impl FrameFactory for FakeFrames {
        fn create(&mut self, _: RawWindowHandle) -> Result<Box<dyn EditorFrameApi>, String> {
            if !self.works {
                return Err("no frames here".into());
            }
            self.log.lock().unwrap().push("frame created".into());
            Ok(Box::new(FakeFrame {
                log: self.log.clone(),
            }))
        }
    }

    fn parent() -> RawWindowHandle {
        RawWindowHandle::Web(raw_window_handle::WebWindowHandle::new(1))
    }

    fn setup(works: bool) -> (PluginEditors, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            PluginEditors::new(Box::new(FakeFrames {
                log: log.clone(),
                works,
            })),
            log,
        )
    }

    fn editor(embed: bool, log: &Arc<Mutex<Vec<String>>>) -> Box<dyn PluginEditor> {
        Box::new(FakeEditor {
            embed,
            log: log.clone(),
        })
    }

    #[test]
    fn an_embeddable_editor_opens_in_a_frame_that_follows_its_window() {
        let (mut eds, log) = setup(true);
        let slot = PluginSlot::Channel(ph_core::ChannelId(3));
        let size = eds.open(slot, editor(true, &log), Some(parent())).unwrap();
        assert_eq!((size.width, size.height), (300, 200));
        assert_eq!(eds.embedded_size(slot), Some(size));
        let drawn: HashMap<_, _> = [(slot, [10, 20, 300, 200])].into_iter().collect();
        eds.place(&drawn);
        eds.place(&drawn); // unchanged: not moved again
        eds.place(&HashMap::new()); // its window was not drawn: hidden
        eds.close(slot);
        assert!(!eds.is_open(slot));
        assert_eq!(
            *log.lock().unwrap(),
            [
                "frame created",
                "open embedded",
                "place [10, 20, 300, 200] true",
                "place [10, 20, 300, 200] false",
                "close",
                "frame dropped",
            ]
        );
    }

    #[test]
    fn editors_float_without_frames_or_embedding_and_close_all() {
        // the plugin cannot embed
        let (mut eds, log) = setup(true);
        let a = PluginSlot::Channel(ph_core::ChannelId(1));
        eds.open(a, editor(false, &log), Some(parent())).unwrap();
        assert_eq!(eds.embedded_size(a), None);
        // the platform has no frames (macOS, Linux for now)
        let (mut eds2, log2) = setup(false);
        let b = PluginSlot::Effect { insert: 2, slot: 0 };
        eds2.open(b, editor(true, &log2), Some(parent())).unwrap();
        assert_eq!(eds2.embedded_size(b), None);
        assert_eq!(*log2.lock().unwrap(), ["open floating"]);
        // no parent at all
        eds.open(b, editor(true, &log), None).unwrap();
        // reopening replaces; close_all closes everything
        eds.open(b, editor(true, &log), None).unwrap();
        eds.close_all();
        assert!(eds.open.is_empty());
        assert_eq!(
            *log.lock().unwrap(),
            [
                "open floating",
                "open floating",
                "close",
                "open floating",
                "close",
                "close",
            ]
        );
    }
}
