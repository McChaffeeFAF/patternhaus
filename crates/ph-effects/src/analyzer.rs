//! `ph.fx.analyzer`: pass-through spectrum, loudness and phase-correlation meter.
//!
//! The audio is copied through bit-exactly. Alongside it the effect measures, per
//! [`HOP`] samples, a Hann-windowed spectrum folded into [`BANDS`] log-spaced bands from
//! 20 Hz to 20 kHz (dBFS, a full-scale sine reads 0 dB), the stereo phase correlation
//! (smoothed Pearson coefficient, 300 ms), per-channel peaks, and, per 100 ms, momentary
//! (400 ms) and short-term (3 s) loudness per ITU-R BS.1770-4 with K-weighting.
//!
//! Results are published lock-free through a [`Feed`] (relaxed atomics, no allocation or
//! locks on the audio thread). A UI finds the feed of the most recently activated analyser
//! with [`latest_feed`].

use crate::common::{Fft, Params, SilenceTracker, impl_processor, input, output, stereo_desc};
use ph_dsp::filter::{Biquad, BiquadCoefs};
use ph_node::*;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Number of published spectrum bands.
pub const BANDS: usize = 96;
/// Lowest and highest band edge, Hz.
pub const F_LO: f32 = 20.0;
pub const F_HI: f32 = 20000.0;
/// Samples between spectrum / correlation / peak updates.
pub const HOP: usize = 1024;
/// Value published for "no signal" loudness.
pub const SILENT_LUFS: f32 = -120.0;

const FFT_SIZE: usize = 0;
const SMOOTHING: usize = 1;
const TILT: usize = 2;

const SIZES: [usize; 3] = [1024, 2048, 4096];
const TILTS: [f32; 3] = [0.0, 3.0, 4.5];

pub fn descriptor() -> NodeDescriptor {
    let params = vec![
        ParamInfo::choice("fft_size", "Resolution", &["1024", "2048", "4096"], 1),
        ParamInfo::new("smoothing", "Smoothing", 0.0, 0.95, 0.8, ParamUnit::Percent),
        ParamInfo::choice("tilt", "Tilt", &["0 dB/oct", "3 dB/oct", "4.5 dB/oct"], 0),
    ];
    stereo_desc("ph.fx.analyzer", "Analyser", params, false)
}

/// Centre frequency of band `k`, Hz.
pub fn band_centre(k: usize) -> f32 {
    F_LO * (F_HI / F_LO).powf((k as f32 + 0.5) / BANDS as f32)
}

/// Lock-free measurements published by one analyser instance.
pub struct Feed {
    spectrum: Vec<AtomicU32>,
    momentary: AtomicU32,
    short_term: AtomicU32,
    correlation: AtomicU32,
    peak: [AtomicU32; 2],
    updates: AtomicU64,
}

impl Feed {
    fn new() -> Self {
        let z = || AtomicU32::new(0f32.to_bits());
        Self {
            spectrum: (0..BANDS)
                .map(|_| AtomicU32::new((-120.0f32).to_bits()))
                .collect(),
            momentary: AtomicU32::new(SILENT_LUFS.to_bits()),
            short_term: AtomicU32::new(SILENT_LUFS.to_bits()),
            correlation: z(),
            peak: [z(), z()],
            updates: AtomicU64::new(0),
        }
    }

    /// Band levels in dBFS (with the tilt applied).
    pub fn spectrum(&self) -> [f32; BANDS] {
        let mut out = [0.0; BANDS];
        for (o, a) in out.iter_mut().zip(&self.spectrum) {
            *o = f32::from_bits(a.load(Ordering::Relaxed));
        }
        out
    }

    /// Momentary (400 ms) loudness, LUFS ([`SILENT_LUFS`] when silent).
    pub fn momentary_lufs(&self) -> f32 {
        f32::from_bits(self.momentary.load(Ordering::Relaxed))
    }

    /// Short-term (3 s) loudness, LUFS.
    pub fn short_term_lufs(&self) -> f32 {
        f32::from_bits(self.short_term.load(Ordering::Relaxed))
    }

    /// Phase correlation, -1 (out of phase) .. +1 (mono).
    pub fn correlation(&self) -> f32 {
        f32::from_bits(self.correlation.load(Ordering::Relaxed))
    }

    /// Decaying peak level per channel, linear.
    pub fn peak(&self) -> [f32; 2] {
        [
            f32::from_bits(self.peak[0].load(Ordering::Relaxed)),
            f32::from_bits(self.peak[1].load(Ordering::Relaxed)),
        ]
    }

    /// Increments with every spectrum update (lets a UI see that the analyser is alive).
    pub fn updates(&self) -> u64 {
        self.updates.load(Ordering::Relaxed)
    }
}

static REGISTRY: Mutex<Vec<Weak<Feed>>> = Mutex::new(Vec::new());

fn register(feed: &Arc<Feed>) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.retain(|w| w.strong_count() > 0);
        r.push(Arc::downgrade(feed));
    }
}

/// The feed of the most recently activated, still existing analyser.
pub fn latest_feed() -> Option<Arc<Feed>> {
    REGISTRY.lock().ok()?.iter().rev().find_map(|w| w.upgrade())
}

/// The two-stage K-weighting filter of BS.1770-4 (head-related shelf, then RLB high-pass).
fn k_weighting(sample_rate: f64) -> [BiquadCoefs; 2] {
    let (f0, g, q) = (
        1681.974450955533f64,
        3.999843853973347f64,
        0.7071752369554196f64,
    );
    let k = (PI * f0 / sample_rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = BiquadCoefs {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    let (f0, q) = (38.13547087602444f64, 0.5003270373238773f64);
    let k = (PI * f0 / sample_rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let rlb = BiquadCoefs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    [shelf, rlb]
}

const SHORT_HOPS: usize = 30;
const MOMENTARY_HOPS: usize = 4;

pub struct Analyzer {
    p: Params,
    sr: f32,
    feed: Arc<Feed>,
    registered: bool,
    ffts: Vec<Fft>,
    windows: Vec<Vec<f32>>,
    ring: Vec<f32>,
    wpos: usize,
    since_hop: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    smooth: [f32; BANDS],
    /// Per size: first and last (exclusive) bin of every band.
    edges: [[(u16, u16); BANDS]; 3],
    kf: [[Biquad; 2]; 2],
    hop_len: usize,
    hop_count: usize,
    hop_sum: f64,
    hops: [f64; SHORT_HOPS],
    hop_n: usize,
    hop_pos: usize,
    s_ll: f64,
    s_rr: f64,
    s_lr: f64,
    corr_c: f64,
    peak: [f32; 2],
    peak_coef: f32,
    silence: SilenceTracker,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let max = *SIZES.last().unwrap();
        let mut s = Self {
            p: Params::new(descriptor()),
            sr: 48000.0,
            feed: Arc::new(Feed::new()),
            registered: false,
            ffts: SIZES.iter().map(|n| Fft::new(*n)).collect(),
            windows: SIZES
                .iter()
                .map(|n| {
                    (0..*n)
                        .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / *n as f64).cos()) as f32)
                        .collect()
                })
                .collect(),
            ring: vec![0.0; max],
            wpos: 0,
            since_hop: 0,
            re: vec![0.0; max],
            im: vec![0.0; max],
            smooth: [-120.0; BANDS],
            edges: [[(0, 0); BANDS]; 3],
            kf: Default::default(),
            hop_len: 4800,
            hop_count: 0,
            hop_sum: 0.0,
            hops: [0.0; SHORT_HOPS],
            hop_n: 0,
            hop_pos: 0,
            s_ll: 0.0,
            s_rr: 0.0,
            s_lr: 0.0,
            corr_c: 0.0,
            peak: [0.0; 2],
            peak_coef: 0.0,
            silence: SilenceTracker::default(),
        };
        s.configure();
        s
    }

    /// The measurements this instance publishes.
    pub fn feed(&self) -> Arc<Feed> {
        self.feed.clone()
    }

    fn configure(&mut self) {
        let sr = self.sr as f64;
        self.hop_len = ((sr * 0.1).round() as usize).max(1);
        let c = k_weighting(sr);
        for ch in &mut self.kf {
            ch[0] = Biquad::new(c[0]);
            ch[1] = Biquad::new(c[1]);
        }
        self.corr_c = 1.0 - (-1.0 / (0.3 * sr)).exp();
        self.peak_coef = (-1.0 / (0.3 * sr as f32 / HOP as f32)).exp();
        for (si, n) in SIZES.iter().enumerate() {
            let bw = sr / *n as f64;
            for k in 0..BANDS {
                let lo = F_LO as f64 * (F_HI as f64 / F_LO as f64).powf(k as f64 / BANDS as f64);
                let hi =
                    F_LO as f64 * (F_HI as f64 / F_LO as f64).powf((k + 1) as f64 / BANDS as f64);
                let a = (lo / bw).round().max(1.0) as usize;
                let b = ((hi / bw).round() as usize).max(a + 1).min(n / 2);
                self.edges[si][k] = (a.min(n / 2 - 1) as u16, b as u16);
            }
        }
    }

    fn apply(&mut self, _i: usize) {}

    fn on_activate(&mut self, info: &ActivateInfo) {
        self.sr = info.sample_rate as f32;
        self.configure();
        if !self.registered {
            register(&self.feed);
            self.registered = true;
        }
    }

    fn on_reset(&mut self) {
        self.ring.fill(0.0);
        self.wpos = 0;
        self.since_hop = 0;
        self.smooth = [-120.0; BANDS];
        for ch in &mut self.kf {
            ch[0].reset();
            ch[1].reset();
        }
        self.hop_count = 0;
        self.hop_sum = 0.0;
        self.hops = [0.0; SHORT_HOPS];
        self.hop_n = 0;
        self.hop_pos = 0;
        self.s_ll = 0.0;
        self.s_rr = 0.0;
        self.s_lr = 0.0;
        self.peak = [0.0; 2];
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self) -> u32 {
        // a meter: never sleeps, so the display keeps decaying
        u32::MAX
    }

    fn lufs(z: f64) -> f32 {
        if z > 1e-12 {
            (-0.691 + 10.0 * z.log10()) as f32
        } else {
            SILENT_LUFS
        }
    }

    /// Mean square of the most recent `n` hops (or of all, if fewer exist).
    fn window_z(&self, n: usize) -> f64 {
        let n = n.min(self.hop_n);
        if n == 0 {
            return 0.0;
        }
        let mut sum = 0.0;
        for k in 0..n {
            sum += self.hops[(self.hop_pos + SHORT_HOPS - 1 - k) % SHORT_HOPS];
        }
        sum / n as f64
    }

    fn end_loudness_hop(&mut self) {
        self.hops[self.hop_pos] = self.hop_sum / self.hop_len as f64;
        self.hop_pos = (self.hop_pos + 1) % SHORT_HOPS;
        self.hop_n = (self.hop_n + 1).min(SHORT_HOPS);
        self.hop_sum = 0.0;
        self.hop_count = 0;
        let m = Self::lufs(self.window_z(MOMENTARY_HOPS));
        let s = Self::lufs(self.window_z(SHORT_HOPS));
        self.feed.momentary.store(m.to_bits(), Ordering::Relaxed);
        self.feed.short_term.store(s.to_bits(), Ordering::Relaxed);
    }

    fn end_spectrum_hop(&mut self) {
        let si = self.p.u(FFT_SIZE).min(2);
        let n = SIZES[si];
        let smoothing = self.p.f(SMOOTHING);
        let tilt = TILTS[self.p.u(TILT).min(2)];
        // the newest n samples, oldest first
        let ring_len = self.ring.len();
        let start = (self.wpos + ring_len - n) % ring_len;
        for i in 0..n {
            self.re[i] = self.ring[(start + i) % ring_len] * self.windows[si][i];
            self.im[i] = 0.0;
        }
        self.ffts[si].forward(&mut self.re[..n], &mut self.im[..n]);
        let scale = 4.0 / n as f32;
        for k in 0..BANDS {
            let (a, b) = self.edges[si][k];
            let mut p = 0.0f32;
            for bin in a as usize..b as usize {
                p = p.max(self.re[bin] * self.re[bin] + self.im[bin] * self.im[bin]);
            }
            let db = (10.0 * (p * scale * scale).max(1e-12).log10()).max(-120.0);
            let s = &mut self.smooth[k];
            *s = if db > *s {
                db
            } else {
                *s * smoothing + db * (1.0 - smoothing)
            };
            let t = tilt * (band_centre(k) / 1000.0).log2();
            self.feed.spectrum[k].store((*s + t).to_bits(), Ordering::Relaxed);
        }
        let denom = (self.s_ll * self.s_rr).sqrt();
        let corr = if denom > 1e-12 {
            (self.s_lr / denom).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.feed
            .correlation
            .store((corr as f32).to_bits(), Ordering::Relaxed);
        for c in 0..2 {
            self.feed.peak[c].store(self.peak[c].to_bits(), Ordering::Relaxed);
            self.peak[c] *= self.peak_coef;
        }
        self.feed.updates.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&mut self, io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
        let ring_len = self.ring.len();
        for i in a..b {
            let (l, r) = (input(io, 0, 0, i), input(io, 0, 1, i));
            output(io, i, l, r);
            self.ring[self.wpos] = 0.5 * (l + r);
            self.wpos = (self.wpos + 1) % ring_len;
            // loudness
            let s1 = self.kf[0][0].process(l as f64);
            let kl = self.kf[0][1].process(s1);
            let s2 = self.kf[1][0].process(r as f64);
            let kr = self.kf[1][1].process(s2);
            self.hop_sum += kl * kl + kr * kr;
            self.hop_count += 1;
            if self.hop_count == self.hop_len {
                self.end_loudness_hop();
            }
            // correlation and peaks, smoothed once per hop below
            let (dl, dr) = (l as f64, r as f64);
            let c = self.corr_c;
            self.s_ll += c * (dl * dl - self.s_ll);
            self.s_rr += c * (dr * dr - self.s_rr);
            self.s_lr += c * (dl * dr - self.s_lr);
            self.peak[0] = self.peak[0].max(l.abs());
            self.peak[1] = self.peak[1].max(r.abs());
            self.since_hop += 1;
            if self.since_hop == HOP {
                self.since_hop = 0;
                self.end_spectrum_hop();
            }
        }
    }
}

impl_processor!(Analyzer);
