//! Factory Haus Synth presets (DESIGN section 7).
//!
//! A preset is a JSON object mapping stable parameter ids to values. The files live in
//! `assets/presets/synth/<category>/<name>.json` and are compiled in, so creating an
//! instrument never touches the file system. A spec's `preset` is the path relative to
//! that folder without extension (`bass/sub`).

use crate::preset_data::FACTORY_SYNTH_PRESETS;

/// Names of all factory Haus Synth presets, sorted.
pub fn synth_preset_names() -> Vec<&'static str> {
    FACTORY_SYNTH_PRESETS.iter().map(|(n, _)| *n).collect()
}

/// The parameter id/value pairs of a factory preset. An unknown name is an error that
/// lists the available presets.
pub fn synth_preset(name: &str) -> Result<Vec<(String, f64)>, String> {
    let Some((_, json)) = FACTORY_SYNTH_PRESETS.iter().find(|(n, _)| *n == name) else {
        return Err(format!(
            "unknown Haus Synth preset `{name}`; available: {}",
            synth_preset_names().join(", ")
        ));
    };
    let map: std::collections::BTreeMap<String, f64> =
        serde_json::from_str(json).map_err(|e| format!("preset `{name}` is malformed: {e}"))?;
    Ok(map.into_iter().collect())
}
