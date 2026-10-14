//! The CLAP host (M2/T3), on `clack-host` and `clack-extensions` (MIT OR Apache-2.0).
//!
//! - [`ClapFormat`] scans `.clap` files and creates [`ClapInstance`]s.
//! - Everything CLAP calls *main-thread* (creation, activation, parameter info, state, the
//!   editor, destruction) runs on the plugins' main thread ([`crate::main_thread`]); the
//!   instance itself is `Send` and holds only the audio processor, so the engine can move it
//!   to the audio thread like any processor.
//! - Every call into plugin code is wrapped in [`crate::guard::guarded`]. A native fault
//!   marks the shared [`FaultHandle`] dead; from then on nothing calls the plugin again and
//!   its instance is leaked.
//! - Real-time: `process`, `set_param` and `reset` allocate nothing on our side. Buffers,
//!   event lists and parameter maps are allocated in `activate`.
//!
//! Event mapping (ph-node → CLAP), sample-accurate within the block:
//! - `NoteOn`/`NoteOff` → CLAP note events (or MIDI note on/off when the plugin's note port
//!   only speaks MIDI); a note's `fine_pitch` and `pan` follow as tuning and pan expressions.
//! - `AllNotesOff` → a note choke for every note (MIDI: CC 123).
//! - `Param` → `CLAP_EVENT_PARAM_VALUE` (automation), by the plugin's parameter id.
//! - `Cc` and `PitchBend` → MIDI events on the note port (CLAP has no other controller event);
//!   dropped for plugins whose note port does not accept MIDI.
//! - `NoteExpression` → `CLAP_EVENT_NOTE_EXPRESSION` (pan converted from -1..1 to 0..1).

use crate::guard::{NativeFault, guarded};
use crate::main_thread::main_thread;
use crate::*;
use clack_extensions::audio_ports::{
    AudioPortInfoBuffer, AudioPortRescanFlags, HostAudioPorts, HostAudioPortsImpl, PluginAudioPorts,
};
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui, Window,
};
use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{
    HostNotePorts, HostNotePortsImpl, NoteDialects, NotePortInfoBuffer, NotePortRescanFlags,
    PluginNotePorts,
};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer,
    ParamRescanFlags, PluginParams,
};
use clack_extensions::state::{HostState, HostStateImpl, PluginState as ClapState};
use clack_extensions::tail::{HostTail, HostTailImpl, PluginTail};
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::entry::PluginEntry;
use clack_host::events::event_types::{
    MidiEvent, NoteChokeEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent,
    ParamValueEvent, TransportEvent,
};
use clack_host::events::io::{EventBuffer, OutputEvents};
use clack_host::events::{Pckn, UnknownEvent};
use clack_host::host::{
    AudioProcessorHandler, HostError, HostExtensions, HostHandlers, HostInfo, MainThreadHandler,
    SharedHandler,
};
use clack_host::plugin::PluginInstance as ClackInstance;
use clack_host::process::audio_buffers::{
    AudioPortBuffer, AudioPortBufferType, AudioPorts, InputChannel,
};
use clack_host::process::{PluginAudioConfiguration, PluginAudioProcessor};
use clack_host::utils::ClapId;
use clap_sys::events::{
    CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_TRANSPORT, CLAP_TRANSPORT_HAS_BEATS_TIMELINE,
    CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_IS_PLAYING,
    clap_event_header, clap_event_param_value, clap_event_transport,
};
use clap_sys::ext::params::{
    CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY,
    CLAP_PARAM_IS_STEPPED,
};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

/// Events one block can carry to a plugin (excess events are dropped).
const EVENT_CAP: usize = 1024;
/// `set_param` calls queued between blocks.
const PENDING_CAP: usize = 256;
/// Parameter changes a plugin can report per block.
const OUT_EVENT_CAP: usize = 512;
/// Parameters whose choice labels are read (stepped enums with at most this many steps).
const MAX_CHOICES: f64 = 64.0;

// ------------------------------------------------------------------------------- format

/// The CLAP format host. Cheap: all state lives on the plugin main thread.
#[derive(Default)]
pub struct ClapFormat;

impl ClapFormat {
    pub fn new() -> Self {
        crate::guard::install();
        Self
    }
}

fn host_info() -> HostInfo {
    HostInfo::new(
        "Patternhaus",
        "Patternhaus",
        "https://github.com/McChaffeeFAF/patternhaus",
        env!("CARGO_PKG_VERSION"),
    )
    .expect("static host info")
}

/// The editor API of this platform.
fn platform_api() -> Option<GuiApiType<'static>> {
    if cfg!(windows) {
        Some(GuiApiType::WIN32)
    } else if cfg!(target_os = "macos") {
        Some(GuiApiType::COCOA)
    } else if cfg!(unix) {
        Some(GuiApiType::X11)
    } else {
        None
    }
}

fn cstr(s: Option<&CStr>) -> String {
    s.map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn bytes_str(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

fn fault_error(f: NativeFault, what: &str) -> PluginError {
    PluginError::Call(format!("the plugin crashed while {what}: {}", f.describe()))
}

/// Standard CLAP folders for this platform, then `CLAP_PATH`.
pub fn clap_default_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        if let Some(p) = env("COMMONPROGRAMFILES") {
            out.push(p.join("CLAP"));
        }
        if let Some(p) = env("LOCALAPPDATA") {
            out.push(p.join("Programs").join("Common").join("CLAP"));
        }
    } else if cfg!(target_os = "macos") {
        out.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Some(h) = env("HOME") {
            out.push(h.join("Library/Audio/Plug-Ins/CLAP"));
        }
    } else {
        if let Some(h) = env("HOME") {
            out.push(h.join(".clap"));
        }
        out.push(PathBuf::from("/usr/lib/clap"));
    }
    if let Some(p) = std::env::var_os("CLAP_PATH") {
        out.extend(std::env::split_paths(&p));
    }
    out
}

impl PluginFormatHost for ClapFormat {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn default_paths(&self) -> Vec<PathBuf> {
        clap_default_paths()
    }

    fn scan(&self, path: &Path) -> PluginResult<Vec<PluginInfo>> {
        let path = path.to_path_buf();
        main_thread().run(move || scan_on_main(&path))
    }

    fn instantiate(&self, info: &PluginInfo) -> PluginResult<Box<dyn PluginInstance>> {
        Ok(Box::new(ClapInstance::new(info)?))
    }
}

/// Loads (or reuses) the entry of a `.clap` file. Main thread only.
fn entry(path: &Path) -> PluginResult<PluginEntry> {
    STORE.with(|s| {
        if let Some(e) = s.borrow().entries.get(path) {
            return Ok(e.clone());
        }
        let p = path.to_path_buf();
        // SAFETY: loading a plugin library runs its initialisers; that is the point, and the
        // guard catches a crash in them.
        let e = guarded(|| unsafe { PluginEntry::load(&p) })
            .map_err(|f| fault_error(f, "loading"))?
            .map_err(|e| PluginError::Call(format!("{}: {e}", path.display())))?;
        s.borrow_mut().entries.insert(path.to_path_buf(), e.clone());
        Ok(e)
    })
}

fn kind_from_features<'a>(features: impl Iterator<Item = &'a CStr>) -> PluginKind {
    let mut kind = PluginKind::Effect;
    for f in features {
        match f.to_bytes() {
            b"instrument" => return PluginKind::Instrument,
            b"note-effect" => kind = PluginKind::NoteEffect,
            _ => {}
        }
    }
    kind
}

fn scan_on_main(path: &Path) -> PluginResult<Vec<PluginInfo>> {
    let is_clap = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("clap"));
    if !is_clap {
        return Err(PluginError::NotAPlugin(path.to_path_buf()));
    }
    let e = entry(path)?;
    let factory = e
        .get_plugin_factory()
        .ok_or_else(|| PluginError::Call("the file has no plugin factory".into()))?;
    let mut out = Vec::new();
    let descriptors: Vec<_> = guarded(|| {
        factory
            .plugin_descriptors()
            .map(|d| {
                (
                    cstr(d.id()),
                    cstr(d.name()),
                    cstr(d.vendor()),
                    cstr(d.version()),
                    kind_from_features(d.features()),
                )
            })
            .collect()
    })
    .map_err(|f| fault_error(f, "listing its plugins"))?;
    for (id, name, vendor, version, kind) in descriptors {
        if id.is_empty() {
            continue;
        }
        let mut info = PluginInfo {
            format: PluginFormat::Clap,
            id,
            name,
            vendor,
            version,
            kind,
            path: path.to_path_buf(),
            has_editor: false,
        };
        // creating the plugin once tells whether it loads at all and whether it has an
        // editor; the scan runs in a helper process, so a crash here is contained
        let probe = Built::create(&info)?;
        info.has_editor = probe.has_gui;
        destroy_slot(probe.key);
        out.push(info);
    }
    Ok(out)
}

// ------------------------------------------------------------------------------- host side

struct PhHost;

#[derive(Default)]
struct Flags {
    callback: AtomicBool,
    restart: AtomicBool,
    latency_changed: AtomicBool,
    dirty: AtomicBool,
    rescan: AtomicBool,
    gui_closed: AtomicBool,
    /// Size the editor asked for (`GuiSize::pack_to_u64`), 0 = none.
    resize: AtomicU64,
}

struct Shared {
    flags: Arc<Flags>,
}

impl<'a> SharedHandler<'a> for Shared {
    fn request_restart(&self) {
        self.flags.restart.store(true, Relaxed);
    }
    fn request_process(&self) {}
    fn request_callback(&self) {
        self.flags.callback.store(true, Relaxed);
    }
}

struct MainH<'a> {
    shared: &'a Shared,
}

impl<'a> MainThreadHandler<'a> for MainH<'a> {}

struct AudioH;
impl<'a> AudioProcessorHandler<'a> for AudioH {}

impl HostHandlers for PhHost {
    type Shared<'a> = Shared;
    type MainThread<'a> = MainH<'a>;
    type AudioProcessor<'a> = AudioH;

    fn declare_extensions(builder: &mut HostExtensions<Self>, _: &Self::Shared<'_>) {
        builder
            .register::<HostLog>()
            .register::<HostThreadCheck>()
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostLatency>()
            .register::<HostTail>()
            .register::<HostGui>()
            .register::<HostAudioPorts>()
            .register::<HostNotePorts>();
    }
}

impl HostLogImpl for Shared {
    fn log(&self, severity: LogSeverity, message: &str) {
        // plugins may log from the audio thread: only main-thread errors are printed
        if main_thread().is_current()
            && matches!(
                severity,
                LogSeverity::Error | LogSeverity::Fatal | LogSeverity::PluginMisbehaving
            )
        {
            eprintln!("[plugin] {message}");
        }
    }
}

impl HostThreadCheckImpl for Shared {
    fn is_main_thread(&self) -> bool {
        main_thread().is_current()
    }
    fn is_audio_thread(&self) -> bool {
        !main_thread().is_current()
    }
}

impl HostParamsImplShared for Shared {
    fn request_flush(&self) {}
}

impl HostParamsImplMainThread for MainH<'_> {
    fn rescan(&self, _: ParamRescanFlags) {
        self.shared.flags.rescan.store(true, Relaxed);
    }
    fn clear(&self, _: ClapId, _: ParamClearFlags) {}
}

impl HostStateImpl for MainH<'_> {
    fn mark_dirty(&self) {
        self.shared.flags.dirty.store(true, Relaxed);
    }
}

impl HostLatencyImpl for MainH<'_> {
    fn changed(&self) {
        self.shared.flags.latency_changed.store(true, Relaxed);
    }
}

impl HostTailImpl for AudioH {
    fn changed(&mut self) {}
}

impl HostGuiImpl for Shared {
    fn resize_hints_changed(&self) {}
    fn request_resize(&self, size: GuiSize) -> Result<(), HostError> {
        self.flags.resize.store(size.pack_to_u64(), Relaxed);
        Ok(())
    }
    fn request_show(&self) -> Result<(), HostError> {
        Ok(())
    }
    fn request_hide(&self) -> Result<(), HostError> {
        Ok(())
    }
    fn closed(&self, _: bool) {
        self.flags.gui_closed.store(true, Relaxed);
    }
}

impl HostAudioPortsImpl for MainH<'_> {
    fn is_rescan_flag_supported(&self, _: AudioPortRescanFlags) -> bool {
        false
    }
    fn rescan(&self, _: AudioPortRescanFlags) {
        self.shared.flags.restart.store(true, Relaxed);
    }
}

impl HostNotePortsImpl for MainH<'_> {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }
    fn rescan(&self, _: NotePortRescanFlags) {}
}

// ------------------------------------------------------------------------------- main-thread store

#[derive(Copy, Clone, Default)]
struct Exts {
    params: Option<PluginParams>,
    state: Option<ClapState>,
    latency: Option<PluginLatency>,
    tail: Option<PluginTail>,
    gui: Option<PluginGui>,
    audio_ports: Option<PluginAudioPorts>,
    note_ports: Option<PluginNotePorts>,
}

struct Slot {
    instance: ClackInstance<PhHost>,
    exts: Exts,
    flags: Arc<Flags>,
    fault: Arc<FaultHandle>,
    /// Editor created (CLAP gui `create` called).
    gui_created: bool,
}

#[derive(Default)]
struct Store {
    entries: HashMap<PathBuf, PluginEntry>,
    slots: HashMap<u64, Slot>,
    next: u64,
}

thread_local! {
    static STORE: std::cell::RefCell<Store> = std::cell::RefCell::new(Store::default());
}

/// Runs `f` on the slot `key` (main thread). `None` when the slot is gone.
fn with_slot<R>(key: u64, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    // the slot is taken out while `f` runs, so a plugin calling back into the host (which
    // may touch the store) cannot hit a double borrow
    let mut slot = STORE.with(|s| s.borrow_mut().slots.remove(&key))?;
    let r = f(&mut slot);
    STORE.with(|s| s.borrow_mut().slots.insert(key, slot));
    Some(r)
}

/// Destroys the instance in `key` unless it faulted (then it is leaked). Main thread.
fn destroy_slot(key: u64) {
    let slot = STORE.with(|s| s.borrow_mut().slots.remove(&key));
    if let Some(mut slot) = slot {
        if slot.fault.is_dead() {
            std::mem::forget(slot);
            return;
        }
        let fault = slot.fault.clone();
        let id = slot_id(&slot);
        let r = guarded(move || {
            if slot.gui_created
                && let Some(gui) = slot.exts.gui
            {
                gui.destroy(&slot.instance.plugin_handle());
            }
            let _ = slot.instance.try_deactivate();
            drop(slot);
        });
        if let Err(f) = r {
            fault.report(PluginFault {
                plugin_id: id,
                call: PluginCall::Activate,
                message: format!("crashed while being destroyed: {}", f.describe()),
            });
        }
    }
}

fn slot_id(slot: &Slot) -> String {
    slot.instance
        .plugin_shared_handle()
        .descriptor()
        .and_then(|d| d.id())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Main-thread idle hook: runs plugin `on_main_thread` callbacks they asked for.
fn idle() {
    let keys: Vec<u64> = STORE.with(|s| {
        s.borrow()
            .slots
            .iter()
            .filter(|(_, slot)| slot.flags.callback.load(Relaxed) && !slot.fault.is_dead())
            .map(|(k, _)| *k)
            .collect()
    });
    for key in keys {
        with_slot(key, |slot| {
            slot.flags.callback.store(false, Relaxed);
            let fault = slot.fault.clone();
            let id = slot_id(slot);
            if let Err(f) = guarded(|| slot.instance.call_on_main_thread_callback()) {
                fault.report(PluginFault {
                    plugin_id: id,
                    call: PluginCall::Param,
                    message: format!("crashed in its main-thread callback: {}", f.describe()),
                });
            }
        });
    }
}

// ------------------------------------------------------------------------------- building

/// Port layout of an instance.
#[derive(Clone, Debug, Default, PartialEq)]
struct Layout {
    /// Channel count per input port (main first).
    inputs: Vec<u32>,
    outputs: Vec<u32>,
    /// Index of the main input/output port.
    main_in: Option<usize>,
    main_out: Option<usize>,
    /// Index of the sidechain input (the first non-main input).
    side_in: Option<usize>,
    /// Note input: (port index, accepts CLAP notes, accepts MIDI).
    notes: Option<(u16, bool, bool)>,
}

/// What creating an instance on the main thread produced.
struct Built {
    key: u64,
    desc: NodeDescriptor,
    param_ids: Vec<ClapId>,
    values: Vec<f64>,
    layout: Layout,
    has_gui: bool,
    latency: u32,
    tail: Option<PluginTail>,
    fault: Arc<FaultHandle>,
}

impl Built {
    /// Creates an instance of `info` on the main thread and reads its ports and parameters.
    fn create(info: &PluginInfo) -> PluginResult<Built> {
        let e = entry(&info.path)?;
        let id =
            CString::new(info.id.clone()).map_err(|_| PluginError::NotFound(info.id.clone()))?;
        let flags = Arc::new(Flags::default());
        let f2 = flags.clone();
        let host = host_info();
        let instance = guarded(move || {
            ClackInstance::<PhHost>::new(
                move |_| Shared { flags: f2 },
                |shared| MainH { shared },
                &e,
                &id,
                &host,
            )
        })
        .map_err(|f| fault_error(f, "being created"))?
        .map_err(|e| PluginError::Call(format!("cannot create {}: {e}", info.id)))?;
        let shared = instance.plugin_shared_handle();
        let exts = Exts {
            params: shared.get_extension(),
            state: shared.get_extension(),
            latency: shared.get_extension(),
            tail: shared.get_extension(),
            gui: shared.get_extension(),
            audio_ports: shared.get_extension(),
            note_ports: shared.get_extension(),
        };
        let fault = Arc::new(FaultHandle::default());
        let key = STORE.with(|s| {
            let mut s = s.borrow_mut();
            s.next += 1;
            let key = s.next;
            s.slots.insert(
                key,
                Slot {
                    instance,
                    exts,
                    flags,
                    fault: fault.clone(),
                    gui_created: false,
                },
            );
            key
        });
        main_thread().add_idle(idle);
        let read = with_slot(key, |slot| guarded(|| read_slot(slot, info)))
            .ok_or_else(|| PluginError::Call("the instance vanished".into()))?;
        match read {
            Ok((desc, param_ids, values, layout, has_gui, latency)) => Ok(Built {
                key,
                desc,
                param_ids,
                values,
                layout,
                has_gui,
                latency,
                tail: exts.tail,
                fault,
            }),
            Err(f) => {
                fault.report(PluginFault {
                    plugin_id: info.id.clone(),
                    call: PluginCall::Activate,
                    message: f.describe(),
                });
                destroy_slot(key);
                Err(fault_error(f, "describing itself"))
            }
        }
    }
}

type Read = (NodeDescriptor, Vec<ClapId>, Vec<f64>, Layout, bool, u32);

fn read_slot(slot: &mut Slot, info: &PluginInfo) -> Read {
    let exts = slot.exts;
    let handle = slot.instance.plugin_handle();
    // ---- ports
    let mut layout = Layout::default();
    if let Some(ap) = exts.audio_ports {
        for is_input in [true, false] {
            let n = ap.count(&handle, is_input);
            let mut buf = AudioPortInfoBuffer::new();
            for i in 0..n {
                let (channels, main) = match ap.get(&handle, i, is_input, &mut buf) {
                    Some(p) => (
                        p.channel_count,
                        p.flags.bits() & clap_sys::ext::audio_ports::CLAP_AUDIO_PORT_IS_MAIN != 0,
                    ),
                    None => (0, false),
                };
                let (ports, main_idx) = if is_input {
                    (&mut layout.inputs, &mut layout.main_in)
                } else {
                    (&mut layout.outputs, &mut layout.main_out)
                };
                if main && main_idx.is_none() {
                    *main_idx = Some(ports.len());
                }
                ports.push(channels);
            }
        }
    }
    if layout.main_out.is_none() && !layout.outputs.is_empty() {
        layout.main_out = Some(0);
    }
    if layout.main_in.is_none() && !layout.inputs.is_empty() {
        layout.main_in = Some(0);
    }
    layout.side_in = (0..layout.inputs.len()).find(|i| Some(*i) != layout.main_in);
    if let Some(np) = exts.note_ports {
        let mut buf = NotePortInfoBuffer::new();
        if np.count(&handle, true) > 0
            && let Some(p) = np.get(&handle, 0, true, &mut buf)
        {
            let d = p.supported_dialects;
            layout.notes = Some((
                0,
                d.contains(NoteDialects::CLAP),
                d.contains(NoteDialects::MIDI) || d.contains(NoteDialects::MIDI_MPE),
            ));
        }
    }
    // ---- parameters
    let mut params = Vec::new();
    let mut ids = Vec::new();
    let mut values = Vec::new();
    if let Some(pp) = exts.params {
        let n = pp.count(&handle);
        let mut buf = ParamInfoBuffer::new();
        for i in 0..n {
            let Some(p) = pp.get_info(&handle, i, &mut buf) else {
                continue;
            };
            let flags = p.flags.bits();
            if flags & CLAP_PARAM_IS_HIDDEN != 0 {
                continue;
            }
            let (id, min, max, default) = (p.id, p.min_value, p.max_value, p.default_value);
            let name = bytes_str(p.name);
            let module = bytes_str(p.module);
            let stepped = flags & CLAP_PARAM_IS_STEPPED != 0;
            let mut choices = Vec::new();
            if stepped && flags & CLAP_PARAM_IS_ENUM != 0 && max - min < MAX_CHOICES {
                let mut text = [0u8; 256];
                let mut v = min.round();
                while v <= max {
                    match pp.value_to_text(&handle, id, v, &mut text) {
                        Ok(t) => choices.push(bytes_str(t)),
                        Err(_) => {
                            choices.clear();
                            break;
                        }
                    }
                    v += 1.0;
                }
            }
            let mut info = ParamInfo::new(
                &id.get().to_string(),
                &if module.is_empty() {
                    name
                } else {
                    format!("{module}/{name}")
                },
                min,
                max.max(min),
                default.clamp(min, max.max(min)),
                if choices.is_empty() {
                    if stepped {
                        ParamUnit::Choice
                    } else {
                        ParamUnit::None
                    }
                } else {
                    ParamUnit::Choice
                },
            );
            info.stepped = stepped;
            info.choices = choices;
            info.automatable =
                flags & CLAP_PARAM_IS_AUTOMATABLE != 0 && flags & CLAP_PARAM_IS_READONLY == 0;
            values.push(pp.get_value(&handle, id).unwrap_or(info.default));
            params.push(info);
            ids.push(id);
        }
    }
    let has_gui = exts.gui.is_some_and(|g| {
        let api = platform_api();
        api.is_some_and(|api| {
            g.is_api_supported(
                &handle,
                GuiConfiguration {
                    api_type: api,
                    is_floating: false,
                },
            ) || g.is_api_supported(
                &handle,
                GuiConfiguration {
                    api_type: api,
                    is_floating: true,
                },
            )
        })
    });
    let latency = exts.latency.map_or(0, |l| l.get(&handle));
    let port = |name: &str, channels: u32, sidechain: bool| PortInfo {
        name: name.into(),
        channels: channels.max(1),
        sidechain,
    };
    let mut audio_inputs = Vec::new();
    if let Some(m) = layout.main_in {
        audio_inputs.push(port("in", layout.inputs[m], false));
        if let Some(s) = layout.side_in {
            audio_inputs.push(port("sidechain", layout.inputs[s], true));
        }
    }
    let kind = match info.kind {
        PluginKind::Instrument => NodeKind::Instrument,
        _ => NodeKind::Effect,
    };
    let desc = NodeDescriptor {
        id: format!("clap:{}", info.id),
        name: info.name.clone(),
        kind,
        audio_inputs,
        audio_outputs: vec![port("out", 2, false)],
        note_input: layout.notes.is_some(),
        params,
    };
    (desc, ids, values, layout, has_gui, latency)
}

// ------------------------------------------------------------------------------- control

/// The [`PluginControl`] of a CLAP instance.
pub struct ClapControl {
    info: PluginInfo,
    key: u64,
    fault: Arc<FaultHandle>,
    param_ids: Vec<ClapId>,
    latency: AtomicU32,
    has_gui: bool,
}

impl ClapControl {
    fn fail(&self, call: PluginCall, f: NativeFault, what: &str) -> PluginError {
        self.fault.report(PluginFault {
            plugin_id: self.info.id.clone(),
            call,
            message: format!("crashed while {what}: {}", f.describe()),
        });
        fault_error(f, what)
    }

    /// Runs `f` on this instance's slot on the main thread, guarded.
    fn on_main<R: Send + 'static>(
        self: &Arc<Self>,
        call: PluginCall,
        what: &'static str,
        f: impl FnOnce(&mut Slot) -> R + Send + 'static,
    ) -> PluginResult<R> {
        if self.fault.is_dead() {
            return Err(PluginError::Dead);
        }
        let key = self.key;
        let r = main_thread()
            .run(move || with_slot(key, |slot| guarded(|| f(slot))))
            .ok_or(PluginError::Dead)?;
        r.map_err(|f| self.fail(call, f, what))
    }
}

/// Saves a CLAP state into bytes (main thread, inside a guard).
fn save_slot(slot: &mut Slot) -> PluginResult<Vec<u8>> {
    let Some(st) = slot.exts.state else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    st.save(&slot.instance.plugin_handle(), &mut out)
        .map_err(|e| PluginError::Call(format!("the plugin could not save its state: {e}")))?;
    Ok(out)
}

fn load_slot(slot: &mut Slot, data: &[u8]) -> PluginResult<()> {
    let Some(st) = slot.exts.state else {
        return if data.is_empty() {
            Ok(())
        } else {
            Err(PluginError::Call("the plugin has no state to load".into()))
        };
    };
    let mut r = data;
    st.load(&slot.instance.plugin_handle(), &mut r)
        .map_err(|e| PluginError::Call(format!("the plugin refused the state: {e}")))
}

/// `Arc` wrapper so the trait object can hand out `self: &Arc<Self>` helpers.
struct ControlHandle(Arc<ClapControl>);

impl PluginControl for ControlHandle {
    fn info(&self) -> &PluginInfo {
        &self.0.info
    }

    fn fault(&self) -> Arc<FaultHandle> {
        self.0.fault.clone()
    }

    fn param_values(&self) -> PluginResult<Vec<f64>> {
        let ids = self.0.param_ids.clone();
        self.0
            .on_main(PluginCall::Param, "reading parameters", move |slot| {
                let Some(pp) = slot.exts.params else {
                    return Vec::new();
                };
                let h = slot.instance.plugin_handle();
                ids.iter()
                    .map(|id| pp.get_value(&h, *id).unwrap_or(0.0))
                    .collect()
            })
    }

    fn param_text(&self, index: usize, value: f64) -> Option<String> {
        let id = *self.0.param_ids.get(index)?;
        self.0
            .on_main(PluginCall::Param, "formatting a value", move |slot| {
                let pp = slot.exts.params?;
                let h = slot.instance.plugin_handle();
                let mut buf = [0u8; 256];
                pp.value_to_text(&h, id, value, &mut buf)
                    .ok()
                    .map(|t| bytes_str(t))
            })
            .ok()
            .flatten()
    }

    fn save_state(&self) -> PluginResult<Vec<u8>> {
        self.0
            .on_main(PluginCall::SaveState, "saving its state", save_slot)?
    }

    fn latency(&self) -> u32 {
        self.0.latency.load(Relaxed)
    }

    fn editor(&self) -> Option<Box<dyn PluginEditor>> {
        if !self.0.has_gui || self.0.fault.is_dead() {
            return None;
        }
        Some(Box::new(ClapEditor {
            control: self.0.clone(),
            size: None,
            embedded: false,
        }))
    }
}

// ------------------------------------------------------------------------------- editor

/// A CLAP plugin's own window. All calls run on the plugin main thread.
pub struct ClapEditor {
    control: Arc<ClapControl>,
    size: Option<EditorSize>,
    embedded: bool,
}

/// The CLAP window for a parent handle (Win32, Cocoa or X11).
///
/// # Safety
/// The window must stay alive while the editor uses it.
unsafe fn clap_window(raw: RawWindowHandle) -> Option<Window<'static, 'static>> {
    match raw {
        // SAFETY: forwarded from the caller
        RawWindowHandle::Win32(h) => {
            Some(unsafe { Window::from_win32_hwnd(h.hwnd.get() as *mut std::ffi::c_void) })
        }
        RawWindowHandle::AppKit(h) => {
            Some(unsafe { Window::from_cocoa_nsview(h.ns_view.as_ptr()) })
        }
        RawWindowHandle::Xlib(h) => Some(Window::from_x11_handle(h.window as _)),
        RawWindowHandle::Xcb(h) => Some(Window::from_x11_handle(h.window.get() as _)),
        _ => None,
    }
}

/// A window handle the main thread may use (the UI keeps the parent alive while open).
struct SendHandle(RawWindowHandle);
// SAFETY: a window handle is an id; the main thread only passes it to the plugin.
unsafe impl Send for SendHandle {}

impl PluginEditor for ClapEditor {
    fn can_embed(&self) -> bool {
        let key = self.control.key;
        main_thread()
            .run(move || {
                with_slot(key, |slot| {
                    let api = platform_api()?;
                    let gui = slot.exts.gui?;
                    let h = slot.instance.plugin_handle();
                    guarded(|| {
                        gui.is_api_supported(
                            &h,
                            GuiConfiguration {
                                api_type: api,
                                is_floating: false,
                            },
                        )
                    })
                    .ok()
                })
                .flatten()
            })
            .unwrap_or(false)
    }

    fn open(&mut self, parent: Option<RawWindowHandle>) -> PluginResult<EditorSize> {
        if self.size.is_some() {
            self.close();
        }
        let parent = parent.map(SendHandle);
        let embedded = parent.is_some();
        let size = self.control.on_main(
            PluginCall::Param,
            "opening its editor",
            move |slot| -> PluginResult<EditorSize> {
                let gui = slot
                    .exts
                    .gui
                    .ok_or_else(|| PluginError::Call("the plugin has no editor".into()))?;
                let api = platform_api()
                    .ok_or_else(|| PluginError::Call("no editor API on this platform".into()))?;
                let h = slot.instance.plugin_handle();
                let config = GuiConfiguration {
                    api_type: api,
                    is_floating: parent.is_none(),
                };
                gui.create(&h, config)
                    .map_err(|e| PluginError::Call(format!("cannot create the editor: {e}")))?;
                slot.gui_created = true;
                if let Some(SendHandle(raw)) = parent {
                    // SAFETY: the UI keeps the parent window alive until `close`.
                    let window = unsafe { clap_window(raw) }.ok_or_else(|| {
                        PluginError::Call("unsupported parent window type".into())
                    })?;
                    // SAFETY: as above
                    unsafe { gui.set_parent(&h, window) }.map_err(|e| {
                        PluginError::Call(format!("the editor refused its parent: {e}"))
                    })?;
                }
                gui.show(&h)
                    .map_err(|e| PluginError::Call(format!("cannot show the editor: {e}")))?;
                let s = gui.get_size(&h).unwrap_or(GuiSize {
                    width: 640,
                    height: 480,
                });
                Ok(EditorSize {
                    width: s.width,
                    height: s.height,
                })
            },
        )??;
        self.size = Some(size);
        self.embedded = embedded;
        Ok(size)
    }

    fn close(&mut self) {
        if self.size.take().is_none() {
            return;
        }
        let _ = self
            .control
            .on_main(PluginCall::Param, "closing its editor", |slot| {
                if let Some(gui) = slot.exts.gui
                    && slot.gui_created
                {
                    let h = slot.instance.plugin_handle();
                    let _ = gui.hide(&h);
                    gui.destroy(&h);
                    slot.gui_created = false;
                }
            });
    }

    fn is_open(&self) -> bool {
        self.size.is_some()
    }

    fn can_resize(&self) -> bool {
        self.control
            .on_main(PluginCall::Param, "reading its editor size", |slot| {
                slot.exts
                    .gui
                    .is_some_and(|g| g.can_resize(&slot.instance.plugin_handle()))
            })
            .unwrap_or(false)
    }

    fn set_size(&mut self, size: EditorSize) -> PluginResult<EditorSize> {
        if self.size.is_none() {
            return Err(PluginError::Call("the editor is closed".into()));
        }
        let s = self.control.on_main(
            PluginCall::Param,
            "resizing its editor",
            move |slot| -> PluginResult<EditorSize> {
                let gui = slot
                    .exts
                    .gui
                    .ok_or_else(|| PluginError::Call("the plugin has no editor".into()))?;
                let h = slot.instance.plugin_handle();
                let want = GuiSize {
                    width: size.width,
                    height: size.height,
                };
                let adjusted = gui.adjust_size(&h, want).unwrap_or(want);
                gui.set_size(&h, adjusted)
                    .map_err(|e| PluginError::Call(format!("cannot resize the editor: {e}")))?;
                Ok(EditorSize {
                    width: adjusted.width,
                    height: adjusted.height,
                })
            },
        )??;
        self.size = Some(s);
        Ok(s)
    }

    fn size(&self) -> Option<EditorSize> {
        self.size
    }
}

impl Drop for ClapEditor {
    fn drop(&mut self) {
        self.close();
    }
}

// ------------------------------------------------------------------------------- instance

/// A bounded output-event sink: parameter values the plugin reports, nothing else.
struct ParamSink {
    changes: Vec<(u32, f64)>,
}

impl clack_host::events::io::OutputEventBuffer for ParamSink {
    fn try_push(
        &mut self,
        event: &UnknownEvent,
    ) -> Result<(), clack_host::events::io::TryPushError> {
        // SAFETY: an event always starts with its header
        let h: &clap_event_header = unsafe { &*event.as_raw() };
        if h.space_id == 0
            && h.type_ == CLAP_EVENT_PARAM_VALUE
            && self.changes.len() < OUT_EVENT_CAP
        {
            // SAFETY: the header says this is a param-value event of at least that size
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_param_value) };
            self.changes.push((e.param_id, e.value));
        }
        Ok(())
    }
}

/// A live CLAP plugin. See the module docs.
pub struct ClapInstance {
    info: PluginInfo,
    desc: NodeDescriptor,
    control: Arc<ClapControl>,
    layout: Layout,
    param_ids: Vec<ClapId>,
    /// Current values, by descriptor index (kept in step with what we send and what the
    /// plugin reports).
    values: Vec<f64>,
    proc: Option<PluginAudioProcessor<PhHost>>,
    tail_ext: Option<PluginTail>,
    tail: u32,
    // ---- allocated in activate
    ins: Vec<Vec<Vec<f32>>>,
    outs: Vec<Vec<Vec<f32>>>,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    events: EventBuffer,
    sink: ParamSink,
    pending: Vec<(u32, f64)>,
    steady: u64,
    max_frames: usize,
}

impl ClapInstance {
    pub fn new(info: &PluginInfo) -> PluginResult<Self> {
        let info2 = info.clone();
        let built = main_thread().run(move || Built::create(&info2))?;
        let control = Arc::new(ClapControl {
            info: info.clone(),
            key: built.key,
            fault: built.fault,
            param_ids: built.param_ids.clone(),
            latency: AtomicU32::new(built.latency),
            has_gui: built.has_gui,
        });
        let mut info = info.clone();
        info.has_editor = built.has_gui;
        Ok(Self {
            info,
            desc: built.desc,
            control,
            layout: built.layout,
            param_ids: built.param_ids,
            values: built.values,
            proc: None,
            tail_ext: built.tail,
            tail: 0,
            ins: Vec::new(),
            outs: Vec::new(),
            in_ports: AudioPorts::with_capacity(0, 0),
            out_ports: AudioPorts::with_capacity(0, 0),
            events: EventBuffer::new(),
            sink: ParamSink {
                changes: Vec::new(),
            },
            pending: Vec::new(),
            steady: 0,
            max_frames: 0,
        })
    }

    fn dead(&self) -> bool {
        self.control.fault.is_dead()
    }

    fn report(&self, call: PluginCall, f: NativeFault, what: &str) {
        // the one-off fault path may allocate, even on the audio thread (contract rule 5)
        assert_no_alloc::permit_alloc(|| {
            self.control.fail(call, f, what);
        });
    }

    /// Stops and deactivates the running processor (off the audio thread).
    fn deactivate(&mut self) {
        let Some(mut p) = self.proc.take() else {
            return;
        };
        if self.dead() {
            std::mem::forget(p);
            return;
        }
        if let Err(f) = guarded(|| {
            if p.is_started() {
                let _ = p.stop_processing();
            }
        }) {
            std::mem::forget(p);
            self.report(PluginCall::Activate, f, "stopping");
            return;
        }
        let stopped = p.into_stopped();
        let key = self.control.key;
        let r = main_thread().run(move || {
            with_slot(key, move |slot| {
                guarded(move || slot.instance.deactivate(stopped))
            })
        });
        if let Some(Err(f)) = r {
            self.report(PluginCall::Activate, f, "being deactivated");
        }
    }

    fn param_index(&self, id: u32) -> Option<usize> {
        self.param_ids.iter().position(|p| p.get() == id)
    }

    fn transport(ctx: &ProcessContext) -> TransportEvent {
        const BEAT: f64 = (1u64 << 31) as f64;
        let beats = ctx.song_pos_ticks / ph_core::PPQ as f64;
        let seconds = ctx.song_pos_samples as f64 / ctx.sample_rate.max(1.0);
        let mut flags = CLAP_TRANSPORT_HAS_TEMPO
            | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
            | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE;
        if ctx.playing {
            flags |= CLAP_TRANSPORT_IS_PLAYING;
        }
        let raw = clap_event_transport {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_transport>() as u32,
                time: 0,
                space_id: 0,
                type_: CLAP_EVENT_TRANSPORT,
                flags: 0,
            },
            flags,
            song_pos_beats: (beats * BEAT).round() as i64,
            song_pos_seconds: (seconds * BEAT).round() as i64,
            tempo: ctx.bpm,
            tempo_inc: 0.0,
            loop_start_beats: 0,
            loop_end_beats: 0,
            loop_start_seconds: 0,
            loop_end_seconds: 0,
            bar_start: 0,
            bar_number: 0,
            tsig_num: 4,
            tsig_denom: 4,
        };
        TransportEvent::from_raw(&raw)
    }

    /// Converts the pending parameter changes and the block's events (real-time).
    fn fill_events(&mut self, events: &[Event]) {
        self.events.clear();
        let cap = EVENT_CAP;
        for (id, v) in self.pending.drain(..) {
            if let Some(cid) = ClapId::from_raw(id) {
                self.events
                    .push(&ParamValueEvent::new(0, cid, Pckn::match_all(), v));
            }
        }
        let notes = self.layout.notes;
        let (port, clap_notes, midi) = match notes {
            Some((p, c, m)) => (p as i16, c, m),
            None => (0, false, false),
        };
        for e in events {
            if self.events.len() as usize >= cap {
                break;
            }
            let t = e.frame;
            let midi_ev = |d: [u8; 3]| MidiEvent::new(t, port as u16, d);
            match e.kind {
                EventKind::NoteOn {
                    note_id,
                    key,
                    velocity,
                    pan,
                    fine_pitch,
                } => {
                    if clap_notes {
                        let pckn = Pckn::from_raw(port, 0, key as i16, note_id as i32);
                        self.events
                            .push(&NoteOnEvent::new(t, pckn, velocity as f64));
                        if fine_pitch != 0.0 {
                            self.events.push(&NoteExpressionEvent::new(
                                t,
                                pckn,
                                NoteExpressionType::Tuning,
                                fine_pitch as f64 / 100.0,
                            ));
                        }
                        if pan != 0.0 {
                            self.events.push(&NoteExpressionEvent::new(
                                t,
                                pckn,
                                NoteExpressionType::Pan,
                                (pan as f64 + 1.0) / 2.0,
                            ));
                        }
                    } else if midi {
                        let v = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                        self.events.push(&midi_ev([0x90, key & 0x7f, v]));
                    }
                }
                EventKind::NoteOff {
                    note_id,
                    key,
                    velocity,
                } => {
                    if clap_notes {
                        let pckn = Pckn::from_raw(port, 0, key as i16, note_id as i32);
                        self.events
                            .push(&NoteOffEvent::new(t, pckn, velocity as f64));
                    } else if midi {
                        let v = (velocity * 127.0).round().clamp(0.0, 127.0) as u8;
                        self.events.push(&midi_ev([0x80, key & 0x7f, v]));
                    }
                }
                EventKind::AllNotesOff => {
                    if clap_notes {
                        self.events
                            .push(&NoteChokeEvent::new(t, Pckn::from_raw(port, -1, -1, -1)));
                    } else if midi {
                        self.events.push(&midi_ev([0xb0, 123, 0]));
                    }
                }
                EventKind::Param { id, value } => {
                    if let (Some(cid), Some(info)) = (
                        self.param_ids.get(id.0 as usize).copied(),
                        self.desc.params.get(id.0 as usize),
                    ) {
                        let v = info.clamp(value);
                        self.values[id.0 as usize] = v;
                        self.events
                            .push(&ParamValueEvent::new(t, cid, Pckn::match_all(), v));
                    }
                }
                EventKind::Cc { controller, value } => {
                    if midi {
                        let v = (value * 127.0).round().clamp(0.0, 127.0) as u8;
                        self.events.push(&midi_ev([0xb0, controller & 0x7f, v]));
                    }
                }
                EventKind::PitchBend { value } => {
                    if midi {
                        let v = ((value.clamp(-1.0, 1.0) as f64 + 1.0) * 8192.0)
                            .round()
                            .clamp(0.0, 16383.0) as u16;
                        self.events
                            .push(&midi_ev([0xe0, (v & 0x7f) as u8, (v >> 7) as u8]));
                    }
                }
                EventKind::NoteExpression {
                    note_id,
                    kind,
                    value,
                } => {
                    if clap_notes {
                        let (ty, v) = match kind {
                            NoteExpressionKind::Volume => {
                                (NoteExpressionType::Volume, value as f64)
                            }
                            NoteExpressionKind::Pan => {
                                (NoteExpressionType::Pan, (value as f64 + 1.0) / 2.0)
                            }
                            NoteExpressionKind::Tuning => {
                                (NoteExpressionType::Tuning, value as f64)
                            }
                            NoteExpressionKind::Vibrato => {
                                (NoteExpressionType::Vibrato, value as f64)
                            }
                            NoteExpressionKind::Brightness => {
                                (NoteExpressionType::Brightness, value as f64)
                            }
                            NoteExpressionKind::Pressure => {
                                (NoteExpressionType::Pressure, value as f64)
                            }
                        };
                        let pckn = Pckn::from_raw(port, -1, -1, note_id as i32);
                        self.events.push(&NoteExpressionEvent::new(t, pckn, ty, v));
                    }
                }
            }
        }
    }
}

impl Processor for ClapInstance {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.desc
    }

    fn activate(&mut self, a: &ActivateInfo) {
        self.deactivate();
        if self.dead() {
            return;
        }
        let frames = a.max_frames.max(1);
        let config = PluginAudioConfiguration {
            sample_rate: a.sample_rate,
            min_frames_count: 1,
            max_frames_count: frames as u32,
        };
        let key = self.control.key;
        let r = main_thread().run(move || {
            with_slot(key, move |slot| {
                guarded(move || {
                    let p = slot.instance.activate(|_, _| AudioH, config);
                    let latency = slot
                        .exts
                        .latency
                        .map_or(0, |l| l.get(&slot.instance.plugin_handle()));
                    (p, latency)
                })
            })
        });
        match r {
            Some(Ok((Ok(p), latency))) => {
                self.proc = Some(p.into());
                self.control.latency.store(latency, Relaxed);
            }
            Some(Ok((Err(e), _))) => {
                eprintln!("{}: activation failed: {e}", self.info.id);
            }
            Some(Err(f)) => self.report(PluginCall::Activate, f, "activating"),
            None => {}
        }
        // buffers for the declared layout
        let alloc = |ports: &[u32]| -> Vec<Vec<Vec<f32>>> {
            ports
                .iter()
                .map(|c| (0..(*c).max(1)).map(|_| vec![0.0f32; frames]).collect())
                .collect()
        };
        self.ins = alloc(&self.layout.inputs);
        self.outs = alloc(&self.layout.outputs);
        let in_ch: usize = self.ins.iter().map(|p| p.len()).sum();
        let out_ch: usize = self.outs.iter().map(|p| p.len()).sum();
        self.in_ports = AudioPorts::with_capacity(in_ch, self.ins.len());
        self.out_ports = AudioPorts::with_capacity(out_ch, self.outs.len());
        self.events = EventBuffer::with_capacity(EVENT_CAP + PENDING_CAP);
        self.sink = ParamSink {
            changes: Vec::with_capacity(OUT_EVENT_CAP),
        };
        self.pending = Vec::with_capacity(PENDING_CAP);
        self.steady = 0;
        self.max_frames = frames;
        self.tail = 0;
        self.refresh_tail();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        let n = ctx.frames.min(self.max_frames);
        if self.dead() || self.proc.is_none() || n == 0 {
            silence(io);
            return ProcessStatus::Sleep;
        }
        // ---- inputs: the engine's main and sidechain ports into the plugin's ports
        for (pi, port) in self.ins.iter_mut().enumerate() {
            let src = if Some(pi) == self.layout.main_in {
                io.inputs.first()
            } else if Some(pi) == self.layout.side_in {
                io.inputs.get(1)
            } else {
                None
            };
            for (c, ch) in port.iter_mut().enumerate() {
                match src.and_then(|s| s.get(c).or_else(|| s.last())) {
                    Some(x) => ch[..n].copy_from_slice(&x[..n]),
                    None => ch[..n].fill(0.0),
                }
            }
        }
        self.fill_events(io.events);
        self.sink.changes.clear();
        let transport = Self::transport(ctx);
        let steady = self.steady;
        let (ins, outs) = (&mut self.ins, &mut self.outs);
        let (in_ports, out_ports) = (&mut self.in_ports, &mut self.out_ports);
        let (events, sink) = (&self.events, &mut self.sink);
        let proc = self.proc.as_mut().expect("checked");
        let result = guarded(|| {
            let started = match proc.ensure_processing_started() {
                Ok(p) => p,
                Err(_) => return None,
            };
            let inputs = in_ports.with_input_buffers(ins.iter_mut().map(|port| AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    port.iter_mut().map(|c| InputChannel::variable(&mut c[..n])),
                ),
            }));
            let mut outputs =
                out_ports.with_output_buffers(outs.iter_mut().map(|port| AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        port.iter_mut().map(|c| &mut c[..n]),
                    ),
                }));
            let mut out_events = OutputEvents::from_buffer(sink);
            started
                .process(
                    &inputs,
                    &mut outputs,
                    &events.as_input(),
                    &mut out_events,
                    Some(steady),
                    Some(&transport),
                )
                .ok()
        });
        self.steady += n as u64;
        let status = match result {
            Ok(s) => s,
            Err(f) => {
                // the fault path may allocate (once per node, contract rule 5)
                self.report(PluginCall::Process, f, "processing");
                silence(io);
                return ProcessStatus::Sleep;
            }
        };
        // ---- outputs: the plugin's main output to the engine's stereo port
        let main = self.layout.main_out.and_then(|m| self.outs.get(m));
        for (c, out) in io
            .outputs
            .iter_mut()
            .flat_map(|p| p.iter_mut())
            .enumerate()
            .take(2)
        {
            match main.and_then(|p| p.get(c).or_else(|| p.last())) {
                Some(x) if status.is_some() => out[..n].copy_from_slice(&x[..n]),
                _ => out[..n].fill(0.0),
            }
        }
        for i in 0..self.sink.changes.len() {
            let (id, v) = self.sink.changes[i];
            if let Some(idx) = self.param_index(id) {
                self.values[idx] = v;
            }
        }
        match status {
            Some(clack_host::process::ProcessStatus::Sleep) => ProcessStatus::Sleep,
            Some(_) => ProcessStatus::Continue,
            None => ProcessStatus::Sleep,
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        let i = id.0 as usize;
        let (Some(cid), Some(info)) = (self.param_ids.get(i), self.desc.params.get(i)) else {
            return;
        };
        let v = info.clamp(value);
        self.values[i] = v;
        if self.pending.len() < self.pending.capacity() {
            self.pending.push((cid.get(), v));
        } else if self.proc.is_none() && !self.dead() {
            // not active yet (no capacity reserved): flush on the main thread instead
            self.flush_param(cid.get(), v);
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.values.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    fn reset(&mut self) {
        if self.dead() {
            return;
        }
        if let Some(p) = self.proc.as_mut()
            && let Err(f) = guarded(|| p.reset())
        {
            self.report(PluginCall::Reset, f, "resetting");
        }
    }

    fn latency(&self) -> u32 {
        self.control.latency.load(Relaxed)
    }

    fn tail(&self) -> u32 {
        self.tail
    }

    fn save_state(&self) -> Vec<u8> {
        ControlHandle(self.control.clone())
            .save_state()
            .unwrap_or_default()
    }

    fn load_state(&mut self, state: &[u8], _: &dyn AssetResolver) -> Result<(), String> {
        let data = state.to_vec();
        let ids = self.param_ids.clone();
        let r = self
            .control
            .on_main(PluginCall::LoadState, "loading its state", move |slot| {
                load_slot(slot, &data)?;
                let values: Vec<f64> = match slot.exts.params {
                    Some(pp) => {
                        let h = slot.instance.plugin_handle();
                        ids.iter()
                            .map(|id| pp.get_value(&h, *id).unwrap_or(0.0))
                            .collect()
                    }
                    None => Vec::new(),
                };
                Ok::<_, PluginError>(values)
            })
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if r.len() == self.values.len() {
            self.values = r;
        }
        Ok(())
    }
}

impl ClapInstance {
    /// Sends one parameter value through `params.flush` (main thread, inactive plugin).
    fn flush_param(&self, id: u32, value: f64) {
        let _ = self
            .control
            .on_main(PluginCall::SetParam, "setting a parameter", move |slot| {
                let Some(pp) = slot.exts.params else {
                    return;
                };
                let Some(cid) = ClapId::from_raw(id) else {
                    return;
                };
                let mut buf = EventBuffer::with_capacity(1);
                buf.push(&ParamValueEvent::new(0, cid, Pckn::match_all(), value));
                let mut out = EventBuffer::new();
                if let Some(mut h) = slot.instance.inactive_plugin_handle() {
                    pp.flush(&mut h, &buf.as_input(), &mut out.as_output());
                }
            });
    }

    /// Updates the cached tail from the plugin (audio thread allowed).
    pub fn refresh_tail(&mut self) {
        if let (Some(t), Some(p)) = (self.tail_ext, self.proc.as_mut())
            && !self.control.fault.is_dead()
        {
            let r = guarded(|| t.get(&p.plugin_handle()));
            if let Ok(t) = r {
                self.tail = t.to_raw();
            }
        }
    }
}

impl PluginInstance for ClapInstance {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        ControlHandle(self.control.clone()).editor()
    }

    fn control(&self) -> Option<Arc<dyn PluginControl>> {
        Some(Arc::new(ControlHandle(self.control.clone())))
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        if self.dead() {
            // never call a faulted plugin again, not even to free it
            std::mem::forget(self.proc.take());
            return;
        }
        self.deactivate();
        if !self.dead() {
            let key = self.control.key;
            main_thread().post(move || destroy_slot(key));
        }
    }
}
