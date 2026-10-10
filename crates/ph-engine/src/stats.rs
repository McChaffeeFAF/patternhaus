//! Shared figures published by the audio thread: transport position, load, overloads,
//! shedding, per-node timing and meters. All lock-free (atomics); the audio thread only
//! stores, the control side only loads.

use ph_core::ObjectPath;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

/// `f32` in an `AtomicU32`.
#[inline]
pub(crate) fn store_f32(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

#[inline]
pub(crate) fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

pub(crate) struct StatEntry {
    pub label: String,
    pub owner: Option<ObjectPath>,
    /// Smoothed processing time per block, µs (f32 bits).
    pub micros: AtomicU32,
}

/// Per-node timing for one plan: nodes, and each insert's effect slots after it.
#[derive(Default)]
pub(crate) struct StatsBoard {
    pub entries: Vec<StatEntry>,
}

impl StatsBoard {
    pub fn push(&mut self, label: String, owner: Option<ObjectPath>) -> usize {
        self.entries.push(StatEntry {
            label,
            owner,
            micros: AtomicU32::new(0),
        });
        self.entries.len() - 1
    }
}

/// One mixer insert's meter: peak since last read and last block's RMS, `[l, r]`.
#[derive(Default)]
pub(crate) struct MeterSlot {
    pub peak: [AtomicU32; 2],
    pub rms: [AtomicU32; 2],
}

pub(crate) struct Shared {
    pub position: AtomicU64,
    pub playing: AtomicBool,
    pub dsp_load: AtomicU32,
    pub overloads: AtomicU64,
    pub shedding: AtomicU8,
    pub suggest_frames: AtomicU32,
    pub meters: Vec<MeterSlot>,
}

impl Shared {
    pub fn new(inserts: usize) -> Self {
        Self {
            position: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            dsp_load: AtomicU32::new(0),
            overloads: AtomicU64::new(0),
            shedding: AtomicU8::new(0),
            suggest_frames: AtomicU32::new(0),
            meters: (0..inserts).map(|_| MeterSlot::default()).collect(),
        }
    }
}
