//! Synthesis voices. Each function turns explicit recipe parameters into a finished mono
//! one-shot. Techniques are standard public drum-synthesis practice: swept sine kicks,
//! membrane modes plus filtered noise for snares and toms (mode ratios are the zeros of
//! the Bessel functions for an ideal circular membrane), inharmonic square-wave clusters
//! for metallic percussion, and randomly tuned decaying plate modes for cymbals.

use crate::dmath::{exp, exp2, ln, pow, sin};
use crate::dsp::*;

/// Named recipe parameters.
pub struct P<'a>(pub &'a [(&'static str, f64)]);

impl P<'_> {
    pub fn g(&self, key: &str) -> f64 {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap_or_else(|| panic!("recipe parameter `{key}` missing"))
            .1
    }
}

/// Ideal circular membrane mode frequency ratios (Bessel zeros j(m,n) / j(0,1)).
const MEMBRANE: [f64; 8] = [1.0, 1.593, 2.136, 2.296, 2.653, 2.918, 3.156, 3.500];

/// Swept-sine kick: pitch falls exponentially from `f_start` to `f_end`.
pub fn kick(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut lp = Biquad::lowpass(p.g("click_lp"), 0.7);
    let (mut ph, mut ph2) = (0.0f64, 0.0f64);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / SR;
        let f = p.g("f_end") + (p.g("f_start") - p.g("f_end")) * exp(-t / p.g("sweep_tau"));
        ph += f / SR;
        ph -= ph.floor();
        let mut x = sin(TAU * ph) * decay(i, p.g("amp_tau"));
        if p.g("p2_level") > 0.0 {
            ph2 += f * p.g("p2_ratio") / SR;
            ph2 -= ph2.floor();
            x += p.g("p2_level") * sin(TAU * ph2) * decay(i, p.g("p2_tau"));
        }
        x += p.g("click") * lp.process(rng.bipolar()) * decay(i, p.g("click_tau"));
        out.push(soft_clip(x, p.g("drive")));
    }
    finish(out, p.g("release"))
}

/// Membrane drum: a bank of pitch-dropping modes, optional snare wires (filtered noise)
/// and a stick click. Used for snares and toms.
pub fn membrane(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let modes = p.g("modes") as usize;
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut hp = Biquad::highpass(p.g("noise_hp"), 0.7);
    let mut lp = Biquad::lowpass(p.g("noise_lp"), 0.7);
    let mut click_lp = Biquad::lowpass(3500.0, 0.7);
    let mut ph = [0.0f64; 8];
    let drop = exp2(p.g("drop")) - 1.0;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / SR;
        let bend = 1.0 + drop * exp(-t / p.g("drop_tau"));
        let mut x = 0.0;
        for k in 0..modes {
            ph[k] += p.g("f0") * MEMBRANE[k] * bend / SR;
            ph[k] -= ph[k].floor();
            let amp = exp(-p.g("rolloff") * ln(k as f64 + 1.0));
            x += amp * sin(TAU * ph[k]) * decay(i, p.g("tau") / (1.0 + k as f64 * p.g("damp")));
        }
        let noise = lp.process(hp.process(rng.bipolar())) * decay(i, p.g("noise_tau"));
        x += p.g("noise") * noise;
        x += p.g("click") * click_lp.process(rng.bipolar()) * decay(i, 0.002);
        out.push(soft_clip(x, p.g("drive")));
    }
    finish(out, p.g("release"))
}

/// Rim shot / side stick: two fast-decaying resonances plus a band-passed noise tick.
pub fn rim(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut bp = Biquad::bandpass(p.g("noise_bp"), 1.2);
    let (mut p1, mut p2) = (0.0f64, 0.0f64);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        p1 += p.g("f1") / SR;
        p1 -= p1.floor();
        p2 += p.g("f2") / SR;
        p2 -= p2.floor();
        let mut x = sin(TAU * p1) * decay(i, p.g("tau"));
        x += 0.8 * sin(TAU * p2) * decay(i, p.g("tau") * 1.6);
        x += p.g("noise") * bp.process(rng.bipolar()) * decay(i, p.g("noise_tau"));
        out.push(soft_clip(x, p.g("drive")));
    }
    finish(out, p.g("release"))
}

/// Hand clap: a few short noise bursts followed by a longer tail, band-passed.
pub fn clap(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let bursts = p.g("bursts") as usize;
    let spacing = frames(p.g("spacing"));
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut bp = Biquad::bandpass(p.g("bp"), p.g("q"));
    let mut hp = Biquad::highpass(300.0, 0.7);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let k = (i / spacing).min(bursts - 1);
        let local = i - k * spacing;
        let env = if k == bursts - 1 {
            decay(local, p.g("tail_tau"))
        } else {
            decay(local, p.g("burst_tau"))
        };
        let y = hp.process(bp.process(rng.bipolar() * env));
        out.push(soft_clip(y, p.g("drive")));
    }
    finish(out, p.g("release"))
}

/// Metallic hat / cymbal from an inharmonic cluster of band-limited square waves plus noise.
pub fn metal(p: &P) -> Vec<f64> {
    const RATIOS: [f64; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
    let n = frames(p.g("dur"));
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut osc: Vec<Square> = (0..6).map(|k| Square::new(k as f64 * 0.137)).collect();
    let mut hp1 = Biquad::highpass(p.g("hp"), 0.7);
    let mut hp2 = Biquad::highpass(p.g("hp"), 0.7);
    let mut bp = Biquad::bandpass(p.g("bp"), p.g("q"));
    let mut lp1 = Biquad::lowpass(p.g("lp"), 0.7);
    let mut lp2 = Biquad::lowpass(p.g("lp"), 0.7);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut x = 0.0;
        for (o, r) in osc.iter_mut().zip(RATIOS) {
            x += o.next(r * p.g("pitch"));
        }
        x = x / 6.0 + p.g("noise") * rng.bipolar();
        let band = bp.process(x);
        let y = lp2.process(lp1.process(hp2.process(hp1.process(x)) + p.g("bp_mix") * band));
        let env = decay(i, p.g("tau"));
        let thud = if p.g("thud") > 0.0 {
            p.g("thud") * sin(TAU * 250.0 * i as f64 / SR) * decay(i, 0.02)
        } else {
            0.0
        };
        out.push(y * env + thud);
    }
    finish(out, p.g("release"))
}

/// Physical-model cymbal / hat: many randomly tuned plate modes that decay faster at
/// higher frequency, plus noise.
pub fn plate(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let modes = p.g("modes") as usize;
    let mut rng = Rng::new(p.g("seed") as u64);
    let (lo, hi) = (p.g("f_lo"), p.g("f_hi"));
    let span = ln(hi / lo);
    let bank: Vec<(f64, f64, f64, f64)> = (0..modes)
        .map(|_| {
            let f = lo * exp(rng.uniform() * span);
            let amp = 0.3 + 0.7 * rng.uniform();
            let tau = p.g("tau") * pow(lo / f, 0.5) * (0.6 + 0.8 * rng.uniform());
            let phase = rng.uniform();
            (f, amp, tau, phase)
        })
        .collect();
    let mut hp = Biquad::highpass(p.g("hp"), 0.7);
    let mut out = vec![0.0f64; n];
    for (f, amp, tau, phase) in bank {
        for (i, o) in out.iter_mut().enumerate() {
            let e = decay(i, tau);
            if e < 1e-5 {
                break;
            }
            *o += amp * e * sin(TAU * (f * i as f64 / SR + phase));
        }
    }
    let norm = 1.0 / (modes as f64).sqrt();
    for (i, o) in out.iter_mut().enumerate() {
        let noise = hp.process(rng.bipolar()) * decay(i, p.g("noise_tau"));
        *o = *o * norm + p.g("noise") * noise;
    }
    finish(out, p.g("release"))
}

/// Cowbell: two detuned squares through a band-pass with a two-stage decay.
pub fn cowbell(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let (mut a, mut b) = (Square::new(0.0), Square::new(0.31));
    let mut bp = Biquad::bandpass(p.g("bp"), p.g("q"));
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = a.next(p.g("f1")) + b.next(p.g("f2"));
        let env =
            p.g("mix_a") * decay(i, p.g("tau_a")) + (1.0 - p.g("mix_a")) * decay(i, p.g("tau_b"));
        out.push(soft_clip(bp.process(x) * env, p.g("drive")));
    }
    finish(out, p.g("release"))
}

/// Shaker: band-passed noise with a soft attack swell.
pub fn shaker(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut bp = Biquad::bandpass(p.g("bp"), p.g("q"));
    let mut hp = Biquad::highpass(p.g("hp"), 0.7);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let env = (1.0 - decay(i, p.g("rise"))) * decay(i, p.g("tau"));
        out.push(hp.process(bp.process(rng.bipolar())) * env);
    }
    finish(out, p.g("release"))
}

/// Clave / wood block: two stiff, quickly damped partials.
pub fn clave(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let mut rng = Rng::new(p.g("seed") as u64);
    let mut tick = Biquad::bandpass(p.g("f1") * 1.5, 1.0);
    let (mut p1, mut p2) = (0.0f64, 0.0f64);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        p1 += p.g("f1") / SR;
        p1 -= p1.floor();
        p2 += p.g("f1") * p.g("ratio") / SR;
        p2 -= p2.floor();
        let mut x = sin(TAU * p1) * decay(i, p.g("tau"));
        x += 0.4 * sin(TAU * p2) * decay(i, p.g("tau") * 0.6);
        x += 0.3 * tick.process(rng.bipolar()) * decay(i, 0.001);
        out.push(soft_clip(x, p.g("drive")));
    }
    finish(out, p.g("release"))
}

/// Long tuned 808-style sub: a sine with a brief downward pitch settle and gentle drive.
pub fn sub(p: &P) -> Vec<f64> {
    let n = frames(p.g("dur"));
    let mut ph = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / SR;
        let f = p.g("f") * exp2(p.g("settle") / 12.0 * exp(-t / p.g("settle_tau")));
        ph += f / SR;
        ph -= ph.floor();
        let attack = (t / p.g("attack")).min(1.0);
        let x = sin(TAU * ph) * attack * decay(i, p.g("tau"));
        out.push(soft_clip(x, p.g("drive")));
    }
    finish(out, p.g("release"))
}
