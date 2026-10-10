//! `ph.fx.eq`: 6-band parametric EQ (RBJ biquads, `f64` TDF-II).
//!
//! Bands: peak, low/high shelf, low/high cut (12–48 dB/oct Butterworth cascades, the last
//! section's Q follows the band Q for a resonant corner), notch and band-pass. Frequency,
//! gain and Q are smoothed in the log domain and coefficients are recomputed on a fixed
//! 16-sample control grid counted from activation, so automation is zipper-free and the
//! output does not depend on block size.

use crate::common::{Params, SilenceTracker, input, output, stereo_desc, switch};
use ph_dsp::filter::{Biquad, BiquadCoefs, BiquadShape};
use ph_dsp::smooth::OnePole;
use ph_node::*;

pub const BANDS: usize = 6;
const PER_BAND: usize = 6;
const ON: usize = 0;
const TYPE: usize = 1;
const FREQ: usize = 2;
const GAIN: usize = 3;
const Q: usize = 4;
const SLOPE: usize = 5;
const OUTPUT: usize = BANDS * PER_BAND;
const CTRL: u32 = 16;
const MAX_SECTIONS: usize = 4;

pub const BAND_TYPES: [&str; 7] = [
    "Peak",
    "Low Shelf",
    "High Shelf",
    "Low Cut",
    "High Cut",
    "Notch",
    "Band Pass",
];

pub fn descriptor() -> NodeDescriptor {
    let defaults: [(bool, usize, f64); BANDS] = [
        (false, 3, 30.0),
        (true, 1, 100.0),
        (true, 0, 300.0),
        (true, 0, 1000.0),
        (true, 0, 3500.0),
        (true, 2, 10000.0),
    ];
    let mut params = Vec::new();
    for (b, (on, ty, f)) in defaults.iter().enumerate() {
        let n = b + 1;
        params.push(switch(&format!("b{n}_on"), &format!("Band {n} On"), *on));
        params.push(ParamInfo::choice(
            &format!("b{n}_type"),
            &format!("Band {n} Type"),
            &BAND_TYPES,
            *ty,
        ));
        params.push(ParamInfo::new(
            &format!("b{n}_freq"),
            &format!("Band {n} Frequency"),
            20.0,
            20000.0,
            *f,
            ParamUnit::Hertz,
        ));
        params.push(ParamInfo::new(
            &format!("b{n}_gain"),
            &format!("Band {n} Gain"),
            -24.0,
            24.0,
            0.0,
            ParamUnit::Decibels,
        ));
        params.push(ParamInfo::new(
            &format!("b{n}_q"),
            &format!("Band {n} Q"),
            0.1,
            18.0,
            std::f64::consts::FRAC_1_SQRT_2,
            ParamUnit::Ratio,
        ));
        params.push(ParamInfo::choice(
            &format!("b{n}_slope"),
            &format!("Band {n} Slope"),
            &["12 dB/oct", "24 dB/oct", "36 dB/oct", "48 dB/oct"],
            0,
        ));
    }
    params.push(ParamInfo::new(
        "output",
        "Output",
        -24.0,
        24.0,
        0.0,
        ParamUnit::Decibels,
    ));
    stereo_desc("ph.fx.eq", "Parametric EQ", params, false)
}

/// Butterworth section Qs for 2..8th order (12..48 dB/oct).
const BUTTER_Q: [&[f64]; 4] = [
    &[std::f64::consts::FRAC_1_SQRT_2],
    &[0.541_196_1, 1.306_563],
    &[0.517_638_1, std::f64::consts::FRAC_1_SQRT_2, 1.931_851_7],
    &[0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
];

struct Band {
    log_f: OnePole,
    gain: OnePole,
    log_q: OnePole,
    sections: usize,
    filt: [[Biquad; MAX_SECTIONS]; 2],
    dirty: bool,
}

impl Band {
    fn new() -> Self {
        Self {
            log_f: OnePole::new(0.0),
            gain: OnePole::new(0.0),
            log_q: OnePole::new(0.0),
            sections: 1,
            filt: Default::default(),
            dirty: true,
        }
    }
}

pub struct Eq {
    p: Params,
    sr: f64,
    bands: Vec<Band>,
    out_gain: OnePole,
    ctrl: u32,
    silence: SilenceTracker,
}

impl Default for Eq {
    fn default() -> Self {
        Self::new()
    }
}

impl Eq {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            bands: (0..BANDS).map(|_| Band::new()).collect(),
            out_gain: OnePole::new(1.0),
            ctrl: 0,
            silence: SilenceTracker::default(),
        };
        s.snap_all();
        s
    }

    fn idx(b: usize, k: usize) -> usize {
        b * PER_BAND + k
    }

    fn set_targets(&mut self, b: usize) {
        let f = self.p.v[Self::idx(b, FREQ)].log2() as f32;
        let g = self.p.f(Self::idx(b, GAIN));
        let q = self.p.v[Self::idx(b, Q)].ln() as f32;
        let band = &mut self.bands[b];
        band.log_f.set_target(f);
        band.gain.set_target(g);
        band.log_q.set_target(q);
        band.dirty = true;
    }

    fn snap_all(&mut self) {
        let ctrl_sr = (self.sr / CTRL as f64) as f32;
        for b in 0..BANDS {
            self.set_targets(b);
            let band = &mut self.bands[b];
            for s in [&mut band.log_f, &mut band.gain, &mut band.log_q] {
                s.set_time(0.02, ctrl_sr);
                let t = s.target();
                s.reset(t);
            }
            Self::design(&self.p, self.sr, b, band);
        }
        self.out_gain.set_time(0.02, self.sr as f32);
        self.out_gain.reset(ph_dsp::db_to_lin(self.p.f(OUTPUT)));
    }

    fn design(p: &Params, sr: f64, b: usize, band: &mut Band) {
        let ty = p.u(Self::idx(b, TYPE));
        let f = (band.log_f.value() as f64).exp2();
        let g = band.gain.value() as f64;
        let q = (band.log_q.value() as f64).exp();
        let slope = p.u(Self::idx(b, SLOPE));
        let single = |shape| (1usize, [BiquadCoefs::design(shape, f, q, sr); MAX_SECTIONS]);
        let (n, coefs) = match ty {
            0 => single(BiquadShape::Peak { gain_db: g }),
            1 => single(BiquadShape::LowShelf { gain_db: g }),
            2 => single(BiquadShape::HighShelf { gain_db: g }),
            3 | 4 => {
                let shape = if ty == 3 {
                    BiquadShape::HighPass
                } else {
                    BiquadShape::LowPass
                };
                let qs = BUTTER_Q[slope.min(3)];
                let mut c = [BiquadCoefs::IDENTITY; MAX_SECTIONS];
                for (i, sq) in qs.iter().enumerate() {
                    // the highest-Q section carries the user's resonance
                    let qq = if i == qs.len() - 1 {
                        sq * q / std::f64::consts::FRAC_1_SQRT_2
                    } else {
                        *sq
                    };
                    c[i] = BiquadCoefs::design(shape, f, qq, sr);
                }
                (qs.len(), c)
            }
            5 => single(BiquadShape::Notch),
            _ => single(BiquadShape::BandPass),
        };
        band.sections = n;
        for ch in &mut band.filt {
            for (s, c) in ch.iter_mut().zip(coefs.iter()) {
                s.c = *c;
            }
        }
        band.dirty = false;
    }

    fn control_tick(&mut self) {
        for b in 0..BANDS {
            let band = &mut self.bands[b];
            let moving =
                !(band.log_f.is_settled() && band.gain.is_settled() && band.log_q.is_settled());
            band.log_f.next();
            band.gain.next();
            band.log_q.next();
            if moving || band.dirty {
                Self::design(&self.p, self.sr, b, band);
            }
        }
    }

    fn apply(&mut self, i: usize) {
        if i == OUTPUT {
            self.out_gain
                .set_target(ph_dsp::db_to_lin(self.p.f(OUTPUT)));
        } else {
            self.set_targets(i / PER_BAND);
        }
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        for i in a..b {
            if self.ctrl == 0 {
                self.control_tick();
            }
            self.ctrl = (self.ctrl + 1) % CTRL;
            let mut x = [input(io, 0, 0, i) as f64, input(io, 0, 1, i) as f64];
            for (bi, band) in self.bands.iter_mut().enumerate() {
                if !self.p.on(Self::idx(bi, ON)) {
                    continue;
                }
                for (ch, xc) in x.iter_mut().enumerate() {
                    for s in &mut band.filt[ch][..band.sections] {
                        *xc = s.process(*xc);
                    }
                }
            }
            let g = self.out_gain.next() as f64;
            output(io, i, (x[0] * g) as f32, (x[1] * g) as f32);
        }
    }
}

impl Processor for Eq {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.p.desc
    }

    fn activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate;
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
        self.ctrl = 0;
        self.silence.reset();
        for band in &mut self.bands {
            for ch in &mut band.filt {
                for s in ch {
                    s.reset();
                }
            }
        }
        self.snap_all();
    }

    fn tail(&self) -> u32 {
        // generous ring-out for a high-Q low band
        (self.sr * 0.5) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_count() {
        assert_eq!(descriptor().params.len(), 37);
        assert!(descriptor().param_id("b6_slope").is_some());
        let _ = Eq::new().p.len();
    }
}
