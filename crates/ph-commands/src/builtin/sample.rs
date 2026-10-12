//! `sample.*`: the sample registry (M2) and `sample.import`, which brings audio files into
//! the project as Sampler channels (dropping files from the OS). The copy into `samples/` is
//! a filesystem side effect, so it goes through [`Host::import_file`]; the channels and the
//! registry entries are an ordinary undoable edit.

use super::util::*;
use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub fn register(r: &mut Registry) {
    r.register(typed(
        "sample.import",
        "Copy audio files into the project's samples folder and add a Sampler channel for each, or replace one channel's sample.",
        Scope::Compose,
        MUTATES,
        import,
    ));
    r.register(typed(
        "sample.list",
        "List the project's sample registry: every audio file audio clips can play, with its format, tempo and origin.",
        Scope::Read,
        READ,
        list,
    ));
    r.register(typed(
        "sample.set_bpm",
        "Set or clear a sample's tempo (used by tempo-synced audio clips and fit to tempo).",
        Scope::Compose,
        MUTATES,
        set_bpm,
    ));
    r.register(typed(
        "sample.set_root_key",
        "Set or clear the key a sample is pitched at.",
        Scope::Compose,
        MUTATES,
        set_root_key,
    ));
    r.register(typed(
        "sample.remove",
        "Remove a sample from the registry (only while no audio clip uses it; the file stays).",
        Scope::Compose,
        MUTATES,
        remove,
    ));
    r.register(typed(
        "sample.detect_bpm",
        "Detect a sample's tempo by onset analysis and store it (unless the user set one).",
        Scope::Compose,
        MUTATES,
        detect_bpm,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SampleOnly {
    /// Sample registry id (see sample.list).
    sample: SampleId,
}

fn list(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    let samples: Vec<_> = ctx
        .project
        .samples
        .values()
        .map(|e| {
            let used = ctx
                .project
                .clips
                .values()
                .filter(|c| c.audio().is_some_and(|a| a.sample == e.id))
                .count();
            json!({
                "id": e.id, "path": e.path, "name": e.name, "seconds": e.seconds(),
                "frames": e.frames, "sample_rate": e.sample_rate, "channels": e.channels,
                "bpm": e.bpm, "root_key": e.root_key, "provenance": e.provenance,
                "clips": used,
            })
        })
        .collect();
    Ok(CommandOutput::value(json!({ "samples": samples })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetBpm {
    /// Sample registry id (see sample.list).
    sample: SampleId,
    /// Tempo in BPM, 10..=999; null clears it.
    bpm: Option<f64>,
}

fn set_bpm(ctx: &mut CommandCtx<'_>, p: SetBpm) -> CmdResult<CommandOutput> {
    let name = ctx.project.sample(p.sample)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.set_sample_bpm(
        p.sample,
        p.bpm.map(|value| SampleBpm {
            value,
            source: BpmSource::User,
        }),
    )?;
    let summary = match p.bpm {
        Some(b) => format!("set {name} to {b} BPM"),
        None => format!("cleared the tempo of {name}"),
    };
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "sample": p.sample, "bpm": p.bpm }),
        summary,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRootKey {
    /// Sample registry id (see sample.list).
    sample: SampleId,
    /// MIDI key 0..=127 (60 = C4); null clears it.
    key: Option<u8>,
}

fn set_root_key(ctx: &mut CommandCtx<'_>, p: SetRootKey) -> CmdResult<CommandOutput> {
    let name = ctx.project.sample(p.sample)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.update_sample(p.sample, |e| e.root_key = p.key)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "sample": p.sample, "key": p.key }),
        format!("set the root key of {name}"),
    ))
}

fn remove(ctx: &mut CommandCtx<'_>, p: SampleOnly) -> CmdResult<CommandOutput> {
    let name = ctx.project.sample(p.sample)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    ed.remove_sample(p.sample)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "sample": p.sample }),
        format!("removed {name} from the sample list"),
    ))
}

fn detect_bpm(ctx: &mut CommandCtx<'_>, p: SampleOnly) -> CmdResult<CommandOutput> {
    ctx.project.sample(p.sample)?;
    unavailable("sample.detect_bpm", "BPM detection", "T1")
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Import {
    /// Audio files to import (WAV, AIFF, FLAC, MP3, OGG). Agents may only name files inside
    /// the project or library folders.
    paths: Vec<String>,
    /// Replace this channel's sample instead of creating channels; then exactly one path.
    #[serde(default)]
    channel: Option<ChannelId>,
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map_or_else(|| "Sample".into(), |s| s.to_string_lossy().into_owned())
}

fn import(ctx: &mut CommandCtx<'_>, p: Import) -> CmdResult<CommandOutput> {
    if p.paths.is_empty() {
        return invalid("paths is empty");
    }
    if p.channel.is_some() && p.paths.len() != 1 {
        return invalid("replacing a channel's sample takes exactly one path");
    }
    let target = match p.channel {
        Some(c) => Some(ctx.project.channel(c)?.clone()),
        None => None,
    };
    let mut done: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for path in &p.paths {
        match ctx
            .host
            .import_file(Path::new(path), ImportKind::Sample, ctx.author)
        {
            Ok(reference) => done.push((path.clone(), reference)),
            Err(e) => skipped.push((path.clone(), e.to_string())),
        }
    }
    if done.is_empty() {
        let why: Vec<String> = skipped.iter().map(|(p, r)| format!("{p}: {r}")).collect();
        return Err(CmdError::Host(why.join("; ")));
    }
    let mut ed = Edit::new(ctx.project);
    // M2: every imported file is in the sample registry (when the host can read it)
    let mut samples = Vec::new();
    for (source, reference) in &done {
        if let Ok(id) = register_sample(&mut ed, &*ctx.host, reference, Some(source)) {
            samples.push(id);
        }
    }
    let mut ids = Vec::new();
    let summary = if let Some(ch) = target {
        let (source, reference) = &done[0];
        let (root_key, params) = match &ch.instrument {
            InstrumentSpec::Sampler {
                root_key, params, ..
            } => (*root_key, params.clone()),
            _ => (60, Default::default()),
        };
        ed.update_channel(ch.id, |c| {
            c.instrument = InstrumentSpec::Sampler {
                sample: Some(reference.clone()),
                root_key,
                params: params.clone(),
            }
        })?;
        ids.push(ch.id);
        format!("replaced {}'s sample with {}", ch.name, stem(source))
    } else {
        for (source, reference) in &done {
            ids.push(ed.add_channel(
                &stem(source),
                InstrumentSpec::Sampler {
                    sample: Some(reference.clone()),
                    root_key: 60,
                    params: Default::default(),
                },
            )?);
        }
        format!(
            "added {}",
            plural(done.len(), "sample channel", "sample channels")
        )
    };
    let summary = if skipped.is_empty() {
        summary
    } else {
        format!(
            "{summary}; skipped {}",
            plural(skipped.len(), "file", "files")
        )
    };
    let references: Vec<&String> = done.iter().map(|(_, r)| r).collect();
    let skipped: Vec<_> = skipped
        .iter()
        .map(|(p, r)| json!({ "path": p, "reason": r }))
        .collect();
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channels": ids, "references": references, "samples": samples, "skipped": skipped }),
        summary,
    ))
}
