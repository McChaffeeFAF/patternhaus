//! The import report: counts per object kind, and every approximation, skipped item and
//! unread event with how often it occurred. Written as `import-report.md` into the project
//! folder and returned by `file.import_flp`.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    /// Carried over, but not exactly (a default, a linear curve, a stand-in).
    Approximated,
    /// Not carried over.
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportItem {
    pub kind: ItemKind,
    pub what: String,
    pub count: u64,
}

/// What an import found and did.
///
/// Count keys (all `u64`): `channels_in_file`, `channels` (Patternhaus channels),
/// `channels.sampler`, `channels.plugin`, `channels.audio_sources`, `channels.automation_sources`,
/// `patterns`, `notes_in_file`, `notes`, `clips_in_file`, `clips`, `clips.pattern`,
/// `clips.audio`, `clips.automation`, `tracks_named`, `inserts_in_file`, `inserts`, `routes`,
/// `effects`, `effects.vst`, `effects.native`, `samples`, `samples_found`, `samples_missing`,
/// `automation_points`, `plugin_files`.
#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub fl_version: Option<String>,
    pub ppq: u16,
    pub counts: BTreeMap<String, u64>,
    items: BTreeMap<(ItemKind, String), u64>,
    /// Events the reader does not interpret, by id.
    pub unread_events: BTreeMap<u8, u64>,
    /// Project information with no field in the model (genre, author, comments, URL).
    pub metadata: Vec<(String, String)>,
}

impl ImportReport {
    pub(crate) fn new(fl_version: Option<String>, ppq: u16) -> Self {
        Self {
            fl_version,
            ppq,
            ..Default::default()
        }
    }

    pub fn count(&self, key: &str) -> u64 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    pub(crate) fn add(&mut self, key: &str, n: u64) {
        *self.counts.entry(key.to_string()).or_insert(0) += n;
    }

    pub(crate) fn approx(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self
                .items
                .entry((ItemKind::Approximated, what.to_string()))
                .or_insert(0) += n;
        }
    }

    pub(crate) fn skip(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self
                .items
                .entry((ItemKind::Skipped, what.to_string()))
                .or_insert(0) += n;
        }
    }

    /// Approximations first, then skipped items, each in name order.
    pub fn items(&self) -> Vec<ReportItem> {
        self.items
            .iter()
            .map(|((kind, what), count)| ReportItem {
                kind: *kind,
                what: what.clone(),
                count: *count,
            })
            .collect()
    }

    pub fn approximated(&self) -> u64 {
        self.items
            .iter()
            .filter(|((k, _), _)| *k == ItemKind::Approximated)
            .map(|(_, n)| *n)
            .sum()
    }

    pub fn skipped(&self) -> u64 {
        self.items
            .iter()
            .filter(|((k, _), _)| *k == ItemKind::Skipped)
            .map(|(_, n)| *n)
            .sum()
    }

    /// One line for the command summary.
    pub fn summary_line(&self) -> String {
        format!(
            "{} channels, {} patterns, {} notes, {} playlist clips, {} mixer routes, {} automation clips ({} approximated, {} skipped)",
            self.count("channels"),
            self.count("patterns"),
            self.count("notes"),
            self.count("clips"),
            self.count("routes"),
            self.count("clips.automation"),
            self.approximated(),
            self.skipped()
        )
    }

    /// The report as JSON (the command result).
    pub fn to_json(&self) -> Value {
        json!({
            "fl_version": self.fl_version,
            "ppq": self.ppq,
            "counts": self.counts,
            "approximated": self.items().iter().filter(|i| i.kind == ItemKind::Approximated)
                .map(|i| json!({ "what": i.what, "count": i.count })).collect::<Vec<_>>(),
            "skipped": self.items().iter().filter(|i| i.kind == ItemKind::Skipped)
                .map(|i| json!({ "what": i.what, "count": i.count })).collect::<Vec<_>>(),
            "unread_events": self.unread_events.iter()
                .map(|(id, n)| (id.to_string(), *n)).collect::<BTreeMap<_, _>>(),
        })
    }

    /// The text of `import-report.md`.
    pub fn to_markdown(&self, title: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Import report: {title}\n");
        let _ = writeln!(
            s,
            "Imported from an FL Studio project (version {}, {} ticks per quarter note).\n",
            self.fl_version.as_deref().unwrap_or("unknown"),
            self.ppq
        );
        let _ = writeln!(s, "## Imported\n\n| What | Count |\n|---|---|");
        for (k, v) in &self.counts {
            let _ = writeln!(s, "| {k} | {v} |");
        }
        for (heading, kind) in [
            ("Approximated", ItemKind::Approximated),
            ("Skipped", ItemKind::Skipped),
        ] {
            let _ = writeln!(s, "\n## {heading}\n");
            let rows: Vec<_> = self
                .items()
                .into_iter()
                .filter(|i| i.kind == kind)
                .collect();
            if rows.is_empty() {
                let _ = writeln!(s, "Nothing.");
            } else {
                let _ = writeln!(s, "| What | Times |\n|---|---|");
                for i in rows {
                    let _ = writeln!(s, "| {} | {} |", i.what.replace('|', "/"), i.count);
                }
            }
        }
        let _ = writeln!(s, "\n## Events not interpreted\n");
        if self.unread_events.is_empty() {
            let _ = writeln!(s, "None.");
        } else {
            let _ = writeln!(
                s,
                "Event ids the importer does not read (mostly interface state), with how many times each occurs.\n"
            );
            let _ = writeln!(s, "| Event id | Times |\n|---|---|");
            for (id, n) in &self.unread_events {
                let _ = writeln!(s, "| {id} | {n} |");
            }
        }
        if !self.metadata.is_empty() {
            let _ = writeln!(s, "\n## Project information (not stored in the project)\n");
            for (k, v) in &self.metadata {
                let _ = writeln!(s, "- {k}: {}", v.replace('\n', " "));
            }
        }
        s
    }
}
