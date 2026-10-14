//! Patternhaus test plugins: one CLAP file (this cdylib, renamed `ph-test-plugins.clap`) with
//! three plugins, so CI never depends on third-party plugins (M2/T3):
//!
//! - [`SYNTH_ID`] `dev.patternhaus.test.synth`: a polyphonic sine/saw synth. Notes as CLAP
//!   note events or MIDI, MIDI pitch bend (±2 semitones) and mod wheel (CC 1 halves the
//!   level), note expressions (volume, tuning). Deterministic and block-size independent, so
//!   the engine's live output must match the offline render bit for bit.
//! - [`GAIN_ID`] `dev.patternhaus.test.gain`: a stereo gain with [`GAIN_LATENCY`] samples of
//!   latency (a delay line), to test latency reporting.
//! - [`CRASH_ID`] `dev.patternhaus.test.crash`: a pass-through whose `crash` parameter makes
//!   it write to an invalid address (a real access violation / SIGSEGV, not a Rust panic)
//!   while processing (1) or while saving its state (2).
//!
//! The plugin is written directly against the CLAP C ABI (`clap-sys`), not a framework, so the
//! host is tested against plain C-style plugin code. Every callback that can crash on demand is
//! `extern "C-unwind"` (cast to the C ABI type CLAP declares), so a hardware fault in it can be
//! unwound by the host's structured exception handler without hitting Rust's abort-on-unwind
//! guard. Nothing here panics.
//!
//! No GUI (tests never open windows).

#![allow(clippy::missing_safety_doc)]

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::ext::state::*;
use clap_sys::ext::tail::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::*;
use clap_sys::process::*;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use std::ffi::{CStr, c_char, c_void};

pub const SYNTH_ID: &str = "dev.patternhaus.test.synth";
pub const GAIN_ID: &str = "dev.patternhaus.test.gain";
pub const CRASH_ID: &str = "dev.patternhaus.test.crash";
/// Latency of the test gain, in samples.
pub const GAIN_LATENCY: u32 = 32;
/// Voices of the test synth.
pub const VOICES: usize = 16;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Synth,
    Gain,
    Crash,
}

struct Ptrs<const N: usize>([*const c_char; N]);
// SAFETY: the pointers are to static C strings.
unsafe impl<const N: usize> Sync for Ptrs<N> {}

static SYNTH_FEATURES: Ptrs<4> = Ptrs([
    c"instrument".as_ptr(),
    c"synthesizer".as_ptr(),
    c"stereo".as_ptr(),
    std::ptr::null(),
]);
static FX_FEATURES: Ptrs<3> = Ptrs([
    c"audio-effect".as_ptr(),
    c"utility".as_ptr(),
    std::ptr::null(),
]);

const fn descriptor(
    id: &'static CStr,
    name: &'static CStr,
    features: *const *const c_char,
) -> clap_plugin_descriptor {
    clap_plugin_descriptor {
        clap_version: CLAP_VERSION,
        id: id.as_ptr(),
        name: name.as_ptr(),
        vendor: c"Patternhaus".as_ptr(),
        url: c"https://github.com/McChaffeeFAF/patternhaus".as_ptr(),
        manual_url: c"".as_ptr(),
        support_url: c"".as_ptr(),
        version: c"1.0.0".as_ptr(),
        description: c"Patternhaus test plugin (not for music)".as_ptr(),
        features,
    }
}

static DESCRIPTORS: [clap_plugin_descriptor; 3] = [
    descriptor(
        c"dev.patternhaus.test.synth",
        c"PH Test Synth",
        SYNTH_FEATURES.0.as_ptr(),
    ),
    descriptor(
        c"dev.patternhaus.test.gain",
        c"PH Test Gain",
        FX_FEATURES.0.as_ptr(),
    ),
    descriptor(
        c"dev.patternhaus.test.crash",
        c"PH Test Crash",
        FX_FEATURES.0.as_ptr(),
    ),
];

// ------------------------------------------------------------------------------- params

struct Param {
    name: &'static str,
    min: f64,
    max: f64,
    default: f64,
    stepped: bool,
    automatable: bool,
}

const SYNTH_PARAMS: &[Param] = &[
    Param {
        name: "Gain",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        stepped: false,
        automatable: true,
    },
    Param {
        name: "Wave",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        stepped: true,
        automatable: true,
    },
    Param {
        name: "Release",
        min: 0.001,
        max: 2.0,
        default: 0.05,
        stepped: false,
        automatable: true,
    },
];
const GAIN_PARAMS: &[Param] = &[Param {
    name: "Gain",
    min: 0.0,
    max: 2.0,
    default: 1.0,
    stepped: false,
    automatable: true,
}];
const CRASH_PARAMS: &[Param] = &[Param {
    name: "Crash",
    min: 0.0,
    max: 2.0,
    default: 0.0,
    stepped: true,
    automatable: false,
}];

fn params_of(kind: Kind) -> &'static [Param] {
    match kind {
        Kind::Synth => SYNTH_PARAMS,
        Kind::Gain => GAIN_PARAMS,
        Kind::Crash => CRASH_PARAMS,
    }
}

// ------------------------------------------------------------------------------- plugin

#[derive(Copy, Clone, Default)]
struct Voice {
    on: bool,
    released: bool,
    note_id: i32,
    channel: i16,
    key: i16,
    velocity: f64,
    phase: f64,
    /// Envelope level 0..1.
    env: f64,
    /// Note expressions.
    volume: f64,
    tuning: f64,
}

struct Plugin {
    clap: clap_plugin,
    kind: Kind,
    sample_rate: f64,
    params: [f64; 3],
    voices: [Voice; VOICES],
    /// MIDI pitch bend, -1..1.
    bend: f64,
    /// Mod wheel 0..1.
    modwheel: f64,
    delay: [[f32; GAIN_LATENCY as usize]; 2],
    delay_pos: usize,
    processing: bool,
}

unsafe fn this<'a>(p: *const clap_plugin) -> &'a mut Plugin {
    unsafe { &mut *((*p).plugin_data as *mut Plugin) }
}

/// A real hardware fault: a store to an unmapped address.
#[inline(never)]
fn crash_now() {
    let p = std::hint::black_box(8usize) as *mut u32;
    // SAFETY: deliberately not safe; this is the crash-on-demand switch.
    unsafe { std::ptr::write_volatile(p, 0xdead) };
}

impl Plugin {
    fn new(kind: Kind) -> Box<Self> {
        let mut params = [0.0; 3];
        for (i, p) in params_of(kind).iter().enumerate() {
            params[i] = p.default;
        }
        let desc = &DESCRIPTORS[kind as usize];
        let mut b = Box::new(Plugin {
            clap: clap_plugin {
                desc,
                plugin_data: std::ptr::null_mut(),
                init: Some(init),
                destroy: Some(destroy),
                activate: Some(activate),
                deactivate: Some(deactivate),
                start_processing: Some(start_processing),
                stop_processing: Some(stop_processing),
                reset: Some(reset),
                // SAFETY: identical signature; only the unwind ABI differs (see the crate docs)
                process: Some(unsafe {
                    std::mem::transmute::<
                        unsafe extern "C-unwind" fn(
                            *const clap_plugin,
                            *const clap_process,
                        ) -> clap_process_status,
                        unsafe extern "C" fn(
                            *const clap_plugin,
                            *const clap_process,
                        ) -> clap_process_status,
                    >(process)
                }),
                get_extension: Some(get_extension),
                on_main_thread: Some(on_main_thread),
            },
            kind,
            sample_rate: 48_000.0,
            params,
            voices: [Voice::default(); VOICES],
            bend: 0.0,
            modwheel: 0.0,
            delay: [[0.0; GAIN_LATENCY as usize]; 2],
            delay_pos: 0,
            processing: false,
        });
        let data: *mut Plugin = &mut *b;
        b.clap.plugin_data = data as *mut c_void;
        b
    }

    fn set_param(&mut self, id: clap_id, value: f64) {
        if let Some(p) = params_of(self.kind).get(id as usize) {
            let mut v = value.clamp(p.min, p.max);
            if p.stepped {
                v = v.round();
            }
            self.params[id as usize] = v;
        }
    }

    fn clear(&mut self) {
        self.voices = [Voice::default(); VOICES];
        self.delay = [[0.0; GAIN_LATENCY as usize]; 2];
        self.delay_pos = 0;
        self.bend = 0.0;
        self.modwheel = 0.0;
    }

    fn note_on(&mut self, note_id: i32, channel: i16, key: i16, velocity: f64) {
        // a free voice, else the quietest
        let slot = self.voices.iter().position(|v| !v.on).unwrap_or_else(|| {
            let mut best = 0;
            for (i, v) in self.voices.iter().enumerate() {
                if v.env < self.voices[best].env {
                    best = i;
                }
            }
            best
        });
        self.voices[slot] = Voice {
            on: true,
            released: false,
            note_id,
            channel,
            key,
            velocity,
            phase: 0.0,
            env: 0.0,
            volume: 1.0,
            tuning: 0.0,
        };
    }

    fn matches(v: &Voice, note_id: i32, channel: i16, key: i16) -> bool {
        v.on && (note_id < 0 || v.note_id == note_id)
            && (channel < 0 || v.channel == channel)
            && (key < 0 || v.key == key)
    }

    fn note_off(&mut self, note_id: i32, channel: i16, key: i16) {
        for v in self.voices.iter_mut() {
            if Self::matches(v, note_id, channel, key) {
                v.released = true;
            }
        }
    }

    unsafe fn handle_event(&mut self, h: *const clap_event_header) {
        let h = unsafe { &*h };
        if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
            return;
        }
        match h.type_ {
            CLAP_EVENT_PARAM_VALUE => {
                let e = unsafe { &*(h as *const _ as *const clap_event_param_value) };
                self.set_param(e.param_id, e.value);
            }
            CLAP_EVENT_NOTE_ON if self.kind == Kind::Synth => {
                let e = unsafe { &*(h as *const _ as *const clap_event_note) };
                self.note_on(e.note_id, e.channel, e.key, e.velocity);
            }
            CLAP_EVENT_NOTE_OFF if self.kind == Kind::Synth => {
                let e = unsafe { &*(h as *const _ as *const clap_event_note) };
                self.note_off(e.note_id, e.channel, e.key);
            }
            CLAP_EVENT_NOTE_CHOKE if self.kind == Kind::Synth => {
                let e = unsafe { &*(h as *const _ as *const clap_event_note) };
                for v in self.voices.iter_mut() {
                    if Self::matches(v, e.note_id, e.channel, e.key) {
                        v.on = false;
                    }
                }
            }
            CLAP_EVENT_NOTE_EXPRESSION if self.kind == Kind::Synth => {
                let e = unsafe { &*(h as *const _ as *const clap_event_note_expression) };
                for v in self.voices.iter_mut() {
                    if Self::matches(v, e.note_id, e.channel, e.key) {
                        match e.expression_id {
                            CLAP_NOTE_EXPRESSION_VOLUME => v.volume = e.value.clamp(0.0, 4.0),
                            CLAP_NOTE_EXPRESSION_TUNING => v.tuning = e.value.clamp(-120.0, 120.0),
                            _ => {}
                        }
                    }
                }
            }
            CLAP_EVENT_MIDI if self.kind == Kind::Synth => {
                let e = unsafe { &*(h as *const _ as *const clap_event_midi) };
                let [status, d1, d2] = e.data;
                let channel = (status & 0x0f) as i16;
                match status & 0xf0 {
                    0x90 if d2 > 0 => self.note_on(-1, channel, d1 as i16, d2 as f64 / 127.0),
                    0x80 | 0x90 => self.note_off(-1, channel, d1 as i16),
                    0xb0 if d1 == 1 => self.modwheel = d2 as f64 / 127.0,
                    0xe0 => {
                        let v = ((d2 as i32) << 7 | d1 as i32) - 8192;
                        self.bend = (v as f64 / 8192.0).clamp(-1.0, 1.0);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Renders frames `from..to` of the synth into `l`, `r`.
    fn render_synth(&mut self, l: &mut [f32], r: &mut [f32], from: usize, to: usize) {
        let gain = self.params[0];
        let saw = self.params[1] >= 0.5;
        let release_step = 1.0 / (self.params[2] * self.sample_rate).max(1.0);
        let attack_step = 1.0 / 64.0;
        let level = gain * (1.0 - 0.5 * self.modwheel);
        for i in from..to {
            let mut acc = 0.0f64;
            for v in self.voices.iter_mut().filter(|v| v.on) {
                let semis = v.key as f64 - 69.0 + 2.0 * self.bend + v.tuning;
                let freq = 440.0 * (semis / 12.0).exp2();
                let s = if saw {
                    2.0 * v.phase - 1.0
                } else {
                    (v.phase * std::f64::consts::TAU).sin()
                };
                v.phase += freq / self.sample_rate;
                v.phase -= v.phase.floor();
                if v.released {
                    v.env -= release_step;
                    if v.env <= 0.0 {
                        v.env = 0.0;
                        v.on = false;
                    }
                } else if v.env < 1.0 {
                    v.env = (v.env + attack_step).min(1.0);
                }
                acc += s * v.env * v.velocity * v.volume;
            }
            let out = (acc * level) as f32;
            l[i] = out;
            r[i] = out;
        }
    }

    fn effect(&mut self, inp: [&[f32]; 2], out: [&mut [f32]; 2], from: usize, to: usize) {
        let [ol, or] = out;
        let outs = [ol, or];
        match self.kind {
            Kind::Gain => {
                let g = self.params[0] as f32;
                for i in from..to {
                    for c in 0..2 {
                        let delayed = self.delay[c][self.delay_pos];
                        self.delay[c][self.delay_pos] = inp[c][i] * g;
                        outs[c][i] = delayed;
                    }
                    self.delay_pos = (self.delay_pos + 1) % GAIN_LATENCY as usize;
                }
            }
            _ => {
                for c in 0..2 {
                    outs[c][from..to].copy_from_slice(&inp[c][from..to]);
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------- plugin ABI

unsafe extern "C" fn init(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    let data = unsafe { (*p).plugin_data as *mut Plugin };
    drop(unsafe { Box::from_raw(data) });
}

unsafe extern "C" fn activate(p: *const clap_plugin, sr: f64, _min: u32, _max: u32) -> bool {
    let me = unsafe { this(p) };
    me.sample_rate = sr;
    me.clear();
    true
}

unsafe extern "C" fn deactivate(_: *const clap_plugin) {}

unsafe extern "C" fn start_processing(p: *const clap_plugin) -> bool {
    unsafe { this(p) }.processing = true;
    true
}

unsafe extern "C" fn stop_processing(p: *const clap_plugin) {
    unsafe { this(p) }.processing = false;
}

unsafe extern "C" fn reset(p: *const clap_plugin) {
    unsafe { this(p) }.clear();
}

/// Channel `c` of port `port`, or `None` (missing port or 64-bit only).
unsafe fn channel<'a>(
    bufs: *const clap_audio_buffer,
    count: u32,
    port: u32,
    c: u32,
    frames: usize,
) -> Option<&'a mut [f32]> {
    if bufs.is_null() || port >= count {
        return None;
    }
    let b = unsafe { &*bufs.add(port as usize) };
    if b.data32.is_null() || b.channel_count == 0 {
        return None;
    }
    let c = c.min(b.channel_count - 1);
    let ptr = unsafe { *b.data32.add(c as usize) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts_mut(ptr, frames) })
}

unsafe extern "C-unwind" fn process(
    p: *const clap_plugin,
    pr: *const clap_process,
) -> clap_process_status {
    let me = unsafe { this(p) };
    let pr = unsafe { &*pr };
    if me.kind == Kind::Crash && me.params[0] == 1.0 {
        crash_now();
    }
    let frames = pr.frames_count as usize;
    let (Some(ol), Some(or)) = (
        unsafe { channel(pr.audio_outputs, pr.audio_outputs_count, 0, 0, frames) },
        unsafe { channel(pr.audio_outputs, pr.audio_outputs_count, 0, 1, frames) },
    ) else {
        return CLAP_PROCESS_ERROR;
    };
    let inputs = if me.kind == Kind::Synth {
        None
    } else {
        match (
            unsafe { channel(pr.audio_inputs, pr.audio_inputs_count, 0, 0, frames) },
            unsafe { channel(pr.audio_inputs, pr.audio_inputs_count, 0, 1, frames) },
        ) {
            (Some(a), Some(b)) => Some((a as &[f32], b as &[f32])),
            _ => None,
        }
    };
    let events = unsafe { &*pr.in_events };
    let n = unsafe { events.size.map_or(0, |f| f(events)) };
    let mut cursor = 0usize;
    let mut e = 0u32;
    loop {
        // the next event time (or the end of the block)
        let next = if e < n {
            let h = unsafe { events.get.map_or(std::ptr::null(), |f| f(events, e)) };
            if h.is_null() {
                e += 1;
                continue;
            }
            (unsafe { (*h).time } as usize).min(frames)
        } else {
            frames
        };
        if next > cursor {
            match me.kind {
                Kind::Synth => me.render_synth(ol, or, cursor, next),
                _ => match inputs {
                    Some((a, b)) => me.effect([a, b], [ol, or], cursor, next),
                    None => {
                        ol[cursor..next].fill(0.0);
                        or[cursor..next].fill(0.0);
                    }
                },
            }
            cursor = next;
        }
        if e >= n {
            break;
        }
        let h = unsafe { events.get.map_or(std::ptr::null(), |f| f(events, e)) };
        if !h.is_null() {
            unsafe { me.handle_event(h) };
        }
        e += 1;
    }
    if me.kind == Kind::Crash && me.params[0] == 1.0 {
        // switched on by an event in this block
        crash_now();
    }
    match me.kind {
        Kind::Synth if !me.voices.iter().any(|v| v.on) => CLAP_PROCESS_SLEEP,
        _ => CLAP_PROCESS_CONTINUE,
    }
}

unsafe extern "C" fn on_main_thread(_: *const clap_plugin) {}

// ------------------------------------------------------------------------------- extensions

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};
static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};
static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};
static LATENCY: clap_plugin_latency = clap_plugin_latency {
    get: Some(latency_get),
};
static TAIL: clap_plugin_tail = clap_plugin_tail {
    get: Some(tail_get),
};

struct State(clap_plugin_state);
// SAFETY: function pointers only.
unsafe impl Sync for State {}
static STATE: State = State(clap_plugin_state {
    // SAFETY: identical signature; only the unwind ABI differs (see the crate docs)
    save: Some(unsafe {
        std::mem::transmute::<
            unsafe extern "C-unwind" fn(*const clap_plugin, *const clap_ostream) -> bool,
            unsafe extern "C" fn(*const clap_plugin, *const clap_ostream) -> bool,
        >(state_save)
    }),
    load: Some(state_load),
});

unsafe extern "C" fn get_extension(p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let me = unsafe { this(p) };
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS && me.kind == Kind::Synth {
        &NOTE_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE.0 as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_TAIL {
        &TAIL as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

fn write_name(dst: &mut [c_char], name: &str) {
    dst.fill(0);
    let room = dst.len().saturating_sub(1);
    for (d, b) in dst.iter_mut().zip(name.bytes().take(room)) {
        *d = b as c_char;
    }
}

unsafe extern "C" fn audio_ports_count(p: *const clap_plugin, is_input: bool) -> u32 {
    match (unsafe { this(p) }.kind, is_input) {
        (Kind::Synth, true) => 0,
        _ => 1,
    }
}

unsafe extern "C" fn audio_ports_get(
    p: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if index != 0 || unsafe { audio_ports_count(p, is_input) } == 0 {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = if is_input { 0 } else { 1 };
    write_name(&mut info.name, if is_input { "In" } else { "Out" });
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = if unsafe { this(p) }.kind == Kind::Synth {
        clap_sys::id::CLAP_INVALID_ID
    } else if is_input {
        1
    } else {
        0
    };
    true
}

unsafe extern "C" fn note_ports_count(_: *const clap_plugin, is_input: bool) -> u32 {
    is_input as u32
}

unsafe extern "C" fn note_ports_get(
    _: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if index != 0 || !is_input {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    write_name(&mut info.name, "Notes");
    true
}

unsafe extern "C" fn params_count(p: *const clap_plugin) -> u32 {
    params_of(unsafe { this(p) }.kind).len() as u32
}

unsafe extern "C" fn params_get_info(
    p: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    let Some(param) = params_of(unsafe { this(p) }.kind).get(index as usize) else {
        return false;
    };
    let info = unsafe { &mut *info };
    info.id = index;
    info.flags = 0;
    if param.stepped {
        info.flags |= CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM;
    }
    if param.automatable {
        info.flags |= CLAP_PARAM_IS_AUTOMATABLE;
    }
    info.cookie = std::ptr::null_mut();
    write_name(&mut info.name, param.name);
    write_name(&mut info.module, "");
    info.min_value = param.min;
    info.max_value = param.max;
    info.default_value = param.default;
    true
}

unsafe extern "C" fn params_get_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    let me = unsafe { this(p) };
    if (id as usize) < params_of(me.kind).len() {
        unsafe { *out = me.params[id as usize] };
        true
    } else {
        false
    }
}

fn value_text(kind: Kind, id: clap_id, v: f64) -> Option<String> {
    Some(match (kind, id) {
        (Kind::Synth, 1) => (if v >= 0.5 { "Saw" } else { "Sine" }).to_string(),
        (Kind::Synth, 2) => format!("{:.0} ms", v * 1000.0),
        (Kind::Crash, 0) => match v.round() as i32 {
            1 => "Crash while processing",
            2 => "Crash while saving",
            _ => "Off",
        }
        .to_string(),
        (_, 0) => format!("{v:.3}"),
        _ => return None,
    })
}

unsafe extern "C" fn params_value_to_text(
    p: *const clap_plugin,
    id: clap_id,
    value: f64,
    out: *mut c_char,
    size: u32,
) -> bool {
    let Some(text) = value_text(unsafe { this(p) }.kind, id, value) else {
        return false;
    };
    if size == 0 {
        return false;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(out, size as usize) };
    write_name(dst, &text);
    true
}

unsafe extern "C" fn params_text_to_value(
    p: *const clap_plugin,
    id: clap_id,
    text: *const c_char,
    out: *mut f64,
) -> bool {
    let me = unsafe { this(p) };
    let Ok(text) = unsafe { CStr::from_ptr(text) }.to_str() else {
        return false;
    };
    let v = match (me.kind, id, text) {
        (Kind::Synth, 1, "Sine") => 0.0,
        (Kind::Synth, 1, "Saw") => 1.0,
        _ => match text.trim_end_matches(" ms").trim().parse::<f64>() {
            Ok(v) if me.kind == Kind::Synth && id == 2 => v / 1000.0,
            Ok(v) => v,
            Err(_) => return false,
        },
    };
    unsafe { *out = v };
    true
}

unsafe extern "C" fn params_flush(
    p: *const clap_plugin,
    input: *const clap_input_events,
    _out: *const clap_output_events,
) {
    let me = unsafe { this(p) };
    if input.is_null() {
        return;
    }
    let events = unsafe { &*input };
    let n = unsafe { events.size.map_or(0, |f| f(events)) };
    for i in 0..n {
        let h = unsafe { events.get.map_or(std::ptr::null(), |f| f(events, i)) };
        if !h.is_null() && unsafe { (*h).type_ } == CLAP_EVENT_PARAM_VALUE {
            unsafe { me.handle_event(h) };
        }
    }
}

unsafe extern "C" fn latency_get(p: *const clap_plugin) -> u32 {
    if unsafe { this(p) }.kind == Kind::Gain {
        GAIN_LATENCY
    } else {
        0
    }
}

unsafe extern "C" fn tail_get(p: *const clap_plugin) -> u32 {
    let me = unsafe { this(p) };
    match me.kind {
        Kind::Synth => (me.params[2] * me.sample_rate).ceil() as u32,
        Kind::Gain => GAIN_LATENCY,
        Kind::Crash => 0,
    }
}

/// State: `PHTP1` then one `index=value` line per parameter.
unsafe extern "C-unwind" fn state_save(p: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let me = unsafe { this(p) };
    if me.kind == Kind::Crash && me.params[0] == 2.0 {
        crash_now();
    }
    let mut text = String::from("PHTP1\n");
    for (i, _) in params_of(me.kind).iter().enumerate() {
        text.push_str(&format!("{i}={}\n", me.params[i]));
    }
    let stream = unsafe { &*stream };
    let Some(write) = stream.write else {
        return false;
    };
    let bytes = text.as_bytes();
    let mut done = 0usize;
    while done < bytes.len() {
        let n = unsafe {
            write(
                stream,
                bytes[done..].as_ptr() as *const c_void,
                (bytes.len() - done) as u64,
            )
        };
        if n <= 0 {
            return false;
        }
        done += n as usize;
    }
    true
}

unsafe extern "C" fn state_load(p: *const clap_plugin, stream: *const clap_istream) -> bool {
    let me = unsafe { this(p) };
    let stream = unsafe { &*stream };
    let Some(read) = stream.read else {
        return false;
    };
    let mut bytes = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = unsafe { read(stream, buf.as_mut_ptr() as *mut c_void, buf.len() as u64) };
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n as usize]);
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return false;
    };
    let mut lines = text.lines();
    if lines.next() != Some("PHTP1") {
        return false;
    }
    for line in lines {
        let Some((k, v)) = line.split_once('=') else {
            return false;
        };
        match (k.parse::<u32>(), v.parse::<f64>()) {
            (Ok(k), Ok(v)) => me.set_param(k, v),
            _ => return false,
        }
    }
    true
}

// ------------------------------------------------------------------------------- factory, entry

unsafe extern "C" fn factory_count(_: *const clap_plugin_factory) -> u32 {
    DESCRIPTORS.len() as u32
}

unsafe extern "C" fn factory_descriptor(
    _: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    DESCRIPTORS
        .get(index as usize)
        .map_or(std::ptr::null(), |d| d as *const _)
}

unsafe extern "C" fn factory_create(
    _: *const clap_plugin_factory,
    _host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    let id = unsafe { CStr::from_ptr(id) };
    let kind = match id.to_bytes() {
        b"dev.patternhaus.test.synth" => Kind::Synth,
        b"dev.patternhaus.test.gain" => Kind::Gain,
        b"dev.patternhaus.test.crash" => Kind::Crash,
        _ => return std::ptr::null(),
    };
    let b = Plugin::new(kind);
    &Box::leak(b).clap
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

/// The CLAP entry point.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};
