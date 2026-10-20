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
