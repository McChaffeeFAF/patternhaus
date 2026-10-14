//! Patternhaus VST3 test plugins (M2/T3): one module (this cdylib, renamed
//! `ph-test-vst3.vst3`) with two single-component classes (`IComponent`, `IAudioProcessor` and
//! `IEditController` on one object), so the VST3 host is tested without third-party plugins:
//!
//! - [`SYNTH_CID`] "PH Test Synth VST3": an instrument with an event input; sine voices on
//!   note-on, released on note-off; parameter 0 "Volume" (normalized, default 0.5) and
//!   parameter 1 "Octave" (a list: Low / Mid / High, default Mid), which the plugin also maps
//!   to MIDI CC 1 through `IMidiMapping`.
//! - [`GAIN_CID`] "PH Test Gain VST3": a stereo effect with [`GAIN_LATENCY`] samples of latency;
//!   parameter 0 "Gain" (plain gain = 2 x normalized, default 0.5 = unity).
//!
//! State: `PHV3` then the two parameter values (normalized, f64 little endian). No editor.
//! Deterministic and block-size independent. Written with the `vst3` crate's bindings and COM
//! helpers (MIT OR Apache-2.0).

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]
// the bindings' enum types differ between platforms, so some casts are no-ops on one of them
#![allow(clippy::unnecessary_cast)]

use std::ffi::{c_char, c_void};
use std::sync::Mutex;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

pub const SYNTH_CID: TUID = uid(0x5048_5453, 0x594E_5448, 0x5653_5433, 0x0000_0001);
pub const GAIN_CID: TUID = uid(0x5048_5447, 0x4149_4E56, 0x5653_5433, 0x0000_0002);
pub const GAIN_LATENCY: u32 = 16;
const VOICES: usize = 8;

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    for (d, s) in dst.iter_mut().zip(src.bytes().chain(std::iter::once(0))) {
        *d = s as c_char;
    }
    let len = dst.len();
    if src.len() >= len
        && let Some(last) = dst.last_mut()
    {
        *last = 0;
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s as TChar;
        n += 1;
    }
    let n = n.min(dst.len() - 1);
    dst[n] = 0;
}

#[derive(Copy, Clone, PartialEq)]
enum Kind {
    Synth,
    Gain,
}

#[derive(Copy, Clone, Default)]
struct Voice {
    on: bool,
    released: bool,
    id: i32,
    pitch: i16,
    phase: f64,
    vel: f64,
    env: f64,
}

struct Inner {
    params: [f64; 2],
    sr: f64,
    voices: [Voice; VOICES],
    delay: [[f32; GAIN_LATENCY as usize]; 2],
    pos: usize,
}

struct Plugin {
    kind: Kind,
    inner: Mutex<Inner>,
}

impl Class for Plugin {
    type Interfaces = (IComponent, IAudioProcessor, IEditController, IMidiMapping);
}

impl Plugin {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            inner: Mutex::new(Inner {
                params: [0.5, if kind == Kind::Synth { 0.5 } else { 0.0 }],
                sr: 48_000.0,
                voices: [Voice::default(); VOICES],
                delay: [[0.0; GAIN_LATENCY as usize]; 2],
                pos: 0,
            }),
        }
    }

    fn n_params(&self) -> i32 {
        match self.kind {
            Kind::Synth => 2,
            Kind::Gain => 1,
        }
    }
}

impl IPluginBaseTrait for Plugin {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Plugin {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        match (media as MediaTypes, dir as BusDirections, self.kind) {
            (MediaTypes_::kAudio, BusDirections_::kOutput, _) => 1,
            (MediaTypes_::kAudio, BusDirections_::kInput, Kind::Gain) => 1,
            (MediaTypes_::kEvent, BusDirections_::kInput, Kind::Synth) => 1,
            _ => 0,
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index != 0 || unsafe { self.getBusCount(media, dir) } == 0 {
            return kInvalidArgument;
        }
        let bus = unsafe { &mut *bus };
        bus.mediaType = media;
        bus.direction = dir;
        bus.channelCount = if media as MediaTypes == MediaTypes_::kAudio {
            2
        } else {
            1
        };
        copy_wstring(
            if dir as BusDirections == BusDirections_::kInput {
                "In"
            } else {
                "Out"
            },
            &mut bus.name,
        );
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        let mut g = self.inner.lock().unwrap();
        g.voices = [Voice::default(); VOICES];
        g.delay = [[0.0; GAIN_LATENCY as usize]; 2];
        g.pos = 0;
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else {
            return kInvalidArgument;
        };
        let mut buf = [0u8; 20];
        let mut n = 0;
        unsafe { s.read(buf.as_mut_ptr() as *mut c_void, 20, &mut n) };
        if n != 20 || &buf[..4] != b"PHV3" {
            return kResultFalse;
        }
        let mut g = self.inner.lock().unwrap();
        for i in 0..2 {
            g.params[i] = f64::from_le_bytes(buf[4 + 8 * i..12 + 8 * i].try_into().unwrap());
        }
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else {
            return kInvalidArgument;
        };
        let g = self.inner.lock().unwrap();
        let mut buf = b"PHV3".to_vec();
        for p in g.params {
            buf.extend_from_slice(&p.to_le_bytes());
        }
        let mut n = 0;
        unsafe { s.write(buf.as_mut_ptr() as *mut c_void, buf.len() as i32, &mut n) };
        kResultOk
    }
}

impl IAudioProcessorTrait for Plugin {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let want_ins = if self.kind == Kind::Gain { 1 } else { 0 };
        if num_ins != want_ins || num_outs != 1 {
            return kResultFalse;
        }
        let ins = unsafe { std::slice::from_raw_parts(inputs, num_ins as usize) };
        if ins.iter().any(|a| *a != SpeakerArr::kStereo)
            || unsafe { *outputs } != SpeakerArr::kStereo
        {
            return kResultFalse;
        }
        kResultOk
    }
    unsafe fn getBusArrangement(
        &self,
        _dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        if self.kind == Kind::Gain {
            GAIN_LATENCY
        } else {
            0
        }
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        self.inner.lock().unwrap().sr = unsafe { (*setup).sampleRate };
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = unsafe { &*data };
        let n = d.numSamples.max(0) as usize;
        let mut g = self.inner.lock().unwrap();
        // parameter changes: (offset, param, value) applied sample-accurately
        let mut changes: Vec<(i32, u32, f64)> = Vec::new();
        if let Some(pc) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
            for i in 0..unsafe { pc.getParameterCount() } {
                if let Some(q) = unsafe { ComRef::from_raw(pc.getParameterData(i)) } {
                    let id = unsafe { q.getParameterId() };
                    for k in 0..unsafe { q.getPointCount() } {
                        let (mut o, mut v) = (0, 0.0);
                        if unsafe { q.getPoint(k, &mut o, &mut v) } == kResultOk {
                            changes.push((o, id, v));
                        }
                    }
                }
            }
        }
        let mut events: Vec<Event> = Vec::new();
        if let Some(el) = unsafe { ComRef::from_raw(d.inputEvents) } {
            for i in 0..unsafe { el.getEventCount() } {
                let mut e: Event = unsafe { std::mem::zeroed() };
                if unsafe { el.getEvent(i, &mut e) } == kResultOk {
                    events.push(e);
                }
            }
        }
        changes.sort_by_key(|c| c.0);
        events.sort_by_key(|e| e.sampleOffset);
        if d.numOutputs < 1 || d.outputs.is_null() {
            return kResultOk;
        }
        let out = unsafe { &*d.outputs };
        if out.numChannels < 2 {
            return kResultOk;
        }
        let outs = unsafe { std::slice::from_raw_parts(out.__field0.channelBuffers32, 2) };
        let (ol, or) = unsafe {
            (
                std::slice::from_raw_parts_mut(outs[0], n),
                std::slice::from_raw_parts_mut(outs[1], n),
            )
        };
        let ins = if self.kind == Kind::Gain && d.numInputs >= 1 && !d.inputs.is_null() {
            let b = unsafe { &*d.inputs };
            let chans = unsafe { std::slice::from_raw_parts(b.__field0.channelBuffers32, 2) };
            Some(unsafe {
                (
                    std::slice::from_raw_parts(chans[0], n),
                    std::slice::from_raw_parts(chans[1], n),
                )
            })
        } else {
            None
        };
        let (mut ci, mut ei) = (0, 0);
        for i in 0..n {
            while ci < changes.len() && changes[ci].0 as usize <= i {
                let (_, id, v) = changes[ci];
                if (id as usize) < 2 {
                    g.params[id as usize] = v.clamp(0.0, 1.0);
                }
                ci += 1;
            }
            while ei < events.len() && events[ei].sampleOffset as usize <= i {
                let e = events[ei];
                match e.r#type as u32 {
                    x if x == Event_::EventTypes_::kNoteOnEvent as u32 => {
                        let on = unsafe { e.__field0.noteOn };
                        if let Some(v) = g.voices.iter_mut().find(|v| !v.on) {
                            *v = Voice {
                                on: true,
                                released: false,
                                id: on.noteId,
                                pitch: on.pitch,
                                phase: 0.0,
                                vel: on.velocity as f64,
                                env: 0.0,
                            };
                        }
                    }
                    x if x == Event_::EventTypes_::kNoteOffEvent as u32 => {
                        let off = unsafe { e.__field0.noteOff };
                        for v in g.voices.iter_mut() {
                            if v.on
                                && (v.id == off.noteId || (off.noteId < 0 && v.pitch == off.pitch))
                            {
                                v.released = true;
                            }
                        }
                    }
                    _ => {}
                }
                ei += 1;
            }
            match self.kind {
                Kind::Synth => {
                    let sr = g.sr;
                    let vol = g.params[0];
                    let octave = ((g.params[1] * 2.0).round() - 1.0) * 12.0;
                    let mut acc = 0.0;
                    for v in g.voices.iter_mut().filter(|v| v.on) {
                        let f = 440.0 * ((v.pitch as f64 - 69.0 + octave) / 12.0).exp2();
                        acc += (v.phase * std::f64::consts::TAU).sin() * v.env * v.vel;
                        v.phase = (v.phase + f / sr).fract();
                        if v.released {
                            v.env -= 1.0 / 480.0;
                            if v.env <= 0.0 {
                                v.on = false;
                            }
                        } else {
                            v.env = (v.env + 1.0 / 64.0).min(1.0);
                        }
                    }
                    ol[i] = (acc * vol) as f32;
                    or[i] = ol[i];
                }
                Kind::Gain => {
                    let gain = (g.params[0] * 2.0) as f32;
                    let pos = g.pos;
                    let (l, r) = ins.map_or((0.0, 0.0), |(a, b)| (a[i], b[i]));
                    ol[i] = g.delay[0][pos];
                    or[i] = g.delay[1][pos];
                    g.delay[0][pos] = l * gain;
                    g.delay[1][pos] = r * gain;
                    g.pos = (pos + 1) % GAIN_LATENCY as usize;
                }
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        if self.kind == Kind::Gain {
            GAIN_LATENCY
        } else {
            480
        }
    }
}

impl IEditControllerTrait for Plugin {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        self.n_params()
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        if index < 0 || index >= self.n_params() {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.id = index as u32;
        let (name, steps) = match (self.kind, index) {
            (Kind::Synth, 0) => ("Volume", 0),
            (Kind::Synth, _) => ("Octave", 2),
            (Kind::Gain, _) => ("Gain", 0),
        };
        copy_wstring(name, &mut info.title);
        copy_wstring(name, &mut info.shortTitle);
        copy_wstring("", &mut info.units);
        info.stepCount = steps;
        info.defaultNormalizedValue = 0.5;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32
            | if steps > 0 {
                ParameterInfo_::ParameterFlags_::kIsList as i32
            } else {
                0
            };
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, v: f64, string: *mut String128) -> tresult {
        let s = match (self.kind, id) {
            (Kind::Synth, 1) => {
                ["Low", "Mid", "High"][(v * 2.0).round().clamp(0.0, 2.0) as usize].to_string()
            }
            (Kind::Gain, 0) => format!("{:.2}x", v * 2.0),
            _ => format!("{v:.3}"),
        };
        copy_wstring(&s, unsafe { &mut *string });
        kResultOk
    }
    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, _id: u32, v: f64) -> f64 {
        v
    }
    unsafe fn plainParamToNormalized(&self, _id: u32, v: f64) -> f64 {
        v
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.inner
            .lock()
            .unwrap()
            .params
            .get(id as usize)
            .copied()
            .unwrap_or(0.0)
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        if let Some(p) = self.inner.lock().unwrap().params.get_mut(id as usize) {
            *p = v.clamp(0.0, 1.0);
        }
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _h: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

impl IMidiMappingTrait for Plugin {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: i32,
        _channel: i16,
        cc: CtrlNumber,
        id: *mut ParamID,
    ) -> tresult {
        if self.kind == Kind::Synth && bus == 0 && cc == 1 {
            unsafe { *id = 1 };
            kResultOk
        } else {
            kResultFalse
        }
    }
}

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

fn class_info(index: i32) -> Option<(TUID, &'static str, &'static str)> {
    match index {
        0 => Some((SYNTH_CID, "PH Test Synth VST3", "Instrument|Synth")),
        1 => Some((GAIN_CID, "PH Test Gain VST3", "Fx")),
        _ => None,
    }
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = unsafe { &mut *info };
        copy_cstring("Patternhaus", &mut info.vendor);
        copy_cstring("https://github.com/McChaffeeFAF/patternhaus", &mut info.url);
        copy_cstring("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        2
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some((cid, name, _)) = class_info(index) else {
            return kInvalidArgument;
        };
        let info = unsafe { &mut *info };
        info.cid = cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_cstring(name, &mut info.name);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let kind = match unsafe { *(cid as *const TUID) } {
            SYNTH_CID => Kind::Synth,
            GAIN_CID => Kind::Gain,
            _ => return kInvalidArgument,
        };
        let instance = ComWrapper::new(Plugin::new(kind))
            .to_com_ptr::<FUnknown>()
            .unwrap();
        let ptr = instance.as_ptr();
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj) }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some((cid, name, subs)) = class_info(index) else {
            return kInvalidArgument;
        };
        let info = unsafe { &mut *info };
        info.cid = cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_cstring(name, &mut info.name);
        info.classFlags = 0;
        copy_cstring(subs, &mut info.subCategories);
        copy_cstring("Patternhaus", &mut info.vendor);
        copy_cstring("1.0.0", &mut info.version);
        copy_cstring("VST 3.7", &mut info.sdkVersion);
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "system" fn ModuleEntry(_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "system" fn ModuleExit() -> bool {
    true
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .unwrap()
        .into_raw()
}
