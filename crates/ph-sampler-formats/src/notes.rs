//! The import report accumulator shared by the importers: approximated, skipped and kept
//! items counted by what they concern, plus the things not understood at all.

use ph_core::{ImportNote, ImportNoteKind};
use std::collections::BTreeMap;

#[derive(Default, Debug)]
pub(crate) struct Notes {
    /// (kind order, what) -> (kind, detail, count). The kind order keeps the report grouped.
    items: BTreeMap<(u8, String), (ImportNoteKind, String, u32)>,
    unknown: BTreeMap<String, u32>,
}

fn order(k: ImportNoteKind) -> u8 {
    match k {
        ImportNoteKind::Approximated => 0,
        ImportNoteKind::Skipped => 1,
        ImportNoteKind::Kept => 2,
    }
}

impl Notes {
    pub fn add(
        &mut self,
        kind: ImportNoteKind,
        what: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let e = self
            .items
            .entry((order(kind), what.into()))
            .or_insert_with(|| (kind, detail.into(), 0));
        e.2 = e.2.saturating_add(1);
    }

    pub fn approx(&mut self, what: impl Into<String>, detail: impl Into<String>) {
        self.add(ImportNoteKind::Approximated, what, detail);
    }

    pub fn skip(&mut self, what: impl Into<String>, detail: impl Into<String>) {
        self.add(ImportNoteKind::Skipped, what, detail);
    }

    pub fn kept(&mut self, what: impl Into<String>, detail: impl Into<String>) {
        self.add(ImportNoteKind::Kept, what, detail);
    }

    /// Something not understood at all (an unknown opcode): listed with its count.
    pub fn unknown(&mut self, what: impl Into<String>) {
        let n = self.unknown.entry(what.into()).or_insert(0);
        *n = n.saturating_add(1);
    }

    /// True when a note about `what` (of any kind) was recorded.
    #[cfg(test)]
    pub fn has(&self, what: &str) -> bool {
        self.items.keys().any(|(_, w)| w == what) || self.unknown.contains_key(what)
    }

    /// The notes (unknown items included as skipped, "not understood", so the native file
    /// keeps them) and the unknown list with counts.
    pub fn finish(self) -> (Vec<ImportNote>, Vec<(String, u32)>) {
        let mut notes: Vec<ImportNote> = self
            .items
            .into_iter()
            .map(|((_, what), (kind, detail, count))| ImportNote {
                kind,
                what,
                detail,
                count,
            })
            .collect();
        for (what, count) in &self.unknown {
            notes.push(ImportNote {
                kind: ImportNoteKind::Skipped,
                what: what.clone(),
                detail: "not understood".into(),
                count: *count,
            });
        }
        (notes, self.unknown.into_iter().collect())
    }
}
