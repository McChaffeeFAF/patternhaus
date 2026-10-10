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
