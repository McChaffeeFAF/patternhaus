//! The Haus Sampler zone map's geometry and gestures (M3/T5), without egui state: where each
//! zone is drawn, what a press hits, and which `sampler.*` requests a drag becomes.
//!
//! Keys run across (one column per key, `key0` at the left edge), velocity up (0 at the
//! bottom, 127 at the top). Zones that are round-robin alternatives of each other (the same
//! group, keys and velocities) are **stacked**: each gets its own band of the shared rectangle,
//! so every one can be seen and picked. Release-trigger zones are drawn as outlines.

use crate::mapping_sampler::{self, Place};
use egui::{Pos2, Rect};
use ph_commands::Request;
use ph_core::{HausInstrument, InstrumentId, KeyRange, VelRange, Zone, ZoneId, ZoneTrigger};
use std::collections::BTreeMap;

/// Pixels from a zone's edge that grab the edge instead of the body.
pub const EDGE: f32 = 5.0;
/// Narrowest and widest key column, px.
pub const KEY_W_MIN: f32 = 3.0;
pub const KEY_W_MAX: f32 = 48.0;

/// The visible part of the map.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MapView {
    pub rect: Rect,
    /// The key at the left edge (fractional while scrolling).
    pub key0: f32,
    /// Width of one key, px.
    pub key_w: f32,
}

impl MapView {
    pub fn x_of(&self, key: f32) -> f32 {
        self.rect.left() + (key - self.key0) * self.key_w
    }

    /// The key under `x` (may lie off the keyboard; clamp when you need a real key).
    pub fn key_at(&self, x: f32) -> f32 {
        self.key0 + (x - self.rect.left()) / self.key_w
    }

    /// The top of velocity row `vel` (0..=128: 128 is the top edge).
    pub fn y_of(&self, vel: f32) -> f32 {
        self.rect.bottom() - vel / 128.0 * self.rect.height()
    }

    pub fn vel_at(&self, y: f32) -> f32 {
        (self.rect.bottom() - y) / self.rect.height() * 128.0
    }

    /// Pixels per velocity step.
    pub fn vel_h(&self) -> f32 {
        self.rect.height() / 128.0
    }

    /// The rectangle of a key and velocity range.
    pub fn range_rect(&self, keys: KeyRange, vel: VelRange) -> Rect {
        Rect::from_min_max(
            Pos2::new(self.x_of(keys.lo as f32), self.y_of(vel.hi as f32 + 1.0)),
            Pos2::new(self.x_of(keys.hi as f32 + 1.0), self.y_of(vel.lo as f32)),
        )
    }

    /// Zooms around the key under `x`, keeping it in place.
    pub fn zoom(&mut self, factor: f32, x: f32) {
        let k = self.key_at(x);
        self.key_w = (self.key_w * factor).clamp(KEY_W_MIN, KEY_W_MAX);
        self.key0 = k - (x - self.rect.left()) / self.key_w;
        self.clamp();
    }

    /// Scrolls by `px` (positive: towards higher keys).
    pub fn scroll(&mut self, px: f32) {
        self.key0 += px / self.key_w;
        self.clamp();
    }

    /// Keeps the keyboard in view.
    pub fn clamp(&mut self) {
        let visible = self.rect.width() / self.key_w;
        let max = (128.0 - visible).max(0.0);
        self.key0 = self.key0.clamp(0.0, max);
    }

    /// Shows keys `lo..=hi` (with a little room either side).
    pub fn fit(&mut self, lo: u8, hi: u8) {
        let (lo, hi) = (lo.saturating_sub(2) as f32, (hi as f32 + 3.0).min(128.0));
        self.key_w = (self.rect.width() / (hi - lo).max(1.0)).clamp(KEY_W_MIN, KEY_W_MAX);
        self.key0 = lo;
        self.clamp();
    }
}

/// How one zone is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneShape {
    pub zone: ZoneId,
    /// The whole key × velocity rectangle.
    pub rect: Rect,
    /// The part it is drawn in and picked by: `rect`, or its round-robin band.
    pub band: Rect,
    /// Release trigger: drawn as an outline.
    pub outlined: bool,
    /// "RR 2" for a round-robin position, else empty.
    pub label: String,
}

/// Where every zone is drawn, in draw order (later ones on top): attack zones, then release
/// zones, each in id order.
pub fn layout(inst: &HausInstrument, view: &MapView) -> Vec<ZoneShape> {
    // round-robin stacks: zones sharing group, keys and velocities
    type StackKey = (u32, u8, u8, u8, u8, bool);
    let key_of = |z: &Zone| -> StackKey {
        (
            z.group.0,
            z.keys.lo,
            z.keys.hi,
            z.velocity.lo,
            z.velocity.hi,
            z.trigger == ZoneTrigger::Release,
        )
    };
    let mut stacks: BTreeMap<StackKey, Vec<&Zone>> = BTreeMap::new();
    for z in inst.zones.values().filter(|z| z.round_robin.is_some()) {
        stacks.entry(key_of(z)).or_default().push(z);
    }
    for s in stacks.values_mut() {
        s.sort_by_key(|z| (z.round_robin, z.id));
    }
    let mut out: Vec<ZoneShape> = Vec::with_capacity(inst.zones.len());
    for release in [false, true] {
        for z in inst
            .zones
            .values()
            .filter(|z| (z.trigger == ZoneTrigger::Release) == release)
        {
            let rect = view.range_rect(z.keys, z.velocity);
            let (band, label) = match z.round_robin {
                Some(pos) => {
                    let stack = &stacks[&key_of(z)];
                    let n = stack.len().max(1) as f32;
                    let i = stack.iter().position(|o| o.id == z.id).unwrap_or(0) as f32;
                    // the first position on top
                    let h = rect.height() / n;
                    (
                        Rect::from_min_max(
                            Pos2::new(rect.left(), rect.top() + h * i),
                            Pos2::new(rect.right(), rect.top() + h * (i + 1.0)),
                        ),
                        format!("RR {}", pos + 1),
                    )
                }
                None => (rect, String::new()),
            };
            out.push(ZoneShape {
                zone: z.id,
                rect,
                band,
                outlined: release,
                label,
            });
        }
    }
    out
}

/// What a press on the map grabs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Grab {
    Move,
    Left,
    Right,
    Top,
    Bottom,
}

/// The zone (topmost first) and part under `p`. A release zone is drawn as an outline over
/// the zones it shares keys with, so it is picked by its outline first, and by its inside only
/// where no filled zone is.
pub fn hit(shapes: &[ZoneShape], p: Pos2) -> Option<(ZoneId, Grab)> {
    let on_border = |s: &ZoneShape| {
        s.band.expand(1.0).contains(p) && !s.band.shrink(EDGE.min(s.band.width() / 3.0)).contains(p)
    };
    let pass = |want: &dyn Fn(&ZoneShape) -> bool| {
        shapes
            .iter()
            .rev()
            .find(|s| want(s))
            .map(|s| (s.zone, grab_of(s, p)))
    };
    pass(&|s| s.outlined && on_border(s))
        .or_else(|| pass(&|s| !s.outlined && s.band.expand(1.0).contains(p)))
        .or_else(|| pass(&|s| s.outlined && s.band.expand(1.0).contains(p)))
}

/// Which part of a shape `p` (inside it) grabs.
fn grab_of(s: &ZoneShape, p: Pos2) -> Grab {
    let r = s.rect;
    let edge_x = (r.width() / 3.0).min(EDGE);
    let edge_y = (s.band.height() / 3.0).min(EDGE);
    if p.x <= r.left() + edge_x {
        Grab::Left
    } else if p.x >= r.right() - edge_x {
        Grab::Right
    } else if p.y <= r.top() + edge_y && s.band.top() <= r.top() + 0.5 {
        Grab::Top
    } else if p.y >= r.bottom() - edge_y && s.band.bottom() >= r.bottom() - 0.5 {
        Grab::Bottom
    } else {
        Grab::Move
    }
}

/// The zones whose band touches `area` (rubber-band selection).
pub fn in_area(shapes: &[ZoneShape], area: Rect) -> Vec<ZoneId> {
    shapes
        .iter()
        .filter(|s| s.band.intersects(area))
        .map(|s| s.zone)
        .collect()
}

/// A drag of `grab` by `dkey` keys and `dvel` velocity steps, applied to one zone: its new key
/// and velocity ranges. Moves are clamped by `allowed` (the shared limits of the selection),
/// edges by the zone itself (a range never turns over).
pub fn apply(
    z: &Zone,
    grab: Grab,
    dkey: i32,
    dvel: i32,
    allowed: ((i32, i32), (i32, i32)),
) -> (KeyRange, VelRange) {
    let (mut k, mut v) = (z.keys, z.velocity);
    let clamp = |x: i32, lo: i32, hi: i32| x.clamp(lo, hi.max(lo));
    match grab {
        Grab::Move => {
            let dk = clamp(dkey, allowed.0.0, allowed.0.1);
            let dv = clamp(dvel, allowed.1.0, allowed.1.1);
            k = KeyRange {
                lo: (k.lo as i32 + dk) as u8,
                hi: (k.hi as i32 + dk) as u8,
            };
            v = VelRange {
                lo: (v.lo as i32 + dv) as u8,
                hi: (v.hi as i32 + dv) as u8,
            };
        }
        Grab::Left => k.lo = clamp(k.lo as i32 + dkey, 0, k.hi as i32) as u8,
        Grab::Right => k.hi = clamp(k.hi as i32 + dkey, k.lo as i32, 127) as u8,
        Grab::Bottom => v.lo = clamp(v.lo as i32 + dvel, 0, v.hi as i32) as u8,
        Grab::Top => v.hi = clamp(v.hi as i32 + dvel, v.lo as i32, 127) as u8,
    }
    (k, v)
}

/// How far a set of zones may move together: ((key min, key max), (vel min, vel max)).
pub fn move_limits(zones: &[Zone]) -> ((i32, i32), (i32, i32)) {
    let kmin = zones.iter().map(|z| z.keys.lo as i32).min().unwrap_or(0);
    let kmax = zones.iter().map(|z| z.keys.hi as i32).max().unwrap_or(127);
    let vmin = zones
        .iter()
        .map(|z| z.velocity.lo as i32)
        .min()
        .unwrap_or(0);
    let vmax = zones
        .iter()
        .map(|z| z.velocity.hi as i32)
        .max()
        .unwrap_or(127);
    ((-kmin, 127 - kmax), (-vmin, 127 - vmax))
}

/// The requests a finished drag sends (empty when nothing changed). More than one request is
/// sent as one batch, so the whole drag is one undo step.
pub fn drag_requests(
    inst: InstrumentId,
    zones: &[Zone],
    grab: Grab,
    dkey: i32,
    dvel: i32,
) -> Vec<Request> {
    let allowed = move_limits(zones);
    let moved: Vec<(&Zone, KeyRange, VelRange)> = zones
        .iter()
        .map(|z| {
            let (k, v) = apply(z, grab, dkey, dvel, allowed);
            (z, k, v)
        })
        .filter(|(z, k, v)| *k != z.keys || *v != z.velocity)
        .collect();
    if moved.is_empty() {
        return Vec::new();
    }
    let ids: Vec<ZoneId> = moved.iter().map(|(z, ..)| z.id).collect();
    match grab {
        Grab::Move => {
            let dk = moved[0].1.lo as i32 - moved[0].0.keys.lo as i32;
            let same_vel = moved.iter().all(|(z, _, v)| *v == z.velocity);
            if same_vel {
                // a sideways move of any selection is one command
                vec![mapping_sampler::zone_move(
                    inst,
                    &ids,
                    &Place {
                        shift: Some(dk),
                        root_follows: true,
                        ..Default::default()
                    },
                )]
            } else {
                moved
                    .iter()
                    .map(|(z, _, v)| {
                        mapping_sampler::zone_move(
                            inst,
                            &[z.id],
                            &Place {
                                shift: (dk != 0).then_some(dk),
                                velocity: Some(*v),
                                root_follows: true,
                                ..Default::default()
                            },
                        )
                    })
                    .collect()
            }
        }
        Grab::Left | Grab::Right => moved
            .iter()
            .map(|(z, k, _)| {
                mapping_sampler::zone_move(
                    inst,
                    &[z.id],
                    &Place {
                        keys: Some(*k),
                        root_follows: false,
                        ..Default::default()
                    },
                )
            })
            .collect(),
        Grab::Top | Grab::Bottom => moved
            .iter()
            .map(|(z, _, v)| {
                mapping_sampler::zone_move(
                    inst,
                    &[z.id],
                    &Place {
                        velocity: Some(*v),
                        root_follows: false,
                        ..Default::default()
                    },
                )
            })
            .collect(),
    }
}

/// The keys any zone covers, and the lowest and highest of them.
pub fn coverage(inst: &HausInstrument) -> ([bool; 128], Option<(u8, u8)>) {
    let mut c = [false; 128];
    for z in inst.zones.values() {
        for k in z.keys.lo..=z.keys.hi.min(127) {
            c[k as usize] = true;
        }
    }
    let lo = c.iter().position(|x| *x).map(|i| i as u8);
    let hi = c.iter().rposition(|x| *x).map(|i| i as u8);
    (c, lo.zip(hi))
}

/// MIDI key name with the octave, C4 = 60 ("C#4").
pub fn key_name(k: u8) -> String {
    crate::piano_roll::note_name(k)
}

pub fn is_black(k: u8) -> bool {
    matches!(k % 12, 1 | 3 | 6 | 8 | 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::{GroupId, SampleRef};

    fn zone(id: u32, keys: (u8, u8), vel: (u8, u8)) -> Zone {
        let mut z = Zone::new(
            GroupId(0),
            SampleRef::File(format!("s{id}.wav")),
            KeyRange {
                lo: keys.0,
                hi: keys.1,
            },
            keys.0,
        );
        z.id = ZoneId(id);
        z.velocity = VelRange {
            lo: vel.0,
            hi: vel.1,
        };
        z
    }

    fn view() -> MapView {
        MapView {
            rect: Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(1280.0, 256.0)),
            key0: 0.0,
            key_w: 10.0,
        }
    }

    #[test]
    fn ranges_map_to_rectangles_and_back() {
        let v = view();
        let r = v.range_rect(KeyRange { lo: 60, hi: 63 }, VelRange { lo: 0, hi: 63 });
        assert_eq!(r.left(), 600.0);
        assert_eq!(r.right(), 640.0);
        assert_eq!(r.bottom(), 256.0);
        assert_eq!(r.top(), 128.0);
        assert_eq!(v.key_at(605.0).floor(), 60.0);
        assert_eq!(v.vel_at(255.0).floor(), 0.0);
    }

    #[test]
    fn round_robins_stack_and_release_zones_draw_last_as_outlines() {
        let mut inst = HausInstrument::new("x");
        let mut a = zone(1, (60, 64), (0, 127));
        a.round_robin = Some(0);
        let mut b = zone(2, (60, 64), (0, 127));
        b.round_robin = Some(1);
        let mut rel = zone(3, (60, 64), (0, 127));
        rel.trigger = ZoneTrigger::Release;
        let plain = zone(4, (70, 72), (0, 127));
        for z in [rel, a, b, plain] {
            inst.zones.insert(z.id, z);
        }
        let shapes = layout(&inst, &view());
        let ids: Vec<u32> = shapes.iter().map(|s| s.zone.0).collect();
        assert_eq!(ids, [1, 2, 4, 3], "release zones on top");
        assert_eq!(shapes[0].label, "RR 1");
        assert!(shapes[0].band.bottom() <= shapes[1].band.top() + 0.01);
        assert_eq!(shapes[0].band.height(), shapes[0].rect.height() / 2.0);
        assert!(shapes[3].outlined && !shapes[0].outlined);
        // each round robin can be picked by its own band
        let p1 = shapes[0].band.center();
        let p2 = shapes[1].band.center();
        assert_eq!(hit(&shapes, p1).unwrap(), (ZoneId(1), Grab::Move));
        assert_eq!(hit(&shapes, p2).unwrap(), (ZoneId(2), Grab::Move));
        // the release outline on top of them is picked by its border
        let r = shapes[3].rect;
        assert_eq!(
            hit(&shapes, Pos2::new(r.left() + 1.0, r.center().y)),
            Some((ZoneId(3), Grab::Left))
        );
    }

    #[test]
    fn edges_are_grabbed_near_the_border() {
        let mut inst = HausInstrument::new("x");
        let z = zone(1, (60, 69), (0, 127));
        inst.zones.insert(z.id, z);
        let s = layout(&inst, &view());
        let r = s[0].rect;
        assert_eq!(
            hit(&s, Pos2::new(r.left() + 1.0, r.center().y)),
            Some((ZoneId(1), Grab::Left))
        );
        assert_eq!(
            hit(&s, Pos2::new(r.right() - 1.0, r.center().y)),
            Some((ZoneId(1), Grab::Right))
        );
        assert_eq!(
            hit(&s, Pos2::new(r.center().x, r.top() + 1.0)),
            Some((ZoneId(1), Grab::Top))
        );
        assert_eq!(
            hit(&s, Pos2::new(r.center().x, r.bottom() - 1.0)),
            Some((ZoneId(1), Grab::Bottom))
        );
        assert_eq!(hit(&s, r.center()), Some((ZoneId(1), Grab::Move)));
        assert_eq!(hit(&s, Pos2::new(10.0, 10.0)), None);
    }

    #[test]
    fn drags_become_zone_moves() {
        let inst = InstrumentId(0);
        let a = zone(1, (60, 64), (0, 63));
        let b = zone(2, (65, 69), (64, 127));
        // sideways: one command for the whole selection
        let r = drag_requests(inst, &[a.clone(), b.clone()], Grab::Move, 2, 0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].command, "sampler.zone_move");
        assert_eq!(r[0].params["shift"], 2);
        assert_eq!(r[0].params["zones"], serde_json::json!([1, 2]));
        // up and over the top: clamped by the highest zone, so b stays at 127
        let r = drag_requests(inst, &[a.clone(), b.clone()], Grab::Move, 0, 10);
        assert!(r.is_empty(), "b already reaches 127: nothing moves");
        let r = drag_requests(inst, std::slice::from_ref(&a), Grab::Move, -1, 10);
        assert_eq!(r[0].params["velocity"], serde_json::json!([10, 73]));
        assert_eq!(r[0].params["shift"], -1);
        // edges keep the root key and never turn a range over
        let r = drag_requests(inst, std::slice::from_ref(&a), Grab::Right, -10, 0);
        assert_eq!(r[0].params["keys"], serde_json::json!([60, 60]));
        assert_eq!(r[0].params["root_follows"], false);
        let r = drag_requests(inst, std::slice::from_ref(&b), Grab::Bottom, 0, -14);
        assert_eq!(r[0].params["velocity"], serde_json::json!([50, 127]));
        assert!(drag_requests(inst, &[a], Grab::Left, 0, 0).is_empty());
    }

    #[test]
    fn zoom_keeps_the_key_under_the_pointer() {
        let mut v = view();
        v.key_w = 10.0;
        v.key0 = 40.0;
        let k = v.key_at(300.0);
        v.zoom(2.0, 300.0);
        assert!((v.key_at(300.0) - k).abs() < 1e-3);
        v.fit(60, 72);
        assert!(v.x_of(58.0) >= v.rect.left() - 0.1);
        assert!(v.x_of(76.0) <= v.rect.right() + 0.1);
    }
}
