//! Shared plumbing for the built-in instruments.

use ph_node::{NodeDescriptor, NodeKind, ParamId, ParamInfo, ParamUnit, PortInfo, ProcessIo};

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
}

pub(crate) fn instrument_desc(id: &str, name: &str, params: Vec<ParamInfo>) -> NodeDescriptor {
    NodeDescriptor {
        id: id.into(),
        name: name.into(),
        kind: NodeKind::Instrument,
        audio_inputs: vec![],
        audio_outputs: vec![PortInfo {
            name: "out".into(),
            channels: 2,
            sidechain: false,
        }],
        note_input: true,
        params,
    }
}

pub(crate) fn switch(id: &str, name: &str, default: bool) -> ParamInfo {
    let mut p = ParamInfo::choice(id, name, &["Off", "On"], default as usize);
    p.unit = ParamUnit::Switch;
    p
}

pub(crate) fn stepped(
    id: &str,
    name: &str,
    min: f64,
    max: f64,
    default: f64,
    unit: ParamUnit,
) -> ParamInfo {
    let mut p = ParamInfo::new(id, name, min, max, default, unit);
    p.stepped = true;
    p
}

/// Balance pan: centre is unity on both sides, hard left is (1, 0).
#[inline]
pub(crate) fn balance(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

/// Zero frames `a..b` of every output channel.
pub(crate) fn zero_outputs(io: &mut ProcessIo<'_, '_>, a: usize, b: usize) {
    for port in io.outputs.iter_mut() {
        for ch in port.iter_mut() {
            ch[a..b].fill(0.0);
        }
    }
}

/// Velocity (0..1) to gain with sensitivity `amount` (0 = ignore velocity).
#[inline]
pub(crate) fn velocity_gain(velocity: f32, amount: f32) -> f32 {
    let v = velocity.clamp(0.0, 1.0);
    1.0 - amount + amount * v * v
}
