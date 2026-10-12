use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Address of one editable object in a [`Project`]. Patches, undo conflicts, agent queries
/// and change events all speak in object paths.
///
/// String form (what agents see): `settings`, `channel/3`, `channel_order`, `pattern/2`,
/// `pattern_order`, `note/<pattern>/<channel>/<note>`, `insert/4`, `track/0`, `clip/7`,
/// `sample/2` and `point/<clip>/<point>` (format 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectPath {
    /// Value: [`ProjectSettings`]. Always present.
    Settings,
    /// Value: [`Channel`].
    Channel(ChannelId),
    /// Value: `Vec<ChannelId>`. Always present.
    ChannelOrder,
    /// Value: [`PatternHeader`] (notes are separate objects).
    Pattern(PatternId),
    /// Value: `Vec<PatternId>`. Always present.
    PatternOrder,
    /// Value: [`Note`].
    Note(PatternId, ChannelId, NoteId),
    /// Value: [`MixerInsert`]. Always present (positional).
    Insert(InsertId),
    /// Value: [`PlaylistTrack`]. Always present (positional).
    Track(u32),
    /// Value: [`Clip`], of any kind. An automation clip's value never carries its points.
    Clip(ClipId),
    /// Value: [`SampleEntry`] (the sample registry).
    Sample(SampleId),
    /// Value: [`AutomationPoint`] of an automation clip.
    Point(ClipId, PointId),
}

impl ObjectPath {
    /// Objects that always exist; a patch may change them but never create or delete them.
    pub fn is_fixed(&self) -> bool {
        matches!(
            self,
            ObjectPath::Settings
                | ObjectPath::ChannelOrder
                | ObjectPath::PatternOrder
                | ObjectPath::Insert(_)
                | ObjectPath::Track(_)
        )
    }

    /// Order lists merge by membership instead of by value during undo (see `ph-commands`).
    pub fn is_order_list(&self) -> bool {
        matches!(self, ObjectPath::ChannelOrder | ObjectPath::PatternOrder)
    }
}

impl fmt::Display for ObjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObjectPath::Settings => write!(f, "settings"),
            ObjectPath::Channel(c) => write!(f, "channel/{c}"),
            ObjectPath::ChannelOrder => write!(f, "channel_order"),
            ObjectPath::Pattern(p) => write!(f, "pattern/{p}"),
            ObjectPath::PatternOrder => write!(f, "pattern_order"),
            ObjectPath::Note(p, c, n) => write!(f, "note/{p}/{c}/{n}"),
            ObjectPath::Insert(i) => write!(f, "insert/{i}"),
            ObjectPath::Track(t) => write!(f, "track/{t}"),
            ObjectPath::Clip(c) => write!(f, "clip/{c}"),
            ObjectPath::Sample(s) => write!(f, "sample/{s}"),
            ObjectPath::Point(c, p) => write!(f, "point/{c}/{p}"),
        }
    }
}

impl FromStr for ObjectPath {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ModelError::Invalid(format!("bad object path {s:?}"));
        let parts: Vec<&str> = s.split('/').collect();
        let num = |i: usize| -> Result<u32, ModelError> {
            parts.get(i).and_then(|p| p.parse().ok()).ok_or_else(bad)
        };
        let path = match (parts[0], parts.len()) {
            ("settings", 1) => ObjectPath::Settings,
            ("channel", 2) => ObjectPath::Channel(ChannelId(num(1)?)),
            ("channel_order", 1) => ObjectPath::ChannelOrder,
            ("pattern", 2) => ObjectPath::Pattern(PatternId(num(1)?)),
            ("pattern_order", 1) => ObjectPath::PatternOrder,
            ("note", 4) => {
                ObjectPath::Note(PatternId(num(1)?), ChannelId(num(2)?), NoteId(num(3)?))
            }
            ("insert", 2) => ObjectPath::Insert(InsertId(num(1)?)),
            ("track", 2) => ObjectPath::Track(num(1)?),
            ("clip", 2) => ObjectPath::Clip(ClipId(num(1)?)),
            ("sample", 2) => ObjectPath::Sample(SampleId(num(1)?)),
            ("point", 3) => ObjectPath::Point(ClipId(num(1)?), PointId(num(2)?)),
            _ => return Err(bad()),
        };
        Ok(path)
    }
}

impl Serialize for ObjectPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ObjectPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ObjectPath {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ObjectPath".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Object path: settings | channel/<id> | channel_order | pattern/<id> | pattern_order | note/<pattern>/<channel>/<note> | insert/<n> | track/<n> | clip/<id> | sample/<id> | point/<clip>/<point>",
            "pattern": "^(settings|channel_order|pattern_order|(channel|pattern|insert|track|clip|sample)/\\d+|point/\\d+/\\d+|note/\\d+/\\d+/\\d+)$"
        })
    }
}
