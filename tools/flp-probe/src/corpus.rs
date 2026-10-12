//! `flp-probe corpus <dir>...`: imports every `.flp` under the given folders (read-only) and
//! prints aggregate numbers only, never names or paths. Local use: the owner's projects are
//! never committed and never run in CI.
//!
//! The check is an *independent reading*: the expected notes and playlist clips are counted
//! from the probe's own container reader (`flp_probe::parse`) with a few lines of record
//! arithmetic written here, not from `ph_formats::flp`. The imported project is then diffed
//! against it as multisets (start, length, key, velocity for notes; start, length, track,
//! kind for clips).

use flp_probe::{Flp, Payload, parse};
use ph_core::{PLAYLIST_TRACKS, Project};
use ph_formats::flp::{ResolvedSample, SampleResolver, import_flp};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Instant;

/// Resolves a sample path by looking for the file (environment placeholders only); does not
/// copy or decode. Mirrors what `file.import_flp` finds, minus the host.
struct ExistsOnly;

impl SampleResolver for ExistsOnly {
    fn resolve(&mut self, p: &str) -> Option<ResolvedSample> {
        let plain = p.replace('\\', "/");
        let path = match plain.strip_prefix('%').and_then(|r| r.split_once('%')) {
            Some((name, tail)) => {
                Path::new(&std::env::var_os(name)?).join(tail.trim_start_matches('/'))
            }
            None => plain.into(),
        };
        path.is_file().then(|| ResolvedSample {
            reference: p.to_string(),
            frames: 0,
            sample_rate: 44_100,
            channels: 2,
        })
    }
}

fn u32le(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u16le(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2)
        .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn ticks(v: u32, ppq: u32) -> u64 {
    (u64::from(v) * 960 + u64::from(ppq) / 2) / u64::from(ppq)
}

type NoteKey = (u64, u64, u16, u32);
type ClipKey = (u64, u64, u32, &'static str);

/// What an independent reading of the file expects the import to contain.
#[derive(Default)]
struct Expect {
    channels: u64,
    rack_channels: u64,
    audio_sources: u64,
    automation_sources: u64,
    notes: Vec<NoteKey>,
    clips: Vec<ClipKey>,
}

fn expect(flp: &Flp<'_>) -> Expect {
    let ppq = u32::from(flp.header.ppq.max(1));
    let major = flp_probe::version(flp)
        .and_then(|v| v.split('.').next().and_then(|m| m.parse::<u32>().ok()))
        .unwrap_or(0);
    let rec = if major >= 21 { 60 } else { 32 };
    let mut e = Expect::default();
    let mut kinds: BTreeMap<u16, u8> = BTreeMap::new();
    let mut cur: Option<u16> = None;
    // first pass: channel types
    for ev in &flp.events {
        match (ev.id, ev.payload) {
            (64, Payload::Fixed(v)) => {
                cur = Some(v as u16);
                e.channels += 1;
            }
            (21, Payload::Fixed(v)) => {
                if let Some(c) = cur {
                    kinds.insert(c, v as u8);
                }
            }
            _ => {}
        }
    }
    for k in kinds.values() {
        match k {
            0 | 2 => e.rack_channels += 1,
            4 => e.audio_sources += 1,
            5 => e.automation_sources += 1,
            _ => {}
        }
    }
    for ev in &flp.events {
        let Payload::Bytes(b) = ev.payload else {
            continue;
        };
        match ev.id {
            224 => {
                for r in b.as_chunks::<24>().0 {
                    let ch = u16le(r, 6);
                    let key = u16le(r, 12);
                    if !matches!(kinds.get(&ch), Some(0 | 2)) || key > 127 {
                        continue;
                    }
                    let len = u32le(r, 8);
                    let length = if len == 0 {
                        240
                    } else {
                        ticks(len, ppq).max(1)
                    };
                    e.notes
                        .push((ticks(u32le(r, 0), ppq), length, key, u32::from(r[21])));
                }
            }
            233 => {
                for r in b.chunks_exact(rec) {
                    let idx = u16le(r, 6);
                    let len = u32le(r, 8);
                    let track = 499u32.saturating_sub(u32le(r, 12));
                    let kind = if idx > 0x5000 {
                        "pattern"
                    } else {
                        match kinds.get(&idx) {
                            Some(4) => "audio",
                            Some(5) => "automation",
                            _ => continue,
                        }
                    };
                    if len == 0 {
                        continue;
                    }
                    e.clips
                        .push((ticks(u32le(r, 0), ppq), ticks(len, ppq).max(1), track, kind));
                }
            }
            _ => {}
        }
    }
    e
}

fn actual_notes(p: &Project) -> Vec<NoteKey> {
    p.patterns
        .values()
        .flat_map(|pat| pat.notes.values().flat_map(|m| m.values()))
        .map(|n| {
            (
                n.start,
                n.length,
                u16::from(n.key),
                (n.velocity * 128.0).round() as u32,
            )
        })
        .collect()
}

fn actual_clips(p: &Project) -> Vec<ClipKey> {
    p.clips
        .values()
        .map(|c| (c.start, c.length, c.track, c.kind_name()))
        .collect()
}

/// Size of the multiset intersection of two vectors.
fn matched<T: Ord>(mut a: Vec<T>, mut b: Vec<T>) -> u64 {
    a.sort();
    b.sort();
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}

#[derive(Default)]
struct Totals {
    files: u64,
    imported: u64,
    invalid: u64,
    panics: u64,
    failures: BTreeMap<String, u64>,
    ms: u128,
    exp: Expect,
    exp_clips_in_range: u64,
    got_notes: u64,
    got_clips: u64,
    got_channels: u64,
    got_patterns: u64,
    matched_notes: u64,
    matched_clips: u64,
    clips_by_kind: BTreeMap<&'static str, (u64, u64)>,
    counts: BTreeMap<String, u64>,
    approx: BTreeMap<String, (u64, u64)>,
    skipped: BTreeMap<String, (u64, u64)>,
    unread: BTreeMap<u8, (u64, u64)>,
    versions: BTreeMap<u32, u64>,
}

fn pct(a: u64, b: u64) -> String {
    if b == 0 {
        "n/a".into()
    } else {
        format!("{:.2}%", 100.0 * a as f64 / b as f64)
    }
}

fn top(map: &BTreeMap<String, (u64, u64)>, n: usize) -> String {
    let mut v: Vec<_> = map.iter().collect();
    v.sort_by_key(|(_, (times, _))| std::cmp::Reverse(*times));
    v.iter()
        .take(n)
        .map(|(k, (times, files))| format!("  {times:>8} x in {files:>3} files  {k}\n"))
        .collect()
}

pub fn run(files: &[std::path::PathBuf]) -> String {
    std::panic::set_hook(Box::new(|_| {}));
    let mut t = Totals::default();
    for path in files {
        t.files += 1;
        let Ok(bytes) = std::fs::read(path) else {
            *t.failures.entry("unreadable file".into()).or_insert(0) += 1;
            continue;
        };
        let Ok(raw) = parse(&bytes) else {
            *t.failures
                .entry("not parsed by the probe".into())
                .or_insert(0) += 1;
            continue;
        };
        let e = expect(&raw);
        let started = Instant::now();
        let out = catch_unwind(AssertUnwindSafe(|| {
            import_flp(&bytes, "corpus", &mut ExistsOnly)
        }));
        t.ms += started.elapsed().as_millis();
        let out = match out {
            Err(_) => {
                t.panics += 1;
                continue;
            }
            Ok(Err(err)) => {
                *t.failures.entry(err.to_string()).or_insert(0) += 1;
                continue;
            }
            Ok(Ok(o)) => o,
        };
        t.imported += 1;
        if ph_formats::validate(&out.project).is_err() {
            t.invalid += 1;
        }
        let p = &out.project;
        let (an, ac) = (actual_notes(p), actual_clips(p));
        t.got_notes += an.len() as u64;
        t.got_clips += ac.len() as u64;
        t.got_channels += p.channels.len() as u64;
        t.got_patterns += p.patterns.len() as u64;
        t.matched_notes += matched(e.notes.clone(), an);
        t.matched_clips += matched(e.clips.clone(), ac.clone());
        t.exp_clips_in_range += e
            .clips
            .iter()
            .filter(|c| (c.2 as usize) < PLAYLIST_TRACKS)
            .count() as u64;
        for kind in ["pattern", "audio", "automation"] {
            let ex: Vec<_> = e.clips.iter().filter(|c| c.3 == kind).copied().collect();
            let ac: Vec<_> = ac.iter().filter(|c| c.3 == kind).copied().collect();
            let entry = t.clips_by_kind.entry(kind).or_insert((0, 0));
            entry.0 += ex.len() as u64;
            entry.1 += matched(ex, ac);
        }
        for (k, v) in &out.report.counts {
            *t.counts.entry(k.clone()).or_insert(0) += v;
        }
        for i in out.report.items() {
            let m = match i.kind {
                ph_formats::flp::ItemKind::Approximated => &mut t.approx,
                ph_formats::flp::ItemKind::Skipped => &mut t.skipped,
            };
            let en = m.entry(i.what).or_insert((0, 0));
            en.0 += i.count;
            en.1 += 1;
        }
        for (id, n) in &out.report.unread_events {
            let en = t.unread.entry(*id).or_insert((0, 0));
            en.0 += n;
            en.1 += 1;
        }
        if let Some(v) = flp_probe::version(&raw) {
            let major = v
                .split('.')
                .next()
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
            *t.versions.entry(major).or_insert(0) += 1;
        }
        t.exp.channels += e.channels;
        t.exp.rack_channels += e.rack_channels;
        t.exp.audio_sources += e.audio_sources;
        t.exp.automation_sources += e.automation_sources;
        t.exp
            .notes
            .extend(std::iter::repeat_n((0, 0, 0, 0), e.notes.len()));
        t.exp
            .clips
            .extend(std::iter::repeat_n((0, 0, 0, ""), e.clips.len()));
    }
    let mut s = String::new();
    use std::fmt::Write;
    let _ = writeln!(s, "FLP corpus check (aggregate numbers only)");
    let _ = writeln!(
        s,
        "files {}  imported {} ({})  failed {}  panics {}  invalid projects {}  import time {} ms",
        t.files,
        t.imported,
        pct(t.imported, t.files),
        t.files - t.imported - t.panics,
        t.panics,
        t.invalid,
        t.ms
    );
    for (k, n) in &t.failures {
        let _ = writeln!(s, "  failure x{n}: {k}");
    }
    let _ = writeln!(s, "FL major versions: {:?}", t.versions);
    let _ = writeln!(
        s,
        "channels: probe reads {} event-64 blocks ({} rack, {} audio-clip sources, {} automation sources); imported {} Patternhaus channels",
        t.exp.channels,
        t.exp.rack_channels,
        t.exp.audio_sources,
        t.exp.automation_sources,
        t.got_channels
    );
    let _ = writeln!(s, "patterns imported: {}", t.got_patterns);
    let _ = writeln!(
        s,
        "notes: expected {}  imported {}  matched exactly {} ({})",
        t.exp.notes.len(),
        t.got_notes,
        t.matched_notes,
        pct(t.matched_notes, t.exp.notes.len() as u64)
    );
    let _ = writeln!(
        s,
        "clips: expected {}  imported {}  matched exactly {} ({});  expected on tracks 1-64: {}",
        t.exp.clips.len(),
        t.got_clips,
        t.matched_clips,
        pct(t.matched_clips, t.exp.clips.len() as u64),
        t.exp_clips_in_range
    );
    for (k, (ex, m)) in &t.clips_by_kind {
        let _ = writeln!(
            s,
            "  {k} clips: expected {ex}, matched {m} ({})",
            pct(*m, *ex)
        );
    }
    let _ = writeln!(s, "counts: {:?}", t.counts);
    let _ = writeln!(s, "top approximations:\n{}", top(&t.approx, 14));
    let _ = writeln!(s, "top skipped:\n{}", top(&t.skipped, 14));
    let mut un: Vec<_> = t.unread.iter().collect();
    un.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    let _ = writeln!(
        s,
        "events not interpreted (id: times): {}",
        un.iter()
            .take(40)
            .map(|(id, (n, _))| format!("{id}:{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    s
}
