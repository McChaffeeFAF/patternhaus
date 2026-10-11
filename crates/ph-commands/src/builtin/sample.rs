//! `sample.import`: bring audio files into the project as Sampler channels (dropping files
//! from the OS). The copy into `samples/` is a filesystem side effect, so it goes through
//! [`Host::import_file`]; the channels are an ordinary undoable edit.

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
        json!({ "channels": ids, "references": references, "skipped": skipped }),
        summary,
    ))
}
