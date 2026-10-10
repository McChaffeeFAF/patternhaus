//! `ph.synth`: Haus Synth, an analog-style subtractive synthesizer (DESIGN §7).
//!
//! Per voice: three band-limited oscillators (4-point PolyBLEP/BLAMP saw, pulse with PWM,
//! triangle, sine) with up to 8-voice unison, a sub-oscillator and seeded noise; two
//! filters (TPT ladder with drive, or TPT SVF; LP/HP/BP/notch) in series or parallel with
//! key tracking and envelope amount; three curved ADSRs; two LFOs (free or tempo-synced,
//! optionally retriggered); and an 8-slot modulation matrix.
//!
//! Voices render oversampled (2× live, 4× high quality) into a shared buffer that is
//! decimated once by linear-phase halfband stages, so oscillator residue and filter-drive
//! harmonics above 20 kHz never fold back. Modulation is evaluated on a 16-sample control
//! grid counted from each voice's note-on, which keeps rendering independent of block size.

use crate::common::{Params, balance, instrument_desc, stepped, switch, velocity_gain};
use ph_dsp::env::Adsr;
use ph_dsp::filter::{FilterMode, Ladder, Svf, res_to_k, tpt_g};
use ph_dsp::lfo::{Lfo, LfoShape, SYNC_DIVISIONS, sync_division_names};
use ph_dsp::osc::{Noise, Osc, SubOsc, Wave};
use ph_dsp::oversample::Oversampler;
use ph_dsp::voice::{SlotState, VoiceAllocator};
use ph_dsp::{Rng, db_to_lin, fast_tanh, key_to_hz};
use ph_node::*;

pub const UNISON_MAX: usize = 8;
pub const MAX_POLY: usize = 32;
const SPARE: usize = 8;
const CTRL: u32 = 16;
const FADE_S: f32 = 0.003;

// Parameter layout (indices into the descriptor; ids are the stable contract).
const OSC: usize = 0;
const OSC_N: usize = 6;
const O_WAVE: usize = 0;
const O_OCT: usize = 1;
const O_SEMI: usize = 2;
const O_FINE: usize = 3;
const O_LEVEL: usize = 4;
const O_PW: usize = 5;
const SUB_LEVEL: usize = 18;
const SUB_OCT: usize = 19;
const SUB_WAVE: usize = 20;
const NOISE_LEVEL: usize = 21;
const FLT: usize = 22;
const FLT_N: usize = 8;
const F_ON: usize = 0;
const F_MODEL: usize = 1;
const F_TYPE: usize = 2;
const F_CUTOFF: usize = 3;
const F_RES: usize = 4;
const F_DRIVE: usize = 5;
const F_KEYTRACK: usize = 6;
const F_ENV: usize = 7;
const ROUTING: usize = 38;
const ENV: usize = 39;
const ENV_N: usize = 5;
const LFO: usize = 54;
const LFO_N: usize = 5;
const L_SHAPE: usize = 0;
const L_RATE: usize = 1;
const L_SYNC: usize = 2;
const L_DIV: usize = 3;
const L_RETRIG: usize = 4;
const MOD: usize = 64;
const MOD_N: usize = 3;
const MOD_SLOTS: usize = 8;
const UNI_VOICES: usize = 88;
const UNI_DETUNE: usize = 89;
const UNI_SPREAD: usize = 90;
const GLIDE_MODE: usize = 91;
const GLIDE_TIME: usize = 92;
const VOICE_MODE: usize = 93;
const POLYPHONY: usize = 94;
const VEL_AMP: usize = 95;
const VOLUME: usize = 96;
const PAN: usize = 97;
const MOD_WHEEL: usize = 98;
const PITCH_BEND: usize = 99;
const BEND_RANGE: usize = 100;

pub const MOD_SOURCES: [&str; 10] = [
    "None",
    "LFO 1",
    "LFO 2",
    "Amp Env",
    "Filter Env",
    "Mod Env",
    "Velocity",
    "Key",
    "Mod Wheel",
    "Note Pan",
];

pub const MOD_DESTINATIONS: [&str; 17] = [
    "None",
    "Pitch",
    "Osc 1 Pitch",
    "Osc 2 Pitch",
    "Osc 3 Pitch",
    "Cutoff",
    "Cutoff 1",
    "Cutoff 2",
    "Resonance",
    "PWM",
    "Osc 1 Level",
    "Osc 2 Level",
    "Osc 3 Level",
    "Sub Level",
    "Noise Level",
    "Pan",
    "Amp",
];

pub fn descriptor() -> NodeDescriptor {
    let mut p = Vec::new();
    let waves = ["Saw", "Pulse", "Triangle", "Sine"];
    for n in 1..=3 {
        let o = format!("osc{n}");
        let on = format!("Osc {n}");
        p.push(ParamInfo::choice(
            &format!("{o}_wave"),
            &format!("{on} Wave"),
            &waves,
            0,
        ));
        p.push(stepped(
            &format!("{o}_oct"),
            &format!("{on} Octave"),
            -3.0,
            3.0,
            0.0,
            ParamUnit::None,
        ));
        p.push(stepped(
            &format!("{o}_semi"),
            &format!("{on} Semitone"),
            -12.0,
            12.0,
            0.0,
            ParamUnit::Semitones,
        ));
        p.push(ParamInfo::new(
            &format!("{o}_fine"),
            &format!("{on} Fine"),
            -100.0,
            100.0,
            0.0,
            ParamUnit::Cents,
        ));
        p.push(ParamInfo::new(
            &format!("{o}_level"),
            &format!("{on} Level"),
            0.0,
            1.0,
            if n == 1 { 1.0 } else { 0.0 },
            ParamUnit::Percent,
        ));
        p.push(ParamInfo::new(
            &format!("{o}_pw"),
            &format!("{on} Pulse Width"),
            0.05,
            0.95,
            0.5,
            ParamUnit::Percent,
        ));
    }
    p.push(ParamInfo::new(
        "sub_level",
        "Sub Level",
        0.0,
        1.0,
        0.0,
        ParamUnit::Percent,
    ));
    p.push(ParamInfo::choice(
        "sub_oct",
        "Sub Octave",
        &["-1 oct", "-2 oct"],
        0,
    ));
    p.push(ParamInfo::choice(
        "sub_wave",
        "Sub Wave",
        &["Square", "Sine"],
        0,
    ));
    p.push(ParamInfo::new(
        "noise_level",
        "Noise Level",
        0.0,
        1.0,
        0.0,
        ParamUnit::Percent,
    ));
    for n in 1..=2 {
        let f = format!("f{n}");
        let fname = format!("Filter {n}");
        p.push(switch(&format!("{f}_on"), &format!("{fname} On"), n == 1));
        p.push(ParamInfo::choice(
            &format!("{f}_model"),
            &format!("{fname} Model"),
            &["Ladder", "SVF"],
            n - 1,
        ));
        p.push(ParamInfo::choice(
            &format!("{f}_type"),
            &format!("{fname} Type"),
            &["Low-Pass", "High-Pass", "Band-Pass", "Notch"],
            0,
        ));
        p.push(ParamInfo::new(
            &format!("{f}_cutoff"),
            &format!("{fname} Cutoff"),
            20.0,
            20000.0,
            if n == 1 { 6000.0 } else { 20000.0 },
            ParamUnit::Hertz,
        ));
        p.push(ParamInfo::new(
            &format!("{f}_res"),
            &format!("{fname} Resonance"),
            0.0,
            1.0,
            0.1,
            ParamUnit::Percent,
        ));
        p.push(ParamInfo::new(
            &format!("{f}_drive"),
            &format!("{fname} Drive"),
            0.0,
            24.0,
            0.0,
            ParamUnit::Decibels,
        ));
        p.push(ParamInfo::new(
            &format!("{f}_keytrack"),
            &format!("{fname} Key Tracking"),
            0.0,
            1.0,
            0.0,
            ParamUnit::Percent,
        ));
        p.push(ParamInfo::new(
            &format!("{f}_env"),
            &format!("{fname} Envelope Amount"),
            -8.0,
            8.0,
            0.0,
            ParamUnit::None,
        ));
    }
    p.push(ParamInfo::choice(
        "filter_routing",
        "Filter Routing",
        &["Serial", "Parallel"],
        0,
    ));
    for (e, name, d) in [
        ("amp", "Amp", (0.002, 0.3, 1.0, 0.15)),
        ("fenv", "Filter Env", (0.002, 0.4, 0.3, 0.3)),
        ("menv", "Mod Env", (0.002, 0.4, 0.0, 0.3)),
    ] {
        p.push(ParamInfo::new(
            &format!("{e}_attack"),
            &format!("{name} Attack"),
            0.0,
            20.0,
            d.0,
            ParamUnit::Seconds,
        ));
        p.push(ParamInfo::new(
            &format!("{e}_decay"),
            &format!("{name} Decay"),
            0.0,
            20.0,
            d.1,
            ParamUnit::Seconds,
        ));
        p.push(ParamInfo::new(
            &format!("{e}_sustain"),
            &format!("{name} Sustain"),
            0.0,
            1.0,
            d.2,
            ParamUnit::Percent,
        ));
        p.push(ParamInfo::new(
            &format!("{e}_release"),
            &format!("{name} Release"),
            0.0,
            20.0,
            d.3,
            ParamUnit::Seconds,
        ));
        p.push(ParamInfo::new(
            &format!("{e}_curve"),
            &format!("{name} Curve"),
            0.0,
            1.0,
            0.5,
            ParamUnit::Percent,
        ));
    }
    let divs = sync_division_names();
    for n in 1..=2 {
        let l = format!("lfo{n}");
        let ln = format!("LFO {n}");
        p.push(ParamInfo::choice(
            &format!("{l}_shape"),
            &format!("{ln} Shape"),
            &LfoShape::NAMES,
            0,
        ));
        p.push(ParamInfo::new(
            &format!("{l}_rate"),
            &format!("{ln} Rate"),
            0.01,
            50.0,
            2.0,
            ParamUnit::Hertz,
        ));
        p.push(switch(&format!("{l}_sync"), &format!("{ln} Sync"), false));
        p.push(ParamInfo::choice(
            &format!("{l}_div"),
            &format!("{ln} Division"),
            &divs,
            10,
        ));
        p.push(switch(
            &format!("{l}_retrig"),
            &format!("{ln} Retrigger"),
            true,
        ));
    }
    for n in 1..=MOD_SLOTS {
        p.push(ParamInfo::choice(
            &format!("mod{n}_src"),
            &format!("Mod {n} Source"),
            &MOD_SOURCES,
            0,
        ));
        p.push(ParamInfo::choice(
            &format!("mod{n}_dst"),
            &format!("Mod {n} Destination"),
            &MOD_DESTINATIONS,
            0,
        ));
        p.push(ParamInfo::new(
            &format!("mod{n}_amt"),
            &format!("Mod {n} Amount"),
            -1.0,
            1.0,
            0.0,
            ParamUnit::None,
        ));
    }
    p.push(stepped(
        "unison_voices",
        "Unison Voices",
        1.0,
        UNISON_MAX as f64,
        1.0,
        ParamUnit::None,
    ));
    p.push(ParamInfo::new(
        "unison_detune",
        "Unison Detune",
        0.0,
        100.0,
        15.0,
        ParamUnit::Cents,
    ));
    p.push(ParamInfo::new(
        "unison_spread",
        "Unison Spread",
        0.0,
        1.0,
        0.5,
        ParamUnit::Percent,
    ));
    p.push(ParamInfo::choice(
        "glide_mode",
        "Glide",
        &["Off", "Legato", "Always"],
        0,
    ));
    p.push(ParamInfo::new(
        "glide_time",
        "Glide Time",
        0.0,
        5.0,
        0.1,
        ParamUnit::Seconds,
    ));
    p.push(ParamInfo::choice(
        "voice_mode",
        "Voice Mode",
        &["Poly", "Mono"],
        0,
    ));
    p.push(stepped(
        "polyphony",
        "Polyphony",
        1.0,
        MAX_POLY as f64,
        16.0,
        ParamUnit::None,
    ));
    p.push(ParamInfo::new(
        "vel_amp",
        "Velocity → Amp",
        0.0,
        1.0,
        0.7,
        ParamUnit::Percent,
    ));
    p.push(ParamInfo::new(
        "volume",
        "Volume",
        -48.0,
        12.0,
        -6.0,
        ParamUnit::Decibels,
    ));
    p.push(ParamInfo::new("pan", "Pan", -1.0, 1.0, 0.0, ParamUnit::Pan));
    p.push(ParamInfo::new(
        "mod_wheel",
        "Mod Wheel",
        0.0,
        1.0,
        0.0,
        ParamUnit::Percent,
    ));
    p.push(ParamInfo::new(
        "pitch_bend",
        "Pitch Bend",
        -1.0,
        1.0,
        0.0,
        ParamUnit::None,
    ));
    p.push(stepped(
        "bend_range",
        "Bend Range",
        0.0,
        24.0,
        2.0,
        ParamUnit::Semitones,
    ));
    instrument_desc("ph.synth", "Haus Synth", p)
}

/// Phase of a free-running LFO derived from an integer sample count (exact regardless of
/// how rendering is split into blocks).
#[derive(Clone, Copy, Default)]
struct Clock {
    base: f64,
    inc: f64,
    count: u64,
}

impl Clock {
    fn phase(&self) -> f64 {
        let p = self.base + self.inc * self.count as f64;
        p - p.floor()
    }

    fn rebase(&mut self, phase: f64, inc: f64) {
        self.base = phase;
        self.inc = inc;
        self.count = 0;
    }
}

#[derive(Clone)]
struct Voice {
    active: bool,
    key: u8,
    vel: f32,
    vel_raw: f32,
    note_pan: f32,
    pitch: f32,
    target: f32,
    glide_step: f32,
    osc: [[Osc; 3]; UNISON_MAX],
    sub: SubOsc,
    noise: Noise,
    rng: Rng,
    amp: Adsr,
    fenv: Adsr,
    menv: Adsr,
    lfo: [Lfo; 2],
    ladder: [[Ladder; 2]; 2],
    svf: [[Svf; 2]; 2],
    ctrl: u32,
    // control-rate results
    lvl: [f32; 3],
    pw: [f32; 3],
    sub_lvl: f32,
    noise_lvl: f32,
    uni: [(f32, f32); UNISON_MAX],
    pan: (f32, f32),
    amp_mod: f32,
    svf_drive: [f32; 2],
}

impl Voice {
    fn new(sr: f32, seed: u64) -> Self {
        Self {
            active: false,
            key: 60,
            vel: 1.0,
            vel_raw: 1.0,
            note_pan: 0.0,
            pitch: 60.0,
            target: 60.0,
            glide_step: 0.0,
            osc: Default::default(),
            sub: SubOsc::default(),
            noise: Noise::new(seed),
            rng: Rng::new(seed),
            amp: Adsr::new(sr),
            fenv: Adsr::new(sr),
            menv: Adsr::new(sr),
            lfo: [Lfo::new(seed), Lfo::new(seed ^ 1)],
            ladder: Default::default(),
            svf: Default::default(),
            ctrl: 0,
            lvl: [0.0; 3],
            pw: [0.5; 3],
            sub_lvl: 0.0,
            noise_lvl: 0.0,
            uni: [(1.0, 1.0); UNISON_MAX],
            pan: (1.0, 1.0),
            amp_mod: 1.0,
            svf_drive: [1.0; 2],
        }
    }
}

/// Parameter snapshot used inside the per-sample loop.
struct Frame {
    wave: [Wave; 3],
    sub_square: bool,
    f_on: [bool; 2],
    f_ladder: [bool; 2],
    f_mode: [FilterMode; 2],
    parallel: bool,
    unison: usize,
    stereo: bool,
    gain: f32,
}

pub struct Synth {
    p: Params,
    sr: f64,
    os_factor: usize,
    os: [Oversampler; 2],
    voices: Vec<Voice>,
    alloc: VoiceAllocator,
    seed: u64,
    note_counter: u64,
    last_pitch: Option<f32>,
    // mono note stack: (note_id, key)
    stack: [(u32, u8); 16],
    stack_len: usize,
    mono_note: Option<u32>,
    clocks: [Clock; 2],
    bpm: f64,
    next_pos: Option<i64>,
    was_playing: bool,
    base: (i64, f64),
    abs_now: i64,
    idle: usize,
}

impl Default for Synth {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn osc_p(o: usize, k: usize) -> usize {
    OSC + o * OSC_N + k
}
#[inline]
fn flt_p(f: usize, k: usize) -> usize {
    FLT + f * FLT_N + k
}
#[inline]
fn env_p(e: usize, k: usize) -> usize {
    ENV + e * ENV_N + k
}
#[inline]
fn lfo_p(l: usize, k: usize) -> usize {
    LFO + l * LFO_N + k
}
#[inline]
fn mod_p(s: usize, k: usize) -> usize {
    MOD + s * MOD_N + k
}

impl Synth {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            os_factor: 2,
            os: [Oversampler::new(2, 1), Oversampler::new(2, 1)],
            voices: Vec::new(),
            alloc: VoiceAllocator::new(MAX_POLY + SPARE, 16),
            seed: 0,
            note_counter: 0,
            last_pitch: None,
            stack: [(0, 0); 16],
            stack_len: 0,
            mono_note: None,
            clocks: [Clock::default(); 2],
            bpm: 120.0,
            next_pos: None,
            was_playing: false,
            base: (0, 0.0),
            abs_now: 0,
            idle: usize::MAX / 2,
        };
        s.alloc_voices();
        s
    }

    fn hi_sr(&self) -> f64 {
        self.sr * self.os_factor as f64
    }

    fn alloc_voices(&mut self) {
        let sr = self.hi_sr() as f32;
        self.voices = (0..MAX_POLY + SPARE)
            .map(|i| Voice::new(sr, self.seed.wrapping_add(i as u64)))
            .collect();
        self.update_envs();
    }

    fn update_envs(&mut self) {
        let e = |k: usize| {
            (
                self.p.f(env_p(k, 0)),
                self.p.f(env_p(k, 1)),
                self.p.f(env_p(k, 2)),
                self.p.f(env_p(k, 3)),
                self.p.f(env_p(k, 4)),
            )
        };
        let (a, f, m) = (e(0), e(1), e(2));
        for v in &mut self.voices {
            v.amp.set(a.0, a.1, a.2, a.3, a.4);
            v.fenv.set(f.0, f.1, f.2, f.3, f.4);
            v.menv.set(m.0, m.1, m.2, m.3, m.4);
        }
    }

    fn lfo_beats(&self, l: usize) -> f64 {
        SYNC_DIVISIONS[self.p.u(lfo_p(l, L_DIV)).min(SYNC_DIVISIONS.len() - 1)].1
    }

    /// LFO frequency in Hz (free or synced).
    fn lfo_hz(&self, l: usize) -> f64 {
        if self.p.on(lfo_p(l, L_SYNC)) {
            self.bpm / 60.0 / self.lfo_beats(l)
        } else {
            self.p.v[lfo_p(l, L_RATE)]
        }
    }

    fn rebase_clock(&mut self, l: usize) {
        let inc = self.lfo_hz(l) / self.hi_sr();
        let ph = self.clocks[l].phase();
        self.clocks[l].rebase(ph, inc);
    }

    fn resync_clock(&mut self, l: usize, abs: i64) {
        if self.p.on(lfo_p(l, L_SYNC)) && self.was_playing {
            let beats = self.base.1 + (abs - self.base.0) as f64 * self.bpm / 60.0 / self.sr;
            let b = self.lfo_beats(l);
            let inc = self.lfo_hz(l) / self.hi_sr();
            self.clocks[l].rebase(beats / b - (beats / b).floor(), inc);
        } else {
            self.rebase_clock(l);
        }
    }

    fn frame(&self) -> Frame {
        let unison = self.p.u(UNI_VOICES).clamp(1, UNISON_MAX);
        Frame {
            wave: [0, 1, 2].map(|o| Wave::from_index(self.p.u(osc_p(o, O_WAVE)))),
            sub_square: self.p.u(SUB_WAVE) == 0,
            f_on: [0, 1].map(|f| self.p.on(flt_p(f, F_ON))),
            f_ladder: [0, 1].map(|f| self.p.u(flt_p(f, F_MODEL)) == 0),
            f_mode: [0, 1].map(|f| FilterMode::from_index(self.p.u(flt_p(f, F_TYPE)))),
            parallel: self.p.u(ROUTING) == 1,
            unison,
            stereo: unison > 1 && self.p.f(UNI_SPREAD) > 0.0,
            gain: db_to_lin(self.p.f(VOLUME)),
        }
    }

    // ---- notes ----------------------------------------------------------------------

    fn glide_applies(&self, legato: bool) -> bool {
        match self.p.u(GLIDE_MODE) {
            1 => legato,
            2 => true,
            _ => false,
        }
    }

    fn glide_step(&self, from: f32, to: f32) -> f32 {
        let ticks = (self.p.v[GLIDE_TIME] * self.hi_sr() / CTRL as f64).max(1.0) as f32;
        (to - from).abs() / ticks
    }

    /// Start (or retrigger) voice `i` for a note.
    fn start_voice(&mut self, i: usize, key: u8, velocity: f32, pan: f32, fine: f32, glide: bool) {
        self.note_counter += 1;
        let target = key as f32 + fine;
        let from = if glide { self.last_pitch } else { None };
        let step = from.map_or(0.0, |f| self.glide_step(f, target));
        let unison = self.p.u(UNI_VOICES).clamp(1, UNISON_MAX);
        let vel = velocity_gain(velocity, self.p.f(VEL_AMP));
        let retrig = [0, 1].map(|l| self.p.on(lfo_p(l, L_RETRIG)));
        let clock_phase = [self.clocks[0].phase(), self.clocks[1].phase()];
        let lfo_tick_sr = self.hi_sr() / CTRL as f64;
        let lfo_hz = [self.lfo_hz(0), self.lfo_hz(1)];
        let seed = self.seed ^ self.note_counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let v = &mut self.voices[i];
        let was_active = v.active;
        v.active = true;
        v.key = key;
        v.vel = vel;
        v.vel_raw = velocity;
        v.note_pan = pan;
        v.target = target;
        v.pitch = from.unwrap_or(target);
        v.glide_step = step;
        if !was_active {
            v.rng = Rng::new(seed);
            v.noise.reseed(seed ^ 0x5555);
            for (u, oscs) in v.osc.iter_mut().enumerate() {
                for o in oscs.iter_mut() {
                    let ph = if unison > 1 && u < unison {
                        v.rng.next_f64()
                    } else {
                        0.0
                    };
                    o.set_phase(ph);
                }
            }
            v.sub.set_phase(0.0);
            for f in 0..2 {
                for c in 0..2 {
                    v.ladder[f][c].reset();
                    v.svf[f][c].reset();
                }
            }
            v.ctrl = 0;
        }
        for l in 0..2 {
            v.lfo[l].set_rate_hz(lfo_hz[l], lfo_tick_sr);
            if retrig[l] {
                v.lfo[l].set_phase(0.0);
            } else {
                v.lfo[l].set_phase(clock_phase[l]);
            }
        }
        v.amp.gate_on();
        v.fenv.gate_on();
        v.menv.gate_on();
        self.last_pitch = Some(target);
    }

    fn note_on(&mut self, note_id: u32, key: u8, velocity: f32, pan: f32, fine: f32) {
        if self.p.u(VOICE_MODE) == 1 {
            // mono: last-note priority with a note stack
            if self.stack_len == self.stack.len() {
                self.stack.copy_within(1.., 0);
                self.stack_len -= 1;
            }
            self.stack[self.stack_len] = (note_id, key);
            self.stack_len += 1;
            let v = &self.voices[0];
            let legato = v.active && v.amp.stage() != ph_dsp::env::Stage::Release;
            let glide = self.glide_applies(legato);
            if legato {
                // legato: move pitch without retriggering the envelopes
                let target = key as f32 + fine;
                let from = self.voices[0].pitch;
                let step = if glide {
                    self.glide_step(from, target)
                } else {
                    0.0
                };
                let v = &mut self.voices[0];
                v.target = target;
                v.glide_step = step;
                if !glide {
                    v.pitch = target;
                }
                v.vel_raw = velocity;
                v.note_pan = pan;
                self.last_pitch = Some(target);
            } else {
                self.start_voice(0, key, velocity, pan, fine, glide);
            }
            self.mono_note = Some(note_id);
            return;
        }
        let legato = self
            .voices
            .iter()
            .any(|v| v.active && v.amp.stage() != ph_dsp::env::Stage::Release);
        let glide = self.glide_applies(legato);
        let voices = &self.voices;
        let a = self.alloc.note_on(note_id, key, |i| voices[i].amp.level());
        if let Some(f) = a.fade {
            self.voices[f].amp.kill(FADE_S);
        }
        if a.hard_cut {
            self.voices[a.slot].active = false;
            self.voices[a.slot].amp.reset();
        }
        self.start_voice(a.slot, key, velocity, pan, fine, glide);
    }

    fn note_off(&mut self, note_id: u32) {
        if self.p.u(VOICE_MODE) == 1 {
            if let Some(k) = self.stack[..self.stack_len]
                .iter()
                .position(|(id, _)| *id == note_id)
            {
                self.stack.copy_within(k + 1..self.stack_len, k);
                self.stack_len -= 1;
            }
            if self.mono_note == Some(note_id) {
                if self.stack_len > 0 {
                    // fall back to the previous held note, legato
                    let (id, key) = self.stack[self.stack_len - 1];
                    let target = key as f32;
                    let glide = self.glide_applies(true);
                    let from = self.voices[0].pitch;
                    let step = if glide {
                        self.glide_step(from, target)
                    } else {
                        0.0
                    };
                    let v = &mut self.voices[0];
                    v.target = target;
                    v.glide_step = step;
                    if !glide {
                        v.pitch = target;
                    }
                    self.mono_note = Some(id);
                    self.last_pitch = Some(target);
                } else {
                    let v = &mut self.voices[0];
                    v.amp.gate_off();
                    v.fenv.gate_off();
                    v.menv.gate_off();
                    self.mono_note = None;
                }
            }
            return;
        }
        if let Some(i) = self.alloc.note_off(note_id) {
            let v = &mut self.voices[i];
            v.amp.gate_off();
            v.fenv.gate_off();
            v.menv.gate_off();
        }
    }

    fn all_off(&mut self) {
        for v in &mut self.voices {
            v.active = false;
            v.amp.reset();
            v.fenv.reset();
            v.menv.reset();
        }
        self.alloc.free_all();
        self.stack_len = 0;
        self.mono_note = None;
    }

    // ---- rendering ------------------------------------------------------------------

    /// Evaluate modulation for one voice at a control tick.
    fn control(p: &Params, v: &mut Voice, hi_sr: f32, mod_wheel: f32) {
        let l = [
            v.lfo[0].next(LfoShape::from_index(p.u(lfo_p(0, L_SHAPE)))),
            v.lfo[1].next(LfoShape::from_index(p.u(lfo_p(1, L_SHAPE)))),
        ];
        let src = [
            0.0,
            l[0],
            l[1],
            v.amp.level(),
            v.fenv.level(),
            v.menv.level(),
            v.vel_raw,
            ((v.key as f32 - 60.0) / 60.0).clamp(-1.0, 1.0),
            mod_wheel,
            v.note_pan,
        ];
        let mut d = [0.0f32; MOD_DESTINATIONS.len()];
        for s in 0..MOD_SLOTS {
            let si = p.u(mod_p(s, 0)).min(src.len() - 1);
            let di = p.u(mod_p(s, 1)).min(d.len() - 1);
            if si != 0 && di != 0 {
                d[di] += src[si] * p.f(mod_p(s, 2));
            }
        }
        // glide
        if v.pitch != v.target {
            if (v.target - v.pitch).abs() <= v.glide_step || v.glide_step == 0.0 {
                v.pitch = v.target;
            } else {
                v.pitch += v.glide_step * (v.target - v.pitch).signum();
            }
        }
        let bend = p.f(PITCH_BEND) * p.f(BEND_RANGE);
        let base = v.pitch + bend + 24.0 * d[1];
        let unison = p.u(UNI_VOICES).clamp(1, UNISON_MAX);
        let detune = p.f(UNI_DETUNE) / 100.0;
        let spread = p.f(UNI_SPREAD);
        let norm = 1.0 / (unison as f32).sqrt();
        let mut osc1_hz = 0.0f64;
        for o in 0..3 {
            let semis = base
                + 12.0 * p.f(osc_p(o, O_OCT))
                + p.f(osc_p(o, O_SEMI))
                + p.f(osc_p(o, O_FINE)) / 100.0
                + 24.0 * d[2 + o];
            v.lvl[o] = (p.f(osc_p(o, O_LEVEL)) + d[10 + o]).clamp(0.0, 1.0);
            v.pw[o] = (p.f(osc_p(o, O_PW)) + 0.45 * d[9]).clamp(0.05, 0.95);
            for u in 0..unison {
                let off = if unison > 1 {
                    detune * (u as f32 / (unison - 1) as f32 - 0.5)
                } else {
                    0.0
                };
                let hz = key_to_hz(semis + off) as f64;
                v.osc[u][o].set_freq(hz, hi_sr as f64);
            }
            if o == 0 {
                osc1_hz = key_to_hz(semis) as f64;
            }
        }
        v.sub
            .set_freq(osc1_hz, 1 + p.u(SUB_OCT) as u32, hi_sr as f64);
        v.sub_lvl = (p.f(SUB_LEVEL) + d[13]).clamp(0.0, 1.0);
        v.noise_lvl = (p.f(NOISE_LEVEL) + d[14]).clamp(0.0, 1.0);
        for u in 0..unison {
            let pan = if unison > 1 {
                spread * (2.0 * u as f32 / (unison - 1) as f32 - 1.0)
            } else {
                0.0
            };
            let (gl, gr) = balance(pan);
            v.uni[u] = (gl * norm, gr * norm);
        }
        for f in 0..2 {
            let mut lc = p.f(flt_p(f, F_CUTOFF)).log2()
                + p.f(flt_p(f, F_KEYTRACK)) * (v.pitch - 60.0) / 12.0
                + p.f(flt_p(f, F_ENV)) * v.fenv.level()
                + 8.0 * (d[5] + d[6 + f]);
            lc = lc.clamp(4.33, 15.0);
            let g = tpt_g(lc.exp2(), hi_sr);
            let res = (p.f(flt_p(f, F_RES)) + d[8]).clamp(0.0, 1.0);
            let drive = db_to_lin(p.f(flt_p(f, F_DRIVE)));
            for c in 0..2 {
                v.ladder[f][c].set_coefs(g, 4.0 * res);
                v.ladder[f][c].set_drive(drive);
                v.svf[f][c].set_coefs(g, res_to_k(res));
            }
            v.svf_drive[f] = drive;
        }
        v.pan = balance(p.f(PAN) + v.note_pan + d[15]);
        v.amp_mod = (1.0 + d[16]).max(0.0);
    }

    #[inline]
    fn apply_filter(v: &mut Voice, fr: &Frame, f: usize, c: usize, x: f32) -> f32 {
        if fr.f_ladder[f] {
            v.ladder[f][c].process(x, fr.f_mode[f])
        } else {
            let d = v.svf_drive[f];
            let xd = if d > 1.0 { fast_tanh(x * d) / d } else { x };
            v.svf[f][c].process(xd, fr.f_mode[f])
        }
    }

    #[inline]
    fn filters(v: &mut Voice, fr: &Frame, c: usize, x: f32) -> f32 {
        let y1 = if fr.f_on[0] {
            Self::apply_filter(v, fr, 0, c, x)
        } else {
            x
        };
        if !fr.f_on[1] {
            return y1;
        }
        if fr.parallel {
            let y2 = Self::apply_filter(v, fr, 1, c, x);
            if fr.f_on[0] { 0.5 * (y1 + y2) } else { y2 }
        } else {
            Self::apply_filter(v, fr, 1, c, y1)
        }
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let n = b - a;
        let os = self.os_factor;
        let fr = self.frame();
        let hi_sr = self.hi_sr() as f32;
        let mod_wheel = self.p.f(MOD_WHEEL);
        let [osl, osr] = &mut self.os;
        let hl = osl.hi_buffer(n);
        let hr = osr.hi_buffer(n);
        hl.fill(0.0);
        hr.fill(0.0);
        let mut any = false;
        for (vi, v) in self.voices.iter_mut().enumerate() {
            if !v.active {
                continue;
            }
            any = true;
            for j in 0..n * os {
                if v.ctrl == 0 {
                    Self::control(&self.p, v, hi_sr, mod_wheel);
                }
                v.ctrl = (v.ctrl + 1) % CTRL;
                let e = v.amp.next();
                v.fenv.next();
                v.menv.next();
                if !v.amp.is_active() {
                    v.active = false;
                    break;
                }
                let mut l = 0.0f32;
                let mut r = 0.0f32;
                for u in 0..fr.unison {
                    let mut s = 0.0f32;
                    for o in 0..3 {
                        if v.lvl[o] > 0.0 {
                            s += v.osc[u][o].next(fr.wave[o], v.pw[o]) * v.lvl[o];
                        }
                    }
                    l += s * v.uni[u].0;
                    r += s * v.uni[u].1;
                }
                let mut extra = 0.0f32;
                if v.sub_lvl > 0.0 {
                    extra += v.sub.next(fr.sub_square) * v.sub_lvl;
                }
                if v.noise_lvl > 0.0 {
                    extra += v.noise.next() * v.noise_lvl;
                }
                let g = e * v.vel * v.amp_mod * fr.gain;
                if fr.stereo {
                    l = Self::filters(v, &fr, 0, l + extra);
                    r = Self::filters(v, &fr, 1, r + extra);
                } else {
                    l = Self::filters(v, &fr, 0, l + extra);
                    r = l;
                }
                hl[j] += l * g * v.pan.0;
                hr[j] += r * g * v.pan.1;
            }
            if !v.active {
                v.amp.reset();
                v.fenv.reset();
                v.menv.reset();
                self.alloc.free(vi);
            }
        }
        if let Some(port) = io.outputs.get_mut(0) {
            if port.len() >= 2 {
                let (lp, rp) = port.split_at_mut(1);
                osl.down(&mut lp[0][a..b]);
                osr.down(&mut rp[0][a..b]);
            } else if let Some(m) = port.first_mut() {
                for (l, r) in hl.iter_mut().zip(hr.iter()) {
                    *l = 0.5 * (*l + *r);
                }
                osl.down(&mut m[a..b]);
            }
        }
        self.idle = if any { 0 } else { self.idle.saturating_add(n) };
        for c in 0..2 {
            self.clocks[c].count += (n * os) as u64;
        }
    }
}

impl Processor for Synth {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate;
        self.seed = info.seed;
        self.os_factor = match info.quality {
            Quality::Live => 2,
            Quality::High => 4,
        };
        let mf = info.max_frames.max(1);
        self.os = [
            Oversampler::new(self.os_factor, mf),
            Oversampler::new(self.os_factor, mf),
        ];
        self.alloc_voices();
        self.alloc = VoiceAllocator::new(MAX_POLY + SPARE, self.p.u(POLYPHONY));
        self.reset();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        let tempo_changed = ctx.bpm > 0.0 && ctx.bpm != self.bpm;
        if tempo_changed {
            self.bpm = ctx.bpm;
        }
        let jumped = !self.was_playing || self.next_pos != Some(ctx.song_pos_samples);
        self.was_playing = ctx.playing;
        if ctx.playing && (jumped || tempo_changed) {
            self.base = (ctx.song_pos_samples, ctx.song_pos_ticks / 960.0);
            for l in 0..2 {
                self.resync_clock(l, ctx.song_pos_samples);
            }
        }
        self.next_pos = Some(ctx.song_pos_samples + ctx.frames as i64);
        let n = ctx.frames;
        let mut pos = 0;
        for ev in io.events {
            let f = (ev.frame as usize).min(n);
            if f > pos {
                self.render(io, pos, f);
                pos = f;
            }
            self.abs_now = ctx.song_pos_samples + f as i64;
            match ev.kind {
                EventKind::NoteOn {
                    note_id,
                    key,
                    velocity,
                    pan,
                    fine_pitch,
                } => self.note_on(note_id, key, velocity, pan, fine_pitch),
                EventKind::NoteOff { note_id, .. } => self.note_off(note_id),
                EventKind::AllNotesOff => self.all_off(),
                EventKind::Param { id, value } => self.set_param(id, value),
            }
        }
        if pos < n {
            self.render(io, pos, n);
        }
        if self.idle > self.os[0].decimation_latency() as usize + 8 {
            ProcessStatus::Sleep
        } else {
            ProcessStatus::Continue
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        let Some(i) = self.p.set(id, value) else {
            return;
        };
        match i {
            ENV..LFO => self.update_envs(),
            POLYPHONY => self.alloc.set_limit(self.p.u(POLYPHONY)),
            VOICE_MODE => {
                for (k, v) in self.voices.iter_mut().enumerate() {
                    if v.active {
                        v.amp.kill(FADE_S);
                        if self.alloc.slot(k).state != SlotState::Free {
                            self.alloc.set_fading(k);
                        }
                    }
                }
                self.alloc.free_all();
                self.stack_len = 0;
                self.mono_note = None;
            }
            LFO..MOD => {
                let l = (i - LFO) / LFO_N;
                let k = (i - LFO) % LFO_N;
                if k == L_RATE || k == L_SYNC || k == L_DIV {
                    if k == L_RATE {
                        self.rebase_clock(l);
                    } else {
                        self.resync_clock(l, self.abs_now);
                    }
                    let hz = self.lfo_hz(l);
                    let tick_sr = self.hi_sr() / CTRL as f64;
                    for v in &mut self.voices {
                        v.lfo[l].set_rate_hz(hz, tick_sr);
                    }
                }
            }
            _ => {}
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.p.get(id)
    }

    fn reset(&mut self) {
        self.all_off();
        for o in &mut self.os {
            o.reset();
        }
        self.last_pitch = None;
        self.note_counter = 0;
        self.next_pos = None;
        self.was_playing = false;
        for l in 0..2 {
            self.clocks[l] = Clock::default();
            self.rebase_clock(l);
        }
        self.idle = usize::MAX / 2;
    }

    fn latency(&self) -> u32 {
        self.os[0].decimation_latency()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_ids() {
        let d = descriptor();
        assert_eq!(d.params.len(), 101);
        let at = |id: &str| d.param_id(id).unwrap().0 as usize;
        assert_eq!(at("osc3_pw"), osc_p(2, O_PW));
        assert_eq!(at("sub_level"), SUB_LEVEL);
        assert_eq!(at("noise_level"), NOISE_LEVEL);
        assert_eq!(at("f2_env"), flt_p(1, F_ENV));
        assert_eq!(at("filter_routing"), ROUTING);
        assert_eq!(at("menv_curve"), env_p(2, 4));
        assert_eq!(at("lfo2_retrig"), lfo_p(1, L_RETRIG));
        assert_eq!(at("mod8_amt"), mod_p(7, 2));
        assert_eq!(at("unison_voices"), UNI_VOICES);
        assert_eq!(at("glide_time"), GLIDE_TIME);
        assert_eq!(at("polyphony"), POLYPHONY);
        assert_eq!(at("bend_range"), BEND_RANGE);
        assert_eq!(at("f1_cutoff"), flt_p(0, F_CUTOFF));
        assert_eq!(at("f1_res"), flt_p(0, F_RES));
        assert_eq!(at("f1_drive"), flt_p(0, F_DRIVE));
        assert_eq!(at("f1_keytrack"), flt_p(0, F_KEYTRACK));
        assert_eq!(at("f1_model"), flt_p(0, F_MODEL));
        assert_eq!(at("f1_type"), flt_p(0, F_TYPE));
        assert_eq!(at("f1_on"), flt_p(0, F_ON));
        assert_eq!(at("osc1_semi"), osc_p(0, O_SEMI));
        assert_eq!(at("osc1_fine"), osc_p(0, O_FINE));
        assert_eq!(at("osc1_oct"), osc_p(0, O_OCT));
        assert_eq!(at("osc1_level"), osc_p(0, O_LEVEL));
        assert_eq!(at("lfo1_rate"), lfo_p(0, L_RATE));
        assert_eq!(at("lfo1_sync"), lfo_p(0, L_SYNC));
        assert_eq!(at("lfo1_div"), lfo_p(0, L_DIV));
        assert_eq!(at("lfo1_shape"), lfo_p(0, L_SHAPE));
        assert_eq!(at("unison_detune"), UNI_DETUNE);
        assert_eq!(at("unison_spread"), UNI_SPREAD);
        assert_eq!(at("glide_mode"), GLIDE_MODE);
        assert_eq!(at("voice_mode"), VOICE_MODE);
        assert_eq!(at("vel_amp"), VEL_AMP);
        assert_eq!(at("volume"), VOLUME);
        assert_eq!(at("pan"), PAN);
        assert_eq!(at("mod_wheel"), MOD_WHEEL);
        assert_eq!(at("pitch_bend"), PITCH_BEND);
        assert_eq!(at("sub_oct"), SUB_OCT);
        assert_eq!(at("sub_wave"), SUB_WAVE);
        assert_eq!(at("osc1_wave"), osc_p(0, O_WAVE));
    }
}
