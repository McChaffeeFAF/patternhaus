//! `query.node_params`: which instruments and effects exist and what their parameters are,
//! as a normal registry command (wraps `ph_engine::catalog`).

use ph_commands::{CmdError, CmdResult, CommandCtx, CommandOutput, READ, Registry, Scope, typed};
use ph_core::{EffectSpec, InstrumentSpec};
use ph_engine::catalog;
use ph_node::{NodeDescriptor, ParamInfo, ParamUnit};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NodeParams {
    /// Instrument kind: `sampler`, `synth` or `multi_sampler`. Omit both `instrument` and
    /// `effect` to list everything that exists, including the factory synth presets.
    #[serde(default)]
    instrument: Option<String>,
    /// Built-in effect name, e.g. `eq`, `compressor`, `reverb`.
    #[serde(default)]
    effect: Option<String>,
    /// A scanned plugin's id (see plugin.list): lists its parameters by the plugin's own ids.
    #[serde(default)]
    plugin: Option<String>,
    /// `clap` or `vst3`, with `plugin` (needed only when both formats have that id).
    #[serde(default)]
    format: Option<ph_core::PluginFormat>,
}

pub fn register(r: &mut Registry) {
    r.register(typed(
        "query.node_params",
        "List instruments, effects and plugins, or the parameters (ids, ranges, defaults, units, choices) of one of them.",
        Scope::Read,
        READ,
        node_params,
    ));
}

fn param_json(p: &ParamInfo) -> Value {
    let unit = match p.unit {
        ParamUnit::None => "",
        ParamUnit::Decibels => "dB",
        ParamUnit::Hertz => "Hz",
        ParamUnit::Seconds => "s",
        ParamUnit::Milliseconds => "ms",
        ParamUnit::Percent => "%",
        ParamUnit::Semitones => "semitones",
        ParamUnit::Cents => "cents",
        ParamUnit::Ratio => "ratio",
        ParamUnit::Pan => "pan",
        ParamUnit::Choice => "choice",
        ParamUnit::Switch => "switch",
    };
    let mut v = json!({
        "id": p.id,
        "name": p.name,
        "min": p.min,
        "max": p.max,
        "default": p.default,
    });
    if !unit.is_empty() {
        v["unit"] = json!(unit);
    }
    if !p.choices.is_empty() {
        v["choices"] = json!(p.choices);
    }
    v
}

fn describe(d: &NodeDescriptor) -> Value {
    json!({
        "id": d.id,
        "name": d.name,
        "note_input": d.note_input,
        "sidechain_input": d.audio_inputs.iter().any(|p| p.sidechain),
        "params": d.params.iter().map(param_json).collect::<Vec<_>>(),
    })
}

fn node_params(ctx: &mut CommandCtx<'_>, p: NodeParams) -> CmdResult<CommandOutput> {
    let bad = |m: String| CmdError::BadParams {
        command: "query.node_params".into(),
        message: m,
    };
    if let Some(id) = p.plugin {
        if p.instrument.is_some() || p.effect.is_some() {
            return Err(bad("give one of `instrument`, `effect` or `plugin`".into()));
        }
        let entry = ctx
            .host
            .plugin_list()?
            .into_iter()
            .find(|e| e.id == id && p.format.is_none_or(|f| f == e.format))
            .ok_or_else(|| bad(format!("no scanned plugin {id:?} (see plugin.list)")))?;
        let d = ctx.host.plugin_describe(entry.format, &entry.id)?;
        let mut v = serde_json::to_value(&d).unwrap_or(Value::Null);
        v["kind"] = json!(entry.kind);
        v["usage"] = json!(
            "add with plugin.add_instrument / plugin.add_effect, then set with plugin.set_param { channel | insert+slot, param, value }; plugin.get_params shows live values"
        );
        return Ok(CommandOutput::value(v));
    }
    let result = match (p.instrument, p.effect) {
        (Some(_), Some(_)) => return Err(bad("give either `instrument` or `effect`".into())),
        (Some(kind), None) => {
            let spec = match kind.as_str() {
                "sampler" => InstrumentSpec::Sampler {
                    slices: Vec::new(),
                    sample: None,
                    root_key: 60,
                    params: Default::default(),
                },
                "synth" => InstrumentSpec::Synth {
                    preset: None,
                    params: Default::default(),
                },
                "multi_sampler" => InstrumentSpec::MultiSampler {
                    library: None,
                    params: Default::default(),
                },
                other => {
                    return Err(bad(format!(
                        "unknown instrument {other:?}: sampler, synth, multi_sampler"
                    )));
                }
            };
            let d = catalog::instrument_descriptor(&spec).map_err(bad)?;
            let mut v = describe(&d);
            v["kind"] = json!(kind);
            v["usage"] = json!(
                "set with channel.set_param { channel, param, value }, or in channel.add's instrument.params"
            );
            v
        }
        (None, Some(name)) => {
            let spec = EffectSpec::Builtin {
                name: name.clone(),
                params: Default::default(),
            };
            let d = catalog::effect_descriptor(&spec).map_err(|e| {
                bad(format!(
                    "{e} (effects: {})",
                    catalog::builtin_effects().join(", ")
                ))
            })?;
            let mut v = describe(&d);
            v["effect"] = json!(name);
            v["usage"] = json!("set with mixer.set_effect_param { insert, index, param, value }");
            v
        }
        (None, None) => json!({
            "instruments": ["sampler", "synth", "multi_sampler"],
            "effects": catalog::builtin_effects(),
            "synth_presets": catalog::synth_presets(),
            "plugins": ctx.host.plugin_list().unwrap_or_default().iter().map(|e| json!({
                "format": e.format, "id": e.id, "name": e.name, "kind": e.kind, "status": e.status,
            })).collect::<Vec<_>>(),
            "hint": "call again with `instrument`, `effect` or `plugin` for the parameter list",
        }),
    };
    Ok(CommandOutput::value(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_commands::{Caller, Request, Session};
    use std::sync::Arc;

    #[test]
    fn registered_and_describes_parameters() {
        let mut reg = Registry::with_builtins();
        register(&mut reg);
        let mut s = Session::new(Arc::new(reg), ph_core::Project::empty("t"));
        let mut host = ph_commands::NullHost::default();
        let who = Caller::human();
        let all = s
            .execute(
                &who,
                Request::new("query.node_params", json!({})),
                &mut host,
            )
            .unwrap();
        assert!(
            all.result["effects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "compressor")
        );
        assert!(!all.result["synth_presets"].as_array().unwrap().is_empty());
        let c = s
            .execute(
                &who,
                Request::new("query.node_params", json!({ "effect": "compressor" })),
                &mut host,
            )
            .unwrap();
        let params = c.result["params"].as_array().unwrap();
        let sc = params.iter().find(|p| p["id"] == "sidechain").unwrap();
        assert_eq!(sc["choices"], json!(["Internal", "External"]));
        assert!(
            s.execute(
                &who,
                Request::new("query.node_params", json!({ "effect": "nope" })),
                &mut host
            )
            .is_err()
        );
    }
}
