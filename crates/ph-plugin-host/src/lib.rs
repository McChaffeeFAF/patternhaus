//! Plugin hosting contract (M2/T0; DESIGN §6). The CLAP and VST3 implementations are M2/T3.
//!
//! - A [`PluginFormatHost`] per format scans plugin files into [`PluginInfo`]s and creates
//!   [`PluginInstance`]s.
//! - A [`PluginInstance`] is a [`Processor`] (so the engine treats it like a built-in) plus
//!   its [`PluginInfo`] and an optional [`PluginEditor`].
//! - The engine never calls an instance directly: it wraps it in a [`PluginNode`], the
//!   **crash-catch boundary** (below).
//! - State is the instance's [`Processor::save_state`] bytes, written by the host to
//!   `plugins/<file>` in the project folder ([`PluginState`], DESIGN §10) and named by
//!   `ph_core::InstrumentSpec::Plugin::state`.
//! - [`fake`] is an in-repo test plugin implementing all of it, so tests never need a
//!   third-party plugin.
//!
//! # The crash-catch boundary (contract)
//!
//! 1. Every call into plugin code goes through [`PluginNode`]; nothing else holds the
//!    instance while it is in the engine.
//! 2. A panic during a call (in our adapter, or in a plugin written in Rust) is caught with
//!    `catch_unwind`. Hardware faults in native plugin code (access violations) are caught
//!    as far as the platform allows by the format adapter (T3: structured exception
//!    handling on Windows, signal handlers on Unix) and reported to the node as a fault.
//! 3. After a fault the node is **dead**: it outputs silence and returns
//!    [`ProcessStatus::Sleep`] for the rest of its life and never calls the plugin again,
//!    not even to drop it (the instance is leaked: its state may be corrupt).
//! 4. The fault is visible off the audio thread through [`PluginNode::fault_handle`]: the
//!    host marks the plugin [`PluginStatus::Isolate`] in its scan cache (it loads
//!    out of process from M4 on), logs it, and the engine reports a warning.
//! 5. Real-time rules hold until the fault; the fault path itself may allocate (the
//!    panic already did) because it happens at most once per node.

pub mod clap;
pub mod embed;
pub mod fake;
pub mod guard;
mod main_thread;
pub mod manager;
pub mod scan;
pub mod testing;
pub mod vst3;

use ph_node::*;
use serde::{Deserialize, Serialize};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use ph_core::PluginFormat;
pub use raw_window_handle::RawWindowHandle;

/// What a plugin is for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Instrument,
    Effect,
    /// Takes and produces note events (arpeggiators, chorders).
    NoteEffect,
}

/// One plugin found by a scan. A file or bundle may hold several.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub format: PluginFormat,
    /// CLAP plugin id (`com.vendor.name`) or VST3 class id (32 hex digits). With `format`,
    /// this is what a project stores (`InstrumentSpec::Plugin::plugin_id`).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub kind: PluginKind,
    /// The file or bundle it was loaded from.
    pub path: PathBuf,
    /// Offers its own editor window.
    #[serde(default)]
    pub has_editor: bool,
}

/// How the host treats a plugin, kept in the scan cache.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    #[default]
    Ok,
    /// Crashed (or set by hand): load it out of process (M4; until then it is not loaded).
    Isolate,
    /// Never load it (failed to scan, or the user said so).
    Blocked,
}

/// The result of scanning folders: what was found and what could not be read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanReport {
    pub found: Vec<PluginInfo>,
    pub failed: Vec<ScanFailure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanFailure {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PluginError {
    #[error("{0}: not a plugin this format can load")]
    NotAPlugin(PathBuf),
    #[error("plugin {0} not found")]
    NotFound(String),
    #[error("plugin call failed: {0}")]
    Call(String),
    #[error("the plugin crashed earlier and is no longer called")]
    Dead,
    #[error("{0} is not available until M2/{1}")]
    NotAvailable(&'static str, &'static str),
}

pub type PluginResult<T> = Result<T, PluginError>;

/// One plugin format (CLAP or VST3). Everything here runs off the audio thread.
pub trait PluginFormatHost: Send + Sync {
    fn format(&self) -> PluginFormat;
    /// The platform's standard plugin folders for this format.
    fn default_paths(&self) -> Vec<PathBuf>;
    /// Reads one plugin file or bundle. May be slow and may crash (a scan of an unknown
    /// plugin should run in a helper process; T3 decides).
    fn scan(&self, path: &Path) -> PluginResult<Vec<PluginInfo>>;
    /// Creates an instance. The caller activates it like any processor.
    fn instantiate(&self, info: &PluginInfo) -> PluginResult<Box<dyn PluginInstance>>;

    /// Scans every path, collecting failures instead of stopping.
    fn scan_all(&self, paths: &[PathBuf]) -> ScanReport {
        let mut r = ScanReport::default();
        for p in paths {
            match self.scan(p) {
                Ok(v) => r.found.extend(v),
                Err(e) => r.failed.push(ScanFailure {
                    path: p.clone(),
                    error: e.to_string(),
                }),
            }
        }
        r
    }
}

/// A live plugin: a [`Processor`] (parameters by the plugin's own ids, as text, in
/// [`ParamInfo::id`]; state through `save_state`/`load_state`) plus what only plugins have.
pub trait PluginInstance: Processor {
    fn info(&self) -> &PluginInfo;

    /// The editor, if the plugin has one. Called on the UI thread before the instance goes
    /// to the engine; the handle stays valid while the instance lives. Hosts must close it
    /// before the instance is dropped.
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        None
    }

    /// A thread-safe handle for everything but processing (M2/T3, added): state, current
    /// parameter values, latency and the editor, usable from any thread while the instance
    /// itself runs inside the engine. Format adapters return one; [`PluginNode::new`] adopts
    /// its [`FaultHandle`], so a fault seen by either side kills both.
    fn control(&self) -> Option<Arc<dyn PluginControl>> {
        None
    }
}

/// Off-audio-thread access to a live plugin (see [`PluginInstance::control`]). Every call is
/// marshalled to the plugin's main thread and crash-guarded; after a fault every call fails
/// with [`PluginError::Dead`].
pub trait PluginControl: Send + Sync {
    fn info(&self) -> &PluginInfo;
    /// The fault handle shared with the instance (and its [`PluginNode`]).
    fn fault(&self) -> Arc<FaultHandle>;
    /// Current plain values of every parameter, in descriptor order.
    fn param_values(&self) -> PluginResult<Vec<f64>>;
    /// The plugin's own text for a value of the parameter at `index` (descriptor order).
    fn param_text(&self, index: usize, value: f64) -> Option<String>;
    /// The plugin's state, as [`Processor::save_state`] would return it.
    fn save_state(&self) -> PluginResult<Vec<u8>>;
    /// Latency in samples, as last reported by the plugin.
    fn latency(&self) -> u32;
    /// The plugin's own editor, if it has one.
    fn editor(&self) -> Option<Box<dyn PluginEditor>>;
}

/// Size of an editor in logical pixels.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSize {
    pub width: u32,
    pub height: u32,
}

/// A plugin's own window. UI thread only.
pub trait PluginEditor: Send {
    /// Can live inside a host window (`open(Some(parent))`).
    fn can_embed(&self) -> bool;
    /// Opens the editor as a child of `parent`, or floating with `None`. Returns its size.
    fn open(&mut self, parent: Option<RawWindowHandle>) -> PluginResult<EditorSize>;
    fn close(&mut self);
    fn is_open(&self) -> bool;
    fn can_resize(&self) -> bool;
    /// Asks for a new size; the plugin may adjust it. Returns the size it took.
    fn set_size(&mut self, size: EditorSize) -> PluginResult<EditorSize>;
    fn size(&self) -> Option<EditorSize>;
}

/// A plugin's saved state as the host writes it to `plugins/<file>`: the instance's
/// `save_state` bytes with enough identity to refuse loading them into another plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginState {
    pub format: PluginFormat,
    pub plugin_id: String,
    /// The plugin version that wrote it (plugins migrate their own states).
    pub version: String,
    pub data: Vec<u8>,
}

impl PluginState {
    pub fn save(instance: &dyn PluginInstance) -> Self {
        let i = instance.info();
        Self {
            format: i.format,
            plugin_id: i.id.clone(),
            version: i.version.clone(),
            data: instance.save_state(),
        }
    }

    /// First line of a state file.
    pub const FILE_MAGIC: &'static str = "PATTERNHAUS-PLUGIN-STATE 1";

    /// The bytes of a `plugins/<file>` state file: a magic line, one JSON line naming the
    /// plugin, then the plugin's own state bytes unchanged.
    pub fn to_file_bytes(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct Header<'a> {
            format: PluginFormat,
            plugin_id: &'a str,
            version: &'a str,
            bytes: usize,
        }
        let header = serde_json::to_string(&Header {
            format: self.format,
            plugin_id: &self.plugin_id,
            version: &self.version,
            bytes: self.data.len(),
        })
        .expect("serialisable");
        let mut out = format!("{}\n{header}\n", Self::FILE_MAGIC).into_bytes();
        out.extend_from_slice(&self.data);
        out
    }

    /// Reads [`Self::to_file_bytes`] output.
    pub fn from_file_bytes(bytes: &[u8]) -> PluginResult<Self> {
        #[derive(Deserialize)]
        struct Header {
            format: PluginFormat,
            plugin_id: String,
            version: String,
            bytes: usize,
        }
        let bad = |m: &str| PluginError::Call(format!("not a plugin state file: {m}"));
        let mut lines = bytes.splitn(3, |b| *b == b'\n');
        if lines.next() != Some(Self::FILE_MAGIC.as_bytes()) {
            return Err(bad("missing header"));
        }
        let header: Header = serde_json::from_slice(lines.next().ok_or_else(|| bad("no header"))?)
            .map_err(|e| bad(&e.to_string()))?;
        let data = lines.next().unwrap_or(&[]).to_vec();
        if data.len() != header.bytes {
            return Err(bad("truncated"));
        }
        Ok(Self {
            format: header.format,
            plugin_id: header.plugin_id,
            version: header.version,
            data,
        })
    }

    /// Loads into `instance` after checking it is the same plugin.
    pub fn load_into(
        &self,
        instance: &mut dyn PluginInstance,
        assets: &dyn AssetResolver,
    ) -> PluginResult<()> {
        let i = instance.info();
        if i.format != self.format || i.id != self.plugin_id {
            return Err(PluginError::Call(format!(
                "state belongs to {} ({:?}), not {}",
                self.plugin_id, self.format, i.id
            )));
        }
        instance
            .load_state(&self.data, assets)
            .map_err(PluginError::Call)
    }
}

/// Which call faulted.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCall {
    Activate,
    Process,
    SetParam,
    Param,
    Reset,
    SaveState,
    LoadState,
}

/// A recorded fault.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginFault {
    pub plugin_id: String,
    pub call: PluginCall,
    pub message: String,
}

/// Shared between a [`PluginNode`] (audio thread) and the host (any thread).
#[derive(Debug, Default)]
pub struct FaultHandle {
    dead: AtomicBool,
    fault: Mutex<Option<PluginFault>>,
}

impl FaultHandle {
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    pub fn fault(&self) -> Option<PluginFault> {
        self.fault.lock().ok().and_then(|f| f.clone())
    }

    /// Records a fault reported by a format adapter (or a caught panic).
    pub fn report(&self, fault: PluginFault) {
        // the audio thread never blocks: if the host is reading, the first fault wins anyway
        if let Ok(mut f) = self.fault.try_lock()
            && f.is_none()
        {
            *f = Some(fault);
        }
        self.dead.store(true, Ordering::Release);
    }
}

/// The crash-catch boundary around a [`PluginInstance`]; see the crate docs.
pub struct PluginNode {
    inner: Option<Box<dyn PluginInstance>>,
    desc: NodeDescriptor,
    id: String,
    fault: Arc<FaultHandle>,
}

impl PluginNode {
    pub fn new(instance: Box<dyn PluginInstance>) -> Self {
        let desc = instance.descriptor().clone();
        let id = instance.info().id.clone();
        let fault = instance.control().map(|c| c.fault()).unwrap_or_default();
        Self {
            inner: Some(instance),
            desc,
            id,
            fault,
        }
    }

    /// The instance's control handle (see [`PluginInstance::control`]); `None` once dead.
    pub fn control(&self) -> Option<Arc<dyn PluginControl>> {
        self.live().and_then(|i| i.control())
    }

    /// For the host: whether and how this node died.
    pub fn fault_handle(&self) -> Arc<FaultHandle> {
        self.fault.clone()
    }

    pub fn info(&self) -> Option<&PluginInfo> {
        self.live().map(|i| i.info())
    }

    fn live(&self) -> Option<&dyn PluginInstance> {
        if self.fault.is_dead() {
            None
        } else {
            self.inner.as_deref()
        }
    }

    /// Runs `f` on the instance unless the node is dead; a panic kills the node.
    fn call<R>(
        &mut self,
        call: PluginCall,
        f: impl FnOnce(&mut dyn PluginInstance) -> R,
    ) -> Option<R> {
        if self.fault.is_dead() {
            return None;
        }
        let inner = self.inner.as_deref_mut()?;
        match catch_unwind(AssertUnwindSafe(|| f(inner))) {
            Ok(r) => Some(r),
            Err(payload) => assert_no_alloc::permit_alloc(|| {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                // never touch the instance again, not even its destructor
                std::mem::forget(self.inner.take());
                // the payload was allocated by the panic; freeing it here is part of the
                // one-off fault path
                drop(payload);
                self.fault.report(PluginFault {
                    plugin_id: self.id.clone(),
                    call,
                    message,
                });
                None
            }),
        }
    }
}

impl Processor for PluginNode {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.call(PluginCall::Activate, |p| p.activate(info));
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        match self.call(PluginCall::Process, |p| p.process(ctx, io)) {
            Some(s) => s,
            None => {
                silence(io);
                ProcessStatus::Sleep
            }
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        self.call(PluginCall::SetParam, |p| p.set_param(id, value));
    }

    fn param(&self, id: ParamId) -> f64 {
        // `&self` cannot record a fault; a dead node reports defaults
        match self.live() {
            Some(p) => catch_unwind(AssertUnwindSafe(|| p.param(id)))
                .unwrap_or_else(|_| self.default_of(id)),
            None => self.default_of(id),
        }
    }

    fn reset(&mut self) {
        self.call(PluginCall::Reset, |p| p.reset());
    }

    fn latency(&self) -> u32 {
        self.live().map_or(0, |p| p.latency())
    }

    fn tail(&self) -> u32 {
        self.live().map_or(0, |p| p.tail())
    }

    fn save_state(&self) -> Vec<u8> {
        self.live()
            .and_then(|p| catch_unwind(AssertUnwindSafe(|| p.save_state())).ok())
            .unwrap_or_default()
    }

    fn load_state(&mut self, state: &[u8], assets: &dyn AssetResolver) -> Result<(), String> {
        self.call(PluginCall::LoadState, |p| p.load_state(state, assets))
            .unwrap_or_else(|| Err(PluginError::Dead.to_string()))
    }
}

impl Drop for PluginNode {
    fn drop(&mut self) {
        // a faulted plugin is never called again, not even its destructor (contract rule 3);
        // this also covers faults reported by the format adapter rather than caught here
        if self.fault.is_dead() {
            std::mem::forget(self.inner.take());
        }
    }
}

impl PluginNode {
    fn default_of(&self, id: ParamId) -> f64 {
        self.desc
            .params
            .get(id.0 as usize)
            .map_or(0.0, |p| p.default)
    }
}

#[cfg(test)]
mod tests;
