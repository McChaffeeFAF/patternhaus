//! The VST3 host (M2/T3), on the `vst3` crate (MIT OR Apache-2.0; the VST 3 SDK is MIT since
//! 3.8, and the crate's bindings are generated from those headers).
//!
//! - [`Vst3Format`] loads a `.vst3` module (a DLL or bundle on Windows, a bundle's `.so` on
//!   Linux), lists its "Audio Module Class" classes from the plugin factory and creates
//!   instances.
//! - [`Vst3Instance`] wraps `IComponent` + `IAudioProcessor` + `IEditController` (one object
//!   or two, connected through `IConnectionPoint`), with the host context, a component handler
//!   for edits made in the plugin's editor, and `IPlugView` editors embedded by `HWND`.
//! - Like the CLAP host, everything but processing runs on the plugin main thread, and every
//!   call into plugin code is crash-guarded.
//! - Parameters: continuous parameters are exposed in VST3's normalized 0..1 range (the
//!   plugin's own text shows the plain value); stepped list parameters as their step index
//!   (0..=stepCount) with the plugin's labels as choices.
//! - Events: notes, note-offs and note expressions (volume, pan, tuning, vibrato, brightness)
//!   become VST3 events; `AllNotesOff` releases every sounding note; `Cc` and `PitchBend` go
//!   through the plugin's `IMidiMapping` to the parameter it assigns (VST3 has no MIDI CC
//!   events); `Param` events are sample-accurate parameter-change points.
//! - State: the component state and the controller state, both saved, both restored.
//!
//! Not done (listed in the T3 report): macOS bundle loading (`bundleEntry`), Linux/macOS
//! editors, `IUnitInfo`/programs, sidechain bus arrangements beyond the plugin's defaults,
//! per-note pressure.

// the bindings' enum types differ between platforms, so some casts are no-ops on one of them
#![allow(clippy::unnecessary_cast)]

mod com;

use crate::guard::{NativeFault, guarded};
use crate::main_thread::main_thread;
use crate::*;
use ::vst3::Steinberg::Vst::*;
use ::vst3::Steinberg::*;
use ::vst3::{ComPtr, ComWrapper, Interface};
use com::*;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU32, Ordering::*};

/// Standard VST3 folders for this platform.
pub fn vst3_default_paths() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(p) = env("COMMONPROGRAMFILES") {
            out.push(p.join("VST3"));
        }
        if let Some(p) = env("LOCALAPPDATA") {
            out.push(p.join("Programs").join("Common").join("VST3"));
        }
    } else if cfg!(target_os = "macos") {
        out.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(h) = env("HOME") {
            out.push(h.join("Library/Audio/Plug-Ins/VST3"));
        }
    } else {
        if let Some(h) = env("HOME") {
            out.push(h.join(".vst3"));
        }
        out.push(PathBuf::from("/usr/lib/vst3"));
        out.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    out
}

/// The VST3 format host.
#[derive(Default)]
pub struct Vst3Format;

impl Vst3Format {
    pub fn new() -> Self {
        crate::guard::install();
        Self
    }
}

fn fault_error(f: NativeFault, what: &str) -> PluginError {
    PluginError::Call(format!("the plugin crashed while {what}: {}", f.describe()))
}

fn cstr_field(s: &[c_char]) -> String {
    let bytes: Vec<u8> = s
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// A class id as 32 hex digits (the bytes as the factory reports them).
pub fn cid_hex(cid: &TUID) -> String {
    cid.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

fn cid_from_hex(s: &str) -> Option<TUID> {
    if s.len() != 32 {
        return None;
    }
    let mut out: TUID = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()? as c_char;
    }
    Some(out)
}

const AUDIO_CLASS: &str = "Audio Module Class";

// ------------------------------------------------------------------------------- modules

/// A loaded module and its factory. Kept loaded for the life of the process (plugins often
/// misbehave when unloaded).
struct Module {
    _lib: libloading::Library,
    factory: ComPtr<IPluginFactory>,
}

/// The binary inside a `.vst3` file or bundle.
fn binary_of(path: &Path) -> PluginResult<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let stem = path
        .file_stem()
        .ok_or_else(|| PluginError::NotAPlugin(path.to_path_buf()))?;
    let contents = path.join("Contents");
    let candidate = if cfg!(windows) {
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64-win"
        } else {
            "x86_64-win"
        };
        contents.join(arch).join(path.file_name().unwrap_or(stem))
    } else if cfg!(target_os = "macos") {
        return Err(PluginError::Call(
            "VST3 bundles on macOS need bundleEntry loading (not implemented yet)".into(),
        ));
    } else {
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64-linux"
        } else {
            "x86_64-linux"
        };
        let mut so = stem.to_os_string();
        so.push(".so");
        contents.join(arch).join(so)
    };
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(PluginError::NotAPlugin(path.to_path_buf()))
    }
}

thread_local! {
    static MODULES: std::cell::RefCell<std::collections::HashMap<PathBuf, std::rc::Rc<Module>>> =
        Default::default();
    static SLOTS: std::cell::RefCell<std::collections::HashMap<u64, Slot>> = Default::default();
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

type GetFactory = unsafe extern "system" fn() -> *mut IPluginFactory;

/// Loads a module (main thread).
fn module(path: &Path) -> PluginResult<std::rc::Rc<Module>> {
    if let Some(m) = MODULES.with(|m| m.borrow().get(path).cloned()) {
        return Ok(m);
    }
    let bin = binary_of(path)?;
    let m = guarded(|| -> PluginResult<Module> {
        // SAFETY: loading a plugin runs its initialisers; the guard catches a crash
        let lib = unsafe { libloading::Library::new(&bin) }
            .map_err(|e| PluginError::Call(format!("cannot load {}: {e}", bin.display())))?;
        #[cfg(windows)]
        {
            // SAFETY: optional module entry of the VST3 Windows module ABI
            if let Ok(init) =
                unsafe { lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") }
            {
                unsafe { init() };
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // SAFETY: the Linux module entry takes the dlopen handle
            if let Ok(entry) = unsafe {
                lib.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"ModuleEntry\0")
            } {
                let handle = unsafe { dlopen_handle(&bin) };
                if !unsafe { entry(handle) } {
                    return Err(PluginError::Call("ModuleEntry failed".into()));
                }
            }
        }
        // SAFETY: the factory export of the VST3 module ABI
        let get = unsafe { lib.get::<GetFactory>(b"GetPluginFactory\0") }
            .map_err(|_| PluginError::NotAPlugin(path.to_path_buf()))?;
        let raw = unsafe { get() };
        // SAFETY: GetPluginFactory returns an owned reference
        let factory = unsafe { ComPtr::from_raw(raw) }
            .ok_or_else(|| PluginError::Call("the module has no plugin factory".into()))?;
        Ok(Module { _lib: lib, factory })
    })
    .map_err(|f| fault_error(f, "loading"))??;
    let m = std::rc::Rc::new(m);
    MODULES.with(|ms| ms.borrow_mut().insert(path.to_path_buf(), m.clone()));
    Ok(m)
}

#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn dlopen_handle(path: &Path) -> *mut c_void {
    // the handle of the already loaded library (dlopen is refcounted)
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flag: i32) -> *mut c_void;
    }
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap_or_default();
    // RTLD_LAZY = 1
    unsafe { dlopen(c.as_ptr(), 1) }
}

/// The audio classes of a factory: (cid, name, vendor, version, sub categories).
fn classes(f: &ComPtr<IPluginFactory>) -> Vec<(TUID, String, String, String, String)> {
    let mut out = Vec::new();
    // SAFETY: plain factory queries
    unsafe {
        let mut finfo: PFactoryInfo = std::mem::zeroed();
        f.getFactoryInfo(&mut finfo);
        let fvendor = cstr_field(&finfo.vendor);
        let f2 = f.cast::<IPluginFactory2>();
        for i in 0..f.countClasses() {
            if let Some(f2) = &f2 {
                let mut c: PClassInfo2 = std::mem::zeroed();
                if f2.getClassInfo2(i, &mut c) == kResultOk {
                    if cstr_field(&c.category) == AUDIO_CLASS {
                        let vendor = cstr_field(&c.vendor);
                        out.push((
                            c.cid,
                            cstr_field(&c.name),
                            if vendor.is_empty() {
                                fvendor.clone()
                            } else {
                                vendor
                            },
                            cstr_field(&c.version),
                            cstr_field(&c.subCategories),
                        ));
                    }
                    continue;
                }
            }
            let mut c: PClassInfo = std::mem::zeroed();
            if f.getClassInfo(i, &mut c) == kResultOk && cstr_field(&c.category) == AUDIO_CLASS {
                out.push((
                    c.cid,
                    cstr_field(&c.name),
                    fvendor.clone(),
                    String::new(),
                    String::new(),
                ));
            }
        }
    }
    out
}

fn scan_on_main(path: &Path) -> PluginResult<Vec<PluginInfo>> {
    let is_vst3 = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vst3"));
    if !is_vst3 {
        return Err(PluginError::NotAPlugin(path.to_path_buf()));
    }
    let m = module(path)?;
    let list =
        guarded(|| classes(&m.factory)).map_err(|f| fault_error(f, "listing its plugins"))?;
    let mut out = Vec::new();
    for (cid, name, vendor, version, subs) in list {
        let kind = if subs.split('|').any(|s| s == "Instrument") {
            PluginKind::Instrument
        } else {
            PluginKind::Effect
        };
        let mut info = PluginInfo {
            format: PluginFormat::Vst3,
            id: cid_hex(&cid),
            name,
            vendor,
            version,
            kind,
            path: path.to_path_buf(),
            has_editor: false,
        };
        // creating it once tells whether it loads and has an editor (in the scan helper)
        let built = Built::create(&info)?;
        info.has_editor = built.has_editor;
        destroy_slot(built.key);
        out.push(info);
    }
    Ok(out)
}

impl PluginFormatHost for Vst3Format {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn default_paths(&self) -> Vec<PathBuf> {
        vst3_default_paths()
    }

    fn scan(&self, path: &Path) -> PluginResult<Vec<PluginInfo>> {
        let path = path.to_path_buf();
        main_thread().run(move || scan_on_main(&path))
    }

    fn instantiate(&self, info: &PluginInfo) -> PluginResult<Box<dyn PluginInstance>> {
        Ok(Box::new(Vst3Instance::new(info)?))
    }
}

// ------------------------------------------------------------------------------- instances

/// The main-thread side of one instance.
struct Slot {
    component: ComPtr<IComponent>,
    controller: Option<ComPtr<IEditController>>,
    /// The controller is the component itself (single-component plugins).
    single: bool,
    connected: bool,
    view: Option<ComPtr<IPlugView>>,
    frame: Option<ComWrapper<Frame>>,
    _host: ComWrapper<HostApp>,
    _handler: Option<ComWrapper<Handler>>,
    fault: Arc<FaultHandle>,
    active: bool,
}

#[derive(Clone, Copy, Default)]
struct Layout {
    /// Channels of each audio input bus (main, sidechain) and of the main output.
    inputs: [u32; 2],
    n_in: usize,
    outputs: [u32; 1],
    n_out: usize,
    events: bool,
}

/// What creating an instance on the main thread produced.
struct Built {
    key: u64,
    processor: ComPtr<IAudioProcessor>,
    desc: NodeDescriptor,
    ids: Vec<ParamID>,
    /// stepCount per parameter (0 = continuous, normalized).
    steps: Vec<i32>,
    values: Vec<f64>,
    layout: Layout,
    has_editor: bool,
    inbox: Arc<EditInbox>,
    midi: Vec<(u16, usize)>,
    fault: Arc<FaultHandle>,
}

fn new_key() -> u64 {
    NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    })
}

type Made = (
    ComPtr<IComponent>,
    ComPtr<IAudioProcessor>,
    Option<ComPtr<IEditController>>,
    bool,
    bool,
    Layout,
);

/// The VST3 instantiation sequence (main thread, inside a guard).
unsafe fn make(
    m: &Module,
    cid: &TUID,
    host: &ComWrapper<HostApp>,
    info: &PluginInfo,
) -> PluginResult<Made> {
    // SAFETY: the documented call sequence on live COM objects
    unsafe {
        let host_ptr = host.as_com_ref::<IHostApplication>().unwrap().as_ptr() as *mut FUnknown;
        let mut obj = std::ptr::null_mut();
        if m.factory.createInstance(
            cid.as_ptr(),
            IComponent::IID.as_ptr() as FIDString,
            &mut obj,
        ) != kResultOk
        {
            return Err(PluginError::Call(format!("cannot create {}", info.id)));
        }
        let component = ComPtr::<IComponent>::from_raw(obj as *mut IComponent)
            .ok_or_else(|| PluginError::Call("null component".into()))?;
        if component.initialize(host_ptr) != kResultOk {
            return Err(PluginError::Call(
                "the component refused to initialize".into(),
            ));
        }
        let processor = component
            .cast::<IAudioProcessor>()
            .ok_or_else(|| PluginError::Call("not an audio processor".into()))?;
        // the controller: the component itself, or a separate class
        let (controller, single) = match component.cast::<IEditController>() {
            Some(c) => (Some(c), true),
            None => {
                let mut ccid: TUID = [0; 16];
                let mut c = None;
                if component.getControllerClassId(&mut ccid) == kResultOk {
                    let mut o = std::ptr::null_mut();
                    if m.factory.createInstance(
                        ccid.as_ptr(),
                        IEditController::IID.as_ptr() as FIDString,
                        &mut o,
                    ) == kResultOk
                        && let Some(ctl) =
                            ComPtr::<IEditController>::from_raw(o as *mut IEditController)
                        && ctl.initialize(host_ptr) == kResultOk
                    {
                        c = Some(ctl);
                    }
                }
                (c, false)
            }
        };
        let mut connected = false;
        if let (Some(ctl), false) = (&controller, single)
            && let (Some(a), Some(b)) = (
                component.cast::<IConnectionPoint>(),
                ctl.cast::<IConnectionPoint>(),
            )
        {
            a.connect(b.as_ptr());
            b.connect(a.as_ptr());
            connected = true;
        }
        // state: component -> controller
        if let (Some(ctl), false) = (&controller, single) {
            let s = Stream::new(Vec::new());
            let sp = s.as_com_ref::<IBStream>().unwrap().as_ptr();
            if component.getState(sp) == kResultOk {
                s.rewind();
                ctl.setComponentState(sp);
            }
        }
        // buses: main audio in (and a sidechain), main audio out, the first event input
        let mut layout = Layout::default();
        let audio = MediaTypes_::kAudio as MediaType;
        let event = MediaTypes_::kEvent as MediaType;
        let input = BusDirections_::kInput as BusDirection;
        let output = BusDirections_::kOutput as BusDirection;
        for i in 0..component.getBusCount(audio, input).min(2) {
            let mut b: BusInfo = std::mem::zeroed();
            component.getBusInfo(audio, input, i, &mut b);
            layout.inputs[i as usize] = b.channelCount.max(0) as u32;
            layout.n_in += 1;
            component.activateBus(audio, input, i, 1);
        }
        if component.getBusCount(audio, output) > 0 {
            let mut b: BusInfo = std::mem::zeroed();
            component.getBusInfo(audio, output, 0, &mut b);
            layout.outputs[0] = b.channelCount.max(0) as u32;
            layout.n_out = 1;
            component.activateBus(audio, output, 0, 1);
        }
        if component.getBusCount(event, input) > 0 {
            layout.events = true;
            component.activateBus(event, input, 0, 1);
        }
        // ask for stereo
        let mut ins = [SpeakerArr::kStereo; 2];
        let mut outs = [SpeakerArr::kStereo; 1];
        if layout.n_out == 1
            && processor.setBusArrangements(
                ins.as_mut_ptr(),
                layout.n_in as int32,
                outs.as_mut_ptr(),
                1,
            ) == kResultOk
        {
            for c in layout.inputs.iter_mut().take(layout.n_in) {
                *c = 2;
            }
            layout.outputs[0] = 2;
        }
        Ok((component, processor, controller, single, connected, layout))
    }
}

impl Built {
    fn create(info: &PluginInfo) -> PluginResult<Built> {
        let m = module(&info.path)?;
        let cid = cid_from_hex(&info.id).ok_or_else(|| PluginError::NotFound(info.id.clone()))?;
        let host = ComWrapper::new(HostApp);
        let fault = Arc::new(FaultHandle::default());
        // SAFETY: see `make`
        let (component, processor, controller, single, connected, layout) =
            guarded(|| unsafe { make(&m, &cid, &host, info) })
                .map_err(|f| fault_error(f, "being created"))??;
        // SAFETY: plain controller queries
        let (desc, ids, steps, values, midi, has_editor) =
            guarded(|| unsafe { read_controller(controller.as_ref(), &layout, info) })
                .map_err(|f| fault_error(f, "describing itself"))?;
        let inbox = Arc::new(EditInbox::new(ids.clone()));
        let handler = controller.as_ref().map(|c| {
            let h = ComWrapper::new(Handler {
                inbox: inbox.clone(),
            });
            // SAFETY: handing the controller our handler
            let _ = guarded(|| unsafe {
                c.setComponentHandler(h.as_com_ref::<IComponentHandler>().unwrap().as_ptr())
            });
            h
        });
        let key = new_key();
        SLOTS.with(|s| {
            s.borrow_mut().insert(
                key,
                Slot {
                    component,
                    controller,
                    single,
                    connected,
                    view: None,
                    frame: None,
                    _host: host,
                    _handler: handler,
                    fault: fault.clone(),
                    active: false,
                },
            )
        });
        Ok(Built {
            key,
            processor,
            desc,
            ids,
            steps,
            values,
            layout,
            has_editor,
            inbox,
            midi,
            fault,
        })
    }
}

type ReadOut = (
    NodeDescriptor,
    Vec<ParamID>,
    Vec<i32>,
    Vec<f64>,
    Vec<(u16, usize)>,
    bool,
);

/// Parameters, MIDI mapping and editor presence (main thread, guarded by the caller).
unsafe fn read_controller(
    ctl: Option<&ComPtr<IEditController>>,
    layout: &Layout,
    info: &PluginInfo,
) -> ReadOut {
    let (mut params, mut ids, mut steps, mut values, mut midi) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut has_editor = false;
    if let Some(c) = ctl {
        // SAFETY: plain controller queries
        unsafe {
            for i in 0..c.getParameterCount() {
                let mut p: ParameterInfo = std::mem::zeroed();
                if c.getParameterInfo(i, &mut p) != kResultOk {
                    continue;
                }
                let flags = p.flags;
                if flags & ParameterInfo_::ParameterFlags_::kIsHidden as i32 != 0 {
                    continue;
                }
                let name = read_tchars(&p.title);
                let norm = c.getParamNormalized(p.id);
                let list = p.stepCount > 0 && p.stepCount < 64;
                let (mut pi, value) = if list {
                    let n = p.stepCount;
                    let mut choices = Vec::new();
                    for s in 0..=n {
                        let mut t: String128 = [0; 128];
                        if c.getParamStringByValue(p.id, s as f64 / n as f64, &mut t) == kResultOk {
                            choices.push(read_tchars(&t));
                        } else {
                            choices.push(s.to_string());
                        }
                    }
                    let mut pi = ParamInfo::new(
                        &p.id.to_string(),
                        &name,
                        0.0,
                        n as f64,
                        (p.defaultNormalizedValue * n as f64).round(),
                        ParamUnit::Choice,
                    );
                    pi.stepped = true;
                    pi.choices = choices;
                    (pi, (norm * n as f64).round())
                } else {
                    (
                        ParamInfo::new(
                            &p.id.to_string(),
                            &name,
                            0.0,
                            1.0,
                            p.defaultNormalizedValue.clamp(0.0, 1.0),
                            ParamUnit::None,
                        ),
                        norm,
                    )
                };
                pi.automatable = flags & ParameterInfo_::ParameterFlags_::kCanAutomate as i32 != 0
                    && flags & ParameterInfo_::ParameterFlags_::kIsReadOnly as i32 == 0;
                params.push(pi);
                ids.push(p.id);
                steps.push(if list { p.stepCount } else { 0 });
                values.push(value);
            }
            if let Some(mm) = c.cast::<IMidiMapping>() {
                for cc in 0..=129u16 {
                    let mut id: ParamID = 0;
                    if mm.getMidiControllerAssignment(0, 0, cc as CtrlNumber, &mut id) == kResultOk
                        && let Some(idx) = ids.iter().position(|x| *x == id)
                    {
                        midi.push((cc, idx));
                    }
                }
            }
            if let Some(v) = ComPtr::<IPlugView>::from_raw(c.createView(ViewType::kEditor)) {
                has_editor = true;
                drop(v);
            }
        }
    }
    let port = |name: &str, channels: u32, sidechain: bool| PortInfo {
        name: name.into(),
        channels: channels.max(1),
        sidechain,
    };
    let mut audio_inputs = Vec::new();
    if layout.n_in > 0 {
        audio_inputs.push(port("in", layout.inputs[0], false));
    }
    if layout.n_in > 1 {
        audio_inputs.push(port("sidechain", layout.inputs[1], true));
    }
    let desc = NodeDescriptor {
        id: format!("vst3:{}", info.id),
        name: info.name.clone(),
        kind: match info.kind {
            PluginKind::Instrument => NodeKind::Instrument,
            _ => NodeKind::Effect,
        },
        audio_inputs,
        audio_outputs: vec![port("out", 2, false)],
        note_input: layout.events,
        params,
    };
    (desc, ids, steps, values, midi, has_editor)
}

/// Tears an instance down (main thread), unless it faulted.
fn destroy_slot(key: u64) {
    let Some(slot) = SLOTS.with(|s| s.borrow_mut().remove(&key)) else {
        return;
    };
    if slot.fault.is_dead() {
        std::mem::forget(slot);
        return;
    }
    let fault = slot.fault.clone();
    let r = guarded(move || {
        // SAFETY: the VST3 teardown sequence
        unsafe {
            if let Some(v) = &slot.view {
                v.removed();
            }
            if slot.active {
                if let Some(p) = slot.component.cast::<IAudioProcessor>() {
                    p.setProcessing(0);
                }
                slot.component.setActive(0);
            }
            if let Some(ctl) = &slot.controller {
                if slot.connected
                    && let (Some(a), Some(b)) = (
                        slot.component.cast::<IConnectionPoint>(),
                        ctl.cast::<IConnectionPoint>(),
                    )
                {
                    a.disconnect(b.as_ptr());
                    b.disconnect(a.as_ptr());
                }
                ctl.setComponentHandler(std::ptr::null_mut());
                if !slot.single {
                    ctl.terminate();
                }
            }
            slot.component.terminate();
        }
        drop(slot);
    });
    if let Err(f) = r {
        fault.report(PluginFault {
            plugin_id: String::new(),
            call: PluginCall::Activate,
            message: format!("crashed while being destroyed: {}", f.describe()),
        });
    }
}

fn with_slot<R>(key: u64, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    let mut slot = SLOTS.with(|s| s.borrow_mut().remove(&key))?;
    let r = f(&mut slot);
    SLOTS.with(|s| s.borrow_mut().insert(key, slot));
    Some(r)
}

// ------------------------------------------------------------------------------- control

/// The [`PluginControl`] of a VST3 instance.
pub struct Vst3Control {
    info: PluginInfo,
    key: u64,
    fault: Arc<FaultHandle>,
    ids: Vec<ParamID>,
    steps: Vec<i32>,
    latency: AtomicU32,
    has_editor: bool,
}

impl Vst3Control {
    fn fail(&self, call: PluginCall, f: NativeFault, what: &str) -> PluginError {
        self.fault.report(PluginFault {
            plugin_id: self.info.id.clone(),
            call,
            message: format!("crashed while {what}: {}", f.describe()),
        });
        fault_error(f, what)
    }

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

    fn to_plain(&self, i: usize, norm: f64) -> f64 {
        match self.steps.get(i).copied().unwrap_or(0) {
            0 => norm,
            n => (norm * n as f64).round(),
        }
    }

    fn to_norm(&self, i: usize, v: f64) -> f64 {
        match self.steps.get(i).copied().unwrap_or(0) {
            0 => v.clamp(0.0, 1.0),
            n => (v / n as f64).clamp(0.0, 1.0),
        }
    }
}

const STATE_MAGIC: &[u8; 8] = b"PHVST3S1";

fn pack_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut out = STATE_MAGIC.to_vec();
    out.extend_from_slice(&(component.len() as u64).to_le_bytes());
    out.extend_from_slice(component);
    out.extend_from_slice(&(controller.len() as u64).to_le_bytes());
    out.extend_from_slice(controller);
    out
}

fn unpack_state(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = b.strip_prefix(STATE_MAGIC.as_slice())?;
    let n = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?) as usize;
    let comp = rest.get(8..8 + n)?;
    let rest = &rest[8 + n..];
    let m = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?) as usize;
    let ctl = rest.get(8..8 + m)?;
    Some((comp, ctl))
}

/// Saves component and controller state (main thread, inside a guard).
fn save_slot(slot: &mut Slot) -> PluginResult<Vec<u8>> {
    let a = Stream::new(Vec::new());
    // SAFETY: state calls with our stream
    unsafe {
        if slot
            .component
            .getState(a.as_com_ref::<IBStream>().unwrap().as_ptr())
            != kResultOk
        {
            return Err(PluginError::Call(
                "the plugin could not save its state".into(),
            ));
        }
    }
    let b = Stream::new(Vec::new());
    if let (Some(ctl), false) = (&slot.controller, slot.single) {
        // SAFETY: as above
        unsafe { ctl.getState(b.as_com_ref::<IBStream>().unwrap().as_ptr()) };
    }
    Ok(pack_state(&a.bytes(), &b.bytes()))
}

fn load_slot(slot: &mut Slot, data: &[u8]) -> PluginResult<()> {
    let (comp, ctl) = unpack_state(data)
        .ok_or_else(|| PluginError::Call("not a VST3 state written by Patternhaus".into()))?;
    let a = Stream::new(comp.to_vec());
    let ap = a.as_com_ref::<IBStream>().unwrap().as_ptr();
    // SAFETY: state calls with our streams
    unsafe {
        if slot.component.setState(ap) != kResultOk {
            return Err(PluginError::Call("the plugin refused the state".into()));
        }
        if let Some(c) = &slot.controller {
            a.rewind();
            c.setComponentState(ap);
            if !ctl.is_empty() && !slot.single {
                let b = Stream::new(ctl.to_vec());
                c.setState(b.as_com_ref::<IBStream>().unwrap().as_ptr());
            }
        }
    }
    Ok(())
}

struct ControlHandle(Arc<Vst3Control>);

impl PluginControl for ControlHandle {
    fn info(&self) -> &PluginInfo {
        &self.0.info
    }

    fn fault(&self) -> Arc<FaultHandle> {
        self.0.fault.clone()
    }

    fn param_values(&self) -> PluginResult<Vec<f64>> {
        let ids = self.0.ids.clone();
        let c = self.0.clone();
        self.0
            .on_main(PluginCall::Param, "reading parameters", move |slot| {
                let Some(ctl) = &slot.controller else {
                    return Vec::new();
                };
                ids.iter()
                    .enumerate()
                    // SAFETY: plain controller query
                    .map(|(i, id)| c.to_plain(i, unsafe { ctl.getParamNormalized(*id) }))
                    .collect()
            })
    }

    fn param_text(&self, index: usize, value: f64) -> Option<String> {
        let id = *self.0.ids.get(index)?;
        let norm = self.0.to_norm(index, value);
        self.0
            .on_main(PluginCall::Param, "formatting a value", move |slot| {
                let ctl = slot.controller.as_ref()?;
                let mut t: String128 = [0; 128];
                // SAFETY: plain controller query
                (unsafe { ctl.getParamStringByValue(id, norm, &mut t) } == kResultOk)
                    .then(|| read_tchars(&t))
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
        if !self.0.has_editor || self.0.fault.is_dead() {
            return None;
        }
        Some(Box::new(Vst3Editor {
            control: self.0.clone(),
            size: None,
        }))
    }
}

// ------------------------------------------------------------------------------- editor

/// A VST3 plugin's `IPlugView`, embedded by `HWND` (Windows only for now).
pub struct Vst3Editor {
    control: Arc<Vst3Control>,
    size: Option<EditorSize>,
}

/// A parent window handle the main thread uses (the UI keeps it alive while open).
struct SendHwnd(isize);
// SAFETY: a window handle is an id.
unsafe impl Send for SendHwnd {}

impl PluginEditor for Vst3Editor {
    fn can_embed(&self) -> bool {
        cfg!(windows)
    }

    fn open(&mut self, parent: Option<RawWindowHandle>) -> PluginResult<EditorSize> {
        self.close();
        let Some(RawWindowHandle::Win32(h)) = parent else {
            return Err(PluginError::Call(
                "VST3 editors open embedded in a Win32 window only (for now)".into(),
            ));
        };
        let hwnd = SendHwnd(h.hwnd.get());
        let size = self.control.on_main(
            PluginCall::Param,
            "opening its editor",
            move |slot| -> PluginResult<EditorSize> {
                let ctl = slot
                    .controller
                    .clone()
                    .ok_or_else(|| PluginError::Call("the plugin has no controller".into()))?;
                // SAFETY: the IPlugView attach sequence with a live parent window
                unsafe {
                    let view = ComPtr::<IPlugView>::from_raw(ctl.createView(ViewType::kEditor))
                        .ok_or_else(|| PluginError::Call("the plugin made no editor".into()))?;
                    if view.isPlatformTypeSupported(kPlatformTypeHWND) != kResultOk {
                        return Err(PluginError::Call(
                            "the editor cannot embed in a Win32 window".into(),
                        ));
                    }
                    let frame = ComWrapper::new(Frame {
                        size: std::sync::Mutex::new(None),
                    });
                    view.setFrame(frame.as_com_ref::<IPlugFrame>().unwrap().as_ptr());
                    if view.attached(hwnd.0 as *mut c_void, kPlatformTypeHWND) != kResultOk {
                        return Err(PluginError::Call("the editor refused its parent".into()));
                    }
                    let mut r: ViewRect = std::mem::zeroed();
                    view.getSize(&mut r);
                    slot.view = Some(view);
                    slot.frame = Some(frame);
                    Ok(EditorSize {
                        width: (r.right - r.left).max(1) as u32,
                        height: (r.bottom - r.top).max(1) as u32,
                    })
                }
            },
        )??;
        self.size = Some(size);
        Ok(size)
    }

    fn close(&mut self) {
        if self.size.take().is_none() {
            return;
        }
        let _ = self
            .control
            .on_main(PluginCall::Param, "closing its editor", |slot| {
                if let Some(v) = slot.view.take() {
                    // SAFETY: detaching the view we attached
                    unsafe {
                        v.removed();
                        v.setFrame(std::ptr::null_mut());
                    }
                }
                slot.frame = None;
            });
    }

    fn is_open(&self) -> bool {
        self.size.is_some()
    }

    fn can_resize(&self) -> bool {
        self.control
            .on_main(PluginCall::Param, "reading its editor", |slot| {
                // SAFETY: plain view query
                slot.view
                    .as_ref()
                    .is_some_and(|v| unsafe { v.canResize() } == kResultTrue)
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
                let v = slot
                    .view
                    .clone()
                    .ok_or_else(|| PluginError::Call("the editor is closed".into()))?;
                let mut r = ViewRect {
                    left: 0,
                    top: 0,
                    right: size.width as i32,
                    bottom: size.height as i32,
                };
                // SAFETY: plain view calls
                unsafe {
                    v.checkSizeConstraint(&mut r);
                    v.onSize(&mut r);
                }
                Ok(EditorSize {
                    width: (r.right - r.left).max(1) as u32,
                    height: (r.bottom - r.top).max(1) as u32,
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

impl Drop for Vst3Editor {
    fn drop(&mut self) {
        self.close();
    }
}

// ------------------------------------------------------------------------------- processing

/// A live VST3 plugin. See the module docs.
pub struct Vst3Instance {
    info: PluginInfo,
    desc: NodeDescriptor,
    control: Arc<Vst3Control>,
    processor: ComPtr<IAudioProcessor>,
    layout: Layout,
    values: Vec<f64>,
    inbox: Arc<EditInbox>,
    /// CC number (0..=127; 128 aftertouch, 129 pitch bend) to parameter index.
    midi: Vec<(u16, usize)>,
    pending: Vec<(usize, f64)>,
    /// Note id per key of the sounding notes (`-1` = none), for `AllNotesOff`.
    sounding: [i32; 128],
    // ---- allocated in activate
    ins: Vec<Vec<Vec<f32>>>,
    outs: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_bufs: Vec<AudioBusBuffers>,
    out_bufs: Vec<AudioBusBuffers>,
    changes: ComWrapper<Changes>,
    out_changes: ComWrapper<Changes>,
    events: ComWrapper<Events>,
    max_frames: usize,
    tail: u32,
    offline: bool,
    active: bool,
}

// SAFETY: the raw pointers point into buffers this instance owns; the COM objects are only
// used by the thread that holds the instance.
unsafe impl Send for Vst3Instance {}

impl Vst3Instance {
    pub fn new(info: &PluginInfo) -> PluginResult<Self> {
        let i2 = info.clone();
        let b = main_thread().run(move || Built::create(&i2))?;
        let control = Arc::new(Vst3Control {
            info: info.clone(),
            key: b.key,
            fault: b.fault,
            ids: b.ids,
            steps: b.steps,
            latency: AtomicU32::new(0),
            has_editor: b.has_editor,
        });
        let mut info = info.clone();
        info.has_editor = b.has_editor;
        Ok(Self {
            info,
            desc: b.desc,
            control,
            processor: b.processor,
            layout: b.layout,
            values: b.values,
            inbox: b.inbox,
            midi: b.midi,
            pending: Vec::new(),
            sounding: [-1; 128],
            ins: Vec::new(),
            outs: Vec::new(),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            changes: Changes::new(),
            out_changes: Changes::new(),
            events: Events::new(),
            max_frames: 0,
            tail: 0,
            offline: false,
            active: false,
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

    fn deactivate(&mut self) {
        if !self.active || self.dead() {
            return;
        }
        let proc = self.processor.clone();
        let key = self.control.key;
        let r = main_thread().run(move || {
            with_slot(key, |slot| {
                slot.active = false;
                // SAFETY: stopping processing and deactivating
                guarded(|| unsafe {
                    proc.setProcessing(0);
                    slot.component.setActive(0);
                })
            })
        });
        self.active = false;
        if let Some(Err(f)) = r {
            self.report(PluginCall::Activate, f, "being deactivated");
        }
    }

    fn event(frame: u32, ty: u32) -> ::vst3::Steinberg::Vst::Event {
        // SAFETY: Event is plain data
        let mut e: ::vst3::Steinberg::Vst::Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = frame as i32;
        e.r#type = ty as u16;
        e
    }

    fn mode(&self) -> i32 {
        if self.offline {
            ProcessModes_::kOffline as i32
        } else {
            ProcessModes_::kRealtime as i32
        }
    }

    /// Converts the block's events and queued parameter changes (real-time).
    fn fill(&mut self, events: &[ph_node::Event]) {
        let ch = &*self.changes;
        ch.clear();
        self.out_changes.clear();
        if self.inbox.any.swap(false, Acquire) {
            for i in 0..self.inbox.ids.len() {
                if self.inbox.dirty[i].swap(false, Acquire) {
                    let norm = f64::from_bits(self.inbox.values[i].load(Acquire));
                    self.values[i] = self.control.to_plain(i, norm);
                }
            }
        }
        for k in 0..self.pending.len() {
            let (i, v) = self.pending[k];
            ch.push(self.control.ids[i], 0, self.control.to_norm(i, v));
        }
        self.pending.clear();
        let evs = &*self.events;
        evs.clear();
        for e in events {
            let t = e.frame;
            match e.kind {
                EventKind::NoteOn {
                    note_id,
                    key,
                    velocity,
                    fine_pitch,
                    ..
                } => {
                    let mut ev = Self::event(t, Event_::EventTypes_::kNoteOnEvent as u32);
                    ev.__field0.noteOn = NoteOnEvent {
                        channel: 0,
                        pitch: key as i16,
                        tuning: fine_pitch,
                        velocity,
                        length: 0,
                        noteId: note_id as i32,
                    };
                    evs.push(ev);
                    self.sounding[(key & 0x7f) as usize] = note_id as i32;
                }
                EventKind::NoteOff {
                    note_id,
                    key,
                    velocity,
                } => {
                    let mut ev = Self::event(t, Event_::EventTypes_::kNoteOffEvent as u32);
                    ev.__field0.noteOff = NoteOffEvent {
                        channel: 0,
                        pitch: key as i16,
                        velocity,
                        noteId: note_id as i32,
                        tuning: 0.0,
                    };
                    evs.push(ev);
                    self.sounding[(key & 0x7f) as usize] = -1;
                }
                EventKind::AllNotesOff => {
                    for k in 0..128 {
                        let id = self.sounding[k];
                        if id >= 0 {
                            let mut ev = Self::event(t, Event_::EventTypes_::kNoteOffEvent as u32);
                            ev.__field0.noteOff = NoteOffEvent {
                                channel: 0,
                                pitch: k as i16,
                                velocity: 0.0,
                                noteId: id,
                                tuning: 0.0,
                            };
                            evs.push(ev);
                            self.sounding[k] = -1;
                        }
                    }
                }
                EventKind::Param { id, value } => {
                    let i = id.0 as usize;
                    if let (Some(pid), Some(info)) =
                        (self.control.ids.get(i).copied(), self.desc.params.get(i))
                    {
                        let v = info.clamp(value);
                        self.values[i] = v;
                        ch.push(pid, t as i32, self.control.to_norm(i, v));
                    }
                }
                EventKind::Cc { controller, value } => {
                    if let Some((_, i)) = self.midi.iter().find(|(c, _)| *c == controller as u16) {
                        ch.push(self.control.ids[*i], t as i32, value.clamp(0.0, 1.0) as f64);
                    }
                }
                EventKind::PitchBend { value } => {
                    if let Some((_, i)) = self.midi.iter().find(|(c, _)| *c == 129) {
                        let norm = (value.clamp(-1.0, 1.0) as f64 + 1.0) / 2.0;
                        ch.push(self.control.ids[*i], t as i32, norm);
                    }
                }
                EventKind::NoteExpression {
                    note_id,
                    kind,
                    value,
                } => {
                    let v = value as f64;
                    let (ty, norm) = match kind {
                        NoteExpressionKind::Volume => (0u32, v / 4.0),
                        NoteExpressionKind::Pan => (1, (v + 1.0) / 2.0),
                        NoteExpressionKind::Tuning => (2, 0.5 + v / 240.0),
                        NoteExpressionKind::Vibrato => (3, v),
                        NoteExpressionKind::Brightness => (5, v),
                        NoteExpressionKind::Pressure => continue,
                    };
                    let mut ev =
                        Self::event(t, Event_::EventTypes_::kNoteExpressionValueEvent as u32);
                    ev.__field0.noteExpressionValue = NoteExpressionValueEvent {
                        typeId: ty,
                        noteId: note_id as i32,
                        value: norm.clamp(0.0, 1.0),
                    };
                    evs.push(ev);
                }
            }
        }
    }
}

impl Processor for Vst3Instance {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.desc
    }

    fn activate(&mut self, a: &ActivateInfo) {
        self.deactivate();
        if self.dead() {
            return;
        }
        let frames = a.max_frames.max(1);
        self.offline = a.quality == Quality::High;
        let mut setup = ProcessSetup {
            processMode: self.mode(),
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: frames as i32,
            sampleRate: a.sample_rate,
        };
        let proc = self.processor.clone();
        let key = self.control.key;
        let r = main_thread().run(move || {
            with_slot(key, |slot| {
                // SAFETY: setup, activation, start of processing
                guarded(|| unsafe {
                    let ok = proc.setupProcessing(&mut setup) == kResultOk
                        && slot.component.setActive(1) == kResultOk;
                    slot.active = ok;
                    if ok {
                        proc.setProcessing(1);
                    }
                    (ok, proc.getLatencySamples(), proc.getTailSamples())
                })
            })
        });
        match r {
            Some(Ok((true, latency, tail))) => {
                self.active = true;
                self.control.latency.store(latency, Relaxed);
                self.tail = tail;
            }
            Some(Ok((false, ..))) => eprintln!("{}: activation failed", self.info.id),
            Some(Err(f)) => self.report(PluginCall::Activate, f, "activating"),
            None => {}
        }
        let alloc = |chs: &[u32]| -> Vec<Vec<Vec<f32>>> {
            chs.iter()
                .map(|c| (0..(*c).max(1)).map(|_| vec![0.0f32; frames]).collect())
                .collect()
        };
        self.ins = alloc(&self.layout.inputs[..self.layout.n_in]);
        self.outs = alloc(&self.layout.outputs[..self.layout.n_out]);
        self.in_ptrs = self
            .ins
            .iter_mut()
            .map(|p| p.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        self.out_ptrs = self
            .outs
            .iter_mut()
            .map(|p| p.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        let bus = |ptrs: &mut Vec<*mut f32>| AudioBusBuffers {
            numChannels: ptrs.len() as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: ptrs.as_mut_ptr(),
            },
        };
        self.in_bufs = self.in_ptrs.iter_mut().map(bus).collect();
        self.out_bufs = self.out_ptrs.iter_mut().map(bus).collect();
        self.changes = Changes::new();
        self.out_changes = Changes::new();
        self.events = Events::new();
        self.pending = Vec::with_capacity(256);
        self.sounding = [-1; 128];
        self.max_frames = frames;
    }

    fn process(
        &mut self,
        ctx: &ph_node::ProcessContext,
        io: &mut ProcessIo<'_, '_>,
    ) -> ProcessStatus {
        let n = ctx.frames.min(self.max_frames);
        if self.dead() || !self.active || n == 0 {
            silence(io);
            return ProcessStatus::Sleep;
        }
        for (pi, port) in self.ins.iter_mut().enumerate() {
            let src = io.inputs.get(pi);
            for (c, ch) in port.iter_mut().enumerate() {
                match src.and_then(|s| s.get(c).or_else(|| s.last())) {
                    Some(x) => ch[..n].copy_from_slice(&x[..n]),
                    None => ch[..n].fill(0.0),
                }
            }
        }
        self.fill(io.events);
        // SAFETY: ProcessContext is plain data
        let mut pc: ::vst3::Steinberg::Vst::ProcessContext = unsafe { std::mem::zeroed() };
        pc.state = (ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | if ctx.playing {
                ProcessContext_::StatesAndFlags_::kPlaying
            } else {
                0
            }) as u32;
        pc.sampleRate = ctx.sample_rate;
        pc.projectTimeSamples = ctx.song_pos_samples;
        pc.projectTimeMusic = ctx.song_pos_ticks / ph_core::PPQ as f64;
        pc.tempo = ctx.bpm;
        pc.timeSigNumerator = 4;
        pc.timeSigDenominator = 4;
        for b in self.in_bufs.iter_mut().chain(self.out_bufs.iter_mut()) {
            b.silenceFlags = 0;
        }
        let mut data = ProcessData {
            processMode: self.mode(),
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: n as i32,
            numInputs: self.in_bufs.len() as i32,
            numOutputs: self.out_bufs.len() as i32,
            inputs: self.in_bufs.as_mut_ptr(),
            outputs: self.out_bufs.as_mut_ptr(),
            inputParameterChanges: self
                .changes
                .as_com_ref::<IParameterChanges>()
                .unwrap()
                .as_ptr(),
            outputParameterChanges: self
                .out_changes
                .as_com_ref::<IParameterChanges>()
                .unwrap()
                .as_ptr(),
            inputEvents: self.events.as_com_ref::<IEventList>().unwrap().as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: &mut pc,
        };
        let proc = &self.processor;
        // SAFETY: every pointer in `data` points into buffers this instance owns
        let r = guarded(|| unsafe { proc.process(&mut data) });
        match r {
            Ok(res) if res == kResultOk => {}
            Ok(_) => {
                silence(io);
                return ProcessStatus::Continue;
            }
            Err(f) => {
                self.report(PluginCall::Process, f, "processing");
                silence(io);
                return ProcessStatus::Sleep;
            }
        }
        let main = self.outs.first();
        for (c, out) in io
            .outputs
            .iter_mut()
            .flat_map(|p| p.iter_mut())
            .enumerate()
            .take(2)
        {
            match main.and_then(|p| p.get(c).or_else(|| p.last())) {
                Some(x) => out[..n].copy_from_slice(&x[..n]),
                None => out[..n].fill(0.0),
            }
        }
        let ids = &self.control.ids;
        let (values, control) = (&mut self.values, &self.control);
        self.out_changes.last_values(|id, v| {
            if let Some(i) = ids.iter().position(|x| *x == id) {
                values[i] = control.to_plain(i, v);
            }
        });
        ProcessStatus::Continue
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        let i = id.0 as usize;
        let Some(info) = self.desc.params.get(i) else {
            return;
        };
        let v = info.clamp(value);
        self.values[i] = v;
        if self.pending.len() < self.pending.capacity() {
            self.pending.push((i, v));
        } else if !self.active && !self.dead() {
            let (pid, norm) = (self.control.ids[i], self.control.to_norm(i, v));
            let _ =
                self.control
                    .on_main(PluginCall::SetParam, "setting a parameter", move |slot| {
                        if let Some(c) = &slot.controller {
                            // SAFETY: plain controller call
                            unsafe { c.setParamNormalized(pid, norm) };
                        }
                    });
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.values.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    fn reset(&mut self) {
        self.sounding = [-1; 128];
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
        let ids = self.control.ids.clone();
        let c = self.control.clone();
        let values = self
            .control
            .on_main(PluginCall::LoadState, "loading its state", move |slot| {
                load_slot(slot, &data)?;
                Ok::<_, PluginError>(match &slot.controller {
                    Some(ctl) => ids
                        .iter()
                        .enumerate()
                        // SAFETY: plain controller query
                        .map(|(i, id)| c.to_plain(i, unsafe { ctl.getParamNormalized(*id) }))
                        .collect(),
                    None => Vec::new(),
                })
            })
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if values.len() == self.values.len() {
            self.values = values;
        }
        Ok(())
    }
}

impl PluginInstance for Vst3Instance {
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

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        if self.dead() {
            // never call a faulted plugin again, not even to release it
            std::mem::forget(self.processor.clone());
            return;
        }
        self.deactivate();
        if !self.dead() {
            let key = self.control.key;
            main_thread().post(move || destroy_slot(key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_and_states_round_trip() {
        let cid: TUID = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, -1, 0];
        let hex = cid_hex(&cid);
        assert_eq!(hex, "0102030405060708090A0B0C0D0EFF00");
        assert_eq!(cid_from_hex(&hex), Some(cid));
        assert_eq!(cid_from_hex("xyz"), None);
        let s = pack_state(b"comp", b"ctl!!");
        assert_eq!(unpack_state(&s), Some((&b"comp"[..], &b"ctl!!"[..])));
        assert!(unpack_state(&s[..s.len() - 1]).is_none());
        assert!(unpack_state(b"nope").is_none());
    }

    #[test]
    fn non_vst3_files_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.dll");
        std::fs::write(&f, b"x").unwrap();
        assert!(matches!(
            Vst3Format::new().scan(&f),
            Err(PluginError::NotAPlugin(_))
        ));
        let g = d.path().join("broken.vst3");
        std::fs::write(&g, b"not a library").unwrap();
        assert!(Vst3Format::new().scan(&g).is_err());
    }
}
