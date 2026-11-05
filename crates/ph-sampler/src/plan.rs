//! The immutable real-time plan an instrument compiles to.
//!
//! Compiled off the audio thread from a [`HausInstrument`] and its [`SampleSet`]; read-only on
//! the audio thread. Zones and groups become dense arrays (indices instead of ids), and every
//! key gets the list of zones whose key range covers it, split by trigger, so a note-on looks
//! at only the zones of its key.

use crate::samples::SampleSet;
use ph_core::{
    ArticulationTrigger, CcCondition, GroupId, HausInstrument, LoopRelease, Polyphony,
    RoundRobinMode, StealMode, ZoneId, ZoneTrigger,
};
use std::ops::Range;

/// A zone, ready for the audio thread.
#[derive(Clone, Debug)]
pub struct PlanZone {
    pub id: ZoneId,
    /// Index into [`Plan::groups`].
    pub group: usize,
    /// Index into the sample set; `None` = the sample failed to load (silent).
    pub sample: Option<usize>,
    pub vel_lo: u8,
    pub vel_hi: u8,
    pub root_key: u8,
    /// Semitones: zone tune + fine/100.
    pub transpose: f64,
    /// Linear gain of the zone volume.
    pub gain: f32,
    pub pan: f32,
    pub start: u64,
    /// One past the last frame (the file end when the zone has none).
    pub end: u64,
    pub looping: Option<PlanLoop>,
    pub reverse: bool,
    pub one_shot: bool,
    pub release_decay_db: f32,
    pub round_robin: Option<u32>,
    pub cc_conditions: Vec<CcCondition>,
}

#[derive(Clone, Debug)]
pub struct PlanLoop {
    pub start: u64,
    pub end: u64,
    pub crossfade: u64,
    pub ping_pong: bool,
    pub exit_on_release: bool,
}

/// A group, ready for the audio thread.
#[derive(Clone, Debug)]
pub struct PlanGroup {
    pub id: GroupId,
    pub gain: f32,
    pub voice_limit: Option<u32>,
    pub stealing: StealMode,
    pub polyphony: Polyphony,
    pub round_robin: RoundRobinMode,
    /// Round-robin cycle length: highest zone position + 1 (0 = no round robin).
    pub rr_len: u32,
    /// Output bus (0 = main).
    pub output: u32,
    /// Articulation indices (into [`Plan::articulations`]) it plays in; empty = all.
    pub articulations: Vec<usize>,
}

/// An articulation switch.
#[derive(Clone, Debug)]
pub struct PlanArticulation {
    pub trigger: ArticulationTrigger,
}

/// The compiled instrument.
#[derive(Clone, Debug)]
pub struct Plan {
    pub zones: Vec<PlanZone>,
    pub groups: Vec<PlanGroup>,
    pub articulations: Vec<PlanArticulation>,
    /// Articulation active before any switch.
    pub default_articulation: usize,
    /// Instrument gain (linear), pan, transpose (semitones incl. tune).
    pub gain: f32,
    pub pan: f32,
    pub transpose: f64,
    pub voice_limit: usize,
    /// Highest output bus used.
    pub max_output: u32,
    /// `attack[key]` / `release[key]`: a range of `lookup` holding zone indices.
    attack: Vec<Range<u32>>,
    release: Vec<Range<u32>>,
    lookup: Vec<u32>,
    /// Keys that are keyswitches (they select an articulation and make no sound).
    keyswitch: [Option<u8>; 128],
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

impl Plan {
    /// Compiles `inst` against its loaded samples. Off the audio thread (allocates).
    pub fn compile(inst: &HausInstrument, samples: &SampleSet) -> Plan {
        let group_ids: Vec<GroupId> = inst.groups.keys().copied().collect();
        let gidx = |g: GroupId| group_ids.iter().position(|x| *x == g);
        let art_ids: Vec<_> = inst.articulations.iter().map(|a| a.id).collect();
        let zones: Vec<PlanZone> = inst
            .zones
            .values()
            .filter_map(|z| {
                let group = gidx(z.group)?;
                let sample = samples.index.get(&z.id).copied();
                let file_end = sample.map_or(0, |s| samples.samples[s].frames);
                Some(PlanZone {
                    id: z.id,
                    group,
                    sample,
                    vel_lo: z.velocity.lo,
                    vel_hi: z.velocity.hi,
                    root_key: z.root_key,
                    transpose: z.tune as f64 + z.fine as f64 / 100.0,
                    gain: db(z.volume_db),
                    pan: z.pan,
                    start: z.start.min(file_end),
                    end: z.end.unwrap_or(file_end).min(file_end),
                    looping: z.looping.as_ref().map(|l| PlanLoop {
                        start: l.start,
                        end: l.end,
                        crossfade: l.crossfade,
                        ping_pong: l.mode == ph_core::LoopMode::PingPong,
                        exit_on_release: l.on_release == LoopRelease::Exit,
                    }),
                    reverse: z.reverse,
                    one_shot: z.one_shot,
                    release_decay_db: z.release_decay_db,
                    round_robin: z.round_robin,
                    cc_conditions: z.cc_conditions.clone(),
                })
            })
            .collect();
        let groups = inst
            .groups
            .values()
            .enumerate()
            .map(|(gi, g)| PlanGroup {
                id: g.id,
                gain: db(g.volume_db),
                voice_limit: g.voice_limit,
                stealing: g.stealing,
                polyphony: g.polyphony,
                round_robin: g.round_robin,
                rr_len: zones
                    .iter()
                    .filter(|z| z.group == gi)
                    .filter_map(|z| z.round_robin.map(|r| r + 1))
                    .max()
                    .unwrap_or(0),
                output: g.output,
                articulations: g
                    .articulations
                    .iter()
                    .filter_map(|a| art_ids.iter().position(|x| x == a))
                    .collect(),
            })
            .collect();
        // plan indices skip zones without a group, so map by id
        let pidx: std::collections::BTreeMap<ZoneId, u32> = zones
            .iter()
            .enumerate()
            .map(|(i, z)| (z.id, i as u32))
            .collect();
        let mut lookup = Vec::new();
        let mut table = |trigger: ZoneTrigger| -> Vec<Range<u32>> {
            (0..128u8)
                .map(|key| {
                    let from = lookup.len() as u32;
                    for z in inst.zones.values() {
                        if z.trigger == trigger
                            && z.keys.contains(key)
                            && let Some(i) = pidx.get(&z.id)
                        {
                            lookup.push(*i);
                        }
                    }
                    from..lookup.len() as u32
                })
                .collect()
        };
        let attack = table(ZoneTrigger::Attack);
        let release = table(ZoneTrigger::Release);
        let mut keyswitch = [None; 128];
        for (i, a) in inst.articulations.iter().enumerate() {
            if let ArticulationTrigger::Key { key } = a.trigger
                && (key as usize) < 128
            {
                keyswitch[key as usize] = Some(i as u8);
            }
        }
        let s = &inst.settings;
        Plan {
            zones,
            groups,
            articulations: inst
                .articulations
                .iter()
                .map(|a| PlanArticulation {
                    trigger: a.trigger.clone(),
                })
                .collect(),
            default_articulation: s
                .default_articulation
                .and_then(|d| art_ids.iter().position(|x| *x == d))
                .unwrap_or(0),
            gain: db(s.volume_db),
            pan: s.pan,
            transpose: s.transpose as f64 + s.tune_cents as f64 / 100.0,
            voice_limit: s.voice_limit.max(1) as usize,
            max_output: inst.max_output(),
            attack,
            release,
            lookup,
            keyswitch,
        }
    }

    /// Zone indices whose key range covers `key`, for `trigger`. Real-time.
    pub fn zones_for(&self, key: u8, trigger: ZoneTrigger) -> &[u32] {
        let table = match trigger {
            ZoneTrigger::Attack => &self.attack,
            ZoneTrigger::Release => &self.release,
        };
        match table.get(key as usize) {
            Some(r) => &self.lookup[r.start as usize..r.end as usize],
            None => &[],
        }
    }

    /// The articulation `key` selects, if it is a keyswitch. Real-time.
    pub fn keyswitch(&self, key: u8) -> Option<usize> {
        self.keyswitch
            .get(key as usize)
            .copied()
            .flatten()
            .map(usize::from)
    }

    /// True when group `g` plays in articulation `art`. Real-time.
    pub fn group_plays(&self, g: usize, art: usize) -> bool {
        let a = &self.groups[g].articulations;
        self.articulations.is_empty() || a.is_empty() || a.contains(&art)
    }
}
