use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Stable id of a channel-rack channel. Never reused within a project.
    ChannelId
);
id_type!(
    /// Stable id of a pattern. Never reused within a project.
    PatternId
);
id_type!(
    /// Stable id of a playlist clip. Never reused within a project.
    ClipId
);
id_type!(
    /// Mixer insert index. 0 is the master; inserts are positional like FL's mixer.
    InsertId
);
id_type!(
    /// Stable id of a note inside one pattern. Lets agents edit or delete a specific note.
    NoteId
);
id_type!(
    /// Stable id of an entry in the project's sample registry ([`crate::SampleEntry`]).
    /// Never reused within a project.
    SampleId
);
id_type!(
    /// Stable id of an automation point ([`crate::AutomationPoint`]). Unique across the
    /// whole project (not just its clip) and never reused.
    PointId
);
