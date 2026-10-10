//! Built-in commands. T0 defines a few reference commands that show the pattern; T1 adds the
//! full M1 set, one module per area (`project`, `channel`, `pattern`, `note`, `playlist`,
//! `mixer`, `transport`, `file`, `ui`).
//!
//! Conventions every command follows:
//! - Parameters are a `#[derive(Deserialize, JsonSchema)]` struct with doc comments on every
//!   field (they become the schema descriptions agents read). Use `#[serde(deny_unknown_fields)]`.
//! - Mutations go through `ph_core::Edit`, returned with `CommandOutput::from_edit` and a
//!   one-line summary written for a musician ("added 4 notes to Kick").
//! - Ids created are returned in `result` so agents can refer to them next.

use crate::*;
use ph_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn register_builtin(r: &mut Registry) {
    r.register(typed(
        "project.get_settings",
        "Read the project title, tempo, time signature and swing.",
        Scope::Read,
        READ,
        get_settings,
    ));
    r.register(typed(
        "project.set_tempo",
        "Set the project tempo in BPM.",
        Scope::Compose,
        MUTATES,
        set_tempo,
    ));
    r.register(typed(
        "channel.add",
        "Add a channel to the channel rack.",
        Scope::Compose,
        MUTATES,
        add_channel,
    ));
    r.register(typed(
        "pattern.add",
        "Create a new empty pattern.",
        Scope::Compose,
        MUTATES,
        add_pattern,
    ));
    r.register(typed(
        "note.add",
        "Add notes for one channel to a pattern.",
        Scope::Compose,
        MUTATES,
        add_notes,
    ));
    r.register(typed(
        "transport.play",
        "Start playback.",
        Scope::Compose,
        READ,
        play,
    ));
    r.register(typed(
        "transport.stop",
        "Stop playback.",
        Scope::Compose,
        READ,
        stop,
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

fn get_settings(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    Ok(CommandOutput::value(&ctx.project.settings))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTempo {
    /// Tempo in beats per minute, 10–999.
    bpm: f64,
}

fn set_tempo(ctx: &mut CommandCtx<'_>, p: SetTempo) -> CmdResult<CommandOutput> {
    let mut ed = Edit::new(ctx.project);
    ed.update_settings(|s| s.bpm = p.bpm)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "bpm": p.bpm }),
        format!("tempo set to {} BPM", p.bpm),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddChannel {
    /// Display name, e.g. "Kick".
    name: String,
    /// What makes the sound.
    instrument: InstrumentSpec,
    /// Mixer insert to route to (0 = master).
    #[serde(default)]
    insert: Option<u32>,
}

fn add_channel(ctx: &mut CommandCtx<'_>, p: AddChannel) -> CmdResult<CommandOutput> {
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_channel(&p.name, p.instrument)?;
    if let Some(i) = p.insert {
        ed.update_channel(id, |c| c.insert = InsertId(i))?;
    }
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "channel": id }),
        format!("added channel {} ({id})", p.name),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddPattern {
    /// Display name; defaults to "Pattern N".
    #[serde(default)]
    name: Option<String>,
}

fn add_pattern(ctx: &mut CommandCtx<'_>, p: AddPattern) -> CmdResult<CommandOutput> {
    let n = ctx.project.patterns.len() + 1;
    let name = p.name.unwrap_or_else(|| format!("Pattern {n}"));
    let mut ed = Edit::new(ctx.project);
    let id = ed.add_pattern(&name)?;
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "pattern": id }),
        format!("created pattern {name} ({id})"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddNotes {
    pattern: PatternId,
    channel: ChannelId,
    /// Notes to add. Times are in ticks (960 per quarter note; one step = 240).
    notes: Vec<NoteSpec>,
}

fn add_notes(ctx: &mut CommandCtx<'_>, p: AddNotes) -> CmdResult<CommandOutput> {
    let name = ctx.project.channel(p.channel)?.name.clone();
    let mut ed = Edit::new(ctx.project);
    let ids = p
        .notes
        .into_iter()
        .map(|n| ed.add_note(p.pattern, p.channel, n))
        .collect::<ModelResult<Vec<_>>>()?;
    let summary = format!("added {} note(s) to {name}", ids.len());
    Ok(CommandOutput::from_edit(
        ed,
        json!({ "notes": ids }),
        summary,
    ))
}

fn play(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    Ok(CommandOutput::value(
        ctx.host.transport(TransportCmd::Play)?,
    ))
}

fn stop(ctx: &mut CommandCtx<'_>, _: NoParams) -> CmdResult<CommandOutput> {
    Ok(CommandOutput::value(
        ctx.host.transport(TransportCmd::Stop)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn session() -> (Session, NullHost) {
        (
            Session::new(Arc::new(Registry::with_builtins()), Project::empty("t")),
            NullHost::default(),
        )
    }

    fn run(
        s: &mut Session,
        h: &mut NullHost,
        who: &Caller,
        cmd: &str,
        params: serde_json::Value,
    ) -> Executed {
        s.execute(who, Request::new(cmd, params), h).unwrap()
    }

    fn beat(s: &mut Session, h: &mut NullHost, who: &Caller) -> (u32, u32) {
        let pat = run(s, h, who, "pattern.add", json!({}));
        let ch = run(
            s,
            h,
            who,
            "channel.add",
            json!({ "name": "Kick", "instrument": { "kind": "sampler", "sample": "factory:kick", "root_key": 60 } }),
        );
        (
            pat.result["pattern"].as_u64().unwrap() as u32,
            ch.result["channel"].as_u64().unwrap() as u32,
        )
    }

    #[test]
    fn every_command_has_an_object_schema() {
        for spec in Registry::with_builtins().list("") {
            assert_eq!(spec.params["type"], "object", "{}", spec.id);
        }
    }

    #[test]
    fn batch_is_one_transaction_and_reports_changes() {
        let (mut s, mut h) = session();
        let human = Caller::human();
        let (pat, ch) = beat(&mut s, &mut h, &human);
        let notes = (0..4)
            .map(|i| json!({ "key": 36, "start": i * 960, "length": 240 }))
            .collect::<Vec<_>>();
        let out = s
            .batch(
                &human,
                vec![
                    Request::new(
                        "note.add",
                        json!({ "pattern": pat, "channel": ch, "notes": notes }),
                    ),
                    Request::new("project.set_tempo", json!({ "bpm": 92 })),
                ],
                &mut h,
            )
            .unwrap();
        assert_eq!(out[0].txn, out[1].txn);
        assert_eq!(out[0].summary.as_deref(), Some("added 4 note(s) to Kick"));
        assert_eq!(s.project().settings.bpm, 92.0);
        assert_eq!(s.history().len(), 3);
    }

    #[test]
    fn failed_batch_changes_nothing() {
        let (mut s, mut h) = session();
        let human = Caller::human();
        let before = s.project().clone();
        let err = s.batch(
            &human,
            vec![
                Request::new("project.set_tempo", json!({ "bpm": 100 })),
                Request::new("project.set_tempo", json!({ "bpm": 5000 })),
            ],
            &mut h,
        );
        assert!(err.is_err());
        assert_eq!(s.project(), &before);
        assert!(s.history().is_empty());
    }

    #[test]
    fn scopes_are_enforced() {
        let (mut s, mut h) = session();
        let reader = Caller::agent("viewer", Grant(vec![Scope::Read]));
        assert!(matches!(
            s.execute(
                &reader,
                Request::new("project.set_tempo", json!({ "bpm": 100 })),
                &mut h
            ),
            Err(CmdError::Scope { .. })
        ));
        assert!(
            s.execute(
                &reader,
                Request::new("project.get_settings", json!({})),
                &mut h
            )
            .is_ok()
        );
    }

    #[test]
    fn bad_params_are_reported_not_panicked() {
        let (mut s, mut h) = session();
        let r = s.execute(
            &Caller::human(),
            Request::new("project.set_tempo", json!({ "tempo": 100 })),
            &mut h,
        );
        assert!(matches!(r, Err(CmdError::BadParams { .. })));
    }

    #[test]
    fn per_author_undo_leaves_other_authors_alone() {
        let (mut s, mut h) = session();
        let human = Caller::human();
        let claude = Caller::agent("claude", Grant(vec![Scope::Compose]));
        let (pat, ch) = beat(&mut s, &mut h, &human);
        run(
            &mut s,
            &mut h,
            &claude,
            "note.add",
            json!({ "pattern": pat, "channel": ch, "notes": [{ "key": 36, "start": 0, "length": 240 }] }),
        );
        run(
            &mut s,
            &mut h,
            &human,
            "project.set_tempo",
            json!({ "bpm": 150 }),
        );

        s.undo(&claude.author).unwrap().unwrap();
        assert_eq!(
            s.project().pattern(PatternId(pat)).unwrap().note_count(),
            0,
            "agent's note undone"
        );
        assert_eq!(s.project().settings.bpm, 150.0, "human's later edit kept");

        s.redo(&claude.author).unwrap().unwrap();
        assert_eq!(s.project().pattern(PatternId(pat)).unwrap().note_count(), 1);
        // Ctrl+Z after redo undoes the same work again, not something older.
        s.undo(&claude.author).unwrap().unwrap();
        assert_eq!(s.project().pattern(PatternId(pat)).unwrap().note_count(), 0);
        assert_eq!(s.project().channels.len(), 1, "human's channel untouched");
    }

    #[test]
    fn conflicting_undo_stops_and_names_the_object() {
        let (mut s, mut h) = session();
        let human = Caller::human();
        let claude = Caller::agent("claude", Grant(vec![Scope::Compose]));
        run(
            &mut s,
            &mut h,
            &claude,
            "project.set_tempo",
            json!({ "bpm": 100 }),
        );
        run(
            &mut s,
            &mut h,
            &human,
            "project.set_tempo",
            json!({ "bpm": 120 }),
        );
        match s.undo(&claude.author) {
            Err(CmdError::UndoConflict(c)) => {
                assert_eq!(c[0].path, ObjectPath::Settings);
                assert_eq!(c[0].changed_by, vec![Author::Human]);
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(s.project().settings.bpm, 120.0);
    }
}
