//! Patternhaus built-in effects (DESIGN §7, M1 essentials and M2 additions).
//!
//! | id | effect |
//! |---|---|
//! | `ph.fx.eq` | 6-band parametric EQ ([`eq`]) |
//! | `ph.fx.compressor` | compressor with sidechain input and lookahead ([`compressor`]) |
//! | `ph.fx.limiter` | true-peak brickwall limiter ([`limiter`]) |
//! | `ph.fx.reverb` | Dattorro plate reverb ([`reverb`]) |
//! | `ph.fx.delay` | stereo / ping-pong delay ([`delay`]) |
//! | `ph.fx.filter` | SVF / ladder filter with LFO ([`filter`]) |
//! | `ph.fx.saturator` | oversampled saturation ([`saturator`]) |
//! | `ph.fx.multiband` | 3-band Linkwitz-Riley compressor ([`multiband`]) |
//! | `ph.fx.gate` | gate / expander with sidechain ([`gate`]) |
//! | `ph.fx.convolution` | zero-latency partitioned-FFT convolution reverb ([`convolution`]) |
//! | `ph.fx.chorus` | multi-voice chorus ([`chorus`]) |
//! | `ph.fx.flanger` | flanger ([`flanger`]) |
//! | `ph.fx.phaser` | all-pass phaser ([`phaser`]) |
//! | `ph.fx.bitcrush` | bit-depth and sample-rate reduction ([`bitcrush`]) |
//! | `ph.fx.stereo` | width, balance, mono-below ([`stereo`]) |
//! | `ph.fx.analyzer` | pass-through spectrum, loudness, correlation ([`analyzer`]) |
//!
//! Every effect is a real-time-safe [`ph_node::Processor`]: allocation happens in
//! `activate`, events are applied sample-accurately, and output is independent of block
//! size.

#![allow(clippy::needless_range_loop)]

pub mod analyzer;
pub mod bitcrush;
pub mod chorus;
mod common;
pub mod compressor;
pub mod convolution;
pub mod delay;
pub mod eq;
pub mod filter;
pub mod flanger;
pub mod gate;
pub mod limiter;
pub mod multiband;
pub mod phaser;
pub mod reverb;
pub mod saturator;
pub mod stereo;

use ph_core::EffectSpec;
use ph_node::Processor;

/// Short names of the built-in effects (`EffectSpec::Builtin::name`); each also answers to
/// its full id `ph.fx.<name>`.
pub const BUILTIN: [&str; 16] = [
    "eq",
    "compressor",
    "limiter",
    "reverb",
    "delay",
    "filter",
    "saturator",
    "multiband",
    "gate",
    "convolution",
    "chorus",
    "flanger",
    "phaser",
    "bitcrush",
    "stereo",
    "analyzer",
];

/// A fresh, un-activated built-in effect by short name or full id.
pub fn builtin(name: &str) -> Option<Box<dyn Processor>> {
    Some(match common::short_name(name) {
        "eq" => Box::new(eq::Eq::new()),
        "compressor" => Box::new(compressor::Compressor::new()),
        "limiter" => Box::new(limiter::Limiter::new()),
        "reverb" => Box::new(reverb::Reverb::new()),
        "delay" => Box::new(delay::Delay::new()),
        "filter" => Box::new(filter::Filter::new()),
        "saturator" => Box::new(saturator::Saturator::new()),
        "multiband" => Box::new(multiband::Multiband::new()),
        "gate" => Box::new(gate::Gate::new()),
        "convolution" => Box::new(convolution::Convolution::new()),
        "chorus" => Box::new(chorus::Chorus::new()),
        "flanger" => Box::new(flanger::Flanger::new()),
        "phaser" => Box::new(phaser::Phaser::new()),
        "bitcrush" => Box::new(bitcrush::Bitcrush::new()),
        "stereo" => Box::new(stereo::Stereo::new()),
        "analyzer" => Box::new(analyzer::Analyzer::new()),
        _ => return None,
    })
}

/// Build an effect from a project spec, applying `params` by stable id.
pub fn create(spec: &EffectSpec) -> Result<Box<dyn Processor>, String> {
    match spec {
        EffectSpec::Builtin { name, params } => {
            let mut p = builtin(name).ok_or_else(|| {
                format!(
                    "unknown built-in effect `{name}` (available: {})",
                    BUILTIN.join(", ")
                )
            })?;
            for (key, value) in params {
                let id = p
                    .descriptor()
                    .param_id(key)
                    .ok_or_else(|| format!("effect `{name}` has no parameter `{key}`"))?;
                p.set_param(id, *value);
            }
            Ok(p)
        }
        EffectSpec::Plugin { .. } => {
            Err("external plugin effects are not available until M2".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn factory_builds_every_effect_and_applies_params() {
        for name in BUILTIN {
            let p = builtin(name).unwrap();
            assert_eq!(p.descriptor().id, format!("ph.fx.{name}"));
            assert!(builtin(&format!("ph.fx.{name}")).is_some());
        }
        let spec = EffectSpec::Builtin {
            name: "reverb".into(),
            params: BTreeMap::from([("decay".to_string(), 99.0)]),
        };
        let p = create(&spec).unwrap();
        let id = p.descriptor().param_id("decay").unwrap();
        assert_eq!(p.param(id), 20.0, "clamped to max");
        let bad = EffectSpec::Builtin {
            name: "reverb".into(),
            params: BTreeMap::from([("nope".to_string(), 1.0)]),
        };
        assert!(create(&bad).err().unwrap().contains("nope"));
        assert!(
            create(&EffectSpec::Builtin {
                name: "nonesuch".into(),
                params: BTreeMap::new()
            })
            .is_err()
        );
    }
}
