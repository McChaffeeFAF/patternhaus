//! `ph.fx.filter`: SVF or ladder filter with drive and a tempo-syncable LFO on cutoff.
//!
//! Runs oversampled (2× live, 4× high quality) because both the drive stage and the
//! ladder's saturating feedback generate harmonics. Cutoff, resonance and drive are
//! smoothed per sample; filter coefficients update on an 8-sample grid counted from
//! activation. A synced LFO re-aligns to the song position whenever the transport starts,
//! jumps or changes tempo, and when sync settings change.

use crate::common::{Params, SilenceTracker, input, output, stereo_desc, switch};
use ph_dsp::delay::DelayLine;
use ph_dsp::filter::{FilterMode, Ladder, Svf, res_to_k, tpt_g};
use ph_dsp::lfo::{Lfo, LfoShape, SYNC_DIVISIONS, sync_division_names};
use ph_dsp::oversample::Oversampler;
use ph_dsp::smooth::OnePole;
use ph_dsp::{db_to_lin, fast_tanh};
use ph_node::*;

const MODEL: usize = 0;
const MODE: usize = 1;
const CUTOFF: usize = 2;
const RESONANCE: usize = 3;
const DRIVE: usize = 4;
const LFO_DEPTH: usize = 5;
const LFO_RATE: usize = 6;
const LFO_SYNC: usize = 7;
const LFO_DIV: usize = 8;
const LFO_SHAPE: usize = 9;
const MIX: usize = 10;

const CTRL: u32 = 8;

pub fn descriptor() -> NodeDescriptor {
    let divs = sync_division_names();
    let params = vec![
        ParamInfo::choice("model", "Model", &["SVF", "Ladder"], 0),
        ParamInfo::choice(
            "mode",
            "Mode",
            &["Low-Pass", "High-Pass", "Band-Pass", "Notch"],
            0,
        ),
        ParamInfo::new("cutoff", "Cutoff", 20.0, 20000.0, 2000.0, ParamUnit::Hertz),
        ParamInfo::new("resonance", "Resonance", 0.0, 1.0, 0.2, ParamUnit::Percent),
        ParamInfo::new("drive", "Drive", 0.0, 24.0, 0.0, ParamUnit::Decibels),
        ParamInfo::new("lfo_depth", "LFO Depth", -4.0, 4.0, 0.0, ParamUnit::None),
        ParamInfo::new("lfo_rate", "LFO Rate", 0.01, 20.0, 1.0, ParamUnit::Hertz),
        switch("lfo_sync", "LFO Sync", false),
        ParamInfo::choice("lfo_div", "LFO Division", &divs, 10),
        ParamInfo::choice("lfo_shape", "LFO Shape", &LfoShape::NAMES, 0),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 1.0, ParamUnit::Percent),
    ];
    stereo_desc("ph.fx.filter", "Filter", params, false)
}

pub struct Filter {
    p: Params,
    sr: f64,
    os_factor: usize,
    os: [Oversampler; 2],
    svf: [Svf; 2],
    ladder: [Ladder; 2],
    dry: [DelayLine; 2],
    tmp: [Vec<f32>; 2],
    log_cut: OnePole,
    res: OnePole,
    drive: OnePole,
    mix: OnePole,
    lfo: Lfo,
    lfo_val: f32,
    ctrl: u32,
    next_pos: Option<i64>,
    was_playing: bool,
    bpm: f64,
    /// Song position (samples, beats) of the last transport discontinuity.
    base: (i64, f64),
    /// Absolute sample position of the event being applied.
    abs_now: i64,
    silence: SilenceTracker,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            os_factor: 2,
            os: [Oversampler::new(2, 1), Oversampler::new(2, 1)],
            svf: [Svf::new(), Svf::new()],
            ladder: [Ladder::new(), Ladder::new()],
            dry: [DelayLine::new(1), DelayLine::new(1)],
            tmp: [Vec::new(), Vec::new()],
            log_cut: OnePole::new(0.0),
            res: OnePole::new(0.0),
            drive: OnePole::new(1.0),
            mix: OnePole::new(1.0),
            lfo: Lfo::new(0),
            lfo_val: 0.0,
            ctrl: 0,
            next_pos: None,
            was_playing: false,
            bpm: 120.0,
            base: (0, 0.0),
            abs_now: 0,
            silence: SilenceTracker::default(),
        };
        s.update_all();
        s
    }

    fn hi_sr(&self) -> f64 {
        self.sr * self.os_factor as f64
    }

    fn update_all(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let hs = self.hi_sr() as f32;
        for s in [
            &mut self.log_cut,
            &mut self.res,
            &mut self.drive,
            &mut self.mix,
        ] {
            s.set_time(0.01, hs);
            let t = s.target();
            s.reset(t);
        }
    }

    fn div_beats(&self) -> f64 {
        SYNC_DIVISIONS[self.p.u(LFO_DIV).min(SYNC_DIVISIONS.len() - 1)].1
    }

    fn update_lfo_rate(&mut self) {
        let hs = self.hi_sr();
        if self.p.on(LFO_SYNC) {
            self.lfo.set_rate_synced(self.div_beats(), self.bpm, hs);
        } else {
            self.lfo.set_rate_hz(self.p.v[LFO_RATE], hs);
        }
    }

    fn apply(&mut self, i: usize) {
        match i {
            CUTOFF => self.log_cut.set_target(self.p.f(CUTOFF).log2()),
            RESONANCE => self.res.set_target(self.p.f(RESONANCE)),
            DRIVE => self.drive.set_target(db_to_lin(self.p.f(DRIVE))),
            MIX => self.mix.set_target(self.p.f(MIX)),
            LFO_RATE | LFO_SYNC | LFO_DIV => {
                self.update_lfo_rate();
                if i != LFO_RATE {
                    self.resync(self.abs_now);
                }
            }
            _ => {}
        }
    }

    /// Align a synced LFO to the song position at absolute sample `abs`. Positions are
    /// derived from the last discontinuity plus whole samples, so the result does not
    /// depend on how the song was cut into blocks.
    fn resync(&mut self, abs: i64) {
        if self.p.on(LFO_SYNC) && self.was_playing {
            let beats = self.base.1 + (abs - self.base.0) as f64 * self.bpm / 60.0 / self.sr;
            self.lfo.sync_to_position(beats, self.div_beats());
        }
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let n = b - a;
        let ladder = self.p.u(MODEL) == 1;
        let mode = FilterMode::from_index(self.p.u(MODE));
        let shape = LfoShape::from_index(self.p.u(LFO_SHAPE));
        let depth = self.p.f(LFO_DEPTH);
        let hs = self.hi_sr() as f32;
        let os = self.os_factor;
        for ch in 0..2 {
            for i in 0..n {
                self.tmp[ch][i] = input(io, 0, ch, a + i);
            }
        }
        let [os0, os1] = &mut self.os;
        let h0 = os0.up(&self.tmp[0][..n]);
        let h1 = os1.up(&self.tmp[1][..n]);
        for j in 0..n * os {
            let lfo = self.lfo.next(shape);
            let cut = self.log_cut.next();
            let res = self.res.next();
            let drive = self.drive.next();
            if self.ctrl == 0 {
                self.lfo_val = lfo;
                let fc = (cut + depth * self.lfo_val).exp2();
                let g = tpt_g(fc, hs);
                for c in 0..2 {
                    self.svf[c].set_coefs(g, res_to_k(res));
                    self.ladder[c].set_coefs(g, 4.0 * res);
                    self.ladder[c].set_drive(drive);
                }
            }
            self.ctrl = (self.ctrl + 1) % CTRL;
            for (c, h) in [&mut *h0, &mut *h1].into_iter().enumerate() {
                let x = h[j];
                h[j] = if ladder {
                    self.ladder[c].process(x, mode)
                } else {
                    let inv = 1.0 / drive;
                    let xd = if drive > 1.0 {
                        fast_tanh(x * drive) * inv
                    } else {
                        x
                    };
                    self.svf[c].process(xd, mode)
                };
            }
        }
        let [t0, t1] = &mut self.tmp;
        os0.down(&mut t0[..n]);
        os1.down(&mut t1[..n]);
        let lat = os0.latency() as usize;
        for i in 0..n {
            let m = self.mix.next();
            let mut o = [0.0f32; 2];
            for (c, oc) in o.iter_mut().enumerate() {
                self.dry[c].push(input(io, 0, c, a + i));
                let d = self.dry[c].read(lat);
                *oc = d * (1.0 - m) + self.tmp[c][i] * m;
            }
            output(io, a + i, o[0], o[1]);
        }
    }
}

impl Processor for Filter {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate;
        self.os_factor = match info.quality {
            Quality::Live => 2,
            Quality::High => 4,
        };
        let mf = info.max_frames.max(1);
        self.os = [
            Oversampler::new(self.os_factor, mf),
            Oversampler::new(self.os_factor, mf),
        ];
        let lat = self.os[0].latency() as usize;
        self.dry = [DelayLine::new(lat + 1), DelayLine::new(lat + 1)];
        self.tmp = [vec![0.0; mf], vec![0.0; mf]];
        self.lfo = Lfo::new(info.seed);
        self.update_all();
        // the mix smoother runs at the base rate
        self.mix.set_time(0.01, self.sr as f32);
        self.reset();
    }

    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        // re-align a synced LFO when the transport starts, jumps or changes tempo
        let tempo_changed = ctx.bpm > 0.0 && ctx.bpm != self.bpm;
        if tempo_changed {
            self.bpm = ctx.bpm;
            self.update_lfo_rate();
        }
        let jumped = !self.was_playing || self.next_pos != Some(ctx.song_pos_samples);
        self.was_playing = ctx.playing;
        if ctx.playing && (jumped || tempo_changed) {
            self.base = (ctx.song_pos_samples, ctx.song_pos_ticks / 960.0);
            self.resync(ctx.song_pos_samples);
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
            if let EventKind::Param { id, value } = ev.kind {
                self.set_param(id, value);
            }
        }
        if pos < n {
            self.render(io, pos, n);
        }
        if self.silence.update(io, n, self.tail()) {
            ProcessStatus::Sleep
        } else {
            ProcessStatus::Continue
        }
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        if let Some(i) = self.p.set(id, value) {
            self.apply(i);
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.p.get(id)
    }

    fn reset(&mut self) {
        for o in &mut self.os {
            o.reset();
        }
        for c in 0..2 {
            self.svf[c].reset();
            self.ladder[c].reset();
            self.dry[c].clear();
        }
        self.lfo.set_phase(0.0);
        self.ctrl = 0;
        self.next_pos = None;
        self.was_playing = false;
        self.silence.reset();
    }

    fn latency(&self) -> u32 {
        self.os[0].latency()
    }

    fn tail(&self) -> u32 {
        // resonance can ring; half a second covers Q ≈ 50 at low cutoffs
        self.latency() + (self.sr * 0.5) as u32
    }
}
