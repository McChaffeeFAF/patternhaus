//! The application shell: menus, transport, the docked browser, the floating-window workspace,
//! dialogs, toasts and key handling.
//!
//! [`PhApp::draw`] draws one whole frame into a `Ui`, so the real window (`eframe`) and the
//! offscreen tests share the same code.

use crate::brand::wordmark;
use crate::core::{
    AfterPrompt, AppCore, AppPaths, Dialog, EffectEdit, Hooks, PanelAction, PanelId, RenameTarget,
    ToastKind,
};
use crate::keymap::{Chord, Context};
use crate::mapping;
use crate::motion;
use crate::panels;
use crate::settings::{self, Layout};
use crate::theme::{TRACK_MENU, caps, caps_sized};
use crate::toolbar;
use crate::widgets::{btn, menu_item};
use crate::windows;
use crate::workspace::{Rect as WRect, WindowId, Workspace, match_effects};
use crate::{dnd, hint};
use egui::{Align2, Id, RichText, Ui};
use ph_control::Studio;
use ph_core::InsertId;
use ph_engine::EngineConfig;
use std::sync::Arc;

pub struct PhApp {
    pub core: AppCore,
    /// The window manager; the source of truth for every floating window.
    pub ws: Workspace,
    /// No saved layout was found, so the defaults are laid out to the real window size on the
    /// first frame.
    fresh_layout: bool,
    /// Where the workspace was drawn on screen last frame (window rectangles are relative to it).
    pub work_rect: egui::Rect,
    theme_applied: bool,
    last_title: String,
    /// The kinds of each insert's effects as the windows last saw them (to follow an effect
    /// that moved), and the snapshot they were read from.
    fx_kinds: std::collections::HashMap<u32, Vec<String>>,
    fx_seen: Option<std::sync::Arc<ph_core::Project>>,
    /// The person has agreed to close (saved, or chose not to), so a close request goes through.
    pub quit_confirmed: bool,
    /// The loading screen, until it has played (see [`PhApp::show_splash`]).
    splash: Option<crate::splash::Splash>,
}

impl PhApp {
    pub fn new(
        ctx: &egui::Context,
        studio: Studio,
        paths: AppPaths,
        hooks: Hooks,
        engine_config: EngineConfig,
        audio_notice: Option<String>,
    ) -> Result<PhApp, String> {
        crate::fonts::install(ctx);
        let (theme, theme_err) = settings::load_theme(paths.config_dir.as_deref());
        let (keymap, keymap_err) = settings::load_keymap(paths.config_dir.as_deref());
        let layout = settings::load_layout(&settings::project_layout_path(&paths.project_dir))
            .or_else(|| {
                paths
                    .config_dir
                    .as_deref()
                    .and_then(|c| settings::load_layout(&settings::layout_path(c)))
            });
        let fresh_layout = layout.is_none();
        let (ws, toolbar) = match layout {
            Some(l) => (l.workspace, l.toolbar),
            None => (
                Workspace::new(WRect::new(0.0, 0.0, 1400.0, 800.0)),
                Default::default(),
            ),
        };
        let prefs = settings::load_prefs(paths.config_dir.as_deref());
        let mut core = AppCore::new(
            studio,
            ctx.clone(),
            paths,
            hooks,
            engine_config,
            theme,
            keymap,
        )?;
        core.toolbar = toolbar;
        core.toolbar.normalize();
        core.reduce_motion = prefs.reduce_motion;
        motion::set_reduce(ctx, prefs.reduce_motion);
        core.audio_notice = audio_notice.clone();
        if let Some(n) = audio_notice {
            core.toast(ToastKind::Warn, n);
        }
        for e in [theme_err, keymap_err].into_iter().flatten() {
            core.toast(ToastKind::Warn, e);
        }
        Ok(PhApp {
            core,
            ws,
            fresh_layout,
            work_rect: egui::Rect::NOTHING,
            theme_applied: false,
            last_title: String::new(),
            fx_kinds: Default::default(),
            fx_seen: None,
            quit_confirmed: false,
            splash: None,
        })
    }

    /// Shows the loading screen over the first frames. The real application asks for it; the
    /// tests do not, so their screenshots are of the workspace.
    pub fn show_splash(&mut self) {
        self.splash = Some(crate::splash::Splash::new());
    }

    // ---- windows ---------------------------------------------------------------------------

    /// The workspace window a named panel maps onto. The browser is docked, not a window, and
    /// the old inspector is now the selected channel's instrument window.
    fn window_of(&self, p: PanelId) -> Option<WindowId> {
        match p {
            PanelId::Browser => None,
            PanelId::ChannelRack => Some(WindowId::ChannelRack),
            PanelId::PianoRoll => Some(WindowId::PianoRoll),
            PanelId::Playlist => Some(WindowId::Playlist),
            PanelId::Mixer => Some(WindowId::Mixer),
            PanelId::History => Some(WindowId::History),
            PanelId::Agents => Some(WindowId::Agents),
            PanelId::Inspector => self.core.sel.channel.map(WindowId::Instrument),
        }
    }

    pub fn panel_visible(&self, p: PanelId) -> bool {
        match (p, self.window_of(p)) {
            (PanelId::Browser, _) => self.ws.browser.visible,
            (_, Some(w)) => self.ws.is_visible(w),
            _ => false,
        }
    }

    /// Show a panel's window and bring it to the front.
    pub fn open_panel(&mut self, p: PanelId) {
        match self.window_of(p) {
            Some(w) => self.ws.show(w),
            None if p == PanelId::Browser => self.ws.browser.visible = true,
            None => {}
        }
    }

    pub fn close_panel(&mut self, p: PanelId) {
        match self.window_of(p) {
            Some(w) => self.ws.hide(w),
            None if p == PanelId::Browser => self.ws.browser.visible = false,
            None => {}
        }
    }

    /// F5-F9 behaviour: show the panel, raise it if it is behind, or hide it when it is
    /// already the one in front.
    pub fn toggle_panel(&mut self, p: PanelId) {
        match self.window_of(p) {
            Some(w) => {
                self.ws.toggle(w);
            }
            None if p == PanelId::Browser => {
                self.ws.toggle_browser();
            }
            None => {}
        }
    }

    fn process_panel_requests(&mut self) {
        for (p, a) in std::mem::take(&mut self.core.panel_requests) {
            match a {
                PanelAction::Open => self.open_panel(p),
                PanelAction::Close => self.close_panel(p),
                PanelAction::Toggle => self.toggle_panel(p),
            }
        }
        for (w, a) in std::mem::take(&mut self.core.window_requests) {
            match a {
                PanelAction::Open => {
                    if self.object_exists(w) {
                        self.ws.show(w);
                    }
                }
                PanelAction::Close => self.ws.hide(w),
                PanelAction::Toggle => {
                    if self.object_exists(w) {
                        self.ws.toggle(w);
                    }
                }
            }
        }
        if std::mem::take(&mut self.core.close_focused_requested) {
            self.ws.close_front_object();
        }
    }

    fn object_exists(&self, w: WindowId) -> bool {
        let p = &self.core.project;
        match w {
            WindowId::Instrument(c) => p.channels.contains_key(&c),
            WindowId::Effect(i, s) => p
                .mixer
                .get(i.0 as usize)
                .is_some_and(|m| (s as usize) < m.effects.len()),
            _ => true,
        }
    }

    /// The window's close button (or Alt+F4): with unsaved changes, hold the close and ask.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_confirmed {
            if self.core.dirty {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.core.dialog = Some(Dialog::Unsaved {
                    then: AfterPrompt::Quit,
                });
            } else {
                self.quit_confirmed = true;
            }
        }
    }

    /// Does what the person asked for once nothing stands in the way.
    fn run_ready(&mut self) {
        for what in std::mem::take(&mut self.core.ready) {
            match what {
                AfterPrompt::Quit => {
                    self.quit_confirmed = true;
                    self.core
                        .ctx
                        .send_viewport_cmd(egui::ViewportCommand::Close);
                }
                AfterPrompt::NewProject => self.core.send(mapping::project_new(None)),
                AfterPrompt::OpenProject { path } => self.core.send(mapping::file_open(&path)),
            }
        }
    }

    /// Effect windows follow their effect (DESIGN section 9). Moves and removals the UI made
    /// itself are applied directly; a new snapshot (an agent, an undo) is matched by insert and
    /// effect kind. A window closes only when its effect is really gone.
    fn follow_effects(&mut self) {
        if std::mem::take(&mut self.core.project_replaced) {
            self.core.effect_edits.clear();
            self.fx_kinds.clear();
            self.fx_seen = None;
            self.ws.prune_objects(|_| true, |_, _| false);
        }
        for e in std::mem::take(&mut self.core.effect_edits) {
            match e {
                EffectEdit::Move { insert, from, to } => {
                    self.ws
                        .remap_effects(InsertId(insert), &Workspace::move_map(from, to));
                    if let Some(k) = self.fx_kinds.get_mut(&insert)
                        && (from as usize) < k.len()
                        && (to as usize) < k.len()
                    {
                        let x = k.remove(from as usize);
                        k.insert(to as usize, x);
                    }
                }
                EffectEdit::Remove { insert, slot } => {
                    let len = self.fx_kinds.get(&insert).map_or(0, |k| k.len() as u32);
                    self.ws.remap_effects(
                        InsertId(insert),
                        &Workspace::remove_map(slot, len.max(slot + 1)),
                    );
                    if let Some(k) = self.fx_kinds.get_mut(&insert)
                        && (slot as usize) < k.len()
                    {
                        k.remove(slot as usize);
                    }
                }
            }
        }
        let p = self.core.project.clone();
        if self.fx_seen.as_ref().is_some_and(|s| Arc::ptr_eq(s, &p)) {
            return;
        }
        for (i, m) in p.mixer.iter().enumerate() {
            let kinds: Vec<String> = m.effects.iter().map(|s| effect_kind(&s.effect)).collect();
            if let Some(old) = self.fx_kinds.get(&(i as u32))
                && *old != kinds
            {
                let map = match_effects(old, &kinds);
                self.ws.remap_effects(InsertId(i as u32), &map);
            }
            self.fx_kinds.insert(i as u32, kinds);
        }
        self.fx_seen = Some(p);
    }

    /// A project snapshot may delete the object behind an instrument or effect window.
    fn prune_windows(&mut self) {
        let p = self.core.project.clone();
        self.ws.prune_objects(
            |c| p.channels.contains_key(&c),
            |i, s| {
                p.mixer
                    .get(i.0 as usize)
                    .is_some_and(|m| (s as usize) < m.effects.len())
            },
        );
    }

    pub fn reset_layout(&mut self) {
        let a = self.ws.area();
        let browser = self.ws.browser;
        self.ws = Workspace::new(a);
        self.ws.browser = browser;
    }

    fn layout(&self) -> Layout {
        Layout {
            workspace: self.ws.clone(),
            toolbar: self.core.toolbar.clone(),
        }
    }

    pub fn save_layout(&mut self, in_project: bool) {
        let path = if in_project {
            Some(settings::project_layout_path(&self.core.paths.project_dir))
        } else {
            self.core
                .paths
                .config_dir
                .as_deref()
                .map(settings::layout_path)
        };
        match path {
            Some(p) => match settings::save_layout(&p, &self.layout()) {
                Ok(()) => self
                    .core
                    .toast(ToastKind::Info, format!("Layout saved to {}", p.display())),
                Err(e) => self.core.toast(ToastKind::Error, e),
            },
            None => self
                .core
                .toast(ToastKind::Warn, "No config folder to save the layout in"),
        }
    }

    pub fn set_theme(&mut self, name: &str) {
        if let Ok(t) = crate::theme::load_from(name, self.core.paths.config_dir.as_deref()) {
            self.core.theme = std::sync::Arc::new(t);
            self.theme_applied = false;
            if let Some(c) = &self.core.paths.config_dir {
                let _ = settings::save_theme(c, name);
            }
        }
    }

    // ---- one frame -------------------------------------------------------------------------

    pub fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !crate::fonts::ensure(&ctx) {
            return;
        }
        if !self.theme_applied {
            self.core.theme.apply(&ctx);
            self.theme_applied = true;
        }
        if self.fresh_layout {
            // lay the defaults out for the real window: the screen minus the bars and browser
            self.fresh_layout = false;
            let r = ctx.content_rect();
            let area = WRect::new(
                0.0,
                0.0,
                (r.width() - self.ws.browser.width).max(400.0),
                (r.height() - 120.0).max(300.0),
            );
            let browser = self.ws.browser;
            self.ws = Workspace::new(area);
            self.ws.browser = browser;
        }
        let now = ctx.input(|i| i.time);
        self.core.update(now);
        // keep polling the session's solo state while the window is idle
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
        self.handle_close_request(&ctx);
        self.run_ready();
        self.follow_effects();
        self.prune_windows();
        self.process_panel_requests();
        self.handle_keys(&ctx);

        let theme = self.core.theme.clone();
        hint::begin_frame(&ctx);
        motion::set_reduce(&ctx, self.core.reduce_motion);
        let chrome = |fill| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(8, 3))
        };
        let line = egui::Stroke::new(1.0, theme.colors.line);
        let r = egui::Panel::top("menu")
            .frame(chrome(theme.colors.bg0))
            .show(ui, |ui| self.menu(ui));
        ui.painter().hline(
            r.response.rect.x_range(),
            r.response.rect.bottom(),
            egui::Stroke::new(1.0, theme.colors.line_soft),
        );
        let r = egui::Panel::top("transport")
            .frame(chrome(theme.colors.bg1).inner_margin(egui::Margin::symmetric(8, 5)))
            .show(ui, |ui| panels::transport::show(ui, &mut self.core));
        ui.painter()
            .hline(r.response.rect.x_range(), r.response.rect.bottom(), line);
        let r = egui::Panel::top("toolbar")
            .frame(chrome(theme.colors.bg1))
            .show(ui, |ui| toolbar::show(ui, &mut self.core));
        ui.painter()
            .hline(r.response.rect.x_range(), r.response.rect.bottom(), line);
        let r = egui::Panel::bottom("status")
            .frame(chrome(theme.colors.bg0))
            .show(ui, |ui| self.hint_bar(ui));
        ui.painter()
            .hline(r.response.rect.x_range(), r.response.rect.top(), line);
        self.core.drop_zones.clear();
        if self.ws.browser.visible {
            let r = egui::Panel::left("browser")
                .resizable(true)
                .default_size(self.ws.browser.width)
                .size_range(140.0..=640.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme.colors.bg1)
                        .inner_margin(egui::Margin::same(6)),
                )
                .show(ui, |ui| panels::browser::show(ui, &mut self.core));
            self.core
                .drop_zones
                .push((r.response.rect, dnd::DropZone::Browser));
            self.ws.set_browser_width(r.response.rect.width());
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme.colors.bg0))
            .show(ui, |ui| {
                self.work_rect = windows::show_workspace(ui, &mut self.ws, &mut self.core);
            });

        self.detached_windows(&ctx);
        self.dialogs(&ctx);
        panels::history::conflict_dialog(&ctx, &mut self.core);
        self.toasts(&ctx);
        if let Some(h) = hint::take_help_request(&ctx) {
            self.core.toast(ToastKind::Info, hint::line(&h));
        }
        dnd::frame(&mut self.core, &ctx);
        self.core.publish_hint(hint::current(&ctx));

        if let Some(sp) = &mut self.splash
            && !sp.show(&ctx, &theme, self.core.reduce_motion)
        {
            self.splash = None;
        }

        let title = window_title(&self.core.project.settings.title, self.core.dirty);
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    /// Windows the user sent to their own OS window (multi-viewport).
    fn detached_windows(&mut self, ctx: &egui::Context) {
        let detached: Vec<_> = self
            .ws
            .ids()
            .filter(|id| self.ws.get(*id).is_some_and(|w| w.visible && w.detached))
            .collect();
        for id in detached {
            let mut close = false;
            let builder = egui::ViewportBuilder::default()
                .with_title(format!(
                    "Patternhaus - {}",
                    windows::title_of(&self.core, id)
                ))
                .with_inner_size([960.0, 620.0]);
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("panel", id)),
                builder,
                |ui, _class| {
                    egui::CentralPanel::default()
                        .show(ui, |ui| windows::show_content(ui, &mut self.core, id));
                    if ui.input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                },
            );
            if close {
                self.ws.set_detached(id, false);
                self.ws.show(id);
            }
        }
    }

    // ---- keys ------------------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let capturing = matches!(
            self.core.dialog,
            Some(Dialog::Keymap { capturing: Some(_) })
        );
        let text_focus = ctx.egui_wants_keyboard_input();
        let events = ctx.input(|i| i.events.clone());
        // typing keyboard -> piano
        if text_focus || capturing {
            for (k, on) in self.core.typing.release_all() {
                if let Some(c) = self.core.sel.channel {
                    self.core.preview(c, k, on);
                }
            }
        } else {
            for (k, on) in self.core.typing.process(&events, false) {
                if let Some(c) = self.core.sel.channel {
                    self.core.preview(c, k, on);
                }
            }
        }
        if text_focus || capturing {
            return;
        }
        for e in &events {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = e
            else {
                continue;
            };
            // a plain letter that plays a note is not also a shortcut
            let bare = !modifiers.command && !modifiers.alt;
            if bare && self.core.typing.is_piano_key(*key) {
                continue;
            }
            let ctx_kind = self.core.hover_context;
            if let Some(b) = self.core.keymap.lookup(*key, modifiers, ctx_kind).cloned() {
                self.core.run_binding(&b);
                self.process_panel_requests();
            }
        }
    }

    // ---- menus -----------------------------------------------------------------------------

    fn menu(&mut self, ui: &mut Ui) {
        let theme = self.core.theme.clone();
        let m = |t: &str| caps_sized(t, 12.5, TRACK_MENU);
        ui.horizontal(|ui| {
            wordmark(ui, &theme);
            ui.add_space(10.0);
            egui::MenuBar::new().ui(ui, |ui| {
                crate::widgets::menu_button_rich(ui, "File", m("File"), |ui| {
                    if menu_item(ui, "New project…", "").clicked() {
                        self.core
                            .app_action("app.new_project", &serde_json::Value::Null);
                        ui.close();
                    }
                    if menu_item(ui, "Open project…", "").clicked() {
                        self.core
                            .app_action("app.open_project", &serde_json::Value::Null);
                        ui.close();
                    }
                    if menu_item(ui, "Save", "Ctrl+S").clicked() {
                        self.core.send(mapping::file_save());
                        ui.close();
                    }
                    if menu_item(ui, "Save as…", "").clicked() {
                        self.core
                            .app_action("app.save_as", &serde_json::Value::Null);
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Export audio…", "Ctrl+R").clicked() {
                        self.core.app_action("app.export", &serde_json::Value::Null);
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Quit", "").clicked() {
                        // goes through the same close request as the window's own close button
                        self.core.ask_or_do(AfterPrompt::Quit);
                        ui.close();
                    }
                });
                crate::widgets::menu_button_rich(ui, "Edit", m("Edit"), |ui| {
                    if menu_item(ui, "Undo (mine)", "Ctrl+Z").clicked() {
                        self.core.send(mapping::undo_mine());
                        ui.close();
                    }
                    if menu_item(ui, "Redo (mine)", "Ctrl+Y").clicked() {
                        self.core.send(mapping::redo_mine());
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Clone pattern", "Ctrl+L").clicked() {
                        self.core
                            .app_action("app.clone_pattern", &serde_json::Value::Null);
                        ui.close();
                    }
                });
                crate::widgets::menu_button_rich(ui, "View", m("View"), |ui| {
                    for p in PanelId::ALL {
                        if p == PanelId::Inspector {
                            continue; // instrument windows open from the channel rack
                        }
                        let open = self.panel_visible(p);
                        if ui.checkbox(&mut { open }, caps(p.title())).clicked() {
                            if open {
                                self.close_panel(p);
                            } else {
                                self.open_panel(p);
                            }
                        }
                    }
                    ui.separator();
                    crate::widgets::menu_button_rich(ui, "Theme", m("Theme"), |ui| {
                        self.theme_menu(ui)
                    });
                    crate::widgets::menu_button_rich(ui, "Layout", m("Layout"), |ui| {
                        if menu_item(ui, "Save for me", "").clicked() {
                            self.save_layout(false);
                            ui.close();
                        }
                        if menu_item(ui, "Save with this project", "").clicked() {
                            self.save_layout(true);
                            ui.close();
                        }
                        if menu_item(ui, "Reset to default", "").clicked() {
                            self.reset_layout();
                            ui.close();
                        }
                    });
                    if menu_item(ui, "Customise toolbar…", "").clicked() {
                        self.core.dialog = Some(Dialog::Toolbar);
                        ui.close();
                    }
                    let mut reduce = self.core.reduce_motion;
                    if ui
                        .checkbox(&mut reduce, caps("Reduce motion"))
                        .on_hover_text("Turns flashes, glow fades, easing and slides into jumps")
                        .changed()
                    {
                        self.set_reduce_motion(reduce);
                    }
                    ui.separator();
                    if menu_item(ui, "Keyboard shortcuts…", "").clicked() {
                        self.core.dialog = Some(Dialog::Keymap { capturing: None });
                        ui.close();
                    }
                });
                crate::widgets::menu_button_rich(ui, "Help", m("Help"), |ui| {
                    if menu_item(ui, "About Patternhaus…", "").clicked() {
                        self.core.dialog = Some(Dialog::About);
                        ui.close();
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                panels::transport::health(ui, &mut self.core);
            });
        });
    }

    /// The Theme submenu: the built-ins and the user's files, switched live.
    fn theme_menu(&mut self, ui: &mut Ui) {
        let (themes, errors) = crate::theme::available(self.core.paths.config_dir.as_deref());
        let current = self.core.theme.id.clone();
        for (id, name) in themes {
            let r = ui.radio(current == id, caps(&name));
            r.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::RadioButton,
                    true,
                    current == id,
                    &name,
                )
            });
            if r.clicked() {
                self.set_theme(&id);
                ui.close();
            }
        }
        if !errors.is_empty() {
            ui.separator();
            for e in errors {
                ui.label(
                    RichText::new(e)
                        .color(self.core.theme.colors.warn)
                        .size(11.0),
                );
            }
        }
        ui.separator();
        if menu_item(ui, "Where do themes go?", "").clicked() {
            self.core.toast(
                ToastKind::Info,
                match &self.core.paths.config_dir {
                    Some(c) => format!("Put theme files in {}", c.join("themes").display()),
                    None => "No config folder is set".to_string(),
                },
            );
            ui.close();
        }
    }

    pub fn set_reduce_motion(&mut self, reduce: bool) {
        self.core.reduce_motion = reduce;
        motion::set_reduce(&self.core.ctx, reduce);
        if let Some(c) = self.core.paths.config_dir.clone() {
            let mut prefs = settings::load_prefs(Some(&c));
            prefs.reduce_motion = reduce;
            let _ = settings::save_prefs(&c, &prefs);
        }
    }

    /// The bar at the bottom: the hint for whatever the pointer is over (its name, value and
    /// gestures), and the project's counters on the right.
    fn hint_bar(&mut self, ui: &mut Ui) {
        let theme = self.core.theme.clone();
        let c = &theme.colors;
        ui.horizontal(|ui| {
            match hint::current(ui.ctx()) {
                Some(h) => {
                    ui.label(
                        RichText::new(&h.name)
                            .font(egui::FontId::new(12.5, crate::fonts::body_medium()))
                            .color(c.text),
                    );
                    if !h.value.is_empty() {
                        ui.label(
                            RichText::new(&h.value)
                                .font(egui::FontId::new(12.0, crate::fonts::mono()))
                                .color(c.accent),
                        );
                    }
                    if !h.gestures.is_empty() {
                        ui.label(RichText::new(&h.gestures).size(12.0).color(c.dim));
                    }
                }
                None => {
                    ui.label(
                        RichText::new("Hover a control for its name, value and gestures.")
                            .size(12.0)
                            .color(c.faint),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mono = |t: String, col| {
                    RichText::new(t)
                        .font(egui::FontId::new(11.5, crate::fonts::mono()))
                        .color(col)
                };
                if self.core.dirty {
                    ui.label(mono("UNSAVED".to_string(), c.warn));
                }
                let n = self.core.dispatch.in_flight();
                if n > 0 {
                    ui.label(mono(format!("{n} PENDING"), c.dim));
                }
                ui.label(mono(
                    format!(
                        "{} CH  {} PAT  {} CLIPS",
                        self.core.project.channels.len(),
                        self.core.project.patterns.len(),
                        self.core.project.clips.len()
                    ),
                    c.dim,
                ));
                ui.label(
                    RichText::new(&self.core.project.settings.title)
                        .font(egui::FontId::new(12.0, crate::fonts::body_medium()))
                        .color(c.text),
                );
            });
        });
    }

    // ---- dialogs ---------------------------------------------------------------------------

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.core.dialog.clone() else {
            return;
        };
        let mut close = false;
        let mut follow_up: Option<Dialog> = None;
        let mut new_state: Option<Dialog> = None;
        match dialog {
            Dialog::Unsaved { then } => {
                let title = match &then {
                    AfterPrompt::Quit => "Quit Patternhaus",
                    AfterPrompt::NewProject => "New project",
                    AfterPrompt::OpenProject { .. } => "Open project",
                };
                modal(ctx, title, |ui| {
                    ui.label(format!(
                        "\"{}\" has unsaved changes.",
                        self.core.project.settings.title
                    ));
                    ui.horizontal(|ui| {
                        if btn(ui, "Save").clicked() {
                            // do it once the save has gone through
                            self.core.after_save = Some(then.clone());
                            self.core.send(mapping::file_save());
                            close = true;
                        }
                        if btn(ui, "Don't save").clicked() {
                            self.core.ready.push(then.clone());
                            close = true;
                        }
                        if btn(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::OpenProject { mut path } => {
                modal(ctx, "Open project", |ui| {
                    ui.label("Project folder (contains project.json):");
                    ui.add(egui::TextEdit::singleline(&mut path).desired_width(420.0));
                    ui.label(
                        RichText::new(
                            "Opening replaces the current project and clears the history.",
                        )
                        .color(self.core.theme.colors.dim),
                    );
                    ui.horizontal(|ui| {
                        if btn(ui, "Open").clicked() {
                            if self.core.dirty {
                                // ask first; this dialog is replaced by the question
                                follow_up = Some(Dialog::Unsaved {
                                    then: AfterPrompt::OpenProject { path: path.clone() },
                                });
                            } else {
                                self.core.send(mapping::file_open(&path));
                            }
                            close = true;
                        }
                        if btn(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                new_state = Some(Dialog::OpenProject { path });
            }
            Dialog::SaveAs {
                mut path,
                mut overwrite,
            } => {
                modal(ctx, "Save project as", |ui| {
                    ui.label("Project folder to save into:");
                    ui.add(egui::TextEdit::singleline(&mut path).desired_width(420.0));
                    ui.checkbox(&mut overwrite, "Replace a project already saved there");
                    ui.horizontal(|ui| {
                        if btn(ui, "Save").clicked() {
                            let mut r = mapping::file_save_as(&path);
                            r.params["overwrite"] = overwrite.into();
                            self.core.send(r);
                            close = true;
                        }
                        if btn(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                new_state = Some(Dialog::SaveAs { path, overwrite });
            }
            Dialog::Export {
                mut path,
                mut overwrite,
            } => {
                modal(ctx, "Export audio", |ui| {
                    ui.label("File to write (.wav or .flac), relative to the project folder:");
                    ui.add(egui::TextEdit::singleline(&mut path).desired_width(420.0));
                    ui.checkbox(&mut overwrite, "Replace the file if it exists");
                    ui.label(
                        RichText::new("Renders offline at 48 kHz / 24 bit with reverb tails.")
                            .color(self.core.theme.colors.dim),
                    );
                    ui.horizontal(|ui| {
                        if btn(ui, "Export").clicked() {
                            self.core.send(mapping::export_audio(&path, overwrite));
                            close = true;
                        }
                        if btn(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                new_state = Some(Dialog::Export { path, overwrite });
            }
            Dialog::Rename { target, mut text } => {
                modal(ctx, "Rename", |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(260.0));
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if btn(ui, "OK").clicked() || enter {
                            let req = match target {
                                RenameTarget::Channel(c) => mapping::channel_rename(c, &text),
                                RenameTarget::Pattern(p) => mapping::pattern_rename(p, &text),
                                RenameTarget::Insert(i) => mapping::mixer_rename(i, &text),
                                RenameTarget::Track(t) => mapping::track_rename(t, &text),
                            };
                            self.core.send(req);
                            close = true;
                        }
                        if btn(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                new_state = Some(Dialog::Rename { target, text });
            }
            Dialog::About => {
                if crate::about::show(ctx, &self.core.theme) {
                    new_state = Some(Dialog::About);
                } else {
                    close = true;
                }
            }
            Dialog::Toolbar => {
                if toolbar::dialog(ctx, &mut self.core) {
                    new_state = Some(Dialog::Toolbar);
                } else {
                    close = true;
                }
            }
            Dialog::Keymap { mut capturing } => {
                let mut open = true;
                egui::Window::new("Keyboard shortcuts")
                    .open(&mut open)
                    .default_width(560.0)
                    .show(ctx, |ui| {
                        ui.label("Click Rebind, then press the new key. Changes apply at once; Save writes your keymap file.");
                        if let Some(i) = capturing
                            && let Some(chord) = captured_chord(ui) {
                                if let Some(old) = self.core.keymap.bindings.get(i).cloned() {
                                    self.core.keymap.rebind(&old, chord);
                                }
                                capturing = None;
                            }
                        egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                            egui::Grid::new("keymap_grid").striped(true).show(ui, |ui| {
                                for (i, b) in self.core.keymap.bindings.clone().iter().enumerate() {
                                    ui.label(RichText::new(match b.context {
                                        Context::Global => "global",
                                        Context::PianoRoll => "piano roll",
                                        Context::Playlist => "playlist",
                                        Context::ChannelRack => "rack",
                                        Context::Mixer => "mixer",
                                    }).color(self.core.theme.colors.dim));
                                    let params = if b.params.is_null() { String::new() } else { format!(" {}", b.params) };
                                    ui.label(format!("{}{}", b.command, params));
                                    let label = if capturing == Some(i) { "press a key…".to_string() } else { b.chord.to_string() };
                                    if ui.button(label).clicked() {
                                        capturing = Some(i);
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            if btn(ui, "Save").clicked() {
                                match self.core.paths.config_dir.clone() {
                                    Some(c) => match settings::save_keymap(&c, &self.core.keymap) {
                                        Ok(()) => self.core.toast(ToastKind::Info, "Keymap saved"),
                                        Err(e) => self.core.toast(ToastKind::Error, e.to_string()),
                                    },
                                    None => self.core.toast(ToastKind::Warn, "No config folder"),
                                }
                            }
                            if btn(ui, "Reset to defaults").clicked() {
                                self.core.keymap = crate::keymap::Keymap::defaults();
                                capturing = None;
                            }
                        });
                    });
                if !open {
                    close = true;
                }
                new_state = Some(Dialog::Keymap { capturing });
            }
        }
        if close {
            self.core.dialog = follow_up;
        } else if let Some(s) = new_state {
            self.core.dialog = Some(s);
        }
    }

    /// Toasts slide in from the right over 200 ms (a jump with reduced motion) and stack up
    /// from the bottom corner.
    fn toasts(&mut self, ctx: &egui::Context) {
        let theme = self.core.theme.clone();
        if self.core.toasts.is_empty() {
            return;
        }
        let now = self.core.now;
        let reduce = self.core.reduce_motion;
        let mut y = 34.0;
        let mut sliding = false;
        for t in self.core.toasts.iter().rev() {
            let c = match t.kind {
                ToastKind::Info => theme.colors.accent,
                ToastKind::Warn => theme.colors.warn,
                ToastKind::Error => theme.colors.error,
            };
            let key = Id::new(("toast", &t.text, (t.at * 1000.0) as i64));
            let k = if reduce {
                1.0
            } else {
                ((now - t.at) / 0.2).clamp(0.0, 1.0) as f32
            };
            let eased = 1.0 - (1.0 - k) * (1.0 - k);
            sliding |= k < 1.0;
            let known: Option<f32> = ctx.data(|d| d.get_temp(key));
            let out = egui::Area::new(key)
                .anchor(Align2::RIGHT_BOTTOM, [-12.0 + (1.0 - eased) * 70.0, -y])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.set_opacity(if known.is_some() { eased } else { 0.0 });
                    egui::Frame::new()
                        .fill(theme.colors.bg3)
                        .stroke(egui::Stroke::new(1.0, theme.colors.line))
                        .corner_radius(theme.metrics.radius_md as u8)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                let (bar, _) = ui.allocate_exact_size(
                                    egui::vec2(3.0, 16.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter()
                                    .rect_filled(bar, egui::CornerRadius::same(1), c);
                                ui.label(&t.text);
                            });
                        })
                });
            let h = out.response.rect.height();
            ctx.data_mut(|d| d.insert_temp(key, h));
            y += h + 6.0;
        }
        // repaint quickly only while something slides, otherwise let the clock expire toasts
        ctx.request_repaint_after(std::time::Duration::from_millis(if sliding {
            16
        } else {
            500
        }));
    }
}

/// The OS window title: the project name, a dot while there are unsaved changes, then the
/// product.
pub fn window_title(project: &str, dirty: bool) -> String {
    format!(
        "{project}{} \u{2014} Patternhaus",
        if dirty { " \u{2022}" } else { "" }
    )
}

/// What identifies an effect inside its insert: its kind.
fn effect_kind(e: &ph_core::EffectSpec) -> String {
    match e {
        ph_core::EffectSpec::Builtin { name, .. } => name.clone(),
        ph_core::EffectSpec::Plugin { plugin_id, .. } => format!("plugin:{plugin_id}"),
    }
}

fn modal(ctx: &egui::Context, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, add);
}

/// The next non-modifier key press as a chord (for rebinding).
fn captured_chord(ui: &Ui) -> Option<Chord> {
    ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some(Chord {
                key: *key,
                ctrl: modifiers.command,
                shift: modifiers.shift,
                alt: modifiers.alt,
            }),
            _ => None,
        })
    })
}

impl eframe::App for PhApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    /// The window layout is remembered per user across runs.
    fn on_exit(&mut self) {
        if let Some(dir) = self.core.paths.config_dir.clone() {
            let _ = settings::save_layout(&settings::layout_path(&dir), &self.layout());
        }
    }
}
