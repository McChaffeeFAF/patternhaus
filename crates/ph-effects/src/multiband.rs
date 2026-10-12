//! `ph.fx.multiband`: three-band compressor on Linkwitz-Riley crossovers.
//!
//! The input is split with 4th-order Linkwitz-Riley filters (two cascaded Butterworth
//! sections; Linkwitz, "Active crossover networks for noncoincident drivers", JAES 1976) at
//! two frequencies. A 4th-order LR low-pass and high-pass sum to an all-pass, so the low band
//! is passed through the upper crossover's all-pass (its own low-pass plus high-pass) and the
//! three bands sum flat in magnitude with no compression applied. Each band has its own
//! stereo-linked, log-domain, soft-knee compressor (the design of `ph.fx.compressor`), a
//! make-up gain, and solo / bypass switches.

use crate::common::{Params, SilenceTracker, impl_processor, input, output, stereo_desc, switch};
use crate::compressor::static_curve;
use ph_dsp::dynamics::time_coef;
use ph_dsp::filter::{Biquad, BiquadCoefs, BiquadShape};
use ph_dsp::smooth::OnePole;
use ph_dsp::{db_to_lin, flush_denormal, lin_to_db};
use ph_node::*;

const XO_LOW: usize = 0;
const XO_HIGH: usize = 1;
const BAND0: usize = 2;
const PER_BAND: usize = 7;
const B_THRESHOLD: usize = 0;
const B_RATIO: usize = 1;
const B_ATTACK: usize = 2;
const B_RELEASE: usize = 3;
const B_GAIN: usize = 4;
const B_SOLO: usize = 5;
const B_BYPASS: usize = 6;
const KNEE: usize = BAND0 + 3 * PER_BAND;
const OUTPUT: usize = KNEE + 1;

const Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

pub fn descriptor() -> NodeDescriptor {
    let mut params = vec![
        ParamInfo::new(
            "xover_low",
            "Low Crossover",
            40.0,
            2000.0,
            200.0,
            ParamUnit::Hertz,
        ),
        ParamInfo::new(
            "xover_high",
            "High Crossover",
            500.0,
            16000.0,
            2500.0,
            ParamUnit::Hertz,
        ),
    ];
    for b in 1..=3 {
        let n = |s: &str| format!("{s} {b}");
        let id = |s: &str| format!("b{b}_{s}");
        params.push(ParamInfo::new(
            &id("threshold"),
            &n("Threshold"),
            -60.0,
            0.0,
            -18.0,
            ParamUnit::Decibels,
        ));
        params.push(ParamInfo::new(
            &id("ratio"),
            &n("Ratio"),
            1.0,
            20.0,
            3.0,
            ParamUnit::Ratio,
        ));
        params.push(ParamInfo::new(
            &id("attack"),
            &n("Attack"),
            0.1,
            200.0,
            10.0,
            ParamUnit::Milliseconds,
        ));
        params.push(ParamInfo::new(
            &id("release"),
            &n("Release"),
            10.0,
            2000.0,
            120.0,
            ParamUnit::Milliseconds,
        ));
        params.push(ParamInfo::new(
            &id("gain"),
            &n("Gain"),
            -12.0,
            12.0,
            0.0,
            ParamUnit::Decibels,
        ));
        params.push(switch(&id("solo"), &n("Solo"), false));
        params.push(switch(&id("bypass"), &n("Bypass"), false));
    }
    params.push(ParamInfo::new(
        "knee",
        "Knee",
        0.0,
        12.0,
        6.0,
        ParamUnit::Decibels,
    ));
    params.push(ParamInfo::new(
        "output",
        "Output",
        -12.0,
        12.0,
        0.0,
        ParamUnit::Decibels,
    ));
    stereo_desc("ph.fx.multiband", "Multiband Compressor", params, false)
}

/// A pair of cascaded identical Butterworth sections (one 4th-order LR half).
#[derive(Clone, Default)]
struct Lr4([Biquad; 2]);

impl Lr4 {
    fn set(&mut self, c: BiquadCoefs) {
        self.0[0].c = c;
        self.0[1].c = c;
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.0[0].process(x);
        self.0[1].process(y)
    }

    fn reset(&mut self) {
        self.0[0].reset();
        self.0[1].reset();
    }
}

/// The crossover network of one channel.
#[derive(Clone, Default)]
struct Split {
    lp1: Lr4,
    hp1: Lr4,
    /// all-pass (lp2 + hp2) applied to the low band
    lo_lp2: Lr4,
    lo_hp2: Lr4,
    mid_lp2: Lr4,
    hi_hp2: Lr4,
}

impl Split {
    #[inline]
    fn tick(&mut self, x: f32) -> [f32; 3] {
        let x = x as f64;
        let low = self.lp1.tick(x);
        let high = self.hp1.tick(x);
        let lo = self.lo_lp2.tick(low) + self.lo_hp2.tick(low);
        let mid = self.mid_lp2.tick(high);
        let hi = self.hi_hp2.tick(high);
        [lo as f32, mid as f32, hi as f32]
    }

    fn reset(&mut self) {
        for f in [
            &mut self.lp1,
            &mut self.hp1,
            &mut self.lo_lp2,
            &mut self.lo_hp2,
            &mut self.mid_lp2,
            &mut self.hi_hp2,
        ] {
            f.reset();
        }
    }
}

#[derive(Clone)]
struct Band {
    att: f32,
    rel: f32,
    gr: f32,
    gain: OnePole,
}

pub struct Multiband {
    p: Params,
    sr: f32,
    split: [Split; 2],
    bands: [Band; 3],
    out: OnePole,
    silence: SilenceTracker,
}

impl Default for Multiband {
    fn default() -> Self {
        Self::new()
    }
}

impl Multiband {
    pub fn new() -> Self {
        let band = Band {
            att: 0.0,
            rel: 0.0,
            gr: 0.0,
            gain: OnePole::new(1.0),
        };
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            split: [Split::default(), Split::default()],
            bands: [band.clone(), band.clone(), band],
            out: OnePole::new(1.0),
            silence: SilenceTracker::default(),
        };
        s.snap();
        s
    }

    fn snap(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let v = self.out.target();
        self.out.reset(v);
        for b in &mut self.bands {
            let v = b.gain.target();
            b.gain.reset(v);
        }
    }

    /// Effective crossover frequencies: the upper one is kept at least half an octave above.
    fn crossovers(&self) -> (f64, f64) {
        let f1 = self.p.v[XO_LOW];
        let f2 = self.p.v[XO_HIGH].max(f1 * 1.5);
        (f1, f2.min(0.45 * self.sr as f64))
    }

    fn apply(&mut self, i: usize) {
        match i {
            XO_LOW | XO_HIGH => {
                let (f1, f2) = self.crossovers();
                let sr = self.sr as f64;
                let lp1 = BiquadCoefs::design(BiquadShape::LowPass, f1, Q, sr);
                let hp1 = BiquadCoefs::design(BiquadShape::HighPass, f1, Q, sr);
                let lp2 = BiquadCoefs::design(BiquadShape::LowPass, f2, Q, sr);
                let hp2 = BiquadCoefs::design(BiquadShape::HighPass, f2, Q, sr);
                for s in &mut self.split {
                    s.lp1.set(lp1);
                    s.hp1.set(hp1);
                    s.lo_lp2.set(lp2);
                    s.lo_hp2.set(hp2);
                    s.mid_lp2.set(lp2);
                    s.hi_hp2.set(hp2);
                }
            }
            i if (BAND0..KNEE).contains(&i) => {
                let b = (i - BAND0) / PER_BAND;
                let base = BAND0 + b * PER_BAND;
                match i - base {
                    B_ATTACK => {
                        self.bands[b].att = time_coef(self.p.f(i) * 1e-3, self.sr);
                    }
                    B_RELEASE => {
                        self.bands[b].rel = time_coef(self.p.f(i) * 1e-3, self.sr);
                    }
                    B_GAIN => {
                        self.bands[b].gain.set_time(0.02, self.sr);
                        self.bands[b].gain.set_target(db_to_lin(self.p.f(i)));
                    }
                    _ => {}
                }
            }
            OUTPUT => {
                self.out.set_time(0.02, self.sr);
                self.out.set_target(db_to_lin(self.p.f(OUTPUT)));
            }
            _ => {}
        }
    }

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        self.snap();
    }

    fn on_reset(&mut self) {
        for s in &mut self.split {
            s.reset();
        }
        for b in &mut self.bands {
            b.gr = 0.0;
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        (self.sr * 0.25) as u32
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let knee = self.p.f(KNEE);
        let mut thr = [0.0f32; 3];
        let mut ratio = [0.0f32; 3];
        let mut bypass = [false; 3];
        let mut solo = [false; 3];
        for k in 0..3 {
            let base = BAND0 + k * PER_BAND;
            thr[k] = self.p.f(base + B_THRESHOLD);
            ratio[k] = self.p.f(base + B_RATIO);
            bypass[k] = self.p.on(base + B_BYPASS);
            solo[k] = self.p.on(base + B_SOLO);
        }
        let any_solo = solo.iter().any(|s| *s);
        for i in a..b {
            let bl = self.split[0].tick(input(io, 0, 0, i));
            let br = self.split[1].tick(input(io, 0, 1, i));
            let og = self.out.next();
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for k in 0..3 {
                let band = &mut self.bands[k];
                let makeup = band.gain.next();
                if any_solo && !solo[k] {
                    continue;
                }
                let g = if bypass[k] {
                    band.gr = 0.0;
                    1.0
                } else {
                    let level = bl[k].abs().max(br[k].abs());
                    let x_db = lin_to_db(level).max(-200.0);
                    let target = x_db - static_curve(x_db, thr[k], ratio[k], knee);
                    let c = if target > band.gr { band.att } else { band.rel };
                    band.gr = flush_denormal(target + (band.gr - target) * c);
                    db_to_lin(-band.gr) * makeup
                };
                l += bl[k] * g;
                r += br[k] * g;
            }
            output(io, i, l * og, r * og);
        }
    }
}

impl_processor!(Multiband);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_layout() {
        let d = descriptor();
        assert_eq!(d.params.len(), 25);
        assert_eq!(d.params[KNEE].id, "knee");
        assert_eq!(d.params[OUTPUT].id, "output");
        assert_eq!(d.params[BAND0 + PER_BAND + B_SOLO].id, "b2_solo");
    }
}
