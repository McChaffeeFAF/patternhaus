//! `AppCore`: everything the panels share. It owns the [`Studio`], turns gestures into commands
//! through the [`Dispatcher`], and keeps the snapshot, selection, history and agent activity the
//! panels draw from.
//!
//! The only way this struct changes the project is `send`, which submits a command as
//! `Caller::human()`.

use crate::browser_model::Kind;
use crate::dispatch::{Dispatcher, Done, Follow};
use crate::keymap::{Binding, Context, Keymap};
use crate::live::{Audition, EngineLink, Live};
use crate::mapping;
use crate::panels::browser::BrowserState;
use crate::panels::inspector::InspectorState;
use crate::panels::mixer::MixerUi;
use crate::panels::rack::RackState;
use crate::piano_roll::{Flash, PianoRollState, PrAction, PrOut, Tool};
use crate::playlist::{PlOut, PlaylistState};
use crate::theme::Theme;
use crate::typing::TypingState;
use crate::workspace::WindowId;
use ph_commands::{
    Change, CmdError, HistoryEntry, Pending, Request, Scope, UiRequest, UndoConflict,
};
use ph_control::Studio;
use ph_core::{ChannelId, InstrumentSpec, NoteId, ObjectPath, PatternId, Project};
use ph_engine::{EngineConfig, PlayMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PanelId {
    Browser,
    ChannelRack,
    PianoRoll,
    Playlist,
    Mixer,
    History,
    Agents,
    Inspector,
}

impl PanelId {
    pub const ALL: [PanelId; 8] = [
        PanelId::Browser,
        PanelId::ChannelRack,
        PanelId::PianoRoll,
        PanelId::Playlist,
        PanelId::Mixer,
        PanelId::History,
        PanelId::Agents,
        PanelId::Inspector,
    ];

    /// Name used in the keymap and in `ui.open_panel`.
    pub fn name(self) -> &'static str {
        match self {
            PanelId::Browser => "browser",
            PanelId::ChannelRack => "channel_rack",
            PanelId::PianoRoll => "piano_roll",
            PanelId::Playlist => "playlist",
            PanelId::Mixer => "mixer",
            PanelId::History => "history",
            PanelId::Agents => "agents",
            PanelId::Inspector => "inspector",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PanelId::Browser => "Browser",
            PanelId::ChannelRack => "Channel rack",
            PanelId::PianoRoll => "Piano roll",
            PanelId::Playlist => "Playlist",
            PanelId::Mixer => "Mixer",
            PanelId::History => "History",
            PanelId::Agents => "Agents",
            PanelId::Inspector => "Inspector",
        }
    }

    pub fn from_name(s: &str) -> Option<PanelId> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }

    pub fn context(self) -> Context {
        match self {
            PanelId::PianoRoll => Context::PianoRoll,
            PanelId::Playlist => Context::Playlist,
            PanelId::ChannelRack => Context::ChannelRack,
            PanelId::Mixer => Context::Mixer,
            _ => Context::Global,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PanelAction {
    Open,
    Close,
    Toggle,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Channel(ChannelId),
    Pattern(PatternId),
    Insert(u32),
    Track(u32),
}

/// What was asked for while the project had unsaved changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterPrompt {
    Quit,
    NewProject,
    OpenProject { path: String },
}

#[derive(Clone, Debug)]
pub enum Dialog {
    OpenProject {
        path: String,
    },
    /// "Save / Don't save / Cancel" before something that would lose unsaved changes.
    Unsaved {
        then: AfterPrompt,
    },
    SaveAs {
        path: String,
        overwrite: bool,
    },
    Export {
        path: String,
        overwrite: bool,
    },
    Rename {
        target: RenameTarget,
        text: String,
    },
    Keymap {
        capturing: Option<usize>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub at: f64,
}

/// An undo that stopped on conflicts, waiting for Skip / Force / Cancel (DESIGN §3).
#[derive(Clone, Debug)]
pub struct ConflictDialog {
    pub request: Request,
    pub conflicts: Vec<UndoConflict>,
}

#[derive(Clone, Debug)]
pub struct ClientInfo {
    pub name: String,
    pub scopes: Vec<Scope>,
    /// Connected right now. A remembered client that is not connected is listed greyed out.
    pub live: bool,
    /// Seconds since the client connected (live clients only).
    pub connected_for: u64,
    /// Seconds since the client last did anything (live clients only).
    pub idle_for: u64,
    /// Tool calls the client has made on this connection (live clients only).
    pub commands: u64,
}

#[derive(Clone, Debug)]
pub struct GrantRequest {
    pub id: u64,
    pub client: String,
    pub requested: Vec<Scope>,
}

/// What the Agents panel needs from the agent interface. T4 implements this over its
/// connection registry and hands it to the app (see `ph-app`'s `AGENT HOOK`).
pub trait AgentDirectory: Send + Sync {
    fn clients(&self) -> Vec<ClientInfo>;
    fn pending_grants(&self) -> Vec<GrantRequest>;
    /// Answer a grant prompt: `Some(scopes)` to allow, `None` to deny.
    fn resolve_grant(&self, id: u64, granted: Option<Vec<Scope>>);
}

#[derive(Clone, Default)]
pub struct Hooks {
    pub agents: Option<Arc<dyn AgentDirectory>>,
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub project_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub library_roots: Vec<PathBuf>,
    /// Where keymap, layout and settings are stored (`None` disables persistence).
    pub config_dir: Option<PathBuf>,
}

/// Current selection across panels.
#[derive(Clone, Debug)]
pub struct Selection {
    pub channel: Option<ChannelId>,
    pub pattern: Option<PatternId>,
    pub insert: u32,
    pub effect: Option<usize>,
}

pub struct AppCore {
    pub ctx: egui::Context,
    pub studio: Studio,
    pub dispatch: Dispatcher,
    pub link: EngineLink,
    pub project: Arc<Project>,
    changes: Receiver<Change>,
    pub theme: Arc<Theme>,
    pub keymap: Keymap,
    pub sel: Selection,
    pub roll: PianoRollState,
    pub playlist: PlaylistState,
    pub rack: RackState,
    pub mixer_ui: MixerUi,
    pub browser: BrowserState,
    pub inspector: InspectorState,
    pub typing: TypingState,
    /// Recently touched objects, by agents: path -> (author, time).
    pub touches: HashMap<ObjectPath, (String, f64)>,
    pub flashes: Vec<Flash>,
    pub agent_select: BTreeSet<ObjectPath>,
    agent_select_at: f64,
    pub history: Vec<HistoryEntry>,
    pub txn_times: HashMap<u64, std::time::SystemTime>,
    history_dirty: bool,
    history_pending: Option<Pending<Vec<HistoryEntry>>>,
    /// An in-flight read of the session's solo state (polled once per frame, see `update`).
    solo_pending: Option<Pending<ph_commands::SoloState>>,
    pub toasts: Vec<Toast>,
    pub conflict: Option<ConflictDialog>,
    pub dialog: Option<Dialog>,
    pub hooks: Hooks,
    pub panel_requests: Vec<(PanelId, PanelAction)>,
    /// Requests for one specific window (an instrument or effect window).
    pub window_requests: Vec<(crate::workspace::WindowId, PanelAction)>,
    /// Escape: close the focused instrument or effect window.
    pub close_focused_requested: bool,
    /// Keymap context under the pointer (set by the panels as they draw).
    pub hover_context: Context,
    pub engine_config: EngineConfig,
    pub paths: AppPaths,
    pub notice_dismissed: bool,
    pub audio_notice: Option<String>,
    pub solo_channels: BTreeSet<ChannelId>,
    pub solo_inserts: BTreeSet<u32>,
    pub now: f64,
    pub dirty: bool,
    /// Size of the window content currently being drawn (windows that scroll give their
    /// panels an unbounded `available_size`, so panels that lay out to a size read this).
    pub view_size: egui::Vec2,
    /// Last scroll state of each scrolling window: offset, content size and viewport size.
    pub scrolls: HashMap<crate::workspace::WindowId, ScrollInfo>,
    /// Effect moves and removals the UI performed itself, for the windows to follow.
    pub effect_edits: Vec<EffectEdit>,
    /// Waiting for a save to finish before doing what the person asked (quit, new, open).
    pub after_save: Option<AfterPrompt>,
    /// Ready to do now: the person said "Don't save", or the save they asked for finished.
    pub ready: Vec<AfterPrompt>,
    /// The whole project was replaced (file.open, new): effect windows cannot follow.
    pub project_replaced: bool,
}

/// A window's scroll area, as last drawn.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ScrollInfo {
    pub offset: egui::Vec2,
    pub content: egui::Vec2,
    pub viewport: egui::Vec2,
}

/// A change to an insert's effect list that the UI itself made (so its windows follow directly).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EffectEdit {
    Move { insert: u32, from: u32, to: u32 },
    Remove { insert: u32, slot: u32 },
}

impl AppCore {
    pub fn new(
        studio: Studio,
        ctx: egui::Context,
        paths: AppPaths,
        hooks: Hooks,
        engine_config: EngineConfig,
        theme: Theme,
        keymap: Keymap,
    ) -> Result<AppCore, String> {
        let raw_changes = studio.session.subscribe().map_err(|e| e.to_string())?;
        // Wake the UI when a snapshot arrives (an agent edit while the window is idle).
        let (fwd_tx, changes) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        std::thread::Builder::new()
            .name("ph-ui-changes".into())
            .spawn(move || {
                for c in raw_changes {
                    if fwd_tx.send(c).is_err() {
                        break;
                    }
                    wake.request_repaint();
                }
            })
            .map_err(|e| e.to_string())?;
        // The session's own snapshot (not `studio.project()`, which trails the engine bridge):
        // everything committed before the subscription is in it, everything after arrives on
        // `changes`.
        let project = studio.session.project().map_err(|e| e.to_string())?;
        let link = EngineLink::spawn(studio.engine.clone(), ctx.clone());
        let dispatch = Dispatcher::new(studio.session.clone());
        let mut core = AppCore {
            ctx,
            studio,
            dispatch,
            link,
            project: project.clone(),
            changes,
            theme: Arc::new(theme),
            keymap,
            sel: Selection {
                channel: None,
                pattern: None,
                insert: 1,
                effect: None,
            },
            roll: PianoRollState::default(),
            playlist: PlaylistState::default(),
            rack: RackState::default(),
            mixer_ui: MixerUi::default(),
            browser: BrowserState::new(&paths),
            inspector: InspectorState::default(),
            typing: TypingState::default(),
            touches: HashMap::new(),
            flashes: Vec::new(),
            agent_select: BTreeSet::new(),
            agent_select_at: 0.0,
            history: Vec::new(),
            txn_times: HashMap::new(),
            history_dirty: true,
            history_pending: None,
            solo_pending: None,
            toasts: Vec::new(),
            conflict: None,
            dialog: None,
            hooks,
            panel_requests: Vec::new(),
            window_requests: Vec::new(),
            close_focused_requested: false,
            hover_context: Context::Global,
            engine_config,
            paths,
            notice_dismissed: false,
            audio_notice: None,
            solo_channels: BTreeSet::new(),
            solo_inserts: BTreeSet::new(),
            now: 0.0,
            dirty: false,
            view_size: egui::Vec2::ZERO,
            scrolls: HashMap::new(),
            effect_edits: Vec::new(),
            project_replaced: false,
            after_save: None,
            ready: Vec::new(),
        };
        core.fix_selection();
        Ok(core)
    }

    // ---- sending -------------------------------------------------------------------------

    /// Submit a command as the human. Never blocks.
    pub fn send(&mut self, req: Request) {
        self.dispatch.send(req);
    }

    pub fn send_follow(&mut self, req: Request, follow: Follow) {
        self.dispatch.send_follow(req, follow);
    }

    pub fn send_batch(&mut self, reqs: Vec<Request>) {
        self.dispatch.send_batch(reqs);
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        if let Some(last) = self.toasts.last_mut()
            && last.text == text
        {
            last.at = self.now; // same message again: keep one toast, refresh its timer
            return;
        }
        self.toasts.push(Toast {
            text,
            kind,
            at: self.now,
        });
        if self.toasts.len() > 6 {
            self.toasts.remove(0);
        }
    }

    pub fn live(&self) -> Live {
        self.link.snapshot()
    }

    // ---- per-frame update ------------------------------------------------------------------

    /// Drains snapshots, command replies and `ui.*` requests. Call once per frame.
    pub fn update(&mut self, now: f64) {
        self.now = now;
        while let Ok(change) = self.changes.try_recv() {
            self.on_change(change);
        }
        for done in self.dispatch.poll() {
            self.on_done(done);
        }
        while let Ok(req) = self.studio.ui_requests.try_recv() {
            self.on_ui_request(req);
        }
        // history refresh (async)
        if let Some(p) = &self.history_pending {
            match p.try_wait() {
                Ok(Some(h)) => {
                    self.history = h;
                    self.history_pending = None;
                }
                Ok(None) => {}
                Err(_) => self.history_pending = None,
            }
        }
        if self.history_dirty && self.history_pending.is_none() {
            self.history_dirty = false;
            self.history_pending = Some(self.studio.session.submit(|s, _| s.history().to_vec()));
        }
        // Solo is session state with no `Change`, so an agent can solo something without any
        // snapshot telling us. Read the session's real state once per frame instead: one small
        // job on the control thread, never waited on.
        if let Some(p) = &self.solo_pending {
            match p.try_wait() {
                Ok(Some(s)) => {
                    self.solo_channels = s.channels;
                    self.solo_inserts = s.inserts;
                    self.solo_pending = None;
                }
                Ok(None) => {}
                Err(_) => self.solo_pending = None,
            }
        }
        if self.solo_pending.is_none() {
            self.solo_pending = Some(self.studio.session.submit(|s, _| s.solo().clone()));
        }
        // fade things out
        self.toasts.retain(|t| now - t.at < 7.0);
        self.touches.retain(|_, (_, at)| now - *at < 12.0);
        self.flashes.retain(|f| now - f.at < 12.0);
        if !self.agent_select.is_empty() && now - self.agent_select_at > 10.0 {
            self.agent_select.clear();
        }
    }

    fn on_change(&mut self, c: Change) {
        self.project = c.project.clone();
        self.history_dirty = true;
        self.dirty = true;
        if c.txn == 0 {
            // wholesale replacement (file.open): start fresh
            self.sel = Selection {
                channel: None,
                pattern: None,
                insert: 1,
                effect: None,
            };
            self.roll = PianoRollState::default();
            self.txn_times.clear();
            self.touches.clear();
            self.flashes.clear();
            self.solo_channels.clear();
            self.solo_inserts.clear();
            self.project_replaced = true;
            self.dirty = false;
        } else {
            self.txn_times.insert(c.txn, std::time::SystemTime::now());
        }
        let author = c.author.to_string();
        if c.author != ph_commands::Author::Human && c.txn != 0 {
            for p in &c.paths {
                self.touches.insert(*p, (author.clone(), self.now));
                if let ObjectPath::Note(_, _, n) = p {
                    self.flashes.push(Flash {
                        note: *n,
                        author: author.clone(),
                        at: self.now,
                    });
                }
            }
            if let Some(s) = &c.summary {
                self.toast(ToastKind::Info, format!("{author}: {s}"));
            }
        }
        self.fix_selection();
    }

    /// Keeps selections pointing at things that exist.
    pub fn fix_selection(&mut self) {
        let p = self.project.clone();
        if self
            .sel
            .channel
            .is_none_or(|c| !p.channels.contains_key(&c))
        {
            self.sel.channel = p.channel_order.first().copied();
        }
        if self
            .sel
            .pattern
            .is_none_or(|x| !p.patterns.contains_key(&x))
        {
            self.sel.pattern = p.pattern_order.first().copied();
        }
        if let (Some(pat), Some(ch)) = (self.sel.pattern, self.sel.channel)
            && (self.roll.pattern.is_none()
                || self.roll.pattern != Some(pat)
                || self.roll.channel != Some(ch))
        {
            self.roll.focus(pat, ch);
        }
    }

    fn on_done(&mut self, d: Done) {
        match d.result {
            Ok(ex) => {
                if d.follow == Follow::SelectNotes
                    && let Some(ids) = ex.result.get("notes").and_then(|v| v.as_array())
                {
                    self.roll.want = ids
                        .iter()
                        .filter_map(|v| v.as_u64())
                        .map(|n| NoteId(n as u32))
                        .collect();
                }
                if d.request.command == "file.save" || d.request.command == "file.save_as" {
                    self.dirty = false;
                    self.toast(ToastKind::Info, "Saved");
                    if let Some(p) = self.after_save.take() {
                        self.ready.push(p);
                    }
                }
                if d.request.command == "file.export_audio" {
                    let path = d.request.params["path"].as_str().unwrap_or("");
                    self.toast(ToastKind::Info, format!("Exported {path}"));
                }
            }
            Err(CmdError::UndoConflict(conflicts)) => {
                self.conflict = Some(ConflictDialog {
                    request: d.request,
                    conflicts,
                });
            }
            Err(e) => {
                if d.request.command.starts_with("file.save") && self.after_save.take().is_some() {
                    self.toast(ToastKind::Warn, "Not saved, so nothing was closed");
                }
                self.toast(ToastKind::Error, e.to_string())
            }
        }
    }

    /// `ui.open_panel` / `ui.close_panel` names: a panel, `instrument:<channel>` or
    /// `effect:<insert>:<slot>`.
    fn queue_panel(&mut self, name: &str, a: PanelAction) {
        if let Some(p) = PanelId::from_name(name) {
            self.panel_requests.push((p, a));
        } else if let Some(w) = WindowId::parse(name) {
            self.window_requests.push((w, a));
        }
    }

    fn on_ui_request(&mut self, r: UiRequest) {
        match r {
            UiRequest::Open { panel } => self.queue_panel(&panel, PanelAction::Open),
            UiRequest::Close { panel } => self.queue_panel(&panel, PanelAction::Close),
            UiRequest::Select { paths } => {
                self.agent_select = paths.iter().copied().collect();
                self.agent_select_at = self.now;
                for p in paths {
                    match p {
                        ObjectPath::Channel(c) => {
                            self.sel.channel = Some(c);
                            self.window_requests
                                .push((WindowId::Instrument(c), PanelAction::Open));
                        }
                        ObjectPath::Pattern(x) => self.sel.pattern = Some(x),
                        ObjectPath::Insert(i) => self.sel.insert = i.0,
                        ObjectPath::Note(pat, ch, _) => {
                            self.sel.pattern = Some(pat);
                            self.sel.channel = Some(ch);
                            self.panel_requests
                                .push((PanelId::PianoRoll, PanelAction::Open));
                        }
                        _ => {}
                    }
                }
                self.fix_selection();
            }
            UiRequest::PianoRoll { pattern, channel } => {
                self.sel.pattern = Some(pattern);
                self.sel.channel = Some(channel);
                self.agent_select = [ObjectPath::Pattern(pattern), ObjectPath::Channel(channel)]
                    .into_iter()
                    .collect();
                self.agent_select_at = self.now;
                self.roll.focus(pattern, channel);
                self.panel_requests
                    .push((PanelId::PianoRoll, PanelAction::Open));
            }
            UiRequest::Notify { message } => self.toast(ToastKind::Info, message),
        }
    }

    // ---- gestures from panels --------------------------------------------------------------

    pub fn preview(&self, channel: ChannelId, key: u8, on: bool) {
        self.link.preview(channel, key, self.typing.velocity, on);
    }

    pub fn apply_roll(&mut self, outs: Vec<PrOut>) {
        let mut it = outs.into_iter().peekable();
        while let Some(o) = it.next() {
            match o {
                PrOut::Cmd(r) => {
                    if matches!(it.peek(), Some(PrOut::SelectCreated)) {
                        it.next();
                        self.send_follow(r, Follow::SelectNotes);
                    } else {
                        self.send(r);
                    }
                }
                PrOut::Batch(reqs) => self.send_batch(reqs),
                PrOut::Preview { key, on } => {
                    if let Some(c) = self.roll.channel {
                        self.preview(c, key, on);
                    }
                }
                PrOut::Seek(t) => self.send(mapping::seek(t)),
                PrOut::SelectCreated => {}
            }
        }
    }

    pub fn apply_playlist(&mut self, outs: Vec<PlOut>) {
        for o in outs {
            match o {
                PlOut::Cmd(r) => self.send(r),
                PlOut::Batch(reqs) => self.send_batch(reqs),
                PlOut::Seek(t) => self.send(mapping::seek(t)),
                PlOut::OpenPattern(p) => {
                    self.sel.pattern = Some(p);
                    self.fix_selection();
                    self.panel_requests
                        .push((PanelId::PianoRoll, PanelAction::Open));
                }
            }
        }
    }

    /// Drop a sample or preset: a new channel with that instrument (a Sampler for samples, a
    /// Synth for presets). `name` is tidied for the rack.
    pub fn add_channel_from(&mut self, name: &str, kind: &Kind) {
        if let Some(inst) = crate::browser_model::instrument_for(kind) {
            self.add_channel(&crate::browser_model::channel_name(name), &inst);
        }
    }

    pub fn add_channel(&mut self, name: &str, inst: &InstrumentSpec) {
        self.send(mapping::channel_add(name, inst, None));
    }

    /// Click-to-preview in the browser: plays the entry on the engine's audition voice,
    /// outside the project (nothing is added to the session or the render plan).
    pub fn audition(&mut self, kind: &Kind) {
        match kind {
            Kind::Sample { reference } => self.link.audition(Audition::Sample(reference.clone())),
            Kind::Preset { .. } => {
                if let Some(instrument) = crate::browser_model::instrument_for(kind) {
                    self.link.audition(Audition::Instrument {
                        instrument,
                        key: 60,
                        hold: Duration::from_millis(1200),
                    });
                }
            }
            Kind::Folder => {}
        }
    }

    /// Name of the transport mode and toggle helper.
    pub fn toggle_mode(&mut self) {
        let live = self.live();
        match live.status.mode {
            PlayMode::Song => self.send(mapping::set_mode_pattern(self.sel.pattern)),
            PlayMode::Pattern(_) => self.send(mapping::set_mode_song()),
        }
    }

    /// Selects a pattern; in pattern mode the transport follows it.
    pub fn select_pattern(&mut self, p: PatternId) {
        self.sel.pattern = Some(p);
        self.fix_selection();
        if matches!(self.live().status.mode, PlayMode::Pattern(_)) {
            self.send(mapping::set_mode_pattern(Some(p)));
        }
    }

    /// Switch the audio buffer size (a brief dropout); used by the "use a larger buffer" notice.
    pub fn set_buffer(&mut self, frames: u32) {
        let mut cfg = self.engine_config.clone();
        cfg.buffer_frames = frames;
        self.engine_config = cfg.clone();
        self.link.reconfigure(cfg);
        self.notice_dismissed = true;
    }

    /// Do `what` now, or first ask Save / Don't save / Cancel when that would lose changes.
    pub fn ask_or_do(&mut self, what: AfterPrompt) {
        if self.dirty {
            self.dialog = Some(Dialog::Unsaved { then: what });
        } else {
            self.ready.push(what);
        }
    }

    pub fn open_rename(&mut self, target: RenameTarget) {
        let text = match target {
            RenameTarget::Channel(c) => self.project.channels.get(&c).map(|c| c.name.clone()),
            RenameTarget::Pattern(p) => self.project.patterns.get(&p).map(|p| p.name.clone()),
            RenameTarget::Insert(i) => self.project.mixer.get(i as usize).map(|m| m.name.clone()),
            RenameTarget::Track(t) => self.project.tracks.get(t as usize).map(|t| t.name.clone()),
        };
        if let Some(text) = text {
            self.dialog = Some(Dialog::Rename { target, text });
        }
    }

    /// Looks up the binding for a key press under the current hover context and runs it.
    pub fn run_binding(&mut self, b: &Binding) {
        if b.is_app_action() {
            self.app_action(&b.command, &b.params);
        } else {
            let mut r = Request::new(
                &b.command,
                if b.params.is_null() {
                    Value::Object(Default::default())
                } else {
                    b.params.clone()
                },
            );
            // Destructive commands bound to a key still ask: the dialogs own the confirm.
            if b.command == "project.new" || b.command == "file.open" {
                r.confirm = true;
            }
            self.send(r);
        }
    }

    pub fn app_action(&mut self, name: &str, params: &Value) {
        let live = self.live();
        match name {
            "app.toggle_panel" => {
                if let Some(p) = params["panel"].as_str().and_then(PanelId::from_name) {
                    self.panel_requests.push((p, PanelAction::Toggle));
                }
            }
            "app.close_window" => self.close_focused_requested = true,
            "app.save_as" => {
                self.dialog = Some(Dialog::SaveAs {
                    path: self.paths.project_dir.to_string_lossy().into_owned(),
                    overwrite: false,
                })
            }
            "app.open_project" => {
                self.dialog = Some(Dialog::OpenProject {
                    path: self.paths.project_dir.to_string_lossy().into_owned(),
                })
            }
            "app.new_project" => self.ask_or_do(AfterPrompt::NewProject),
            "app.export" => {
                self.dialog = Some(Dialog::Export {
                    path: "exports/mix.wav".into(),
                    overwrite: false,
                })
            }
            "app.clone_pattern" => {
                if let Some(p) = self.sel.pattern {
                    self.send(mapping::pattern_clone(p));
                }
            }
            "app.toggle_mode" => self.toggle_mode(),
            "app.toggle_metronome" => self.send(mapping::set_metronome(!live.status.metronome)),
            "app.toggle_typing_piano" => self.typing.enabled = !self.typing.enabled,
            "app.octave" => self
                .typing
                .shift_octave(params["delta"].as_i64().unwrap_or(0) as i32),
            "app.roll.tool" => {
                let t = match params["tool"].as_str() {
                    Some("select") => Tool::Select,
                    Some("delete") => Tool::Delete,
                    Some("slice") => Tool::Slice,
                    Some("mute") => Tool::Mute,
                    _ => Tool::Draw,
                };
                self.roll.tool = t;
            }
            "app.roll.duplicate" => self.roll_action(PrAction::Duplicate),
            "app.roll.quantize" => self.roll_action(PrAction::Quantize),
            "app.roll.select_all" => self.roll_action(PrAction::SelectAll),
            "app.roll.delete" => self.roll_action(PrAction::DeleteSelected),
            "app.roll.copy" => self.roll_action(PrAction::Copy),
            "app.roll.cut" => self.roll_action(PrAction::Cut),
            "app.roll.paste" => {
                let at = self.roll_cursor_tick();
                let outs = self.roll.paste(at);
                self.apply_roll(outs);
            }
            "app.roll.transpose" => {
                let s = params["semitones"].as_i64().unwrap_or(0);
                let a = match s {
                    1 => PrAction::TransposeUp,
                    -1 => PrAction::TransposeDown,
                    12 => PrAction::OctaveUp,
                    -12 => PrAction::OctaveDown,
                    _ => return,
                };
                self.roll_action(a);
            }
            "app.playlist.duplicate" => {
                let outs = self.playlist.duplicate();
                self.apply_playlist(outs);
            }
            "app.playlist.delete" => {
                let outs = self.playlist.delete_selected();
                self.apply_playlist(outs);
            }
            "app.playlist.select_all" => self.playlist.select_all(),
            other => self.toast(ToastKind::Warn, format!("unknown action {other}")),
        }
    }

    fn roll_action(&mut self, a: PrAction) {
        let outs = self.roll.action(a);
        self.apply_roll(outs);
    }

    /// Where a paste lands: the pattern playhead if playing it, else the view's left edge.
    fn roll_cursor_tick(&self) -> u64 {
        self.roll.view.x0.max(0.0) as u64
    }

    /// Handles the answer to the conflict dialog.
    pub fn resolve_conflict(&mut self, choice: ConflictChoice) {
        let Some(d) = self.conflict.take() else {
            return;
        };
        let mode = match choice {
            ConflictChoice::Skip => "skip",
            ConflictChoice::Force => "force",
            ConflictChoice::Cancel => return,
        };
        let mut r = d.request;
        if let Some(o) = r.params.as_object_mut() {
            o.insert("mode".into(), Value::String(mode.into()));
        } else {
            r.params = serde_json::json!({ "mode": mode });
        }
        self.send(r);
    }

    pub fn authors(&self) -> Vec<String> {
        let mut a: Vec<String> = self
            .history
            .iter()
            .map(|e| e.author.to_string())
            .filter(|a| a != "human")
            .collect();
        a.sort();
        a.dedup();
        a
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    Skip,
    Force,
    Cancel,
}
