//! The merge behind per-author undo (DESIGN §3).
//!
//! Undoing a transaction is a **three-way merge, field by field**, of
//! - `before`: what each object looked like when the transaction started,
//! - `after`: what the transaction left,
//! - `current`: what the object looks like now (others may have edited it since).
//!
//! For every object the transaction touched:
//! - a field the transaction did not change is left alone;
//! - a field nobody changed since goes back to its `before` value;
//! - a field somebody else changed since is a **conflict**.
//!
//! JSON objects are compared recursively; arrays and scalars count as one value. Order
//! lists (`channel_order`, `pattern_order`) merge by membership instead: undoing an
//! insertion removes the id if it is still there, undoing a removal puts the id back at its
//! old index (clamped), and a reorder is undone only if nobody reordered the same ids since.
//!
//! The result is a new set of patches whose `before` values are the *current* values, so the
//! undo is an ordinary history entry that can itself be undone (redo) with the same merge.

use crate::*;
use ph_core::{ObjectPath, Patch, PatchSet, Project, get, set};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// What to do when undo meets a later change by someone else.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UndoMode {
    /// Cancel: change nothing and report every conflict.
    #[default]
    Stop,
    /// Revert everything except the conflicting fields.
    Skip,
    /// Revert everything, overwriting the later changes.
    Force,
}

/// The outcome of planning one revert.
pub(crate) struct Revert {
    /// Patches to apply, in order, with `before` set to the current values.
    pub patches: PatchSet,
    /// Conflicts that were skipped (`Skip`) or overridden (`Force`). Always empty for `Stop`
    /// (a `Stop` revert with conflicts is an error instead).
    pub conflicts: Vec<UndoConflict>,
}

/// Plans the revert of `entry` against `project`. `later` are the history entries after it,
/// used only to name who changed a conflicting field.
///
/// Patches are reverted one at a time, newest first, each merged against the *current* value
/// of its object. That keeps the order the model needs (an order list shrinks before the
/// channel it lists is deleted, and so on) exactly as a plain inverse would, while still
/// leaving alone whatever others changed since.
pub(crate) fn plan_revert(
    project: &Project,
    history: &[HistoryEntry],
    pos: usize,
    mode: UndoMode,
) -> CmdResult<Revert> {
    let entry = &history[pos];
    let later = &history[pos + 1..];
    let owner = effective_author(history, entry);
    let force = mode == UndoMode::Force;
    let mut work = project.clone();
    let mut out = Vec::new();
    let mut conflicts: Vec<UndoConflict> = Vec::new();

    for patch in entry.patches.iter().rev() {
        if patch.before == patch.after {
            continue;
        }
        let current = get(&work, patch.path);
        let mut fields = Vec::new();
        let target = if patch.path.is_order_list() {
            match merge_order(
                patch.before.as_ref(),
                patch.after.as_ref(),
                current.as_ref(),
                force,
            ) {
                Ok(v) => v,
                Err(()) => {
                    fields.push(String::new());
                    current.clone()
                }
            }
        } else {
            let touched = |ptr: &str| {
                later.iter().any(|e| {
                    e.patches
                        .iter()
                        .any(|p| p.path == patch.path && patch_touches(p, ptr))
                        && effective_author(history, e) != owner
                })
            };
            merge(
                patch.before.as_ref(),
                patch.after.as_ref(),
                current.as_ref(),
                "",
                &Ctx {
                    top: true,
                    force,
                    touched: &touched,
                },
                &mut fields,
            )
        };

        let mut applied = target;
        if applied != current {
            let mut trial = work.clone();
            match set(&mut trial, patch.path, applied.as_ref()) {
                Ok(()) => work = trial,
                Err(e) if force => return Err(CmdError::Model(e)),
                // The model refuses the revert (a note whose channel is gone, a pattern that
                // gained notes): report the object as conflicting.
                Err(_) => {
                    fields = vec![String::new()];
                    applied = current.clone();
                }
            }
        }
        if applied != current {
            out.push(Patch {
                path: patch.path,
                before: current.clone(),
                after: applied,
            });
        }
        for field in fields {
            note_conflict(&mut conflicts, patch.path, field, history, later);
        }
    }

    // A skipped conflict can leave a channel or pattern in place whose order-list entry was
    // already reverted away (its deletion was refused). Put such entries back where they were
    // so every object stays listed.
    for path in [ObjectPath::ChannelOrder, ObjectPath::PatternOrder] {
        let ids = |v: Option<Value>| -> Vec<u64> {
            v.and_then(|v| v.as_array().cloned())
                .map(|a| a.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default()
        };
        let exists = |w: &Project, id: u64| match path {
            ObjectPath::ChannelOrder => w.channels.contains_key(&ph_core::ChannelId(id as u32)),
            _ => w.patterns.contains_key(&ph_core::PatternId(id as u32)),
        };
        let (original, current) = (ids(get(project, path)), get(&work, path));
        let mut fixed = ids(current.clone());
        let mut changed = false;
        for (i, id) in original.iter().enumerate() {
            if !fixed.contains(id) && exists(&work, *id) {
                fixed.insert(i.min(fixed.len()), *id);
                changed = true;
            }
        }
        if changed {
            let after = Some(Value::Array(fixed.into_iter().map(Value::from).collect()));
            let mut trial = work.clone();
            if set(&mut trial, path, after.as_ref()).is_ok() {
                work = trial;
                out.push(Patch {
                    path,
                    before: current,
                    after,
                });
            }
        }
    }

    if mode == UndoMode::Stop && !conflicts.is_empty() {
        return Err(CmdError::UndoConflict(conflicts));
    }
    Ok(Revert {
        patches: PatchSet(out),
        conflicts,
    })
}

/// Whose work an entry is. A normal entry is its author's; an undo or redo is the work it
/// reverts, so a person undoing an agent's edits does not count as editing those objects.
fn effective_author(history: &[HistoryEntry], e: &HistoryEntry) -> Author {
    match &e.kind {
        EntryKind::Do => e.author.clone(),
        EntryKind::Undo { of } | EntryKind::Redo { of } => of
            .first()
            .and_then(|t| history.iter().find(|h| h.txn == *t))
            .map_or_else(|| e.author.clone(), |t| effective_author(history, t)),
    }
}

/// Records one conflicting field (`""` = the whole object), merged into the object's entry.
fn note_conflict(
    conflicts: &mut Vec<UndoConflict>,
    path: ObjectPath,
    field: String,
    history: &[HistoryEntry],
    later: &[HistoryEntry],
) {
    let who = changed_by(history, later, path, &field);
    let c = match conflicts.iter().position(|c| c.path == path) {
        Some(i) => &mut conflicts[i],
        None => {
            conflicts.push(UndoConflict {
                path,
                fields: Vec::new(),
                changed_by: Vec::new(),
            });
            conflicts.last_mut().expect("just pushed")
        }
    };
    if !field.is_empty() && !c.fields.contains(&field) {
        c.fields.push(field);
    }
    for a in who {
        if !c.changed_by.contains(&a) {
            c.changed_by.push(a);
        }
    }
}

/// Authors of later patches that changed `field` of `path` (any change, for `""`).
fn changed_by(
    history: &[HistoryEntry],
    later: &[HistoryEntry],
    path: ObjectPath,
    field: &str,
) -> Vec<Author> {
    let mut who = Vec::new();
    for e in later {
        let touched = e.patches.iter().any(|p| {
            p.path == path
                && (field.is_empty()
                    || p.before.as_ref().and_then(|v| v.pointer(field))
                        != p.after.as_ref().and_then(|v| v.pointer(field)))
        });
        let author = effective_author(history, e);
        if touched && !who.contains(&author) {
            who.push(author);
        }
    }
    who
}

/// What a merge needs to know besides the three values.
struct Ctx<'a> {
    /// At the object itself (not a field inside it).
    top: bool,
    force: bool,
    /// Did another author change this field (`""` = the object) in a later transaction?
    /// Their change counts even when it left the same value behind: undo must not wipe out
    /// an edit someone made on purpose.
    touched: &'a dyn Fn(&str) -> bool,
}

/// Does `p` change the field at JSON pointer `ptr` (`""` = anything)?
fn patch_touches(p: &Patch, ptr: &str) -> bool {
    if ptr.is_empty() {
        return true;
    }
    match (&p.before, &p.after) {
        (Some(b), Some(a)) => b.pointer(ptr) != a.pointer(ptr),
        _ => true, // created or deleted: every field changed
    }
}

/// Three-way merge of one JSON value. Returns the value to put there (`None` = absent) and
/// pushes the JSON pointers of conflicting fields onto `conflicts` (`""` = the whole value).
fn merge(
    before: Option<&Value>,
    after: Option<&Value>,
    current: Option<&Value>,
    ptr: &str,
    cx: &Ctx<'_>,
    conflicts: &mut Vec<String>,
) -> Option<Value> {
    let (top, force) = (cx.top, cx.force);
    let cx = &Ctx { top: false, ..*cx };
    if before == after {
        return current.cloned(); // this transaction did not change it
    }
    if after == current && !(cx.touched)(ptr) {
        return before.cloned(); // nobody changed it since: plain revert
    }
    if before == current {
        // Somebody already put it back. Except: the transaction created an object that
        // someone else has since deleted; that is reported, as DESIGN §3 asks.
        if top && before.is_none() {
            conflicts.push(String::new());
        }
        return current.cloned();
    }
    if let (Some(Value::Object(b)), Some(Value::Object(a)), Some(Value::Object(c))) =
        (before, after, current)
    {
        let mut keys: BTreeSet<&String> = b.keys().collect();
        keys.extend(a.keys());
        keys.extend(c.keys());
        let mut merged = Map::new();
        for k in keys {
            let sub = format!("{ptr}/{k}");
            if let Some(v) = merge(b.get(k), a.get(k), c.get(k), &sub, cx, conflicts) {
                merged.insert(k.clone(), v);
            }
        }
        return Some(Value::Object(merged));
    }
    conflicts.push(ptr.to_string());
    if force {
        before.cloned()
    } else {
        current.cloned()
    }
}

/// Membership merge for an order list. `Err` = somebody reordered the same ids since.
fn merge_order(
    before: Option<&Value>,
    after: Option<&Value>,
    current: Option<&Value>,
    force: bool,
) -> Result<Option<Value>, ()> {
    let list = |v: Option<&Value>| -> Vec<u64> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default()
    };
    let (b, a, c) = (list(before), list(after), list(current));
    if a == c {
        return Ok(before.cloned());
    }
    let (b_set, a_set): (BTreeSet<u64>, BTreeSet<u64>) =
        (b.iter().copied().collect(), a.iter().copied().collect());
    // 1. Undo insertions: drop what the transaction added.
    let mut r: Vec<u64> = c
        .iter()
        .copied()
        .filter(|id| !(a_set.contains(id) && !b_set.contains(id)))
        .collect();

    // 2. Undo a reorder of the ids that survive in all three lists, unless somebody else
    //    reordered them too.
    let common: BTreeSet<u64> = b_set.intersection(&a_set).copied().collect();
    let restrict = |v: &[u64], keep: &BTreeSet<u64>| -> Vec<u64> {
        v.iter().copied().filter(|id| keep.contains(id)).collect()
    };
    let live: BTreeSet<u64> = r.iter().copied().filter(|id| common.contains(id)).collect();
    let (b_rel, a_rel, r_rel) = (
        restrict(&b, &live),
        restrict(&a, &live),
        restrict(&r, &live),
    );
    if b_rel != a_rel {
        if r_rel != a_rel && !force {
            return Err(());
        }
        let mut slots = b_rel.into_iter();
        for id in r.iter_mut().filter(|id| live.contains(id)) {
            *id = slots.next().expect("same count");
        }
    }

    // 3. Undo removals: put each back at its old index (clamped).
    for (i, id) in b.iter().enumerate() {
        if !a_set.contains(id) && !r.contains(id) {
            r.insert(i.min(r.len()), *id);
        }
    }
    Ok(Some(Value::Array(r.into_iter().map(Value::from).collect())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(b: Value, a: Value, c: Value, force: bool) -> (Option<Value>, Vec<String>) {
        let mut conflicts = Vec::new();
        let r = merge(
            Some(&b),
            Some(&a),
            Some(&c),
            "",
            &Ctx {
                top: true,
                force,
                touched: &|_| false,
            },
            &mut conflicts,
        );
        (r, conflicts)
    }

    #[test]
    fn untouched_fields_and_clean_fields() {
        // I changed velocity 1->2; someone else changed start 0->9. Reverting gives velocity
        // back and keeps their start.
        let (r, c) = m(
            json!({"velocity": 1, "start": 0}),
            json!({"velocity": 2, "start": 0}),
            json!({"velocity": 2, "start": 9}),
            false,
        );
        assert!(c.is_empty());
        assert_eq!(r.unwrap(), json!({"velocity": 1, "start": 9}));
    }

    #[test]
    fn same_field_conflicts_and_modes() {
        let b = json!({"velocity": 1, "start": 0});
        let a = json!({"velocity": 2, "start": 5});
        let c = json!({"velocity": 3, "start": 5});
        let (r, conflicts) = m(b.clone(), a.clone(), c.clone(), false);
        assert_eq!(conflicts, vec!["/velocity"]);
        assert_eq!(
            r.unwrap(),
            json!({"velocity": 3, "start": 0}),
            "skip keeps theirs"
        );
        let (r, conflicts) = m(b, a, c, true);
        assert_eq!(conflicts, vec!["/velocity"]);
        assert_eq!(
            r.unwrap(),
            json!({"velocity": 1, "start": 0}),
            "force overwrites"
        );
    }

    #[test]
    fn nested_objects_recurse_and_arrays_are_atomic() {
        let (r, c) = m(
            json!({"params": {"cutoff": 1, "res": 1}, "tags": [1, 2]}),
            json!({"params": {"cutoff": 2, "res": 1}, "tags": [1, 2, 3]}),
            json!({"params": {"cutoff": 2, "res": 7}, "tags": [1, 2, 3]}),
            false,
        );
        assert!(c.is_empty());
        assert_eq!(
            r.unwrap(),
            json!({"params": {"cutoff": 1, "res": 7}, "tags": [1, 2]})
        );
        let (_, c) = m(
            json!({"t": [1]}),
            json!({"t": [1, 2]}),
            json!({"t": [1, 3]}),
            false,
        );
        assert_eq!(c, vec!["/t"]);
    }

    #[test]
    fn someone_else_already_reverted_is_not_a_conflict() {
        let (r, c) = m(json!({"v": 1}), json!({"v": 2}), json!({"v": 1}), false);
        assert!(c.is_empty());
        assert_eq!(r.unwrap(), json!({"v": 1}));
    }

    #[test]
    fn creation_modified_or_deleted_later_conflicts() {
        let mut c = Vec::new();
        let obj = json!({"v": 1});
        // Created by me, modified by someone else.
        let r = merge(None, Some(&obj), Some(&json!({"v": 2})), "", &TOP, &mut c);
        assert_eq!(r, Some(json!({"v": 2})));
        assert_eq!(c, vec![""]);
        // Created by me, deleted by someone else.
        let mut c = Vec::new();
        let r = merge(None, Some(&obj), None, "", &TOP, &mut c);
        assert_eq!(r, None);
        assert_eq!(c, vec![""]);
        // Untouched since: removed cleanly.
        let mut c = Vec::new();
        let r = merge(None, Some(&obj), Some(&obj), "", &TOP, &mut c);
        assert_eq!((r, c.len()), (None, 0));
        // Deleted by me, nobody touched it: restored.
        let mut c = Vec::new();
        let r = merge(Some(&obj), None, None, "", &TOP, &mut c);
        assert_eq!((r, c.len()), (Some(obj), 0));
    }

    const TOP: Ctx<'static> = Ctx {
        top: true,
        force: false,
        touched: &|_| false,
    };

    fn order(b: &[u64], a: &[u64], c: &[u64]) -> Result<Vec<u64>, ()> {
        let v = |x: &[u64]| Value::from(x.to_vec());
        merge_order(Some(&v(b)), Some(&v(a)), Some(&v(c)), false)
            .map(|r| serde_json::from_value(r.unwrap()).unwrap())
    }

    #[test]
    fn order_lists_merge_by_membership() {
        // I appended 3; someone appended 4 after: undo removes only 3.
        assert_eq!(order(&[1, 2], &[1, 2, 3], &[1, 2, 3, 4]), Ok(vec![1, 2, 4]));
        // I removed 2 from [1,2,3]; someone appended 4: 2 comes back at its old index.
        assert_eq!(order(&[1, 2, 3], &[1, 3], &[1, 3, 4]), Ok(vec![1, 2, 3, 4]));
        // Old index clamped when the list shrank.
        assert_eq!(order(&[1, 2, 3, 4], &[1, 2, 3], &[1]), Ok(vec![1, 4]));
        // Clean case restores exactly.
        assert_eq!(order(&[3, 1, 2], &[1, 2], &[1, 2]), Ok(vec![3, 1, 2]));
    }

    #[test]
    fn order_list_reorders() {
        // I moved 3 to the front; someone appended 4: undo moves it back, keeps 4.
        assert_eq!(
            order(&[1, 2, 3], &[3, 1, 2], &[3, 1, 2, 4]),
            Ok(vec![1, 2, 3, 4])
        );
        // Someone else reordered the same ids too: conflict.
        assert_eq!(order(&[1, 2, 3], &[3, 1, 2], &[2, 3, 1]), Err(()));
    }
}
