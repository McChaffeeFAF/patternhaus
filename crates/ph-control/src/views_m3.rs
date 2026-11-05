//! Readable views of the M3 model for agents: `get_state { view: "zones" }`, the Haus Sampler
//! zone maps. M3/T0 fixes the shape; M3/T7 completes it (note names, overlaps and gaps,
//! round-robin and layer summaries) without renaming any key below.

use ph_core::{HausInstrument, InstrumentId, Project, SampleRef, ZoneTrigger};
use serde_json::{Value, json};

/// `{ instruments: [...] }`, or one instrument with `args.instrument`.
///
/// Each instrument: `instrument` (id), `name`, `channels` (ids playing it), `folder`, `source`,
/// `settings`, `articulations` ([{id, name, trigger}]), `default_articulation`,
/// `modulation_routes` (count), `import_notes` (count), and `groups`, each with its `id`,
/// `name`, `output`, `polyphony`, `round_robin`, `articulations` and its `zones`, compact:
/// `{ id, keys: [lo, hi], velocity: [lo, hi], root, sample, rr?, release? }` (`sample` is a
/// registry id or a path relative to the folder; `rr` the round-robin position; `release`
/// true for release-trigger zones).
pub fn zones_view(p: &Project, args: &Value) -> Result<Value, String> {
    let only = match args.get("instrument") {
        None | Some(Value::Null) => None,
        Some(v) => Some(InstrumentId(
            v.as_u64()
                .ok_or("args.instrument is an instrument id (a number)")? as u32,
        )),
    };
    if let Some(i) = only {
        p.instrument(i).map_err(|e| e.to_string())?;
    }
    let list: Vec<Value> = p
        .instruments
        .values()
        .filter(|i| only.is_none_or(|o| o == i.id))
        .map(|i| instrument(p, i))
        .collect();
    Ok(json!({ "instruments": list }))
}

fn instrument(p: &Project, i: &HausInstrument) -> Value {
    let groups: Vec<Value> = i
        .groups
        .values()
        .map(|g| {
            let zones: Vec<Value> = i
                .zones_of(g.id)
                .map(|z| {
                    let mut v = json!({
                        "id": z.id,
                        "keys": z.keys,
                        "velocity": z.velocity,
                        "root": z.root_key,
                        "sample": match &z.sample {
                            SampleRef::Registry(s) => json!(s),
                            SampleRef::File(f) => json!(f),
                        },
                    });
                    if let Some(rr) = z.round_robin {
                        v["rr"] = json!(rr);
                    }
                    if z.trigger == ZoneTrigger::Release {
                        v["release"] = json!(true);
                    }
                    v
                })
                .collect();
            json!({
                "id": g.id,
                "name": g.name,
                "output": g.output,
                "polyphony": g.polyphony,
                "round_robin": g.round_robin,
                "articulations": g.articulations,
                "zones": zones,
            })
        })
        .collect();
    let articulations: Vec<Value> = i
        .articulations
        .iter()
        .map(|a| json!({ "id": a.id, "name": a.name, "trigger": a.trigger }))
        .collect();
    json!({
        "instrument": i.id,
        "name": i.name,
        "channels": p.channels_playing(i.id).map(|c| c.id).collect::<Vec<_>>(),
        "folder": i.folder,
        "source": i.source,
        "settings": i.settings,
        "articulations": articulations,
        "default_articulation": i.settings.default_articulation,
        "modulation_routes": i.modulation.len(),
        "import_notes": i.import_notes.len(),
        "groups": groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::*;

    #[test]
    fn zones_view_has_the_t0_shape() {
        let mut e = Edit::new(&Project::empty("z"));
        let i = e.add_instrument(HausInstrument::new("Piano")).unwrap();
        let g = e.add_group(i, Group::new("Main")).unwrap();
        let mut z = Zone::new(
            g,
            SampleRef::File("samples/C4.wav".into()),
            KeyRange { lo: 55, hi: 66 },
            60,
        );
        z.round_robin = Some(1);
        e.add_zone(i, z).unwrap();
        let mut r = Zone::new(
            g,
            SampleRef::File("samples/rel.wav".into()),
            KeyRange::ALL,
            60,
        );
        r.trigger = ZoneTrigger::Release;
        e.add_zone(i, r).unwrap();
        let p = e.finish().0;
        let v = zones_view(&p, &json!({})).unwrap();
        let inst = &v["instruments"][0];
        assert_eq!(inst["instrument"], 0);
        assert_eq!(inst["name"], "Piano");
        let zones = &inst["groups"][0]["zones"];
        assert_eq!(
            zones[0],
            json!({ "id": 0, "keys": [55, 66], "velocity": [0, 127], "root": 60,
                    "sample": "samples/C4.wav", "rr": 1 })
        );
        assert_eq!(zones[1]["release"], true);
        assert_eq!(zones_view(&p, &json!({ "instrument": 0 })).unwrap(), v);
        assert!(zones_view(&p, &json!({ "instrument": 4 })).is_err());
        assert!(zones_view(&p, &json!({ "instrument": "x" })).is_err());
    }
}
