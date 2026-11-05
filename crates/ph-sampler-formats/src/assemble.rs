//! Builds a [`HausInstrument`] from zones that each carry their own group-level settings.
//!
//! The model keeps envelopes, filter, LFOs, voice limits, round-robin mode, output and
//! effects per **group**, and modulation routes per instrument (optionally scoped to a
//! group). Formats that set these per zone (SF2 instrument zones, DecentSampler samples)
//! hand each zone a draft group; zones whose drafts agree share one Haus group, the others
//! split into `Name (2)`, `Name (3)`, ... so nothing is averaged away. The SFZ mapper
//! follows the same rule with its own buckets.

use ph_core::*;
use std::collections::BTreeMap;

/// The group-level part of one zone: the group (name = the base name; id ignored) and the
/// routes that apply to its voices only (their `group` is filled in here).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GroupDraft {
    pub group: Group,
    pub routes: Vec<ModRoute>,
}

impl GroupDraft {
    pub fn new(name: &str) -> Self {
        Self {
            group: Group::new(name),
            routes: Vec::new(),
        }
    }

    /// Adds a route (dropped when its depth is zero).
    pub fn route(&mut self, source: ModSource, target: ModTarget, depth: f64, curve: ModCurve) {
        if depth != 0.0 && depth.is_finite() {
            self.routes.push(ModRoute {
                source,
                target,
                depth: depth as f32,
                curve,
                group: None,
            });
        }
    }
}

#[derive(Default)]
pub(crate) struct Assembler {
    drafts: Vec<GroupDraft>,
    names: BTreeMap<String, u32>,
    group_names: Vec<String>,
    zones: Vec<Zone>,
    /// Routes for every group (`group: None`).
    pub global_routes: Vec<ModRoute>,
}

impl Assembler {
    /// The group for a draft: an existing one with the same settings, or a new one.
    pub fn group(&mut self, draft: &GroupDraft) -> GroupId {
        if let Some(i) = self.drafts.iter().position(|d| d == draft) {
            return GroupId(i as u32);
        }
        let base = if draft.group.name.is_empty() {
            "Group".to_string()
        } else {
            draft.group.name.clone()
        };
        let n = self.names.entry(base.clone()).or_insert(0);
        *n += 1;
        self.group_names.push(if *n == 1 {
            base
        } else {
            format!("{base} ({n})")
        });
        self.drafts.push(draft.clone());
        GroupId(self.drafts.len() as u32 - 1)
    }

    /// Adds a zone to `group` (its id is assigned here).
    pub fn zone(&mut self, mut z: Zone, group: GroupId) {
        z.id = ZoneId(self.zones.len() as u32);
        z.group = group;
        self.zones.push(z);
    }

    /// The groups and zones, moved into `inst` (groups without zones are dropped and the
    /// rest renumbered in order).
    pub fn finish(self, inst: &mut HausInstrument) {
        let mut used = vec![false; self.drafts.len()];
        for z in &self.zones {
            used[z.group.0 as usize] = true;
        }
        let mut new_id = vec![GroupId(0); self.drafts.len()];
        let mut routes = self.global_routes;
        let mut next = 0u32;
        for (i, d) in self.drafts.into_iter().enumerate() {
            if !used[i] {
                continue;
            }
            let gid = GroupId(next);
            next += 1;
            new_id[i] = gid;
            let mut g = d.group;
            g.id = gid;
            g.name = self.group_names[i].clone();
            for mut r in d.routes {
                r.group = Some(gid);
                routes.push(r);
            }
            inst.groups.insert(gid, g);
        }
        for mut z in self.zones {
            z.group = new_id[z.group.0 as usize];
            inst.zones.insert(z.id, z);
        }
        inst.modulation = routes;
    }
}

/// Splits `cents` into whole semitones and cents within ±50 (tune clamped to ±48, fine to
/// ±100). Returns `(tune, fine, clamped)`.
pub(crate) fn split_cents(cents: f64) -> (i32, f32, bool) {
    if !cents.is_finite() {
        return (0, 0.0, true);
    }
    let tune = (cents / 100.0).round();
    let clamped_tune = tune.clamp(-48.0, 48.0);
    let fine = (cents - clamped_tune * 100.0).clamp(-100.0, 100.0);
    let clamped = clamped_tune != tune || (cents - clamped_tune * 100.0).abs() > 100.0;
    (clamped_tune as i32, fine as f32, clamped)
}

/// A finite value clamped to `lo..=hi` (NaN → `lo`).
pub(crate) fn clampf(v: f64, lo: f64, hi: f64) -> f64 {
    if v.is_nan() { lo } else { v.clamp(lo, hi) }
}

/// Safe file-name part for a sample name.
pub(crate) fn safe_file(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "sample".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_split_into_semitones_and_fine() {
        assert_eq!(split_cents(0.0), (0, 0.0, false));
        assert_eq!(split_cents(-4.0), (0, -4.0, false));
        assert_eq!(split_cents(1250.0), (13, -50.0, false));
        assert_eq!(split_cents(1240.0), (12, 40.0, false));
        assert_eq!(split_cents(-1260.0), (-13, 40.0, false));
        assert_eq!(split_cents(10_000.0), (48, 100.0, true));
    }

    #[test]
    fn equal_drafts_share_a_group_and_names_number() {
        let mut a = Assembler::default();
        let d1 = GroupDraft::new("Kit");
        let mut d2 = GroupDraft::new("Kit");
        d2.group.voice_limit = Some(1);
        let g1 = a.group(&d1);
        let g2 = a.group(&d2);
        assert_eq!(a.group(&d1), g1);
        assert_ne!(g1, g2);
        let z = Zone::new(g1, SampleRef::File("a.wav".into()), KeyRange::ALL, 60);
        a.zone(z.clone(), g1);
        a.zone(z, g2);
        let mut inst = HausInstrument::new("x");
        a.finish(&mut inst);
        let names: Vec<_> = inst.groups.values().map(|g| g.name.clone()).collect();
        assert_eq!(names, ["Kit", "Kit (2)"]);
    }
}
