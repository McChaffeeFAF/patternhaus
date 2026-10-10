//! `ph.fx.compressor`: feed-forward, log-domain compressor.
//!
//! After Giannoulis, Massberg & Reiss, "Digital Dynamic Range Compressor Design — A
//! Tutorial and Analysis" (JAES 2012): stereo-linked level detection (instantaneous peak,
//! or RMS over a 10 ms window), a soft-knee gain computer in dB, and a smooth branching
//! peak detector on the gain reduction for attack/release. The detector can listen to the
//! sidechain input (port 1) through an optional high-pass. Lookahead delays the audio path
//! and is reported through `latency()`.

use crate::common::{Params, SilenceTracker, input, output, stereo_desc};
use ph_dsp::delay::DelayLine;
use ph_dsp::dynamics::time_coef;
use ph_dsp::filter::{Svf, tpt_g};
use ph_dsp::smooth::OnePole;
use ph_dsp::{db_to_lin, lin_to_db};
use ph_node::*;

const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const KNEE: usize = 2;
const ATTACK: usize = 3;
const RELEASE: usize = 4;
const MAKEUP: usize = 5;
const DETECT: usize = 6;
const LOOKAHEAD: usize = 7;
const SIDECHAIN: usize = 8;
const SC_HPF: usize = 9;

const MAX_LOOKAHEAD_MS: f64 = 5.0;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new(
            "threshold",
            "Threshold",
            -60.0,
            0.0,
            -18.0,
            ParamUnit::Decibels,
        ),
        ParamInfo::new("ratio", "Ratio", 1.0, 30.0, 4.0, ParamUnit::Ratio),
        ParamInfo::new("knee", "Knee", 0.0, 24.0, 6.0, ParamUnit::Decibels),
        ParamInfo::new(
            "attack",
            "Attack",
            0.05,
            250.0,
            10.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new(
            "release",
            "Release",
            5.0,
            2500.0,
            150.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new("makeup", "Makeup", -12.0, 24.0, 0.0, ParamUnit::Decibels),
        ParamInfo::choice("detector", "Detector", &["Peak", "RMS"], 0),
        ParamInfo::new(
            "lookahead",
            "Lookahead",
            0.0,
            MAX_LOOKAHEAD_MS,
            0.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::choice("sidechain", "Sidechain", &["Internal", "External"], 0),
        ParamInfo::new(
            "sc_hpf",
            "Sidechain High-Pass",
            20.0,
            500.0,
            20.0,
            ParamUnit::Hertz,
        ),
    ];
    stereo_desc("ph.fx.compressor", "Compressor", params, true)
}

/// Static gain computer: output level (dB) for input level `x` (dB).
pub fn static_curve(x: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let d = x - threshold;
    if 2.0 * d < -knee {
        x
    } else if knee > 0.0 && 2.0 * d.abs() <= knee {
        let t = d + knee * 0.5;
        x + (1.0 / ratio - 1.0) * t * t / (2.0 * knee)
    } else {
        threshold + d / ratio
    }
}

pub struct Compressor {
    p: Params,
    sr: f32,
    att: f32,
    rel: f32,
    rms_coef: f32,
    ms: f32,
    gr: f32,
    makeup: OnePole,
    lookahead: usize,
    delay: [DelayLine; 2],
    hpf: [Svf; 2],
    silence: SilenceTracker,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            att: 0.0,
            rel: 0.0,
            rms_coef: 0.0,
            ms: 0.0,
            gr: 0.0,
            makeup: OnePole::new(1.0),
            lookahead: 0,
            delay: [DelayLine::new(1), DelayLine::new(1)],
            hpf: [Svf::new(), Svf::new()],
            silence: SilenceTracker::default(),
        };
        s.update_all();
        s
    }

    fn update_all(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let t = self.makeup.target();
        self.makeup.reset(t);
    }

    fn apply(&mut self, i: usize) {
        match i {
            ATTACK => self.att = time_coef(self.p.f(ATTACK) * 1e-3, self.sr),
            RELEASE => self.rel = time_coef(self.p.f(RELEASE) * 1e-3, self.sr),
            MAKEUP => {
                self.makeup.set_time(0.02, self.sr);
                self.makeup.set_target(db_to_lin(self.p.f(MAKEUP)));
            }
            LOOKAHEAD => {
                let max = self.delay[0].max_delay();
                self.lookahead =
                    ((self.p.v[LOOKAHEAD] * 1e-3 * self.sr as f64).round() as usize).min(max);
            }
            SC_HPF => {
                let g = tpt_g(self.p.f(SC_HPF), self.sr);
                for h in &mut self.hpf {
                    h.set_coefs(g, std::f32::consts::SQRT_2);
                }
            }
            DETECT => self.rms_coef = time_coef(0.01, self.sr),
            _ => {}
        }
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let thr = self.p.f(THRESHOLD);
        let ratio = self.p.f(RATIO);
        let knee = self.p.f(KNEE);
        let rms = self.p.u(DETECT) == 1;
        let sc_port = if self.p.u(SIDECHAIN) == 1 { 1 } else { 0 };
        let hpf_on = self.p.f(SC_HPF) > 20.5;
        for i in a..b {
            let l = input(io, 0, 0, i);
            let r = input(io, 0, 1, i);
            let mut dl = input(io, sc_port, 0, i);
            let mut dr = input(io, sc_port, 1, i);
            if hpf_on {
                dl = self.hpf[0].tick(dl).2;
                dr = self.hpf[1].tick(dr).2;
            }
            // stereo-linked level
            let level = if rms {
                let m = 0.5 * (dl * dl + dr * dr);
                self.ms = ph_dsp::flush_denormal(m + (self.ms - m) * self.rms_coef);
                self.ms.sqrt()
            } else {
                dl.abs().max(dr.abs())
            };
            let x_db = lin_to_db(level).max(-200.0);
            let target = x_db - static_curve(x_db, thr, ratio, knee); // reduction, ≥ 0
            let c = if target > self.gr { self.att } else { self.rel };
            self.gr = ph_dsp::flush_denormal(target + (self.gr - target) * c);
            let g = db_to_lin(-self.gr) * self.makeup.next();
            self.delay[0].push(l);
            self.delay[1].push(r);
            let ol = self.delay[0].read(self.lookahead);
            let or = self.delay[1].read(self.lookahead);
            output(io, i, ol * g, or * g);
        }
    }
}

impl Processor for Compressor {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        let max = (MAX_LOOKAHEAD_MS * 1e-3 * info.sample_rate).ceil() as usize + 1;
        self.delay = [DelayLine::new(max), DelayLine::new(max)];
        self.update_all();
        self.reset();
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
        self.ms = 0.0;
        self.gr = 0.0;
        for d in &mut self.delay {
            d.clear();
        }
        for h in &mut self.hpf {
            h.reset();
        }
        self.silence.reset();
    }

    fn latency(&self) -> u32 {
        self.lookahead as u32
    }

    fn tail(&self) -> u32 {
        self.lookahead as u32 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_curve_shape() {
        // below knee: unity
        assert_eq!(static_curve(-40.0, -20.0, 4.0, 6.0), -40.0);
        // far above: threshold + excess/ratio
        assert!((static_curve(0.0, -20.0, 4.0, 6.0) + 15.0).abs() < 1e-5);
        // continuous at the knee edges
        let lo = static_curve(-23.0, -20.0, 4.0, 6.0);
        let hi = static_curve(-17.0, -20.0, 4.0, 6.0);
        assert!((lo + 23.0).abs() < 1e-5);
        assert!((hi - (-20.0 + 3.0 / 4.0)).abs() < 1e-5);
        assert_eq!(descriptor().params.len(), 10);
    }
}
