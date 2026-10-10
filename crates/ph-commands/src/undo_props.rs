//! Property tests for per-author undo (AGENTS.md: property tests for anything history- or
//! merge-related). Two authors make random interleaved edits through real commands.

use crate::*;
use ph_core::*;
use proptest::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One random edit by one of the two authors. Fields are resolved against whatever exists
/// when the edit runs, so every sequence is meaningful.
#[derive(Clone, Debug)]
struct Op {
    agent: bool,
    kind: u8,
    a: u16,
    b: u16,
    c: u16,
}

fn op() -> impl Strategy<Value = Op> {
    (
        any::<bool>(),
        0u8..17,
        any::<u16>(),
        any::<u16>(),
        any::<u16>(),
    )
        .prop_map(|(agent, kind, a, b, c)| Op {
            agent,
            kind,
            a,
            b,
            c,
        })
}

struct Rig {
    s: Session,
    h: NullHost,
    human: Caller,
    agent: Caller,
}

impl Rig {
    fn new() -> Self {
        Self {
            s: Session::new(Arc::new(Registry::with_builtins()), Project::empty("p")),
            h: NullHost::default(),
            human: Caller::human(),
            agent: Caller::agent("claude", Grant::full()),
        }
    }

    fn pick<T: Copy>(v: &[T], i: u16) -> Option<T> {
        (!v.is_empty()).then(|| v[i as usize % v.len()])
    }

    fn channels(&self) -> Vec<u32> {
        self.s.project().channel_order.iter().map(|c| c.0).collect()
    }

    fn patterns(&self) -> Vec<u32> {
        self.s.project().pattern_order.iter().map(|c| c.0).collect()
    }

    fn all_notes(&self) -> Vec<(u32, u32, u32)> {
        let mut v = Vec::new();
        for p in self.s.project().patterns.values() {
            for (c, m) in &p.notes {
                for n in m.keys() {
                    v.push((p.id.0, c.0, n.0));
                }
            }
        }
        v
    }

    /// Runs one op as its author. Edits the model refuses are skipped.
    fn run(&mut self, op: &Op) {
        let (cmd, params): (&str, Value) = match op.kind {
            0 => ("project.set_tempo", json!({ "bpm": 60 + op.a % 100 })),
            1 => (
                "project.set_swing",
                json!({ "swing": (op.a % 101) as f32 / 100.0 }),
            ),
            2 => match Self::pick(&self.channels(), op.a) {
                Some(c) => (
                    "channel.set_volume",
                    json!({ "channel": c, "volume": (op.b % 101) as f32 / 100.0 }),
                ),
                None => return,
            },
            3 => match Self::pick(&self.channels(), op.a) {
                Some(c) => (
                    "channel.rename",
                    json!({ "channel": c, "name": format!("ch{}", op.b) }),
                ),
                None => return,
            },
            4 => match Self::pick(&self.channels(), op.a) {
                Some(c) => (
                    "channel.set_mute",
                    json!({ "channel": c, "muted": op.b.is_multiple_of(2) }),
                ),
                None => return,
            },
            5 => (
                "channel.add",
                json!({ "name": format!("c{}", op.a % 50), "instrument": { "kind": "synth", "preset": null } }),
            ),
            6 => ("pattern.add", json!({ "name": format!("p{}", op.a % 50) })),
            7 => {
                let (Some(p), Some(c)) = (
                    Self::pick(&self.patterns(), op.a),
                    Self::pick(&self.channels(), op.b),
                ) else {
                    return;
                };
                (
                    "note.add",
                    json!({ "pattern": p, "channel": c, "notes": [
                        { "key": 30 + op.c % 60, "start": (op.a % 16) as u64 * 240, "length": 240 }] }),
                )
            }
            8 => match Self::pick(&self.all_notes(), op.a) {
                Some((p, c, n)) => (
                    "note.update",
                    json!({ "pattern": p, "channel": c, "updates": [
                        { "id": n, "velocity": (op.b % 101) as f32 / 100.0, "start": (op.c % 32) as u64 * 120 }] }),
                ),
                None => return,
            },
            9 => match Self::pick(&self.all_notes(), op.a) {
                Some((p, c, n)) => (
                    "note.remove",
                    json!({ "pattern": p, "channel": c, "ids": [n] }),
                ),
                None => return,
            },
            10 => (
                "mixer.set_volume",
                json!({ "insert": op.a % 33, "volume": (op.b % 101) as f32 / 100.0 }),
            ),
            11 => match Self::pick(&self.patterns(), op.a) {
                Some(p) => (
                    "pattern.rename",
                    json!({ "pattern": p, "name": format!("n{}", op.b) }),
                ),
                None => return,
            },
            12 => match Self::pick(&self.channels(), op.a) {
                Some(c) => ("channel.move", json!({ "channel": c, "index": op.b % 5 })),
                None => return,
            },
            13 => match Self::pick(&self.patterns(), op.a) {
                Some(p) => (
                    "playlist.add_clip",
                    json!({ "track": op.b % 4, "pattern": p, "start": (op.c % 8) as u64 * 960 }),
                ),
                None => return,
            },
            14 => match Self::pick(&self.channels(), op.a) {
                Some(c) => ("channel.remove", json!({ "channel": c })),
                None => return,
            },
            15 => match Self::pick(&self.patterns(), op.a) {
                Some(p) => ("pattern.move", json!({ "pattern": p, "index": op.b % 4 })),
                None => return,
            },
            _ => match Self::pick(&self.patterns(), op.a) {
                Some(p) => ("pattern.remove", json!({ "pattern": p })),
                None => return,
            },
        };
        let who = if op.agent { &self.agent } else { &self.human };
        let _ = self.s.execute(who, Request::new(cmd, params), &mut self.h);
    }
}

/// Who last changed each top-level field of each object, from the history so far. A pseudo
/// field `<exists>` tracks creation and deletion.
fn last_writers(history: &[HistoryEntry]) -> BTreeMap<(ObjectPath, String), Author> {
    let mut last = BTreeMap::new();
    for e in history {
        for p in e.patches.iter() {
            match (&p.before, &p.after) {
                (Some(Value::Object(b)), Some(Value::Object(a))) => {
                    for k in b.keys().chain(a.keys()) {
                        if b.get(k) != a.get(k) {
                            last.insert((p.path, k.clone()), e.author.clone());
                        }
                    }
                }
                (Some(b), Some(a)) if b != a => {
                    last.insert((p.path, String::new()), e.author.clone());
                }
                _ => {
                    if p.before.is_some() != p.after.is_some() {
                        last.insert((p.path, "<exists>".into()), e.author.clone());
                    }
                }
            }
        }
    }
    last
}

fn field(v: &Option<Value>, key: &str) -> Option<Value> {
    match key {
        "<exists>" => Some(Value::Bool(v.is_some())),
        "" => v.clone(),
        k => v.as_ref().and_then(|v| v.get(k)).cloned(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Undo all of the agent's work with Skip: nothing the human last wrote changes, objects
    /// only the agent touched go back to the start, and human-created order entries stay.
    #[test]
    fn skip_undo_leaves_the_other_author_untouched(ops in proptest::collection::vec(op(), 1..45)) {
        let mut r = Rig::new();
        let base = r.s.project().clone();
        for o in &ops {
            r.run(o);
        }
        let before = r.s.project().clone();
        let writers = last_writers(r.s.history());
        let history: Vec<HistoryEntry> = r.s.history().to_vec();

        let reports = r
            .s
            .undo_author(&Author::Agent("claude".into()), &Author::Human, UndoMode::Skip)
            .expect("skip never stops");
        let after = r.s.project().clone();
        let _ = reports;

        // 1. Fields whose last writer was the human are exactly as they were.
        for ((path, key), who) in &writers {
            // (An object the agent deleted comes back when its deletion is undone.)
            // (Order lists merge by membership and are checked separately below.)
            let existed = !path.is_order_list() && key == "<exists>" || get(&before, *path).is_some();
            if *who == Author::Human && existed && !path.is_order_list() {
                prop_assert_eq!(
                    field(&get(&after, *path), key),
                    field(&get(&before, *path), key),
                    "human-written {} {} changed", path, key
                );
            }
        }

        // 2. Objects only the agent ever touched are back to their starting value, unless
        //    something the human made depends on them (a clip on the agent's pattern, a note
        //    in the agent's channel), which keeps them alive.
        let mut touched_by: BTreeMap<ObjectPath, (bool, bool)> = BTreeMap::new();
        for e in &history {
            for p in e.patches.iter() {
                let t = touched_by.entry(p.path).or_default();
                if e.author == Author::Human { t.0 = true } else { t.1 = true }
            }
        }
        let human_depends = |path: ObjectPath| {
            history.iter().filter(|e| e.author == Author::Human).any(|e| {
                e.patches.iter().any(|p| match (path, p.path) {
                    (ObjectPath::Channel(c), ObjectPath::Note(_, nc, _)) => c == nc,
                    (ObjectPath::Pattern(x), ObjectPath::Note(np, _, _)) => x == np,
                    (ObjectPath::Pattern(x), ObjectPath::Clip(_)) => p
                        .after
                        .as_ref()
                        .is_some_and(|v| v["pattern"] == json!(x.0)),
                    _ => false,
                })
            })
        };
        for (path, (human, agent)) in &touched_by {
            if *agent && !*human && !path.is_order_list() && !human_depends(*path) {
                prop_assert_eq!(
                    get(&after, *path),
                    get(&base, *path),
                    "{} was only touched by the agent but is not back to the start", path
                );
            }
        }

        // The model's own invariant: every channel and pattern is listed exactly once.
        let listed: std::collections::BTreeSet<_> = after.channel_order.iter().collect();
        prop_assert_eq!(listed.len(), after.channel_order.len());
        prop_assert!(after.channels.keys().all(|c| listed.contains(c)) && listed.len() == after.channels.len());
        let listed: std::collections::BTreeSet<_> = after.pattern_order.iter().collect();
        prop_assert!(after.patterns.keys().all(|c| listed.contains(c)) && listed.len() == after.patterns.len());

        // 3. Anything the human put in an order list is still there.
        for (order_path, ids) in [
            (ObjectPath::ChannelOrder, before.channel_order.iter().map(|c| c.0).collect::<Vec<_>>()),
            (ObjectPath::PatternOrder, before.pattern_order.iter().map(|c| c.0).collect::<Vec<_>>()),
        ] {
            let now: Vec<u32> = get(&after, order_path)
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect();
            for id in ids {
                let path = if order_path == ObjectPath::ChannelOrder {
                    ObjectPath::Channel(ChannelId(id))
                } else {
                    ObjectPath::Pattern(PatternId(id))
                };
                let human_made = history.iter().any(|e| {
                    e.author == Author::Human
                        && e.patches.iter().any(|p| p.path == path && p.before.is_none())
                });
                let human_removed = history.iter().any(|e| {
                    e.author == Author::Human
                        && e.patches.iter().any(|p| p.path == path && p.after.is_none())
                });
                if human_made && !human_removed {
                    prop_assert!(now.contains(&id), "{} dropped from {}", path, order_path);
                }
            }
        }
    }

    /// Stop either undoes everything or changes nothing, and names the conflicts.
    #[test]
    fn stop_undo_is_all_or_nothing(ops in proptest::collection::vec(op(), 1..45)) {
        let mut r = Rig::new();
        for o in &ops {
            r.run(o);
        }
        let before = r.s.project().clone();
        let len = r.s.history().len();
        match r.s.undo_author(&Author::Agent("claude".into()), &Author::Human, UndoMode::Stop) {
            Ok(_) => {}
            Err(CmdError::UndoConflict(c)) => {
                prop_assert!(!c.is_empty());
                prop_assert_eq!(r.s.project(), &before);
                prop_assert_eq!(r.s.history().len(), len);
            }
            Err(e) => prop_assert!(false, "unexpected {e}"),
        }
    }

    /// Alone, an author can undo everything and redo it all back to the exact same project.
    #[test]
    fn undo_then_redo_round_trips_exactly(
        ops in proptest::collection::vec(op(), 1..45),
        agent in any::<bool>(),
    ) {
        let mut r = Rig::new();
        let me = if agent { Author::Agent("claude".into()) } else { Author::Human };
        let base = r.s.project().clone();
        for o in &ops {
            r.run(&Op { agent, ..o.clone() });
        }
        let done = r.s.project().clone();

        r.s.undo_author(&me, &me, UndoMode::Stop).expect("nobody else edited, so no conflict");
        // Everything is back to the start; ids are never freed, so next_ids stays.
        let mut expected = (*base).clone();
        expected.next_ids = done.next_ids.clone();
        prop_assert_eq!(r.s.project().as_ref(), &expected);

        while r.redo(&me) {}
        prop_assert_eq!(r.s.project().as_ref(), done.as_ref());
    }

    /// Step by step: one undo then one redo is the identity at every point in a session.
    #[test]
    fn single_undo_redo_is_the_identity(ops in proptest::collection::vec(op(), 1..30)) {
        let mut r = Rig::new();
        for o in &ops {
            r.run(&Op { agent: false, ..o.clone() });
            let now = r.s.project().clone();
            if r.s.undo(&Author::Human).expect("alone").is_some() {
                prop_assert!(r.redo(&Author::Human));
                prop_assert_eq!(r.s.project().as_ref(), now.as_ref());
            }
        }
    }
}

impl Rig {
    fn redo(&mut self, who: &Author) -> bool {
        self.s
            .redo(who)
            .expect("redo alone never conflicts")
            .is_some()
    }
}
