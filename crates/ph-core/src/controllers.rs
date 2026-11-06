//! Controller lanes (M3/T10): MIDI controller and pitch-bend curves that live in a pattern.
//!
//! # Contract
//!
//! - A pattern holds, per channel, **points** ([`CcPoint`]); the points of one
//!   [`Controller`] on one channel make a **lane**. Like notes, points are separate objects
//!   (`cc/<pattern>/<channel>/<point>`), so two authors can edit different points of one lane
//!   without conflict, and one edit is one small patch. Point ids come from the project-wide
//!   `next_ids.point` counter (shared with automation points, never reused).
//! - Point ticks are **pattern-relative**, like note starts: a pattern clip plays the points
//!   in `[offset, offset + length)` at `clip.start + tick - offset`, exactly as for notes.
//!   Points at or past the pattern's length are not played.
//! - Semantics are those of automation points: the same [`CurveKind`] shapes and
//!   [`value_at`] evaluation (order `(tick, id)`; before the first point the first value, after
//!   the last the last value; two points on one tick make a jump).
//! - Values are normalised: CC `0..=1` (sent as the 7-bit `round(v * 127) / 127`),
//!   pitch bend `-1..=1` (sent as the 14-bit `round(v * 8192).clamp(-8192, 8191) / 8192`).
//!   [`Controller::wire`] is that rounding; the engine, MIDI export and the views agree on it.

use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// What a lane controls: a MIDI controller number 0..=119 or the pitch wheel. (120..=127 are
/// channel-mode messages, not controllers.)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Controller {
    /// MIDI CC, 0..=119.
    Cc(u8),
    PitchBend,
}

/// Highest controller number a lane can hold.
pub const MAX_CC: u8 = 119;

impl Controller {
    pub fn cc(n: u8) -> ModelResult<Controller> {
        if n > MAX_CC {
            return Err(ModelError::Invalid(format!(
                "controller {n} outside 0..={MAX_CC}"
            )));
        }
        Ok(Controller::Cc(n))
    }

    /// The conventional name: "Mod Wheel", "Expression", "Pitch Bend"; `CC<n>` otherwise.
    pub fn name(self) -> String {
        match self {
            Controller::PitchBend => "Pitch Bend".into(),
            Controller::Cc(n) => match Self::known(n) {
                Some(s) => s.into(),
                None => format!("CC{n}"),
            },
        }
    }

    /// `"CC1 Mod Wheel"`, `"Pitch Bend"`, `"CC3"`.
    pub fn label(self) -> String {
        match self {
            Controller::PitchBend => "Pitch Bend".into(),
            Controller::Cc(n) => match Self::known(n) {
                Some(s) => format!("CC{n} {s}"),
                None => format!("CC{n}"),
            },
        }
    }

    fn known(n: u8) -> Option<&'static str> {
        Some(match n {
            0 => "Bank Select",
            1 => "Mod Wheel",
            2 => "Breath",
            4 => "Foot",
            5 => "Portamento Time",
            7 => "Volume",
            8 => "Balance",
            10 => "Pan",
            11 => "Expression",
            64 => "Sustain",
            65 => "Portamento",
            66 => "Sostenuto",
            67 => "Soft Pedal",
            68 => "Legato",
            71 => "Resonance",
            72 => "Release",
            73 => "Attack",
            74 => "Brightness",
            91 => "Reverb",
            93 => "Chorus",
            _ => return None,
        })
    }

    /// The value range of the lane: `(0, 1)` or `(-1, 1)`.
    pub fn range(self) -> (f64, f64) {
        match self {
            Controller::Cc(_) => (0.0, 1.0),
            Controller::PitchBend => (-1.0, 1.0),
        }
    }

    /// The integer a MIDI message would carry: 0..=127, or -8192..=8191 for the bend.
    pub fn quantize(self, v: f64) -> i32 {
        match self {
            Controller::Cc(_) => (v.clamp(0.0, 1.0) * 127.0).round() as i32,
            Controller::PitchBend => {
                ((v.clamp(-1.0, 1.0) * 8192.0).round() as i32).clamp(-8192, 8191)
            }
        }
    }

    /// The normalised value of a quantised integer (the inverse of [`quantize`](Self::quantize)).
    pub fn dequantize(self, q: i32) -> f32 {
        match self {
            Controller::Cc(_) => q.clamp(0, 127) as f32 / 127.0,
            Controller::PitchBend => q.clamp(-8192, 8191) as f32 / 8192.0,
        }
    }

    /// `v` rounded to what a MIDI message can carry; this is what the engine sends.
    pub fn wire(self, v: f64) -> f32 {
        self.dequantize(self.quantize(v))
    }

    /// Parses a number (`1`, `"1"`, `"cc1"`, `"CC 1"`) or a name (`"mod wheel"`, `"expression"`,
    /// `"pitch bend"`, `"pitchbend"`, `"pitch_bend"`), case-insensitively.
    pub fn parse(s: &str) -> ModelResult<Controller> {
        let t: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| !matches!(c, ' ' | '_' | '-'))
            .collect();
        if matches!(t.as_str(), "pitchbend" | "pitch" | "bend" | "pb") {
            return Ok(Controller::PitchBend);
        }
        let digits = t.strip_prefix("cc").unwrap_or(&t);
        if let Ok(n) = digits.parse::<u8>() {
            return Controller::cc(n);
        }
        // "cc1modwheel": a number followed by its name (the label form)
        let lead: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !lead.is_empty()
            && t.starts_with("cc")
            && let Ok(n) = lead.parse::<u8>()
            && Self::known(n).is_some_and(|k| digits[lead.len()..] == normalise(k))
        {
            return Controller::cc(n);
        }
        for n in 0..=MAX_CC {
            if Self::known(n).is_some_and(|k| normalise(k) == t) {
                return Ok(Controller::Cc(n));
            }
        }
        match t.as_str() {
            "mod" | "modulation" => Ok(Controller::Cc(1)),
            "vol" => Ok(Controller::Cc(7)),
            "expr" => Ok(Controller::Cc(11)),
            "sustainpedal" | "damper" => Ok(Controller::Cc(64)),
            _ => Err(ModelError::Invalid(format!(
                "unknown controller {s:?} (a number 0..={MAX_CC}, a name such as \"mod wheel\", or \"pitch bend\")"
            ))),
        }
    }
}

fn normalise(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .filter(|c| *c != ' ')
        .collect()
}

impl fmt::Display for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl FromStr for Controller {
    type Err = ModelError;
    fn from_str(s: &str) -> ModelResult<Self> {
        Controller::parse(s)
    }
}

/// Saved and sent as the CC number, or `"pitch_bend"`; read from a number, a numeric string
/// or any name [`Controller::parse`] knows.
impl Serialize for Controller {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Controller::Cc(n) => s.serialize_u8(*n),
            Controller::PitchBend => s.serialize_str("pitch_bend"),
        }
    }
}

impl<'de> Deserialize<'de> for Controller {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(u64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(n) => u8::try_from(n)
                .map_err(|_| ())
                .and_then(|n| Controller::cc(n).map_err(|_| ()))
                .map_err(|_| {
                    serde::de::Error::custom(format!("controller {n} outside 0..={MAX_CC}"))
                }),
            Raw::Text(s) => Controller::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

impl JsonSchema for Controller {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Controller".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A controller: its MIDI CC number 0..=119 (1 = Mod Wheel, 7 = Volume, 11 = Expression, 64 = Sustain), a name (\"mod wheel\", \"expression\", \"cc74\"), or \"pitch bend\".",
            "anyOf": [
                { "type": "integer", "minimum": 0, "maximum": 119 },
                { "type": "string" }
            ]
        })
    }
}

/// One point of a controller lane, in a pattern. Semantics as [`AutomationPoint`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CcPoint {
    pub id: PointId,
    pub controller: Controller,
    /// Pattern-relative position in ticks.
    pub tick: Tick,
    /// Normalised: CC 0..=1, pitch bend -1..=1.
    pub value: f64,
    /// Shape of the segment from this point to the next one of the same lane.
    #[serde(default)]
    pub curve: CurveKind,
    /// -1..=1; bends `Linear` and `SCurve` segments.
    #[serde(default)]
    pub tension: f32,
}

impl CcPoint {
    /// The point as an automation point, so [`value_at`] evaluates lanes too.
    pub fn as_auto(&self) -> AutomationPoint {
        AutomationPoint {
            id: self.id,
            tick: self.tick,
            value: self.value,
            curve: self.curve,
            tension: self.tension,
        }
    }
}

/// The editable content of a new lane point (everything but the id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CcPointSpec {
    pub controller: Controller,
    pub tick: Tick,
    pub value: f64,
    #[serde(default)]
    pub curve: CurveKind,
    #[serde(default)]
    pub tension: f32,
}

/// One lane as the pattern holds it: its points in evaluation order `(tick, id)`.
#[derive(Clone, Debug, PartialEq)]
pub struct CcLane {
    pub controller: Controller,
    pub points: Vec<CcPoint>,
}

impl CcLane {
    /// The points as automation points (for [`value_at`]).
    pub fn auto_points(&self) -> Vec<AutomationPoint> {
        self.points.iter().map(CcPoint::as_auto).collect()
    }

    /// The value at pattern tick `tick` (fractional); `None` for an empty lane.
    pub fn value_at(&self, tick: f64) -> Option<f64> {
        value_at(&self.auto_points(), tick)
    }
}

pub(crate) fn validate_cc_point(p: &CcPoint) -> ModelResult<()> {
    if let Controller::Cc(n) = p.controller
        && n > MAX_CC
    {
        return Err(ModelError::Invalid(format!(
            "controller {n} outside 0..={MAX_CC}"
        )));
    }
    let (lo, hi) = p.controller.range();
    if !p.value.is_finite() || p.value < lo || p.value > hi {
        return Err(ModelError::Invalid(format!(
            "{} value {} outside {lo}..{hi}",
            p.controller.name(),
            p.value
        )));
    }
    if !p.tension.is_finite() || !(-1.0..=1.0).contains(&p.tension) {
        return Err(ModelError::Invalid(format!(
            "tension {} outside -1..1",
            p.tension
        )));
    }
    Ok(())
}
