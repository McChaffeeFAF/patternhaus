//! Haus Sampler gestures -> `sampler.*` requests (M3/T5).
//!
//! Like [`crate::mapping`], these are plain functions of ids and values. The sampler window,
//! the drop handling and the browser build every edit here, so the UI changes an instrument
//! only through the same commands an agent uses (one undo step each, co-editing safe).

use ph_commands::Request;
use ph_core::{
    ArticulationId, ArticulationTrigger, ChannelId, GroupId, InstrumentId, KeyRange, ModRoute,
    VelRange, ZoneId,
};
use serde_json::{Map, Value, json};

fn req(command: &str, params: Value) -> Request {
    Request::new(command, params)
}

fn obj(fields: &[(&str, Value)]) -> Map<String, Value> {
    fields
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// Rename an instrument or change its global settings (`volume_db`, `pan`, ...).
pub fn set(inst: InstrumentId, name: Option<&str>, settings: &[(&str, Value)]) -> Request {
    let mut p = json!({ "instrument": inst, "set": obj(settings) });
    if let Some(n) = name {
        p["name"] = n.into();
    }
    req("sampler.set", p)
}

pub fn group_add(inst: InstrumentId, name: &str) -> Request {
    req(
        "sampler.group_add",
        json!({ "instrument": inst, "name": name }),
    )
}

/// Change group fields; a `null` clears an optional one (`filter`, `voice_limit`, ...).
pub fn group_set(inst: InstrumentId, group: GroupId, fields: &[(&str, Value)]) -> Request {
    req(
        "sampler.group_set",
        json!({ "instrument": inst, "group": group, "set": obj(fields) }),
    )
}

pub fn group_remove(inst: InstrumentId, group: GroupId) -> Request {
    req(
        "sampler.group_remove",
        json!({ "instrument": inst, "group": group }),
    )
}

/// Where a zone move puts its zones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    pub shift: Option<i32>,
    pub keys: Option<KeyRange>,
    pub velocity: Option<VelRange>,
    pub group: Option<GroupId>,
    /// The root key moves with the keys (a move); false keeps it (an edge resize).
    pub root_follows: bool,
}

pub fn zone_move(inst: InstrumentId, zones: &[ZoneId], place: &Place) -> Request {
    let mut p = json!({ "instrument": inst, "zones": zones, "root_follows": place.root_follows });
    if let Some(s) = place.shift {
        p["shift"] = s.into();
    }
    if let Some(k) = place.keys {
        p["keys"] = json!([k.lo, k.hi]);
    }
    if let Some(v) = place.velocity {
        p["velocity"] = json!([v.lo, v.hi]);
    }
    if let Some(g) = place.group {
        p["group"] = json!(g);
    }
    req("sampler.zone_move", p)
}

/// The same fields on every zone given; a `null` clears an optional field.
pub fn zone_set(inst: InstrumentId, zones: &[ZoneId], fields: &[(&str, Value)]) -> Request {
    req(
        "sampler.zone_set",
        json!({ "instrument": inst, "zones": zones, "set": obj(fields) }),
    )
}

pub fn zone_remove(inst: InstrumentId, zones: &[ZoneId]) -> Request {
    req(
        "sampler.zone_remove",
        json!({ "instrument": inst, "zones": zones }),
    )
}

/// What an articulation edit changes. `None` fields are left as they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArticulationEdit {
    /// `None` adds a new articulation.
    pub id: Option<ArticulationId>,
    pub name: Option<String>,
    pub trigger: Option<ArticulationTrigger>,
    pub color: Option<u32>,
    pub groups: Option<Vec<GroupId>>,
    pub default: bool,
    pub remove: bool,
}

pub fn keyswitch_set(inst: InstrumentId, e: &ArticulationEdit) -> Request {
    let mut p = json!({ "instrument": inst });
    if let Some(a) = e.id {
        p["articulation"] = json!(a);
    }
    if let Some(n) = &e.name {
        p["name"] = n.as_str().into();
    }
    if let Some(t) = &e.trigger {
        p["trigger"] = serde_json::to_value(t).unwrap_or(Value::Null);
    }
    if let Some(c) = e.color {
        p["color"] = c.into();
    }
    if let Some(g) = &e.groups {
        p["groups"] = json!(g);
    }
    if e.default {
        p["default"] = true.into();
    }
    if e.remove {
        p["remove"] = true.into();
    }
    req("sampler.keyswitch_set", p)
}

/// Replaces the whole modulation matrix.
pub fn modulation_set(inst: InstrumentId, routes: &[ModRoute]) -> Request {
    req(
        "sampler.modulation_set",
        json!({ "instrument": inst, "routes": routes }),
    )
}

pub fn report(inst: InstrumentId) -> Request {
    req("sampler.report", json!({ "instrument": inst }))
}

/// Import an instrument file or a folder of samples onto a new channel (or `channel`).
pub fn import(path: &str, preset: Option<&str>, channel: Option<ChannelId>) -> Request {
    let mut p = json!({ "path": path });
    if let Some(pr) = preset {
        p["preset"] = pr.into();
    }
    if let Some(c) = channel {
        p["channel"] = json!(c);
    }
    req("sampler.import", p)
}

/// Load a native instrument folder onto a new channel (or `channel`).
pub fn load(path: &str, channel: Option<ChannelId>) -> Request {
    let mut p = json!({ "path": path });
    if let Some(c) = channel {
        p["channel"] = json!(c);
    }
    req("sampler.load", p)
}

pub fn save_native(inst: InstrumentId, path: &str) -> Request {
    req(
        "sampler.save_native",
        json!({ "instrument": inst, "path": path }),
    )
}

pub fn collect(inst: InstrumentId) -> Request {
    req("sampler.collect", json!({ "instrument": inst }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, Registry, Session};
    use ph_core::{ModCurve, ModSource, ModTarget, Project};
    use std::sync::Arc;

    /// Every sampler request the UI builds is accepted by the real registry.
    #[test]
    fn sampler_requests_are_accepted_by_the_registry() {
        let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("t"));
        let mut host = ph_commands::NullHost::default();
        let who = Caller::human();
        let mut run = |r: Request| {
            s.execute(&who, r.clone(), &mut host)
                .unwrap_or_else(|e| panic!("{}: {e}", r.command))
        };
        let made = run(Request::new("sampler.new", json!({ "name": "Keys" })));
        let inst = InstrumentId(made.result["instrument"].as_u64().unwrap() as u32);
        let g0 = GroupId(made.result["group"].as_u64().unwrap() as u32);
        let g1 = GroupId(
            run(group_add(inst, "Soft")).result["group"]
                .as_u64()
                .unwrap() as u32,
        );
        let z = run(Request::new(
            "sampler.zone_add",
            json!({ "instrument": inst, "zones": [
                { "group": g0, "sample": "samples/c4.wav", "root_key": 60, "keys": [55, 65] },
            ]}),
        ));
        let zone = ZoneId(z.result["zones"][0].as_u64().unwrap() as u32);
        run(set(inst, Some("Grand"), &[("volume_db", json!(-3.0))]));
        run(group_set(
            inst,
            g1,
            &[("voice_limit", json!(8)), ("round_robin", json!("random"))],
        ));
        run(zone_move(
            inst,
            &[zone],
            &Place {
                shift: Some(2),
                velocity: Some(VelRange { lo: 10, hi: 100 }),
                root_follows: true,
                ..Default::default()
            },
        ));
        run(zone_move(
            inst,
            &[zone],
            &Place {
                keys: Some(KeyRange { lo: 50, hi: 70 }),
                group: Some(g1),
                ..Default::default()
            },
        ));
        run(zone_set(
            inst,
            &[zone],
            &[("tune", json!(1)), ("end", Value::Null)],
        ));
        let a = run(keyswitch_set(
            inst,
            &ArticulationEdit {
                name: Some("Staccato".into()),
                trigger: Some(ArticulationTrigger::Key { key: 24 }),
                groups: Some(vec![g1]),
                default: true,
                ..Default::default()
            },
        ));
        let art = ArticulationId(a.result["articulation"].as_u64().unwrap() as u32);
        run(keyswitch_set(
            inst,
            &ArticulationEdit {
                id: Some(art),
                color: Some(0xff8800),
                ..Default::default()
            },
        ));
        run(modulation_set(
            inst,
            &[ModRoute {
                source: ModSource::Cc { controller: 1 },
                target: ModTarget::Volume,
                depth: -6.0,
                curve: ModCurve::Concave,
                group: Some(g1),
            }],
        ));
        run(report(inst));
        run(keyswitch_set(
            inst,
            &ArticulationEdit {
                id: Some(art),
                remove: true,
                ..Default::default()
            },
        ));
        run(zone_remove(inst, &[zone]));
        run(group_remove(inst, g0));
        // the file commands are checked against a real host by the UI tests; here only that
        // the registry knows them with these parameter shapes
        for r in [
            import("lib/x.sfz", Some("0:1"), Some(ChannelId(0))),
            load("lib/inst", None),
            save_native(inst, "saved/x"),
            collect(inst),
        ] {
            if let Err(e) = s.execute(&who, r.clone(), &mut host) {
                assert!(
                    !matches!(
                        e,
                        ph_commands::CmdError::BadParams { .. }
                            | ph_commands::CmdError::UnknownCommand(_)
                    ),
                    "{}: {e}",
                    r.command
                );
            }
        }
    }
}
