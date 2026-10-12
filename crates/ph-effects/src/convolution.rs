//! `ph.fx.convolution`: zero-latency partitioned-FFT convolution reverb.
//!
//! The impulse response comes from an [`AssetResolver`] (`factory:ir/<name>` or any decodable
//! sample reference) or is generated in code (`generated:room|hall|plate|spring`). It is
//! normalised to unit energy, resampled to the engine rate if needed, and split into
//! partitions of [`BLOCK`] samples.
//!
//! The first partition is convolved directly in the time domain, so the effect adds no
//! latency; all later partitions use the uniformly partitioned overlap-add scheme with a
//! frequency-domain delay line (Wefers, "Partitioned convolution algorithms for real-time
//! auralization", 2015; Gardner, "Efficient convolution without input-output delay", JAES
//! 1995). Everything advances sample by sample and the FFTs run at fixed absolute sample
//! positions, so the output does not depend on the host's block size. The wet signal passes
//! through a pre-delay, low-cut and high-cut filters and a gain before the dry/wet mix.
//!
//! The reference is the processor's state ([`Processor::save_state`] / `load_state`), not a
//! parameter: choosing an impulse response allocates, which parameter changes may not.

use crate::common::{
    Fft, Params, SilenceTracker, crossfade, impl_processor, input, output, stereo_desc,
};
use ph_dsp::delay::DelayLine;
use ph_dsp::filter::{Svf, tpt_g};
use ph_dsp::interp::hermite;
use ph_dsp::smooth::OnePole;
use ph_dsp::{Rng, db_to_lin, flush_denormal};
use ph_node::*;
use std::sync::Arc;

/// Partition length (and the size of the time-domain head).
const BLOCK: usize = 256;
const FFT_LEN: usize = 2 * BLOCK;
const BINS: usize = BLOCK + 1;
/// Longest impulse response used, in seconds.
const MAX_IR_SECONDS: f64 = 6.0;
const MAX_PREDELAY_MS: f64 = 250.0;

const PREDELAY: usize = 0;
const MIX: usize = 1;
const LOW_CUT: usize = 2;
const HIGH_CUT: usize = 3;
const TRIM: usize = 4;
const GAIN: usize = 5;

/// Built-in generated impulse responses, selectable as `generated:<name>`.
pub const GENERATED: [&str; 4] = ["room", "hall", "plate", "spring"];
/// The reference used until another is loaded.
pub const DEFAULT_REFERENCE: &str = "generated:hall";

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::new(
            "predelay",
            "Pre-Delay",
            0.0,
            MAX_PREDELAY_MS,
            0.0,
            ParamUnit::Milliseconds,
        ),
        ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.3, ParamUnit::Percent),
        ParamInfo::new("low_cut", "Low Cut", 20.0, 1000.0, 20.0, ParamUnit::Hertz),
        ParamInfo::new(
            "high_cut",
            "High Cut",
            1000.0,
            20000.0,
            20000.0,
            ParamUnit::Hertz,
        ),
        ParamInfo::new("trim", "Length", 0.05, 1.0, 1.0, ParamUnit::Percent),
        ParamInfo::new("gain", "Wet Gain", -24.0, 12.0, 0.0, ParamUnit::Decibels),
    ];
    stereo_desc("ph.fx.convolution", "Convolution Reverb", params, false)
}

/// Generate a stereo impulse response (`kind` in [`GENERATED`]) at `sr`: exponentially
/// decaying noise with a frequency-dependent decay (a one-pole low-pass that closes with
/// time), early reflections for the rooms, and a periodic echo burst train for the spring.
pub fn generate_ir(kind: &str, sr: f64) -> Option<Vec<Vec<f32>>> {
    // (rt60 s, damping per second, early reflections, er window s, spring)
    let (rt60, damp, er, er_win, spring) = match kind {
        "room" => (0.45, 5.0, 14usize, 0.025, false),
        "hall" => (2.2, 0.9, 24, 0.08, false),
        "plate" => (1.5, 0.35, 0, 0.0, false),
        "spring" => (1.8, 1.6, 0, 0.0, true),
        _ => return None,
    };
    let n = (rt60 * 1.15 * sr) as usize;
    let seed = kind.bytes().fold(7u64, |h, b| h * 131 + b as u64);
    let chans = (0..2u64)
        .map(|c| {
            let mut rng = Rng::new(seed + c * 977);
            let mut lp = 0.0f32;
            let mut ir = vec![0.0f32; n];
            for (i, v) in ir.iter_mut().enumerate() {
                let t = i as f64 / sr;
                let env = (-6.9078 * t / rt60).exp();
                let alpha = (1.0 - damp * t * 0.25).clamp(0.04, 1.0) as f32;
                let x = rng.bipolar();
                lp += (x - lp) * alpha;
                let mut s = lp as f64 * env;
                if spring {
                    // echoes every ~37 ms: gate the noise into bursts that fade per echo
                    let period = 0.037;
                    let ph = (t % period) / period;
                    let burst = (-ph * 18.0).exp();
                    s *= burst * 2.0;
                }
                *v = s as f32;
            }
            // early reflections
            for k in 0..er {
                let at = (rng.next_f64() * er_win * sr) as usize + 24;
                if at < n {
                    let a = (0.9 - 0.7 * at as f64 / (er_win * sr)) as f32;
                    ir[at] += if rng.next_f32() < 0.5 { -a } else { a };
                }
                let _ = k;
            }
            // 1 ms fade-in and a fade-out over the last 5 % so the response ends at zero
            let fin = ((0.001 * sr) as usize).max(2);
            for (i, v) in ir.iter_mut().take(fin).enumerate() {
                *v *= i as f32 / fin as f32;
            }
            let fout = n / 20;
            for i in 0..fout {
                let g = 1.0 - (i + 1) as f32 / fout as f32;
                ir[n - fout + i] *= g;
            }
            ir
        })
        .collect();
    Some(chans)
}

/// Resample `x` from `from` to `to` Hz with cubic Hermite interpolation.
fn resample(x: &[f32], from: f64, to: f64) -> Vec<f32> {
    if (from - to).abs() < 1e-6 || x.is_empty() {
        return x.to_vec();
    }
    let ratio = from / to;
    let n = (x.len() as f64 / ratio).ceil() as usize;
    let at = |i: i64| x.get(i.max(0) as usize).copied().unwrap_or(0.0);
    (0..n)
        .map(|k| {
            let p = k as f64 * ratio;
            let i = p.floor() as i64;
            let t = (p - i as f64) as f32;
            hermite(at(i - 1), at(i), at(i + 1), at(i + 2), t)
        })
        .collect()
}

/// The convolution engine: stereo in, stereo out, one sample at a time.
struct Engine {
    fft: Fft,
    nch: usize,
    taps: usize,
    parts: usize,
    active: usize,
    slots: usize,
    /// First partition, time-reversed, per IR channel.
    h0r: Vec<Vec<f32>>,
    /// Spectra of partitions 1.., `(parts - 1) * BINS` each, per IR channel.
    hre: Vec<Vec<f32>>,
    him: Vec<Vec<f32>>,
    fre: [Vec<f32>; 2],
    fim: [Vec<f32>; 2],
    fpos: usize,
    hist: [Vec<f32>; 2],
    w: usize,
    inbuf: [Vec<f32>; 2],
    fill: usize,
    tail: [Vec<f32>; 2],
    overlap: [Vec<f32>; 2],
    re: Vec<f32>,
    im: Vec<f32>,
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
}

impl Engine {
    /// `ir[channel][frame]` (1 or 2 channels), already at the engine rate. Normalised here.
    fn new(ir: &[Vec<f32>]) -> Self {
        let nch = ir.len().clamp(1, 2);
        let taps = ir.iter().map(|c| c.len()).max().unwrap_or(0).max(1);
        let energy: f64 = ir
            .iter()
            .take(nch)
            .map(|c| c.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>())
            .sum::<f64>()
            / nch as f64;
        let norm = if energy > 1e-20 {
            (1.0 / energy.sqrt()) as f32
        } else {
            1.0
        };
        let parts = taps.div_ceil(BLOCK).max(1);
        let slots = (parts - 1).max(1);
        let fft = Fft::new(FFT_LEN);
        let mut h0r = Vec::new();
        let mut hre = Vec::new();
        let mut him = Vec::new();
        let mut re = vec![0.0f32; FFT_LEN];
        let mut im = vec![0.0f32; FFT_LEN];
        for c in 0..nch {
            let h = &ir[c];
            let mut first = vec![0.0f32; BLOCK];
            for t in 0..BLOCK.min(h.len()) {
                first[BLOCK - 1 - t] = h[t] * norm;
            }
            h0r.push(first);
            let mut sr_ = vec![0.0f32; (parts - 1) * BINS];
            let mut si_ = vec![0.0f32; (parts - 1) * BINS];
            for p in 1..parts {
                re.fill(0.0);
                im.fill(0.0);
                for t in 0..BLOCK {
                    re[t] = h.get(p * BLOCK + t).copied().unwrap_or(0.0) * norm;
                }
                fft.forward(&mut re, &mut im);
                sr_[(p - 1) * BINS..p * BINS].copy_from_slice(&re[..BINS]);
                si_[(p - 1) * BINS..p * BINS].copy_from_slice(&im[..BINS]);
            }
            hre.push(sr_);
            him.push(si_);
        }
        let z = |n: usize| [vec![0.0f32; n], vec![0.0f32; n]];
        Self {
            fft,
            nch,
            taps,
            parts,
            active: parts,
            slots,
            h0r,
            hre,
            him,
            fre: z(slots * BINS),
            fim: z(slots * BINS),
            fpos: 0,
            hist: z(2 * BLOCK),
            w: 0,
            inbuf: z(BLOCK),
            fill: 0,
            tail: z(BLOCK),
            overlap: z(BLOCK),
            re,
            im,
            acc_re: vec![0.0; BINS],
            acc_im: vec![0.0; BINS],
        }
    }

    /// Use the first `fraction` of the response (at least one partition).
    fn set_length(&mut self, fraction: f32) {
        let taps = (self.taps as f32 * fraction) as usize;
        self.active = taps.div_ceil(BLOCK).clamp(1, self.parts);
    }

    fn clear(&mut self) {
        for c in 0..2 {
            self.fre[c].fill(0.0);
            self.fim[c].fill(0.0);
            self.hist[c].fill(0.0);
            self.inbuf[c].fill(0.0);
            self.tail[c].fill(0.0);
            self.overlap[c].fill(0.0);
        }
        self.fpos = 0;
        self.w = 0;
        self.fill = 0;
    }

    #[inline]
    fn tick(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mut y = [0.0f32; 2];
        let w = self.w;
        for c in 0..2 {
            self.hist[c][w] = x[c];
            self.hist[c][w + BLOCK] = x[c];
            let window = &self.hist[c][w + 1..w + 1 + BLOCK];
            let h = &self.h0r[c.min(self.nch - 1)];
            let mut head = 0.0f32;
            for (a, b) in h.iter().zip(window) {
                head += a * b;
            }
            y[c] = head + self.tail[c][self.fill];
            self.inbuf[c][self.fill] = x[c];
        }
        self.w = (w + 1) % BLOCK;
        self.fill += 1;
        if self.fill == BLOCK {
            self.fill = 0;
            self.block();
        }
        y
    }

    /// One hop is complete: transform it, accumulate partitions 1.. against the delay line
    /// and prepare the next hop's tail contribution.
    fn block(&mut self) {
        for c in 0..2 {
            let hc = c.min(self.nch - 1);
            self.re[..BLOCK].copy_from_slice(&self.inbuf[c]);
            self.re[BLOCK..].fill(0.0);
            self.im.fill(0.0);
            self.fft.forward(&mut self.re, &mut self.im);
            let slot = self.fpos;
            self.fre[c][slot * BINS..(slot + 1) * BINS].copy_from_slice(&self.re[..BINS]);
            self.fim[c][slot * BINS..(slot + 1) * BINS].copy_from_slice(&self.im[..BINS]);
            self.acc_re.fill(0.0);
            self.acc_im.fill(0.0);
            for p in 1..self.active {
                let s = (self.fpos + self.slots - (p - 1)) % self.slots;
                let xr = &self.fre[c][s * BINS..(s + 1) * BINS];
                let xi = &self.fim[c][s * BINS..(s + 1) * BINS];
                let hr = &self.hre[hc][(p - 1) * BINS..p * BINS];
                let hi = &self.him[hc][(p - 1) * BINS..p * BINS];
                for k in 0..BINS {
                    self.acc_re[k] += xr[k] * hr[k] - xi[k] * hi[k];
                    self.acc_im[k] += xr[k] * hi[k] + xi[k] * hr[k];
                }
            }
            self.re[..BINS].copy_from_slice(&self.acc_re);
            self.im[..BINS].copy_from_slice(&self.acc_im);
            for k in BINS..FFT_LEN {
                self.re[k] = self.acc_re[FFT_LEN - k];
                self.im[k] = -self.acc_im[FFT_LEN - k];
            }
            self.fft.inverse(&mut self.re, &mut self.im);
            for i in 0..BLOCK {
                self.tail[c][i] = flush_denormal(self.re[i] + self.overlap[c][i]);
                self.overlap[c][i] = flush_denormal(self.re[BLOCK + i]);
            }
        }
        self.fpos = (self.fpos + 1) % self.slots;
    }
}

pub struct Convolution {
    p: Params,
    sr: f32,
    reference: String,
    /// The decoded response behind `reference` (None for generated ones), kept so a rate
    /// change at `activate` can rebuild without the resolver.
    source: Option<Arc<SampleData>>,
    engine: Engine,
    ir_frames: usize,
    pre: [DelayLine; 2],
    predelay: OnePole,
    lo: [Svf; 2],
    hi: [Svf; 2],
    mix: OnePole,
    gain: OnePole,
    silence: SilenceTracker,
}

impl Default for Convolution {
    fn default() -> Self {
        Self::new()
    }
}

impl Convolution {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            reference: DEFAULT_REFERENCE.into(),
            source: None,
            engine: Engine::new(&[vec![1.0]]),
            ir_frames: 1,
            pre: [DelayLine::new(1), DelayLine::new(1)],
            predelay: OnePole::new(0.0),
            lo: [Svf::new(), Svf::new()],
            hi: [Svf::new(), Svf::new()],
            mix: OnePole::new(0.3),
            gain: OnePole::new(1.0),
            silence: SilenceTracker::default(),
        };
        s.build_engine();
        s.snap();
        s
    }

    /// Rebuild the engine for the current reference at the current rate (off the audio
    /// thread).
    fn build_engine(&mut self) {
        let sr = self.sr as f64;
        let max = (MAX_IR_SECONDS * sr) as usize;
        let ir: Vec<Vec<f32>> = match &self.source {
            Some(data) => data
                .channels
                .iter()
                .take(2)
                .map(|c| {
                    let mut v = resample(c, data.sample_rate, sr);
                    v.truncate(max);
                    v
                })
                .collect(),
            None => {
                let name = self.reference.strip_prefix("generated:").unwrap_or("hall");
                generate_ir(name, sr).unwrap_or_else(|| vec![vec![1.0]])
            }
        };
        let ir = if ir.is_empty() || ir[0].is_empty() {
            vec![vec![1.0]]
        } else {
            ir
        };
        self.ir_frames = ir[0].len();
        self.engine = Engine::new(&ir);
        self.engine.set_length(self.p.f(TRIM));
    }

    fn snap(&mut self) {
        for i in 0..self.p.len() {
            self.apply(i);
        }
        for s in [&mut self.predelay, &mut self.mix, &mut self.gain] {
            let v = s.target();
            s.reset(v);
        }
    }

    fn apply(&mut self, i: usize) {
        let sr = self.sr;
        match i {
            PREDELAY => {
                self.predelay.set_time(0.03, sr);
                self.predelay.set_target(self.p.f(PREDELAY) * 1e-3 * sr);
            }
            MIX => {
                self.mix.set_time(0.02, sr);
                self.mix.set_target(self.p.f(MIX));
            }
            GAIN => {
                self.gain.set_time(0.02, sr);
                self.gain.set_target(db_to_lin(self.p.f(GAIN)));
            }
            LOW_CUT => {
                let g = tpt_g(self.p.f(LOW_CUT), sr);
                for f in &mut self.lo {
                    f.set_coefs(g, std::f32::consts::SQRT_2);
                }
            }
            HIGH_CUT => {
                let g = tpt_g(self.p.f(HIGH_CUT), sr);
                for f in &mut self.hi {
                    f.set_coefs(g, std::f32::consts::SQRT_2);
                }
            }
            TRIM => self.engine.set_length(self.p.f(TRIM)),
            _ => {}
        }
    }

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        let n = (MAX_PREDELAY_MS * 1e-3 * info.sample_rate) as usize + 8;
        self.pre = [DelayLine::new(n), DelayLine::new(n)];
        self.build_engine();
        self.snap();
    }

    fn on_reset(&mut self) {
        self.engine.clear();
        for d in &mut self.pre {
            d.clear();
        }
        for f in self.lo.iter_mut().chain(self.hi.iter_mut()) {
            f.reset();
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        let ir = (self.ir_frames as f32 * self.p.f(TRIM)) as u32;
        ir + (self.p.v[PREDELAY] * 1e-3 * self.sr as f64) as u32 + 2 * BLOCK as u32
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let lo_on = self.p.f(LOW_CUT) > 20.5;
        let hi_on = self.p.f(HIGH_CUT) < 19999.0;
        let max_pre = (self.pre[0].max_delay() - 2) as f32;
        for i in a..b {
            let m = self.mix.next();
            let g = self.gain.next();
            let d = self.predelay.next().clamp(0.0, max_pre);
            let x = [input(io, 0, 0, i), input(io, 0, 1, i)];
            for c in 0..2 {
                self.pre[c].push(x[c]);
            }
            let w = [self.pre[0].read_linear(d), self.pre[1].read_linear(d)];
            let mut y = self.engine.tick(w);
            for c in 0..2 {
                if lo_on {
                    y[c] = self.lo[c].tick(y[c]).2;
                }
                if hi_on {
                    y[c] = self.hi[c].tick(y[c]).0;
                }
                y[c] *= g;
            }
            output(io, i, crossfade(x[0], y[0], m), crossfade(x[1], y[1], m));
        }
    }
}

impl_processor!(Convolution;
    fn save_state(&self) -> Vec<u8> {
        self.reference.as_bytes().to_vec()
    }

    fn load_state(&mut self, state: &[u8], assets: &dyn AssetResolver) -> Result<(), String> {
        let text = std::str::from_utf8(state)
            .map_err(|_| "impulse response reference is not UTF-8".to_string())?
            .trim();
        let text = if text.is_empty() { DEFAULT_REFERENCE } else { text };
        let source = match text.strip_prefix("generated:") {
            Some(name) => {
                if generate_ir(name, 48000.0).is_none() {
                    return Err(format!(
                        "unknown generated impulse response `{name}` (available: {})",
                        GENERATED.join(", ")
                    ));
                }
                None
            }
            None => Some(assets.sample(text)?),
        };
        self.reference = text.to_string();
        self.source = source;
        self.build_engine();
        self.silence.reset();
        Ok(())
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_matches_direct_convolution() {
        let mut rng = Rng::new(3);
        let ir: Vec<f32> = (0..1700).map(|_| rng.bipolar()).collect();
        let mut eng = Engine::new(std::slice::from_ref(&ir));
        let energy: f64 = ir.iter().map(|v| (*v as f64).powi(2)).sum();
        let norm = (1.0 / energy.sqrt()) as f32;
        let x: Vec<f32> = (0..4000).map(|_| rng.bipolar()).collect();
        for n in 0..x.len() {
            let y = eng.tick([x[n], x[n]]);
            let mut want = 0.0f64;
            for (t, h) in ir.iter().enumerate() {
                if t <= n {
                    want += (*h * norm) as f64 * x[n - t] as f64;
                }
            }
            assert!(
                (y[0] as f64 - want).abs() < 2e-4,
                "n={n}: {} vs {want}",
                y[0]
            );
        }
    }

    #[test]
    fn generated_irs_exist_and_end_quietly() {
        for k in GENERATED {
            let ir = generate_ir(k, 48000.0).unwrap();
            assert_eq!(ir.len(), 2);
            assert!(ir[0].len() > 10000, "{k}");
            assert_eq!(*ir[0].last().unwrap(), 0.0, "{k}");
        }
        assert!(generate_ir("nope", 48000.0).is_none());
    }
}
