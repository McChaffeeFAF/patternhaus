//! The application shell: menus, transport, docked panels, dialogs, toasts, key handling and
//! panels that float in their own OS windows.
//!
//! [`PhApp::draw`] draws one whole frame into a `Ui`, so the real window (`eframe`) and the
//! offscreen tests share the same code.

use crate::core::{
    AppCore, AppPaths, Dialog, Hooks, PanelAction, PanelId, RenameTarget, ToastKind,
};
use crate::keymap::{Chord, Context};
use crate::mapping;
use crate::panels;
use crate::settings::{self, Layout};
use crate::theme::Theme;
use egui::{Align2, Color32, Id, RichText, Ui};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use ph_control::Studio;
use ph_engine::EngineConfig;
use std::collections::BTreeSet;

pub struct PhApp {
    pub core: AppCore,
    pub dock: DockState<PanelId>,
    pub detached: BTreeSet<PanelId>,
    theme_applied: bool,
    last_title: String,
}

struct Viewer<'a> {
    core: &'a mut AppCore,
    detach: Vec<PanelId>,
}

pub fn show_panel(ui: &mut Ui, core: &mut AppCore, panel: PanelId) {
    match panel {
        PanelId::Browser => panels::browser::show(ui, core),
        PanelId::ChannelRack => panels::rack::show(ui, core),
        PanelId::PianoRoll => panels::piano_roll::show(ui, core),
        PanelId::Playlist => panels::playlist::show(ui, core),
        PanelId::Mixer => panels::mixer::show(ui, core),
        PanelId::History => panels::history::show(ui, core),
        PanelId::Agents => panels::agents::show(ui, core),
        PanelId::Inspector => panels::inspector::show(ui, core),
    }
}

impl TabViewer for Viewer<'_> {
    type Tab = PanelId;

    fn id(&mut self, tab: &mut PanelId) -> Id {
        Id::new(("panel", tab.name()))
    }

    fn title(&mut self, tab: &mut PanelId) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut PanelId) {
        show_panel(ui, self.core, *tab);
    }

    fn scroll_bars(&self, _tab: &PanelId) -> [bool; 2] {
        [false, false] // panels scroll themselves
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut PanelId, _path: egui_dock::NodePath) {
        if ui.button("Move to its own window").clicked() {
            self.detach.push(*tab);
            ui.close();
        }
    }
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
        let (theme, theme_err) = settings::load_theme(paths.config_dir.as_deref());
        let (keymap, keymap_err) = settings::load_keymap(paths.config_dir.as_deref());
        let layout = settings::load_layout(&settings::project_layout_path(&paths.project_dir))
            .or_else(|| {
                paths
                    .config_dir
                    .as_deref()
                    .and_then(|c| settings::load_layout(&settings::layout_path(c)))
            });
        let (dock, detached) = match layout {
            Some(l) => (l.dock, l.detached),
            None => (settings::default_dock(), BTreeSet::new()),
        };
        let mut core = AppCore::new(
            studio,
            ctx.clone(),
            paths,
            hooks,
            engine_config,
            theme,
            keymap,
        )?;
        core.audio_notice = audio_notice.clone();
        if let Some(n) = audio_notice {
            core.toast(ToastKind::Warn, n);
        }
        for e in [theme_err, keymap_err].into_iter().flatten() {
            core.toast(ToastKind::Warn, e);
        }
        Ok(PhApp {
            core,
            dock,
            detached,
            theme_applied: false,
            last_title: String::new(),
        })
    }

    // ---- panels ----------------------------------------------------------------------------

    fn is_docked(&self, p: PanelId) -> bool {
        self.dock.find_tab(&p).is_some()
    }

    pub fn open_panel(&mut self, p: PanelId) {
        if self.detached.contains(&p) {
            return; // already in its own window
        }
        match self.dock.find_tab(&p) {
            Some(path) => {
                let _ = self.dock.set_active_tab(path);
            }
            None => self.dock.push_to_focused_leaf(p),
        }
    }

    pub fn close_panel(&mut self, p: PanelId) {
        self.detached.remove(&p);
        if let Some(path) = self.dock.find_tab(&p) {
            self.dock.remove_tab(path);
        }
    }

    /// F5-F9 behaviour: show the panel, or hide it when it is already the one in front.
    pub fn toggle_panel(&mut self, p: PanelId) {
        if self.detached.contains(&p) {
            self.close_panel(p);
            return;
        }
        match self.dock.find_tab(&p) {
            Some(path) => {
                let active = self
                    .dock
                    .leaf(egui_dock::NodePath {
                        surface: path.surface,
                        node: path.node,
                    })
                    .ok()
                    .and_then(|l| l.tabs.get(l.active.0))
                    .is_some_and(|t| *t == p);
                if active {
                    self.close_panel(p);
                } else {
                    let _ = self.dock.set_active_tab(path);
                }
            }
            None => self.dock.push_to_focused_leaf(p),
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
    }

    pub fn reset_layout(&mut self) {
        self.dock = settings::default_dock();
        self.detached.clear();
    }

    fn layout(&self) -> Layout {
        Layout {
            dock: self.dock.clone(),
            detached: self.detached.clone(),
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
        if let Ok(t) = crate::theme::load(name) {
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
        if !self.theme_applied {
            self.core.theme.apply(&ctx);
            self.theme_applied = true;
        }
        let now = ctx.input(|i| i.time);
        self.core.update(now);
        self.process_panel_requests();
        self.handle_keys(&ctx);

        let theme = self.core.theme.clone();
        egui::Panel::top("menu").show(ui, |ui| self.menu(ui));
        egui::Panel::top("transport")
            .frame(
                egui::Frame::new()
                    .fill(theme.colors.panel)
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| panels::transport::show(ui, &mut self.core));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme.colors.bg))
            .show(ui, |ui| {
                let mut viewer = Viewer {
                    core: &mut self.core,
                    detach: Vec::new(),
                };
                DockArea::new(&mut self.dock)
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_add_buttons(false)
                    .show_close_buttons(true)
                    .show_inside(ui, &mut viewer);
                let detach = std::mem::take(&mut viewer.detach);
                for p in detach {
                    if let Some(path) = self.dock.find_tab(&p) {
                        self.dock.remove_tab(path);
                    }
                    self.detached.insert(p);
                }
            });

        self.detached_windows(&ctx);
        self.dialogs(&ctx);
        panels::history::conflict_dialog(&ctx, &mut self.core);
        self.toasts(&ctx);

        let title = format!(
            "{}{} - Patternhaus",
            self.core.project.settings.title,
            if self.core.dirty { " *" } else { "" }
        );
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn detached_windows(&mut self, ctx: &egui::Context) {
        for p in self.detached.clone() {
            let mut close = false;
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Patternhaus - {}", p.title()))
                .with_inner_size([960.0, 620.0]);
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("panel", p.name())),
                builder,
                |ui, _class| {
                    egui::CentralPanel::default().show(ui, |ui| show_panel(ui, &mut self.core, p));
                    if ui.input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                },
            );
            if close {
                self.detached.remove(&p);
                self.open_panel(p);
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
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New project…").clicked() {
                    self.core.dialog = Some(Dialog::NewProject);
                    ui.close();
                }
                if ui.button("Open project…").clicked() {
                    self.core
                        .app_action("app.open_project", &serde_json::Value::Null);
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    self.core.send(mapping::file_save());
                    ui.close();
                }
                if ui.button("Save as…").clicked() {
                    self.core
                        .app_action("app.save_as", &serde_json::Value::Null);
                    ui.close();
                }
                ui.separator();
                if ui.button("Export audio…").clicked() {
                    self.core.app_action("app.export", &serde_json::Value::Null);
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.button("Undo (mine)").clicked() {
                    self.core.send(mapping::undo_mine());
                    ui.close();
                }
                if ui.button("Redo (mine)").clicked() {
                    self.core.send(mapping::redo_mine());
                    ui.close();
                }
                ui.separator();
                if ui.button("Clone pattern").clicked() {
                    self.core
                        .app_action("app.clone_pattern", &serde_json::Value::Null);
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                for p in PanelId::ALL {
                    let open = self.is_docked(p) || self.detached.contains(&p);
                    if ui.checkbox(&mut { open }, p.title()).clicked() {
                        self.toggle_panel(p);
                    }
                }
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    if ui.button("Dark").clicked() {
                        self.set_theme("dark");
                        ui.close();
                    }
                    if ui.button("Light").clicked() {
                        self.set_theme("light");
                        ui.close();
                    }
                });
                ui.menu_button("Layout", |ui| {
                    if ui.button("Save for me").clicked() {
                        self.save_layout(false);
                        ui.close();
                    }
                    if ui.button("Save with this project").clicked() {
                        self.save_layout(true);
                        ui.close();
                    }
                    if ui.button("Reset to default").clicked() {
                        self.reset_layout();
                        ui.close();
                    }
                });
                if ui.button("Keyboard shortcuts…").clicked() {
                    self.core.dialog = Some(Dialog::Keymap { capturing: None });
                    ui.close();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let theme = self.core.theme.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.core.project.settings.title).strong());
            ui.label(
                RichText::new(format!(
                    "{} channels · {} patterns · {} clips",
                    self.core.project.channels.len(),
                    self.core.project.patterns.len(),
                    self.core.project.clips.len()
                ))
                .color(theme.colors.text_dim),
            );
            let n = self.core.dispatch.in_flight();
            if n > 0 {
                ui.label(RichText::new(format!("{n} pending")).color(theme.colors.text_dim));
            }
            if self.core.dirty {
                ui.label(RichText::new("unsaved changes").color(theme.colors.warn));
            }
            if let Some(n) = &self.core.audio_notice {
                ui.label(RichText::new(n).color(theme.colors.warn));
            }
        });
    }

    // ---- dialogs ---------------------------------------------------------------------------

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.core.dialog.clone() else {
            return;
        };
        let mut close = false;
        let mut new_state: Option<Dialog> = None;
        match dialog {
            Dialog::NewProject => {
                modal(ctx, "New project", |ui| {
                    ui.label("Discard the current project's unsaved changes and start a new one?");
                    ui.horizontal(|ui| {
                        if ui.button("New project").clicked() {
                            self.core.send(mapping::project_new(None));
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
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
                        .color(Color32::GRAY),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            self.core.send(mapping::file_open(&path));
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
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
                        if ui.button("Save").clicked() {
                            let mut r = mapping::file_save_as(&path);
                            r.params["overwrite"] = overwrite.into();
                            self.core.send(r);
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
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
                            .color(Color32::GRAY),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Export").clicked() {
                            self.core.send(mapping::export_audio(&path, overwrite));
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
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
                        if ui.button("OK").clicked() || enter {
                            let req = match target {
                                RenameTarget::Channel(c) => mapping::channel_rename(c, &text),
                                RenameTarget::Pattern(p) => mapping::pattern_rename(p, &text),
                                RenameTarget::Insert(i) => mapping::mixer_rename(i, &text),
                                RenameTarget::Track(t) => mapping::track_rename(t, &text),
                            };
                            self.core.send(req);
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                new_state = Some(Dialog::Rename { target, text });
            }
            Dialog::Keymap { mut capturing } => {
                let mut open = true;
                egui::Window::new("Keyboard shortcuts")
                    .open(&mut open)
                    .default_width(560.0)
                    .show(ctx, |ui| {
                        ui.label("Click Rebind, then press the new key. Changes apply at once; Save writes your keymap file.");
                        if let Some(i) = capturing {
                            if let Some(chord) = captured_chord(ui) {
                                if let Some(old) = self.core.keymap.bindings.get(i).cloned() {
                                    self.core.keymap.rebind(&old, chord);
                                }
                                capturing = None;
                            }
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
                                    }).color(Color32::GRAY));
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
                            if ui.button("Save").clicked() {
                                match self.core.paths.config_dir.clone() {
                                    Some(c) => match settings::save_keymap(&c, &self.core.keymap) {
                                        Ok(()) => self.core.toast(ToastKind::Info, "Keymap saved"),
                                        Err(e) => self.core.toast(ToastKind::Error, e.to_string()),
                                    },
                                    None => self.core.toast(ToastKind::Warn, "No config folder"),
                                }
                            }
                            if ui.button("Reset to defaults").clicked() {
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
            self.core.dialog = None;
        } else if let Some(s) = new_state {
            self.core.dialog = Some(s);
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let theme = self.core.theme.clone();
        if self.core.toasts.is_empty() {
            return;
        }
        egui::Area::new(Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, [-12.0, -34.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for t in self.core.toasts.iter().rev() {
                    let c = match t.kind {
                        ToastKind::Info => theme.colors.accent,
                        ToastKind::Warn => theme.colors.warn,
                        ToastKind::Error => theme.colors.error,
                    };
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.5, c))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.label(&t.text);
                        });
                }
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
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
}

#[allow(dead_code)]
fn _assert_theme_send(_: &Theme) {}

#[allow(dead_code)]
fn _node(_: NodeIndex) {}
