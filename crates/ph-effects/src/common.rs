//! Shared plumbing for the built-in effects: parameter storage, descriptors, I/O access.

use ph_node::{NodeDescriptor, NodeKind, ParamId, ParamInfo, PortInfo, ProcessIo};

/// Parameter values stored by index, clamped through their [`ParamInfo`].
pub(crate) struct Params {
    pub desc: NodeDescriptor,
    pub v: Vec<f64>,
}

impl Params {
    pub fn new(desc: NodeDescriptor) -> Self {
        let v = desc.params.iter().map(|p| p.default).collect();
        Self { desc, v }
    }

    /// Clamp and store; returns the index if `id` is valid.
    #[inline]
    pub fn set(&mut self, id: ParamId, value: f64) -> Option<usize> {
        let i = id.0 as usize;
        let p = self.desc.params.get(i)?;
        self.v[i] = p.clamp(value);
        Some(i)
    }

    #[inline]
    pub fn get(&self, id: ParamId) -> f64 {
        self.v.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    #[inline]
    pub fn f(&self, i: usize) -> f32 {
        self.v[i] as f32
    }

    #[inline]
    pub fn u(&self, i: usize) -> usize {
        self.v[i].max(0.0) as usize
    }

    #[inline]
    pub fn on(&self, i: usize) -> bool {
        self.v[i] >= 0.5
    }
    pub fn len(&self) -> usize {
        self.v.len()
    }
}

pub(crate) fn port(name: &str, sidechain: bool) -> PortInfo {
    PortInfo {
        name: name.into(),
        channels: 2,
        sidechain,
    }
}

/// Stereo in, stereo out effect descriptor (plus an optional stereo sidechain input).
pub(crate) fn stereo_desc(
    id: &str,
    name: &str,
    params: Vec<ParamInfo>,
    sidechain: bool,
) -> NodeDescriptor {
    let mut inputs = vec![port("in", false)];
    if sidechain {
        inputs.push(port("sidechain", true));
    }
    NodeDescriptor {
        id: id.into(),
        name: name.into(),
        kind: NodeKind::Effect,
        audio_inputs: inputs,
        audio_outputs: vec![port("out", false)],
        note_input: false,
        params,
    }
}

/// A switch parameter (0/1).
pub(crate) fn switch(id: &str, name: &str, default: bool) -> ParamInfo {
    let mut p = ParamInfo::choice(id, name, &["Off", "On"], default as usize);
    p.unit = ph_node::ParamUnit::Switch;
    p
}

/// Input sample `i` of `port`/`ch` (a mono port feeds both channels; a missing port is
/// silence).
#[inline]
pub(crate) fn input(io: &ProcessIo<'_, '_>, port: usize, ch: usize, i: usize) -> f32 {
    match io.inputs.get(port) {
        Some(p) => match p.get(ch).or_else(|| p.first()) {
            Some(c) => c.get(i).copied().unwrap_or(0.0),
            None => 0.0,
        },
        None => 0.0,
    }
}

/// Write a stereo sample to output port 0 (tolerates a mono output port).
#[inline]
pub(crate) fn output(io: &mut ProcessIo<'_, '_>, i: usize, l: f32, r: f32) {
    if let Some(p) = io.outputs.get_mut(0) {
        match p.len() {
            0 => {}
            1 => p[0][i] = 0.5 * (l + r),
            _ => {
                p[0][i] = l;
                p[1][i] = r;
            }
        }
    }
}

/// Counts consecutive silent input samples so an effect can report `Sleep` once its tail
/// has fully decayed.
#[derive(Default)]
pub(crate) struct SilenceTracker {
    run: u64,
}

impl SilenceTracker {
    /// Feed one block's input; returns true if the processor may sleep given `tail`.
    pub fn update(&mut self, io: &ProcessIo<'_, '_>, frames: usize, tail: u32) -> bool {
        let silent = io.inputs.iter().take(1).all(|p| {
            p.iter()
                .all(|c| c.iter().take(frames).all(|v| v.abs() < 1e-9))
        });
        if silent {
            self.run = self.run.saturating_add(frames as u64);
        } else {
            self.run = 0;
        }
        tail != u32::MAX && self.run > tail as u64
    }

    pub fn reset(&mut self) {
        self.run = 0;
    }
}

/// Map `"eq"` or `"ph.fx.eq"` to the short name.
pub(crate) fn short_name(name: &str) -> &str {
    name.strip_prefix("ph.fx.").unwrap_or(name)
}

/// Implements [`ph_node::Processor`] for an effect struct with fields `p: Params` and
/// `silence: SilenceTracker` and the inherent methods `on_activate(&mut self, &ActivateInfo)`,
/// `on_reset(&mut self)`, `apply(&mut self, usize)` (a parameter changed),
/// `render(&mut self, &mut ProcessIo, usize, usize)`, `latency_samples(&self)` and
/// `tail_samples(&self)`. Events are applied sample-accurately between rendered segments.
macro_rules! impl_processor {
    ($t:ty $(; $($extra:item)*)?) => {
        impl ::ph_node::Processor for $t {
            fn descriptor(&self) -> &::ph_node::NodeDescriptor {
                &self.p.desc
            }

            fn activate(&mut self, info: &::ph_node::ActivateInfo) {
                self.on_activate(info);
                self.on_reset();
                self.silence.reset();
            }

            fn process(
                &mut self,
                ctx: &::ph_node::ProcessContext,
                io: &mut ::ph_node::ProcessIo<'_, '_>,
            ) -> ::ph_node::ProcessStatus {
                let n = ctx.frames;
                let mut pos = 0;
                for ev in io.events {
                    let f = (ev.frame as usize).min(n);
                    if f > pos {
                        self.render(io, pos, f);
                        pos = f;
                    }
                    if let ::ph_node::EventKind::Param { id, value } = ev.kind {
                        ::ph_node::Processor::set_param(self, id, value);
                    }
                }
                if pos < n {
                    self.render(io, pos, n);
                }
                let tail = ::ph_node::Processor::tail(self);
                if self.silence.update(io, n, tail) {
                    ::ph_node::ProcessStatus::Sleep
                } else {
                    ::ph_node::ProcessStatus::Continue
                }
            }

            fn set_param(&mut self, id: ::ph_node::ParamId, value: f64) {
                if let Some(i) = self.p.set(id, value) {
                    self.apply(i);
                }
            }

            fn param(&self, id: ::ph_node::ParamId) -> f64 {
                self.p.get(id)
            }

            fn reset(&mut self) {
                self.on_reset();
                self.silence.reset();
            }

            fn latency(&self) -> u32 {
                self.latency_samples()
            }

            fn tail(&self) -> u32 {
                self.tail_samples()
            }

            $($($extra)*)?
        }
    };
}
pub(crate) use impl_processor;

/// Complex radix-2 FFT in `f32` with precomputed twiddles (allocation-free transforms).
pub(crate) struct Fft {
    n: usize,
    rev: Vec<u32>,
    wr: Vec<f32>,
    wi: Vec<f32>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        let half = n / 2;
        let ang = |k: usize| -2.0 * std::f64::consts::PI * k as f64 / n as f64;
        Self {
            n,
            rev,
            wr: (0..half).map(|k| ang(k).cos() as f32).collect(),
            wi: (0..half).map(|k| ang(k).sin() as f32).collect(),
        }
    }

    fn run(&self, re: &mut [f32], im: &mut [f32], sign: f32) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (cr, ci) = (self.wr[k * step], sign * self.wi[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * cr - im[b] * ci;
                    let ti = re[b] * ci + im[b] * cr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }

    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        self.run(re, im, 1.0);
    }

    /// Inverse transform, scaled by `1/n`.
    pub fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        self.run(re, im, -1.0);
        let s = 1.0 / self.n as f32;
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v *= s;
        }
    }
}

/// Smoothed linear mix: `dry * (1 - m) + wet * m`.
#[inline]
pub(crate) fn crossfade(dry: f32, wet: f32, m: f32) -> f32 {
    dry * (1.0 - m) + wet * m
}

/// Mark a continuous parameter as taking whole values only (counts, stage numbers).
pub(crate) fn whole(mut p: ParamInfo) -> ParamInfo {
    p.stepped = true;
    p
}

/// Wrap a phase in turns to `0..1`.
#[inline]
pub(crate) fn wrap01(t: f64) -> f64 {
    t - t.floor()
}
