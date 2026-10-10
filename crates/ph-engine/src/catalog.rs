//! What instruments and effects exist and which parameters they have.
//!
//! Front ends (the UI's knobs, agents' `query.node_params`) need parameter descriptors but
//! may not depend on `ph-instruments` / `ph-effects` directly (DESIGN §2 layering); the
//! engine re-exports them here. Off the audio thread only (it instantiates processors).

use ph_core::{EffectSpec, InstrumentSpec};
use ph_node::{AssetResolver, NodeDescriptor, SampleData};
use std::sync::Arc;

/// Built-in effect names accepted by `EffectSpec::Builtin { name }`.
pub fn builtin_effects() -> &'static [&'static str] {
    &ph_effects::BUILTIN
}

/// Factory Haus Synth preset names (`category/name`).
pub fn synth_presets() -> Vec<&'static str> {
    ph_instruments::presets::synth_preset_names()
}

/// Parameter descriptor for an instrument kind. Sample, library and preset references are
/// ignored: only the kind matters.
pub fn instrument_descriptor(spec: &InstrumentSpec) -> Result<NodeDescriptor, String> {
    let bare = match spec {
        InstrumentSpec::Sampler { root_key, .. } => InstrumentSpec::Sampler {
            sample: None,
            root_key: *root_key,
            params: Default::default(),
        },
        InstrumentSpec::Synth { .. } => InstrumentSpec::Synth {
            preset: None,
            params: Default::default(),
        },
        other => other.clone(),
    };
    ph_instruments::create(&bare, &NoAssets).map(|p| p.descriptor().clone())
}

/// Parameter descriptor for an effect.
pub fn effect_descriptor(spec: &EffectSpec) -> Result<NodeDescriptor, String> {
    let bare = match spec {
        EffectSpec::Builtin { name, .. } => EffectSpec::Builtin {
            name: name.clone(),
            params: Default::default(),
        },
        other => other.clone(),
    };
    ph_effects::create(&bare).map(|p| p.descriptor().clone())
}

struct NoAssets;

impl AssetResolver for NoAssets {
    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        Err(format!("catalog lookups do not load samples ({reference})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_has_a_descriptor_with_params() {
        for name in builtin_effects() {
            let d = effect_descriptor(&EffectSpec::Builtin {
                name: name.to_string(),
                params: Default::default(),
            })
            .unwrap();
            assert!(!d.params.is_empty(), "{name}");
        }
        let synth = instrument_descriptor(&InstrumentSpec::Synth {
            preset: Some("bass/sub".into()),
            params: Default::default(),
        })
        .unwrap();
        assert!(synth.params.len() > 50);
        let sampler = instrument_descriptor(&InstrumentSpec::Sampler {
            sample: Some("factory:drums/haus808/kick".into()),
            root_key: 60,
            params: Default::default(),
        })
        .unwrap();
        assert!(sampler.param_id("root_key").is_some() || !sampler.params.is_empty());
        assert!(synth_presets().contains(&"bass/sub"));
    }
}
