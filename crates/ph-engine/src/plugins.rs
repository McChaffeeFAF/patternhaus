//! Plugin hosting in the engine (M2/T3).
//!
//! `InstrumentSpec::Plugin` and `EffectSpec::Plugin` compile like built-ins: the plan
//! instantiates them through a [`PluginEnv`] (the [`PluginHost`] plus the project folder the
//! `plugins/<file>` states live in), wraps each in a [`PluginNode`] (the crash-catch boundary),
//! loads its state, activates it and applies the spec's `params`. Unchanged plugins are reused
//! across edits like any processor; parameter-only edits become parameter sets at the swap.
//!
//! The control side keeps a [`LivePlugin`] per plugin node (its descriptor, its
//! [`PluginControl`] and its [`FaultHandle`]) so front ends can read live parameter values,
//! save states and open editors while the instance runs on the audio thread. A node whose
//! plugin faulted plays silence; [`crate::Engine::plugin_faults`] reports it once, and the
//! plugin is marked *isolate* in the scan cache (DESIGN §6).
//!
//! Test plugins (`test-plugins/`) are deterministic, so live output must equal the offline
//! render bit for bit; third-party plugins are exempt.

pub use ph_core::PluginFormat;
use ph_core::{ChannelId, EffectSpec, InstrumentSpec};
use ph_node::{ActivateInfo, AssetResolver, NodeDescriptor, Processor};
pub use ph_plugin_host::embed::{EMBEDDING, EditorFrame};
pub use ph_plugin_host::manager::{HostConfig, ListedPlugin, PluginHost, find_scan_helper};
pub use ph_plugin_host::scan::ScanProgress;
pub use ph_plugin_host::{
    EditorSize, FaultHandle, PluginControl, PluginEditor, PluginError, PluginFault, PluginInfo,
    PluginInstance, PluginKind, PluginNode, PluginState, PluginStatus, RawWindowHandle,
    ScanFailure, ScanReport,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Where plugin instances come from and where their states live.
#[derive(Clone)]
pub struct PluginEnv {
    pub host: Arc<PluginHost>,
    project_dir: Arc<RwLock<PathBuf>>,
    /// State file contents by reference, used instead of the files (an offline render of the
    /// live session uses the live states).
    overrides: Arc<BTreeMap<String, Vec<u8>>>,
}

impl std::fmt::Debug for PluginEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginEnv")
            .field("project_dir", &self.project_dir())
            .field("overrides", &self.overrides.len())
            .finish()
    }
}

impl PluginEnv {
    pub fn new(host: Arc<PluginHost>, project_dir: &Path) -> Self {
        Self {
            host,
            project_dir: Arc::new(RwLock::new(project_dir.to_path_buf())),
            overrides: Arc::new(BTreeMap::new()),
        }
    }

    /// The project folder moved (file.open): later loads read states from `dir`. Shared by
    /// every clone of this env.
    pub fn set_project_dir(&self, dir: &Path) {
        *self.project_dir.write().unwrap_or_else(|e| e.into_inner()) = dir.to_path_buf();
    }

    pub fn project_dir(&self) -> PathBuf {
        self.project_dir
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// A copy that loads these state files (by reference) instead of reading them from disk.
    pub fn with_states(&self, states: BTreeMap<String, Vec<u8>>) -> Self {
        Self {
            overrides: Arc::new(states),
            ..self.clone()
        }
    }

    /// The state file `reference` (`plugins/<file>`), if there is one.
    pub fn read_state(&self, reference: &str) -> Option<Vec<u8>> {
        if let Some(b) = self.overrides.get(reference) {
            return Some(b.clone());
        }
        let rel = Path::new(reference);
        if rel.is_absolute() || rel.components().any(|c| c.as_os_str() == "..") {
            return None;
        }
        std::fs::read(self.project_dir().join(rel)).ok()
    }
}

/// Where a plugin sits in the project.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PluginSlot {
    /// A channel's instrument.
    Channel(ChannelId),
    /// Effect `slot` of mixer insert `insert`.
    Effect { insert: u32, slot: u32 },
}

impl std::fmt::Display for PluginSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginSlot::Channel(c) => write!(f, "channel {}", c.0),
            PluginSlot::Effect { insert, slot } => write!(f, "insert {insert} slot {slot}"),
        }
    }
}

/// A plugin node of the running plan, as the control side sees it.
#[derive(Clone)]
pub struct LivePlugin {
    pub slot: PluginSlot,
    pub info: PluginInfo,
    pub desc: Arc<NodeDescriptor>,
    /// The spec's `plugins/<file>` state reference.
    pub state_ref: Option<String>,
    /// `None` for plugins without one (the in-repo fake).
    pub control: Option<Arc<dyn PluginControl>>,
    pub fault: Arc<FaultHandle>,
}

impl LivePlugin {
    /// Current parameter values (descriptor order) from the running instance.
    pub fn values(&self) -> Option<Vec<f64>> {
        self.control.as_ref()?.param_values().ok()
    }

    /// Latency in samples as the plugin reports it.
    pub fn latency(&self) -> u32 {
        self.control.as_ref().map_or(0, |c| c.latency())
    }

    /// The state as a file (`plugins/<file>` contents).
    pub fn state_file(&self) -> Result<Vec<u8>, String> {
        let c = self
            .control
            .as_ref()
            .ok_or("this plugin cannot save its state while running")?;
        let data = c.save_state().map_err(|e| e.to_string())?;
        Ok(PluginState {
            format: self.info.format,
            plugin_id: self.info.id.clone(),
            version: self.info.version.clone(),
            data,
        }
        .to_file_bytes())
    }
}

/// A plugin that crashed, reported once by [`crate::Engine::plugin_faults`].
#[derive(Clone, Debug, PartialEq)]
pub struct PluginFaultReport {
    pub slot: PluginSlot,
    pub plugin: PluginInfo,
    pub fault: PluginFault,
}

impl PluginFaultReport {
    pub fn message(&self) -> String {
        format!(
            "plugin {:?} ({}) crashed in {:?}: {}; it plays silence from now on and is marked isolate",
            self.plugin.name, self.slot, self.fault.call, self.fault.message
        )
    }
}

/// The plugin of a spec, if it is one.
pub fn instrument_plugin(spec: &InstrumentSpec) -> Option<(PluginFormat, &str, Option<&str>)> {
    match spec {
        InstrumentSpec::Plugin {
            format,
            plugin_id,
            state,
            ..
        } => Some((*format, plugin_id, state.as_deref())),
        _ => None,
    }
}

pub fn effect_plugin(spec: &EffectSpec) -> Option<(PluginFormat, &str, Option<&str>)> {
    match spec {
        EffectSpec::Plugin {
            format,
            plugin_id,
            state,
            ..
        } => Some((*format, plugin_id, state.as_deref())),
        _ => None,
    }
}

/// Instantiates, loads the state of, activates and parameterises a plugin node. Off the
/// audio thread.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create(
    env: Option<&PluginEnv>,
    slot: PluginSlot,
    format: PluginFormat,
    id: &str,
    state_ref: Option<&str>,
    params: &BTreeMap<String, f64>,
    activate: &ActivateInfo,
    assets: &dyn AssetResolver,
) -> Result<(Box<dyn Processor>, LivePlugin), String> {
    let env = env.ok_or("plugin hosting is not enabled in this engine")?;
    let instance = env
        .host
        .instantiate(format, id)
        .map_err(|e| e.to_string())?;
    let info = instance.info().clone();
    let control = instance.control();
    let mut node = PluginNode::new(instance);
    if let Some(r) = state_ref
        && let Some(bytes) = env.read_state(r)
    {
        let state = PluginState::from_file_bytes(&bytes).map_err(|e| format!("{r}: {e}"))?;
        if state.plugin_id != info.id || state.format != info.format {
            return Err(format!(
                "{r} belongs to {}, not {}",
                state.plugin_id, info.id
            ));
        }
        node.load_state(&state.data, assets)
            .map_err(|e| format!("{r}: {e}"))?;
    }
    node.activate(activate);
    let desc = Arc::new(node.descriptor().clone());
    for (k, v) in params {
        if let Some(p) = desc.param_id(k) {
            node.set_param(p, *v);
        }
    }
    let live = LivePlugin {
        slot,
        info,
        desc,
        state_ref: state_ref.map(str::to_string),
        control,
        fault: node.fault_handle(),
    };
    Ok((Box::new(node), live))
}

/// Why a plugin spec will not play (not scanned, isolated, blocked), for `project_warnings`.
pub(crate) fn unavailable(
    env: Option<&PluginEnv>,
    format: PluginFormat,
    id: &str,
) -> Option<String> {
    let Some(env) = env else {
        return Some("plugin hosting is not enabled in this engine".into());
    };
    match env.host.find(format, id) {
        None => Some(format!(
            "plugin {id} has not been found by a plugin scan (run plugin.scan)"
        )),
        Some(p) => match p.status {
            PluginStatus::Ok => None,
            PluginStatus::Isolate => Some(format!(
                "plugin {:?} is isolated after a crash ({})",
                p.info.name, p.reason
            )),
            PluginStatus::Blocked => Some(format!(
                "plugin {:?} is blocked ({})",
                p.info.name, p.reason
            )),
        },
    }
}
