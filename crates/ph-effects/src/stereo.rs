//! `ph.fx.stereo`: stereo utility.
//!
//! Signal path: `swap` -> polarity inversion -> mid/side width (`S' = S * width`, so width 0
//! is mono and 1 is the identity; Blumlein) -> balance (linear: the quieter side is
//! attenuated, the other left at unity) -> mono-below (the content under `mono_below` Hz,
//! split with a 4th-order Linkwitz-Riley crossover, is summed to mono; the high part is the
//! matching high-pass, so the two parts recombine flat in magnitude).

use crate::common::{Params, SilenceTracker, impl_processor, input, output, stereo_desc, switch};
use ph_dsp::filter::{Svf, tpt_g};
use ph_dsp::smooth::OnePole;
use ph_node::*;

const WIDTH: usize = 0;
const BALANCE: usize = 1;
const MONO_BELOW: usize = 2;
const INVERT_L: usize = 3;
const INVERT_R: usize = 4;
const SWAP: usize = 5;

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new("width", "Width", 0.0, 2.0, 1.0, ParamUnit::Percent),
        ParamInfo::new("balance", "Balance", -1.0, 1.0, 0.0, ParamUnit::Pan),
        ParamInfo::new(
            "mono_below",
            "Mono Below",
            0.0,
            500.0,
            0.0,
            ParamUnit::Hertz,
        ),
        switch("invert_l", "Invert Left", false),
        switch("invert_r", "Invert Right", false),
        switch("swap", "Swap L/R", false),
    ];
    stereo_desc("ph.fx.stereo", "Stereo Tools", params, false)
}

pub struct Stereo {
    p: Params,
    sr: f32,
    width: OnePole,
    balance: OnePole,
    lp: [[Svf; 4]; 2],
    silence: SilenceTracker,
}

impl Default for Stereo {
    fn default() -> Self {
        Self::new()
    }
}

impl Stereo {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            width: OnePole::new(1.0),
            balance: OnePole::new(0.0),
            lp: [0; 2].map(|_| [0; 4].map(|_| Svf::new())),
            silence: SilenceTracker::default(),
        };
        s.snap();
        s
    }

    fn snap(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        let (w, b) = (self.width.target(), self.balance.target());
        self.width.reset(w);
        self.balance.reset(b);
    }

    fn apply(&mut self, i: usize) {
        match i {
            WIDTH => {
                self.width.set_time(0.01, self.sr);
                self.width.set_target(self.p.f(WIDTH));
            }
            BALANCE => {
                self.balance.set_time(0.01, self.sr);
                self.balance.set_target(self.p.f(BALANCE));
            }
            MONO_BELOW => {
                let f = self.p.f(MONO_BELOW);
                for ch in &mut self.lp {
                    for s in ch.iter_mut() {
                        s.set_coefs(tpt_g(f, self.sr), std::f32::consts::SQRT_2);
                        if f <= 0.0 {
                            s.reset();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        self.snap();
    }

    fn on_reset(&mut self) {
        for ch in &mut self.lp {
            for s in ch.iter_mut() {
                s.reset();
            }
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        0
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let swap = self.p.on(SWAP);
        let (il, ir) = (
            if self.p.on(INVERT_L) { -1.0f32 } else { 1.0 },
            if self.p.on(INVERT_R) { -1.0f32 } else { 1.0 },
        );
        let mono = self.p.f(MONO_BELOW) > 0.0;
        for i in a..b {
            let width = self.width.next();
            let bal = self.balance.next();
            let (mut l, mut r) = (input(io, 0, 0, i), input(io, 0, 1, i));
            if swap {
                std::mem::swap(&mut l, &mut r);
            }
            l *= il;
            r *= ir;
            let mid = 0.5 * (l + r);
            let side = 0.5 * (l - r) * width;
            l = (mid + side) * (1.0 - bal).min(1.0);
            r = (mid - side) * (1.0 + bal).min(1.0);
            if mono {
                let y = [l, r];
                let mut low = [0.0f32; 2];
                let mut high = [0.0f32; 2];
                for c in 0..2 {
                    let s = self.lp[c][0].tick(y[c]).0;
                    let h = self.lp[c][2].tick(y[c]).2;
                    high[c] = self.lp[c][3].tick(h).2;
                    low[c] = self.lp[c][1].tick(s).0;
                }
                let m = 0.5 * (low[0] + low[1]);
                l = high[0] + m;
                r = high[1] + m;
            }
            output(io, i, l, r);
        }
    }
}

impl_processor!(Stereo);
