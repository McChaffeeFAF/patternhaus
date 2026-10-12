//! Automation in the model (M2): [`ParamPath`] targets, automation clips and their points.
//!
//! # Contract (M2/T0)
//!
//! - **Values are plain**, in the target's own unit: BPM for `tempo`, 0..=1 channel volume,
//!   -1..=1 pan, 0..=2 insert volume, 0/1 mute, and a processor parameter's own range
//!   (`ph_node::ParamInfo::min..=max`). The engine clamps on apply, like `set_param`.
//! - **Points are separate objects** (`point/<clip>/<point>`), so editing one point is one
//!   small patch and two authors can edit different points of one clip without conflict.
//!   A clip's patch value never carries points.
//! - **Point ticks are relative to the clip's source**: the point at tick `t` sounds at song
//!   tick `clip.start + t - clip.offset`, like notes in a pattern clip.
//! - **Order:** points are stored by id; evaluation order is `(tick, id)`. Two points on the
//!   same tick make a jump: the earlier id is the value arriving, the later the value leaving.
//! - **Segments:** a point's `curve` and `tension` shape the segment from it to the next
//!   point. Before the first point the value is the first point's; after the last, the last's.
//! - **Overlaps:** where several clips automate one target at once, the clip that starts
//!   latest wins (then the higher clip id). Outside every clip the target keeps the value
//!   the model holds (the knob position).

use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Something automation can drive.
///
/// String form (what agents use): `tempo`, `channel/<id>/volume`, `channel/<id>/pan`,
/// `channel/<id>/param/<param>`, `insert/<n>/volume`, `insert/<n>/pan`, `insert/<n>/mute`,
/// `insert/<n>/slot/<s>/param/<param>`. `<param>` is the processor's stable parameter id
/// (`ph_node::ParamInfo::id`; for plugins, the plugin's own parameter id as text). It may
/// not contain `/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ParamPath {
    /// Project tempo, in BPM (10..=999). Drives the engine's `TempoMap`.
    Tempo,
    /// Channel volume, 0..=1.
    ChannelVolume(ChannelId),
    /// Channel pan, -1..=1.
    ChannelPan(ChannelId),
    /// A parameter of the channel's instrument.
    ChannelParam(ChannelId, String),
    /// Mixer insert volume, 0..=2.
    InsertVolume(InsertId),
    /// Mixer insert pan, -1..=1.
    InsertPan(InsertId),
    /// Mixer insert mute: 0 = on, ≥ 0.5 = muted.
    InsertMute(InsertId),
    /// A parameter of the effect in `slot` (index into the insert's effect list) of `insert`.
    EffectParam {
        insert: InsertId,
        slot: u32,
        param: String,
    },
}

impl ParamPath {
    /// The model object whose value this target overrides while automated.
    pub fn object(&self) -> ObjectPath {
        match self {
            ParamPath::Tempo => ObjectPath::Settings,
            ParamPath::ChannelVolume(c)
            | ParamPath::ChannelPan(c)
            | ParamPath::ChannelParam(c, _) => ObjectPath::Channel(*c),
            ParamPath::InsertVolume(i)
            | ParamPath::InsertPan(i)
            | ParamPath::InsertMute(i)
            | ParamPath::EffectParam { insert: i, .. } => ObjectPath::Insert(*i),
        }
    }

    /// The value range of targets the model defines itself; `None` for processor
    /// parameters, whose range comes from their descriptor.
    pub fn fixed_range(&self) -> Option<(f64, f64)> {
        match self {
            ParamPath::Tempo => Some((10.0, 999.0)),
            ParamPath::ChannelVolume(_) => Some((0.0, 1.0)),
            ParamPath::ChannelPan(_) | ParamPath::InsertPan(_) => Some((-1.0, 1.0)),
            ParamPath::InsertVolume(_) => Some((0.0, 2.0)),
            ParamPath::InsertMute(_) => Some((0.0, 1.0)),
            ParamPath::ChannelParam(..) | ParamPath::EffectParam { .. } => None,
        }
    }

    /// The model's current (unautomated) value of a model-defined target; `None` for
    /// processor parameters not stored in the model, or a missing channel/insert.
    pub fn model_value(&self, p: &Project) -> Option<f64> {
        match self {
            ParamPath::Tempo => Some(p.settings.bpm),
            ParamPath::ChannelVolume(c) => p.channels.get(c).map(|c| c.volume as f64),
            ParamPath::ChannelPan(c) => p.channels.get(c).map(|c| c.pan as f64),
            ParamPath::InsertVolume(i) => p.insert(*i).ok().map(|x| x.volume as f64),
            ParamPath::InsertPan(i) => p.insert(*i).ok().map(|x| x.pan as f64),
            ParamPath::InsertMute(i) => p.insert(*i).ok().map(|x| x.muted as u8 as f64),
            ParamPath::ChannelParam(c, id) => match &p.channels.get(c)?.instrument {
                InstrumentSpec::Sampler { params, .. }
                | InstrumentSpec::Synth { params, .. }
                | InstrumentSpec::MultiSampler { params, .. }
                | InstrumentSpec::Plugin { params, .. } => params.get(id).copied(),
            },
            ParamPath::EffectParam {
                insert,
                slot,
                param,
            } => match &p.insert(*insert).ok()?.effects.get(*slot as usize)?.effect {
                EffectSpec::Builtin { params, .. } | EffectSpec::Plugin { params, .. } => {
                    params.get(param).copied()
                }
            },
        }
    }
}

impl fmt::Display for ParamPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamPath::Tempo => write!(f, "tempo"),
            ParamPath::ChannelVolume(c) => write!(f, "channel/{c}/volume"),
            ParamPath::ChannelPan(c) => write!(f, "channel/{c}/pan"),
            ParamPath::ChannelParam(c, p) => write!(f, "channel/{c}/param/{p}"),
            ParamPath::InsertVolume(i) => write!(f, "insert/{i}/volume"),
            ParamPath::InsertPan(i) => write!(f, "insert/{i}/pan"),
            ParamPath::InsertMute(i) => write!(f, "insert/{i}/mute"),
            ParamPath::EffectParam {
                insert,
                slot,
                param,
            } => write!(f, "insert/{insert}/slot/{slot}/param/{param}"),
        }
    }
}

impl FromStr for ParamPath {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ModelError::Invalid(format!("bad parameter path {s:?}"));
        let parts: Vec<&str> = s.split('/').collect();
        let num = |i: usize| -> Result<u32, ModelError> {
            parts.get(i).and_then(|p| p.parse().ok()).ok_or_else(bad)
        };
        let param = |i: usize| -> Result<String, ModelError> {
            match parts.get(i) {
                Some(p) if !p.is_empty() => Ok(p.to_string()),
                _ => Err(bad()),
            }
        };
        Ok(match parts.as_slice() {
            ["tempo"] => ParamPath::Tempo,
            ["channel", _, "volume"] => ParamPath::ChannelVolume(ChannelId(num(1)?)),
            ["channel", _, "pan"] => ParamPath::ChannelPan(ChannelId(num(1)?)),
            ["channel", _, "param", _] => ParamPath::ChannelParam(ChannelId(num(1)?), param(3)?),
            ["insert", _, "volume"] => ParamPath::InsertVolume(InsertId(num(1)?)),
            ["insert", _, "pan"] => ParamPath::InsertPan(InsertId(num(1)?)),
            ["insert", _, "mute"] => ParamPath::InsertMute(InsertId(num(1)?)),
            ["insert", _, "slot", _, "param", _] => ParamPath::EffectParam {
                insert: InsertId(num(1)?),
                slot: num(3)?,
                param: param(5)?,
            },
            _ => return Err(bad()),
        })
    }
}

impl Serialize for ParamPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ParamPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ParamPath {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ParamPath".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Automation target: tempo | channel/<id>/volume | channel/<id>/pan | channel/<id>/param/<param> | insert/<n>/volume | insert/<n>/pan | insert/<n>/mute | insert/<n>/slot/<s>/param/<param>",
            "pattern": "^(tempo|channel/\\d+/(volume|pan|param/[^/]+)|insert/\\d+/(volume|pan|mute|slot/\\d+/param/[^/]+))$"
        })
    }
}

/// An automation clip's settings (the playlist position is on [`crate::Clip`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationClip {
    pub target: ParamPath,
    /// The points, by id. Edited only through `point/<clip>/<id>` patches; empty in a
    /// clip's own patch value. Use [`AutomationClip::points_in_order`] to evaluate.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub points: BTreeMap<PointId, AutomationPoint>,
}

impl AutomationClip {
    pub fn new(target: ParamPath) -> Self {
        Self {
            target,
            points: BTreeMap::new(),
        }
    }

    /// Points in evaluation order: by tick, then by id.
    pub fn points_in_order(&self) -> Vec<&AutomationPoint> {
        let mut v: Vec<&AutomationPoint> = self.points.values().collect();
        v.sort_by_key(|p| (p.tick, p.id));
        v
    }

    /// The value at source tick `tick` (fractional), or `None` without points.
    pub fn value_at(&self, tick: f64) -> Option<f64> {
        value_at(&self.points_in_order(), tick)
    }
}

/// The value of an ordered point list (see [`AutomationClip::points_in_order`]) at `tick`.
/// Allocation-free, so the engine can call it on prepared slices.
pub fn value_at(points: &[&AutomationPoint], tick: f64) -> Option<f64> {
    let first = points.first()?;
    if tick < first.tick as f64 {
        return Some(first.value);
    }
    // last point at or before `tick` (the later id on a shared tick)
    let i = points.partition_point(|p| p.tick as f64 <= tick) - 1;
    let a = points[i];
    let Some(b) = points.get(i + 1) else {
        return Some(a.value);
    };
    let span = (b.tick - a.tick) as f64;
    let t = if span > 0.0 {
        (tick - a.tick as f64) / span
    } else {
        1.0
    };
    let s = match a.curve {
        CurveKind::Smooth => {
            // monotone cubic Hermite (Fritsch–Carlson) on the neighbouring slopes
            let prev = i.checked_sub(1).map(|j| points[j]);
            let next = points.get(i + 2).copied();
            return Some(smooth(prev, a, b, next, t));
        }
        k => k.shape(a.tension, t),
    };
    Some(a.value + (b.value - a.value) * s)
}

fn smooth(
    prev: Option<&AutomationPoint>,
    a: &AutomationPoint,
    b: &AutomationPoint,
    next: Option<&AutomationPoint>,
    t: f64,
) -> f64 {
    let h = (b.tick - a.tick) as f64;
    if h <= 0.0 {
        return b.value;
    }
    let d = (b.value - a.value) / h;
    let slope = |p: Option<&AutomationPoint>, q: &AutomationPoint, before: bool| -> f64 {
        let Some(p) = p else { return d };
        let w = if before {
            (q.tick - p.tick) as f64
        } else {
            (p.tick - q.tick) as f64
        };
        if w <= 0.0 {
            return 0.0;
        }
        let other = if before {
            (q.value - p.value) / w
        } else {
            (p.value - q.value) / w
        };
        if other * d <= 0.0 {
            0.0
        } else {
            // harmonic mean keeps the curve monotone between points
            2.0 * other * d / (other + d)
        }
    };
    let m0 = slope(prev, a, true);
    let m1 = slope(next, b, false);
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    let v = h00 * a.value + h10 * h * m0 + h01 * b.value + h11 * h * m1;
    // numerical safety: never leave the segment's value range
    v.clamp(a.value.min(b.value), a.value.max(b.value))
}

/// One automation point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AutomationPoint {
    pub id: PointId,
    /// Position in ticks, relative to the clip's source (see the module docs).
    pub tick: Tick,
    /// Plain value in the target's unit.
    pub value: f64,
    /// Shape of the segment from this point to the next.
    #[serde(default)]
    pub curve: CurveKind,
    /// -1..=1; bends `Linear` and `SCurve` segments (0 = their plain shape). Ignored by
    /// `Hold` and `Smooth`.
    #[serde(default)]
    pub tension: f32,
}

/// Segment shapes. With `s` the shape in 0..=1 at position `t` of a segment from `a` to `b`,
/// the value is `a + (b − a)·s`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurveKind {
    /// `s = t^p`, `p = 2^(−3·tension)`: straight at tension 0 (FL's "single curve").
    #[default]
    Linear,
    /// Keep `a` until the next point, then jump.
    Hold,
    /// Monotone cubic through the neighbouring points: smooth, never overshoots.
    Smooth,
    /// Ease in and out: `s = ½(2t)^q` below the middle, mirrored above, with
    /// `q = 2^(1 + 2·tension)` (quadratic at tension 0).
    SCurve,
}

impl CurveKind {
    /// The 0..=1 shape at `t` in 0..=1 (`Smooth` needs neighbours and is handled by
    /// [`value_at`]; here it falls back to the S-curve). Exact at both ends.
    pub fn shape(self, tension: f32, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        let tension = (tension as f64).clamp(-1.0, 1.0);
        match self {
            CurveKind::Hold => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            CurveKind::Linear => t.powf(2f64.powf(-3.0 * tension)),
            CurveKind::SCurve | CurveKind::Smooth => {
                let q = 2f64.powf(1.0 + 2.0 * tension);
                if t < 0.5 {
                    0.5 * (2.0 * t).powf(q)
                } else {
                    1.0 - 0.5 * (2.0 - 2.0 * t).powf(q)
                }
            }
        }
    }
}

pub(crate) fn validate_point(p: &AutomationPoint) -> ModelResult<()> {
    if !p.value.is_finite() {
        return Err(ModelError::Invalid("point value must be finite".into()));
    }
    if !p.tension.is_finite() || !(-1.0..=1.0).contains(&p.tension) {
        return Err(ModelError::Invalid(format!(
            "tension {} outside -1..1",
            p.tension
        )));
    }
    Ok(())
}
