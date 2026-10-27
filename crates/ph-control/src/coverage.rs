//! Criterion 8 (DESIGN §12): every command a front end serves has a schema test and a
//! behaviour test. `ph-commands` guards its own built-ins; this guard covers the full
//! registry agents see ([`crate::studio_registry`]), including commands `ph-control` adds,
//! so a new command anywhere fails CI until it is tested.

use crate::studio_registry;
use ph_commands::Scope;

/// Test sources that exercise commands by id. A command counts as covered when one of them
/// calls it by its quoted id.
const BEHAVIOUR_SOURCES: &[&str] = &[
    include_str!("../../ph-commands/src/builtin/behaviour.rs"),
    include_str!("../../ph-commands/src/builtin/tests.rs"),
    include_str!("../../ph-commands/src/builtin/m2_behaviour.rs"),
    include_str!("../../ph-commands/src/builtin/plugin_behaviour.rs"),
    include_str!("../../ph-commands/src/builtin/record_behaviour.rs"),
    include_str!("../../ph-commands/src/builtin/editor_behaviour.rs"),
    include_str!("../../ph-commands/src/builtin/proposal_behaviour.rs"),
    include_str!("node_params.rs"),
    include_str!("tests.rs"),
];

/// The schema test, run for every registered command: an object schema whose properties
/// all carry a description, a one-sentence summary, and a scope consistent with the flags.
#[test]
fn every_served_command_passes_the_schema_test() {
    let reg = studio_registry();
    assert!(reg.get("query.node_params").is_ok());
    for s in reg.list("") {
        assert_eq!(
            s.params["type"], "object",
            "{}: schema is not an object",
            s.id
        );
        if let Some(props) = s.params["properties"].as_object() {
            for (name, p) in props {
                assert!(
                    p.get("description").and_then(|d| d.as_str()).is_some(),
                    "{}.{name} has no description",
                    s.id
                );
            }
        }
        assert!(
            s.summary.ends_with('.') && s.summary.len() < 200,
            "{}: summary",
            s.id
        );
        assert!(
            !(s.scope == Scope::Read && s.mutating),
            "{}: a Read-scope command must not mutate",
            s.id
        );
    }
}

#[test]
fn every_served_command_has_a_behaviour_test() {
    let sources = BEHAVIOUR_SOURCES.join("\n");
    let untested: Vec<_> = studio_registry()
        .list("")
        .iter()
        .filter(|s| !sources.contains(&format!("\"{}\"", s.id)))
        .map(|s| s.id)
        .collect();
    assert!(untested.is_empty(), "no behaviour test calls {untested:?}");
}

/// The M2 command families (M2 criterion 8), and the M2 commands outside them.
const M2_FAMILIES: &[&str] = &[
    "audio.",
    "automation.",
    "plugin.",
    "record.",
    "midi.",
    "slicer.",
    "editor.",
    "sample.",
];
const M2_SINGLES: &[&str] = &["file.import_flp", "query.node_params"];

fn is_m2(id: &str) -> bool {
    M2_FAMILIES.iter().any(|f| id.starts_with(f)) || M2_SINGLES.contains(&id)
}

/// True when `"id"` appears in `src` as a command a test runs: the first argument of a call
/// (`run("id"`, `Request::new("id"`), or the second after a borrowed context (`run(&s, "id"`,
/// `s.run(&agent, "id"`), not merely somewhere in a string or a comment.
fn called_in(src: &str, id: &str) -> bool {
    let quoted = format!("\"{id}\"");
    src.match_indices(&quoted).any(|(at, _)| {
        let before = src[..at].trim_end();
        if before.ends_with('(') {
            return true;
        }
        let Some(rest) = before.strip_suffix(',') else {
            return false;
        };
        // `(&<one short argument>,`: a borrowed context first (`&s`, `&agent`, `&who`), with no
        // quote, closing paren or other comma in it
        let Some(open) = rest.rfind('(') else {
            return false;
        };
        let arg = rest[open + 1..].trim_start();
        arg.starts_with('&') && arg.len() <= 40 && !arg.contains(['"', ')', ',', ';', '\n'])
    })
}

/// M2 criterion 8, beyond the M1 guards: every M2 command is *called* by a behaviour test (not
/// just named in one), and its schema refuses unknown parameters (`deny_unknown_fields`), so a
/// misspelt parameter from an agent is an error instead of being ignored.
#[test]
fn every_m2_command_is_called_by_a_behaviour_test_and_refuses_unknown_params() {
    let sources = BEHAVIOUR_SOURCES.join("\n");
    let reg = studio_registry();
    let m2: Vec<_> = reg.list("").into_iter().filter(|s| is_m2(s.id)).collect();
    assert!(m2.len() >= 45, "the M2 commands are served: {}", m2.len());
    let uncalled: Vec<_> = m2
        .iter()
        .filter(|s| !called_in(&sources, s.id))
        .map(|s| s.id)
        .collect();
    assert!(uncalled.is_empty(), "no behaviour test runs {uncalled:?}");
    let open: Vec<_> = m2
        .iter()
        .filter(|s| s.params["additionalProperties"] != serde_json::json!(false))
        .map(|s| s.id)
        .collect();
    assert!(open.is_empty(), "these accept unknown params: {open:?}");
}

#[test]
fn called_in_tells_a_call_from_a_mention() {
    assert!(called_in(
        r#"r.run("audio.split", json!({}))"#,
        "audio.split"
    ));
    assert!(called_in(r#"run(&s, "audio.split", p)"#, "audio.split"));
    assert!(called_in(
        r#"s.run(&agent, "audio.split", p)"#,
        "audio.split"
    ));
    assert!(called_in(
        r#"Request::new("audio.split", p)"#,
        "audio.split"
    ));
    assert!(!called_in(r#"// see "audio.split""#, "audio.split"));
    assert!(!called_in(r#"assert_eq!(x, "audio.split")"#, "audio.split"));
    assert!(!called_in(
        r#"let ids = ["audio.split", "audio.duplicate"];"#,
        "audio.split"
    ));
}
