//! `ph.sampler`: one-shot / looped sample player.
//!
//! Pitch-shifts by resampling: 4-point Hermite interpolation in `Quality::Live`, a 32-tap
//! Kaiser-windowed sinc with anti-aliasing cutoff scaling in `Quality::High`. Loops play
//! forward or ping-pong over the file's loop (WAV `smpl`) or the start/end range; reverse
//! plays from the end. Each voice has an amp ADSR and an optional TPT SVF whose cutoff
//! follows the envelope. The sample is resolved off the audio thread (in [`crate::create`]).

use crate::common::{
    Params, balance, instrument_desc, stepped, switch, velocity_gain, zero_outputs,
};
use ph_dsp::db_to_lin;
use ph_dsp::env::Adsr;
use ph_dsp::filter::{FilterMode, Svf, res_to_k, tpt_g};
use ph_dsp::interp::{SincInterpolator, hermite};
use ph_dsp::voice::VoiceAllocator;
use ph_node::*;
use std::sync::Arc;

const ROOT_KEY: usize = 0;
const TUNE: usize = 1;
const FINE: usize = 2;
const START: usize = 3;
const END: usize = 4;
const LOOP_MODE: usize = 5;
const LOOP_SOURCE: usize = 6;
const ATTACK: usize = 7;
const DECAY: usize = 8;
const SUSTAIN: usize = 9;
const RELEASE: usize = 10;
const FILTER_TYPE: usize = 11;
const CUTOFF: usize = 12;
const RESONANCE: usize = 13;
const FILTER_ENV: usize = 14;
const VEL_AMP: usize = 15;
const PAN: usize = 16;
const POLYPHONY: usize = 17;
const REVERSE: usize = 18;
const CHOKE: usize = 19;
const VOLUME: usize = 20;

const MAX_VOICES: usize = 64;
const SPARE: usize = 8;
const CTRL: u32 = 16;
const FADE_S: f32 = 0.003;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        stepped("root_key", "Root Key", 0.0, 127.0, 60.0, ParamUnit::None),
        ParamInfo::new("tune", "Tune", -48.0, 48.0, 0.0, ParamUnit::Semitones),
        ParamInfo::new("fine", "Fine Tune", -100.0, 100.0, 0.0, ParamUnit::Cents),
        ParamInfo::new("start", "Start", 0.0, 1.0, 0.0, ParamUnit::Percent),
        ParamInfo::new("end", "End", 0.0, 1.0, 1.0, ParamUnit::Percent),
        ParamInfo::choice("loop_mode", "Loop", &["Off", "Forward", "Ping-Pong"], 0),
        ParamInfo::choice("loop_source", "Loop Points", &["File", "Start/End"], 0),
        ParamInfo::new("attack", "Attack", 0.0, 10.0, 0.001, ParamUnit::Seconds),
        ParamInfo::new("decay", "Decay", 0.0, 10.0, 0.5, ParamUnit::Seconds),
        ParamInfo::new("sustain", "Sustain", 0.0, 1.0, 1.0, ParamUnit::Percent),
        ParamInfo::new("release", "Release", 0.0, 10.0, 0.05, ParamUnit::Seconds),
        ParamInfo::choice(
            "filter_type",
            "Filter",
            &["Off", "Low-Pass", "High-Pass", "Band-Pass"],
            0,
        ),
        ParamInfo::new("cutoff", "Cutoff", 20.0, 20000.0, 20000.0, ParamUnit::Hertz),
        ParamInfo::new("resonance", "Resonance", 0.0, 1.0, 0.0, ParamUnit::Percent),
        ParamInfo::new(
            "filter_env",
            "Filter Envelope",
            -8.0,
            8.0,
            0.0,
            ParamUnit::None,
        ),
        ParamInfo::new(
            "vel_amp",
            "Velocity → Amp",
            0.0,
            1.0,
            1.0,
            ParamUnit::Percent,
        ),
        ParamInfo::new("pan", "Pan", -1.0, 1.0, 0.0, ParamUnit::Pan),
        stepped(
            "polyphony",
            "Polyphony",
            1.0,
            MAX_VOICES as f64,
            16.0,
            ParamUnit::None,
        ),
        switch("reverse", "Reverse", false),
        stepped(
            "choke_group",
            "Choke Group",
            0.0,
            16.0,
            0.0,
            ParamUnit::None,
        ),
        ParamInfo::new("volume", "Volume", -48.0, 12.0, 0.0, ParamUnit::Decibels),
    ];
    instrument_desc("ph.sampler", "Sampler", params)
}

#[derive(Clone)]
struct Voice {
    active: bool,
    pos: f64,
    dir: f64,
    rate: f64,
    env: Adsr,
    filt: [Svf; 2],
    gl: f32,
    gr: f32,
    vel: f32,
    ctrl: u32,
}

impl Voice {
    fn new(sr: f32) -> Self {
        Self {
            active: false,
            pos: 0.0,
            dir: 1.0,
            rate: 1.0,
            env: Adsr::new(sr),
            filt: [Svf::new(), Svf::new()],
            gl: 1.0,
            gr: 1.0,
            vel: 1.0,
            ctrl: 0,
        }
    }
}

/// Playback region in frames: `[start, end)` and the loop `[ls, le)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Region {
    start: f64,
    end: f64,
    ls: f64,
    le: f64,
}

pub struct Sampler {
    p: Params,
    sample: Option<Arc<SampleData>>,
    reference: Option<String>,
    sr: f64,
    quality: Quality,
    voices: Vec<Voice>,
    alloc: VoiceAllocator,
    sinc: Option<SincInterpolator>,
    region: Region,
}

impl Sampler {
    /// `sample` already decoded (resolve it off the audio thread); `reference` is kept for
    /// `save_state`.
    pub fn new(sample: Option<Arc<SampleData>>, reference: Option<String>) -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sample,
            reference,
            sr: 48000.0,
            quality: Quality::Live,
            voices: Vec::new(),
            alloc: VoiceAllocator::new(MAX_VOICES + SPARE, 16),
            sinc: None,
            region: Region {
                start: 0.0,
                end: 0.0,
                ls: 0.0,
                le: 0.0,
            },
        };
        s.alloc_voices();
        s.update_region();
        s
    }

    fn alloc_voices(&mut self) {
        self.voices = vec![Voice::new(self.sr as f32); MAX_VOICES + SPARE];
        self.update_env();
    }

    fn update_env(&mut self) {
        let (a, d, s, r) = (
            self.p.f(ATTACK),
            self.p.f(DECAY),
            self.p.f(SUSTAIN),
            self.p.f(RELEASE),
        );
        for v in &mut self.voices {
            v.env.set(a, d, s, r, 0.5);
        }
    }

    fn update_region(&mut self) {
        let frames = self.sample.as_ref().map_or(0, |s| s.frames()) as f64;
        let mut start = (self.p.v[START] * frames).floor();
        let mut end = (self.p.v[END] * frames).floor();
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        let end = end.max(start + 1.0).min(frames.max(1.0));
        let (mut ls, mut le) = (start, end);
        if self.p.u(LOOP_SOURCE) == 0
            && let Some((a, b)) = self.sample.as_ref().and_then(|s| s.loop_range)
        {
            let (a, b) = (a as f64, b as f64);
            if b > a + 1.0 && b <= frames {
                ls = a;
                le = b;
            }
        }
        self.region = Region {
            start,
            end,
            ls,
            le: le.max(ls + 1.0),
        };
    }

    fn note_on(&mut self, note_id: u32, key: u8, velocity: f32, pan: f32, fine_pitch: f32) {
        let Some(sample) = self.sample.as_ref() else {
            return;
        };
        if sample.frames() == 0 {
            return;
        }
        if self.p.u(CHOKE) > 0 {
            for (i, v) in self.voices.iter_mut().enumerate() {
                if v.active {
                    v.env.kill(FADE_S);
                    self.alloc.set_fading(i);
                }
            }
        }
        let voices = &self.voices;
        let a = self.alloc.note_on(note_id, key, |i| voices[i].env.level());
        if let Some(f) = a.fade {
            self.voices[f].env.kill(FADE_S);
        }
        let semis = key as f64 - self.p.v[ROOT_KEY]
            + self.p.v[TUNE]
            + self.p.v[FINE] / 100.0
            + fine_pitch as f64;
        let rate = (semis / 12.0).exp2() * sample.sample_rate / self.sr;
        let reverse = self.p.on(REVERSE);
        let reg = self.region;
        let (gl, gr) = balance(self.p.f(PAN) + pan);
        let vel = velocity_gain(velocity, self.p.f(VEL_AMP));
        let v = &mut self.voices[a.slot];
        if a.hard_cut {
            v.env.reset();
        }
        v.active = true;
        v.rate = rate;
        v.dir = if reverse { -1.0 } else { 1.0 };
        v.pos = if reverse { reg.end - 1.0 } else { reg.start };
        v.gl = gl;
        v.gr = gr;
        v.vel = vel;
        v.ctrl = 0;
        v.filt[0].reset();
        v.filt[1].reset();
        v.env.gate_on();
    }

    fn note_off(&mut self, note_id: u32) {
        if let Some(i) = self.alloc.note_off(note_id) {
            self.voices[i].env.gate_off();
        }
    }

    fn all_off(&mut self) {
        for v in &mut self.voices {
            v.active = false;
            v.env.reset();
        }
        self.alloc.free_all();
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        zero_outputs(io, a, b);
        let Some(sample) = self.sample.clone() else {
            return;
        };
        let reg = self.region;
        let loop_mode = self.p.u(LOOP_MODE);
        let ftype = self.p.u(FILTER_TYPE);
        let fmode = match ftype {
            2 => FilterMode::HighPass,
            3 => FilterMode::BandPass,
            _ => FilterMode::LowPass,
        };
        let cut_log = self.p.f(CUTOFF).log2();
        let env_oct = self.p.f(FILTER_ENV);
        let k = res_to_k(self.p.f(RESONANCE));
        let vol = db_to_lin(self.p.f(VOLUME));
        let sr = self.sr as f32;
        let chans = sample.channels.len().min(2);
        let frames = sample.frames() as i64;
        let sinc = self.sinc.as_ref();
        let len = reg.le - reg.ls;
        for (vi, v) in self.voices.iter_mut().enumerate() {
            if !v.active {
                continue;
            }
            for i in a..b {
                let e = v.env.next();
                if !v.env.is_active() {
                    v.active = false;
                    break;
                }
                // read
                let pos = v.pos;
                // a forward loop's interpolation window wraps across the loop seam
                let in_loop = loop_mode == 1 && pos >= reg.ls && pos < reg.le;
                let ip = pos.floor();
                let t = (pos - ip) as f32;
                let ip = ip as i64;
                let mut s = [0.0f32; 2];
                for (c, sc) in s.iter_mut().enumerate().take(chans) {
                    let data = &sample.channels[c];
                    let fetch = |j: i64| {
                        let mut j = j;
                        if in_loop {
                            let (lo, hi) = (reg.ls as i64, reg.le as i64);
                            if j >= hi {
                                j -= hi - lo;
                            } else if j < lo {
                                j += hi - lo;
                            }
                        }
                        if j >= 0 && j < frames {
                            data[j as usize]
                        } else {
                            0.0
                        }
                    };
                    *sc = match sinc {
                        Some(si) => si.interpolate(pos, v.rate, fetch),
                        None => hermite(fetch(ip - 1), fetch(ip), fetch(ip + 1), fetch(ip + 2), t),
                    };
                }
                if chans == 1 {
                    s[1] = s[0];
                }
                // filter
                if ftype != 0 {
                    if v.ctrl == 0 {
                        let fc = (cut_log + env_oct * e).exp2();
                        let g = tpt_g(fc, sr);
                        v.filt[0].set_coefs(g, k);
                        v.filt[1].set_coefs(g, k);
                    }
                    s[0] = v.filt[0].process(s[0], fmode);
                    s[1] = v.filt[1].process(s[1], fmode);
                }
                v.ctrl = (v.ctrl + 1) % CTRL;
                let g = e * v.vel * vol;
                if let Some(p) = io.outputs.get_mut(0) {
                    if p.len() >= 2 {
                        p[0][i] += s[0] * g * v.gl;
                        p[1][i] += s[1] * g * v.gr;
                    } else if let Some(m) = p.first_mut() {
                        m[i] += 0.5 * (s[0] * v.gl + s[1] * v.gr) * g;
                    }
                }
                // advance
                v.pos += v.rate * v.dir;
                let fwd = v.dir > 0.0;
                match loop_mode {
                    1 if fwd && v.pos >= reg.le => {
                        v.pos = reg.ls + (v.pos - reg.le) % len;
                    }
                    1 if !fwd && v.pos < reg.ls => {
                        v.pos = reg.le - (reg.ls - v.pos) % len;
                    }
                    2 if fwd && v.pos >= reg.le => {
                        v.pos = (2.0 * reg.le - v.pos).max(reg.ls);
                        v.dir = -1.0;
                    }
                    2 if !fwd && v.pos < reg.ls => {
                        v.pos = (2.0 * reg.ls - v.pos).min(reg.le);
                        v.dir = 1.0;
                    }
                    0 if (fwd && v.pos >= reg.end) || (!fwd && v.pos < reg.start) => {
                        v.active = false;
                        break;
                    }
                    _ => {}
                }
            }
            if !v.active {
                v.env.reset();
                self.alloc.free(vi);
            }
        }
        // keep the allocator in sync with voices that finished on their own
        for (i, v) in self.voices.iter().enumerate() {
            if !v.active && !self.alloc.is_free(i) {
                self.alloc.free(i);
            }
        }
    }

    fn any_active(&self) -> bool {
        self.voices.iter().any(|v| v.active)
    }
}

impl Processor for Sampler {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate;
        self.quality = info.quality;
        self.sinc = match info.quality {
            Quality::Live => None,
            Quality::High => Some(SincInterpolator::new(16, 4.0)),
        };
        self.alloc_voices();
        self.alloc = VoiceAllocator::new(MAX_VOICES + SPARE, self.p.u(POLYPHONY));
        self.update_region();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        let n = ctx.frames;
        let mut pos = 0;
        for ev in io.events {
            let f = (ev.frame as usize).min(n);
            if f > pos {
                self.render(io, pos, f);
                pos = f;
            }
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
        if self.any_active() {
            ProcessStatus::Continue
        } else {
            ProcessStatus::Sleep
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        if let Some(i) = self.p.set(id, value) {
            match i {
                ATTACK | DECAY | SUSTAIN | RELEASE => self.update_env(),
                START | END | LOOP_SOURCE => self.update_region(),
                POLYPHONY => self.alloc.set_limit(self.p.u(POLYPHONY)),
                _ => {}
            }
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.p.get(id)
    }

    fn reset(&mut self) {
        self.all_off();
    }

    fn save_state(&self) -> Vec<u8> {
        self.reference.clone().unwrap_or_default().into_bytes()
    }

    /// The sample itself comes from the project's `InstrumentSpec` (resolved in
    /// [`crate::create`]); a saved state is accepted when it names the same sample.
    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let r = std::str::from_utf8(state).map_err(|e| e.to_string())?;
        if r.is_empty() || Some(r) == self.reference.as_deref() {
            Ok(())
        } else {
            Err(format!(
                "sampler state names `{r}`; change samples through the instrument spec \
                 (load_state has no asset resolver)"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_count() {
        assert_eq!(descriptor().params.len(), 21);
    }
}
