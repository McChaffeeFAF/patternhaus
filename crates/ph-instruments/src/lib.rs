//! Patternhaus built-in instruments (DESIGN §7).
//!
//! | id | instrument |
//! |---|---|
//! | `ph.sampler` | one-shot / looped sample player ([`sampler`]) |
//! | `ph.synth` | Haus Synth, subtractive synthesizer ([`synth`]) |
//!
//! Both are real-time-safe [`ph_node::Processor`]s: voices are allocated in `activate`,
//! notes and parameter changes apply sample-accurately, and output is bit-identical for
//! any block size.

#![allow(clippy::needless_range_loop)]

mod common;
mod preset_data;
pub mod presets;
pub mod sampler;
pub mod synth;

use ph_core::InstrumentSpec;
use ph_node::{AssetResolver, Processor};

/// Build an instrument from a project spec, resolving its sample (off the audio thread)
/// and applying `params` by stable id.
pub fn create(
    spec: &InstrumentSpec,
    assets: &dyn AssetResolver,
) -> Result<Box<dyn Processor>, String> {
    let mut preset_values: Vec<(String, f64)> = Vec::new();
    let (mut p, params): (Box<dyn Processor>, _) = match spec {
        InstrumentSpec::Sampler {
            sample,
            root_key,
            params,
        } => {
            let data = match sample {
                Some(r) => Some(
                    assets
                        .sample(r)
                        .map_err(|e| format!("sampler: cannot load `{r}`: {e}"))?,
                ),
                None => None,
            };
            let mut s = sampler::Sampler::new(data, sample.clone());
            let id = s.descriptor().param_id("root_key").expect("root_key");
            s.set_param(id, *root_key as f64);
            (Box::new(s), params)
        }
        // A preset loads first; the spec's own `params` then override it.
        InstrumentSpec::Synth { preset, params } => {
            if let Some(name) = preset {
                preset_values = presets::synth_preset(name)?;
            }
            (Box::new(synth::Synth::new()), params)
        }
        InstrumentSpec::MultiSampler { .. } => {
            return Err("Haus Sampler (multi-sampler) is not available until M3".into());
        }
        InstrumentSpec::Plugin { .. } => {
            return Err("external plugin instruments are not available until M2".into());
        }
    };
    let overrides = params.iter().map(|(k, v)| (k.as_str(), *v));
    let preset_iter = preset_values.iter().map(|(k, v)| (k.as_str(), *v));
    for (key, value) in preset_iter.chain(overrides) {
        let id = p
            .descriptor()
            .param_id(key)
            .ok_or_else(|| format!("{} has no parameter `{key}`", p.descriptor().id))?;
        p.set_param(id, value);
    }
    Ok(p)
}
