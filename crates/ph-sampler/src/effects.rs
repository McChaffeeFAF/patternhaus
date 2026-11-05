//! Per-group insert chains (M3/T2).
//!
//! # Decision: the engine composes the chain
//!
//! A group's `effects` are built-in mixer effects (`eq`, `compressor`, `reverb`, ...), which
//! live in `ph-effects`. `ph-sampler` sits beside `ph-effects` in the building-block layer
//! and may not depend on it (DESIGN §2), and the layer stays as it is: the **host builds the
//! effect processors** (the engine through `ph_effects::create`) and hands them to the
//! sampler with [`crate::HausSampler::set_group_effects`] before `activate`. The sampler only
//! sees `ph_node::Processor`s. It owns, activates and runs them, renders each group that has
//! a chain into its own stereo buffer, runs the chain over the whole block, applies each
//! slot's dry/wet `mix`, and adds the result to the group's output port. Bypassed slots are
//! left out by the host. A sampler without a host-built chain plays its groups dry (that is
//! the M3/T0 behaviour, and what `ph_sampler::create` alone gives).
//!
//! Chains run on every block, silent or not (the engine never skips a sleeping processor
//! either), so tails and effect state do not depend on where blocks start: live output stays
//! bit-identical to offline.

use ph_node::{ActivateInfo, ProcessContext, ProcessIo, ProcessStatus, Processor};

/// One slot of a group's chain.
pub struct GroupFx {
    pub fx: Box<dyn Processor>,
    /// Dry/wet, 0..=1.
    pub mix: f32,
}

/// Every group's chain and the buffers they run in. Allocated in `activate`.
#[derive(Default)]
pub struct Chains {
    /// Per plan group: its chain (empty = plays straight into its port).
    pub chains: Vec<Vec<GroupFx>>,
    /// Per plan group with a chain: its buffer number.
    pub buffer_of: Vec<Option<usize>>,
    /// `[buffer][channel][frame]`, flattened: `2 × max_frames` per buffer.
    pub bufs: Box<[f32]>,
    /// Effect output scratch, `2 × max_frames`.
    scratch: Box<[f32]>,
    max_frames: usize,
}

impl Chains {
    /// True when no group has a chain.
    pub fn is_empty(&self) -> bool {
        self.buffer_of.iter().all(|b| b.is_none())
    }

    /// Sizes the buffers and activates every effect. Off the audio thread.
    pub fn activate(&mut self, groups: usize, info: &ActivateInfo) {
        self.chains.resize_with(groups, Vec::new);
        self.max_frames = info.max_frames.max(1);
        let mut n = 0;
        self.buffer_of = self
            .chains
            .iter()
            .map(|c| {
                (!c.is_empty()).then(|| {
                    n += 1;
                    n - 1
                })
            })
            .collect();
        self.bufs = vec![0.0; n * 2 * self.max_frames].into_boxed_slice();
        self.scratch = vec![0.0; 2 * self.max_frames].into_boxed_slice();
        for (g, chain) in self.chains.iter_mut().enumerate() {
            for (k, slot) in chain.iter_mut().enumerate() {
                slot.fx.activate(&ActivateInfo {
                    seed: info
                        .seed
                        .wrapping_add((g as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
                        .wrapping_add(k as u64),
                    ..*info
                });
            }
        }
    }

    /// Back to silence (transport stop, reset). Real-time.
    pub fn reset(&mut self) {
        for chain in self.chains.iter_mut() {
            for slot in chain.iter_mut() {
                slot.fx.reset();
            }
        }
        self.bufs.fill(0.0);
    }

    /// The left and right buffer of chain buffer `b`, `n` frames.
    pub fn buffer(&mut self, b: usize, n: usize) -> (&mut [f32], &mut [f32]) {
        let m = self.max_frames;
        let (l, r) = self.bufs[b * 2 * m..(b + 1) * 2 * m].split_at_mut(m);
        (&mut l[..n], &mut r[..n])
    }

    /// Clears every chain buffer for a block of `n` frames. Real-time.
    pub fn begin(&mut self, n: usize) {
        let m = self.max_frames;
        for b in 0..self.bufs.len() / (2 * m).max(1) {
            self.bufs[b * 2 * m..b * 2 * m + n].fill(0.0);
            self.bufs[b * 2 * m + m..b * 2 * m + m + n].fill(0.0);
        }
    }

    /// Runs group `g`'s chain over its buffer (`n` frames) and adds the result to `out_l` /
    /// `out_r`. Returns true while an effect still sounds (a tail). Real-time.
    pub fn run(
        &mut self,
        g: usize,
        ctx: &ProcessContext,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let Some(b) = self.buffer_of.get(g).copied().flatten() else {
            return false;
        };
        let n = ctx.frames;
        let m = self.max_frames;
        let mut alive = false;
        let (bufs, scratch) = (&mut self.bufs, &mut self.scratch);
        let (gl, gr) = bufs[b * 2 * m..(b + 1) * 2 * m].split_at_mut(m);
        let (gl, gr) = (&mut gl[..n], &mut gr[..n]);
        let (sl, sr) = scratch.split_at_mut(m);
        let (sl, sr) = (&mut sl[..n], &mut sr[..n]);
        for slot in self.chains[g].iter_mut() {
            {
                let inputs: [&[f32]; 2] = [gl, gr];
                let in_ports: [&[&[f32]]; 1] = [&inputs];
                let mut outs: [&mut [f32]; 2] = [&mut *sl, &mut *sr];
                let mut out_ports: [&mut [&mut [f32]]; 1] = [&mut outs];
                let mut io = ProcessIo {
                    inputs: &in_ports,
                    outputs: &mut out_ports,
                    events: &[],
                };
                if slot.fx.process(ctx, &mut io) == ProcessStatus::Continue {
                    alive = true;
                }
            }
            let mix = slot.mix.clamp(0.0, 1.0);
            if mix >= 1.0 {
                gl.copy_from_slice(sl);
                gr.copy_from_slice(sr);
            } else {
                for i in 0..n {
                    gl[i] += (sl[i] - gl[i]) * mix;
                    gr[i] += (sr[i] - gr[i]) * mix;
                }
            }
        }
        for i in 0..n {
            out_l[i] += gl[i];
            out_r[i] += gr[i];
        }
        alive
    }
}
