//! Regions to the Haus Sampler model (`docs/formats/sfz.md` §4–§13).
//!
//! The model keeps envelopes, filter, LFOs, voice limits, round-robin mode, output and
//! effects per **group**, and modulation routes per instrument (optionally scoped to a
//! group), while SFZ sets all of them per region. Regions under one `<group>` header whose
//! group-level settings agree share a Haus group; regions that differ split into more groups
//! (`Name (2)`, ...), so nothing is averaged away.

use super::opcodes::{Leftover, leftover, lfo_rest};
use super::tree::{Curve, Ops, Parsed, Region};
use crate::notes::Notes;
use crate::probe::{SampleInfo, probe};
use crate::{Generated, ImportedSample, SampleSource};
use ph_core::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

// ------------------------------------------------------------------------- opcode access

struct Op<'a> {
    ops: &'a Ops,
    used: BTreeSet<String>,
    notes: &'a mut Notes,
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v}");
    s.trim_end_matches(".0").to_string()
}

impl<'a> Op<'a> {
    fn new(ops: &'a Ops, notes: &'a mut Notes) -> Self {
        Self {
            ops,
            used: BTreeSet::new(),
            notes,
        }
    }

    fn raw(&mut self, n: &str) -> Option<&'a str> {
        let v = self.ops.get(n)?;
        self.used.insert(n.to_string());
        Some(v.as_str())
    }

    fn has(&self, n: &str) -> bool {
        self.ops.contains_key(n)
    }

    /// A float, clamped to `lo..=hi` (reported), `None` when unset or unparseable (reported).
    fn num(&mut self, n: &str, lo: f64, hi: f64) -> Option<f64> {
        let v = self.raw(n)?;
        match v.trim().parse::<f64>() {
            Ok(x) if x.is_finite() => {
                if x < lo || x > hi {
                    self.notes.approx(
                        format!("opcode:{n}"),
                        format!(
                            "values outside {}..{} were clamped",
                            fmt_num(lo),
                            fmt_num(hi)
                        ),
                    );
                }
                Some(x.clamp(lo, hi))
            }
            _ => {
                self.notes.skip(
                    format!("opcode:{n}"),
                    format!("the value {v:?} does not parse; left unset"),
                );
                None
            }
        }
    }

    fn num_or(&mut self, n: &str, lo: f64, hi: f64, default: f64) -> f64 {
        self.num(n, lo, hi).unwrap_or(default)
    }

    /// An integer: a float value is truncated toward zero (reported).
    fn int(&mut self, n: &str, lo: f64, hi: f64) -> Option<i64> {
        let raw = self.ops.get(n).cloned();
        let x = self.num(n, lo, hi)?;
        if x.fract() != 0.0 && raw.as_deref().is_some_and(|r| r.contains('.')) {
            self.notes.approx(
                format!("opcode:{n}"),
                "a fractional value for a whole-number opcode was truncated",
            );
        }
        Some(x.trunc() as i64)
    }

    fn kw(&mut self, n: &str) -> Option<String> {
        self.raw(n).map(|v| v.trim().to_ascii_lowercase())
    }

    /// The numbers N of opcodes `<prefix>N`, ascending.
    fn numbered(&self, prefix: &str) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .ops
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .filter_map(|(k, _)| {
                let n = &k[prefix.len()..];
                if super::lex::is_digits(n) {
                    n.parse().ok()
                } else {
                    None
                }
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Reports a must opcode the model cannot represent, when it is set to something other
    /// than its default.
    fn gap(&mut self, n: &str, default: f64, what: &str) {
        if let Some(v) = self.num(n, f64::MIN, f64::MAX)
            && v != default
        {
            self.notes.skip(
                format!("opcode:{n}"),
                format!("the sampler has no {what} yet"),
            );
        }
    }
}

// ------------------------------------------------------------------------- samples

/// A resolved `sample` value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Res {
    File(PathBuf),
    Gen(Generated),
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/') || (p.len() >= 2 && p.as_bytes()[1] == b':')
}

/// Lexical normalisation (no file system access).
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `p` as it is on disk: each component matched exactly first, then ignoring case
/// (§3.4). The flag is true when a component's case differed. `dirs` caches listings.
fn find_on_disk(
    p: &Path,
    dirs: &mut BTreeMap<PathBuf, Vec<std::ffi::OsString>>,
) -> Option<(PathBuf, bool)> {
    let mut cur = PathBuf::new();
    let mut fixed = false;
    for c in p.components() {
        let Component::Normal(name) = c else {
            cur.push(c.as_os_str());
            continue;
        };
        let dir = if cur.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            cur.clone()
        };
        let names = dirs.entry(dir.clone()).or_insert_with(|| {
            std::fs::read_dir(&dir)
                .map(|r| r.flatten().map(|e| e.file_name()).collect())
                .unwrap_or_default()
        });
        if names.iter().any(|n| n == name) {
            cur.push(name);
            continue;
        }
        let want = name.to_string_lossy().to_lowercase();
        let hit = names
            .iter()
            .filter(|n| n.to_string_lossy().to_lowercase() == want)
            .min()?
            .clone();
        cur.push(hit);
        fixed = true;
    }
    cur.is_file().then_some((cur, fixed))
}

struct Samples<'a> {
    main_dir: Option<&'a Path>,
    probes: BTreeMap<PathBuf, Result<SampleInfo, String>>,
    dirs: BTreeMap<PathBuf, Vec<std::ffi::OsString>>,
}

impl Samples<'_> {
    fn resolve(&mut self, value: &str, default_path: &str, notes: &mut Notes) -> Option<Res> {
        let v = value.trim().replace('\\', "/");
        if let Some(gen_name) = v.strip_prefix('*') {
            return match gen_name {
                "sine" => Some(Res::Gen(Generated::Sine)),
                "silence" => Some(Res::Gen(Generated::Silence)),
                other => {
                    notes.skip(
                        format!("sample:*{other}"),
                        "built-in generator not supported; its regions were skipped",
                    );
                    None
                }
            };
        }
        let joined = format!("{default_path}{v}");
        let path = if is_absolute(&joined) {
            PathBuf::from(&joined)
        } else {
            match self.main_dir {
                Some(d) => d.join(&joined),
                None => {
                    notes.skip(
                        format!("sample:{joined}"),
                        "no folder to find samples in; its regions were skipped",
                    );
                    return None;
                }
            }
        };
        let path = normalise(&path);
        match find_on_disk(&path, &mut self.dirs) {
            Some((p, fixed)) => {
                if fixed {
                    notes.approx(
                        format!("sample:{joined}"),
                        "the file name matched only when ignoring case",
                    );
                }
                Some(Res::File(p))
            }
            None => {
                notes.skip(
                    format!("sample:{joined}"),
                    "missing sample file; its regions were skipped",
                );
                None
            }
        }
    }

    fn info(&mut self, r: &Res) -> Result<SampleInfo, String> {
        match r {
            Res::Gen(g) => Ok(g.info()),
            Res::File(p) => self
                .probes
                .entry(p.clone())
                .or_insert_with(|| probe(p))
                .clone(),
        }
    }
}

// ------------------------------------------------------------------------- curves

/// A route curve for `_curveccN = index` (§3.5): built-in 0, 4, 5 are exact; anything else is
/// approximated by the nearest of linear, convex and concave.
fn route_curve(
    index: Option<i64>,
    curves: &BTreeMap<u32, Curve>,
    notes: &mut Notes,
    op: &str,
) -> ModCurve {
    let Some(i) = index else {
        return ModCurve::Linear;
    };
    match i {
        0 => ModCurve::Linear,
        4 => ModCurve::Convex,
        5 => ModCurve::Concave,
        _ => {
            let mid = match i {
                1 => 0.0,
                2 | 3 => 0.5,
                6 => 0.707,
                _ => match curves.get(&(i as u32)) {
                    Some(c) => (c.0[63] + c.0[64]) / 2.0,
                    None => {
                        notes.skip(
                            format!("opcode:{op}"),
                            format!("curve {i} is not defined; linear used"),
                        );
                        return ModCurve::Linear;
                    }
                },
            };
            notes.approx(
                format!("opcode:{op}"),
                format!("curve {i} played as the nearest of linear, convex (x²) and concave (√x)"),
            );
            [
                (ModCurve::Convex, 0.25f32),
                (ModCurve::Linear, 0.5),
                (ModCurve::Concave, 0.707),
            ]
            .into_iter()
            .min_by(|a, b| (a.1 - mid).abs().total_cmp(&(b.1 - mid).abs()))
            .map(|x| x.0)
            .unwrap_or(ModCurve::Linear)
        }
    }
}

/// A modulation source for a CC number, extended CCs included (§4.7). The flag is true when
/// the mapping is an approximation.
fn cc_source(n: u32) -> Option<(ModSource, bool)> {
    Some(match n {
        0..=127 => (
            ModSource::Cc {
                controller: n as u8,
            },
            false,
        ),
        128 => (ModSource::PitchBend, false),
        129 => (ModSource::Aftertouch, false),
        130 => (ModSource::PolyPressure, false),
        131 => (ModSource::Velocity, false),
        133 => (ModSource::Key, true),
        135 => (ModSource::Random, false),
        136 => (ModSource::Random, true),
        _ => return None,
    })
}

// ------------------------------------------------------------------------- drafts

/// What decides a region's Haus group, besides its `<group>` header.
#[derive(Clone, Debug, PartialEq)]
struct GroupDraft {
    group: Group,
    routes: Vec<ModRoute>,
    seq_length: Option<u32>,
    random: bool,
    art_keys: Vec<u8>,
}

struct Draft {
    group_ix: usize,
    label: Option<String>,
    g: GroupDraft,
    zone: Zone,
    sample: Res,
    rand: Option<(f32, f32)>,
    sw_default: Option<u8>,
    bend: Option<(f32, f32)>,
}

struct Mapper<'a> {
    notes: &'a mut Notes,
    curves: &'a BTreeMap<u32, Curve>,
    effects: &'a [MappedEffect],
    samples: Samples<'a>,
    /// Exclusive-group ids some region's `off_by` names.
    chokers: BTreeSet<i64>,
}

/// An `<effect>` block that maps to a built-in effect.
pub struct MappedEffect {
    name: &'static str,
    /// `None` = the main bus (every region); `Some(n)` = the `fx<n>` bus, fed by `effect<n>`.
    bus: Option<u32>,
}

fn route(
    routes: &mut Vec<ModRoute>,
    source: ModSource,
    target: ModTarget,
    depth: f64,
    curve: ModCurve,
) {
    if depth != 0.0 {
        routes.push(ModRoute {
            source,
            target,
            depth: depth as f32,
            curve,
            group: None,
        });
    }
}

/// `<prefix>N` CC-modulation opcodes (`gain_oncc`, ...) to routes.
fn cc_routes(
    o: &mut Op<'_>,
    curves: &BTreeMap<u32, Curve>,
    routes: &mut Vec<ModRoute>,
    prefix: &str,
    curve_prefix: &str,
    target: ModTarget,
    scale: f64,
    range: f64,
) {
    for n in o.numbered(prefix) {
        let name = format!("{prefix}{n}");
        let Some(depth) = o.num(&name, -range, range) else {
            continue;
        };
        let curve_op = format!("{curve_prefix}{n}");
        let ci = if o.has(&curve_op) {
            o.int(&curve_op, 0.0, 255.0)
        } else {
            None
        };
        let curve = route_curve(ci, curves, o.notes, &curve_op);
        match cc_source(n) {
            Some((src, approx)) => {
                if approx {
                    o.notes.approx(
                        format!("opcode:{name}"),
                        "extended controller played through the nearest sampler source",
                    );
                }
                route(routes, src, target, depth * scale, curve);
            }
            None => o.notes.skip(
                format!("opcode:{name}"),
                "extended controller the sampler does not have",
            ),
        }
    }
}

fn envelope(o: &mut Op<'_>, p: &str) -> Envelope {
    let mut e = Envelope {
        delay: o.num_or(&format!("{p}_delay"), 0.0, 60.0, 0.0) as f32,
        attack: o.num_or(&format!("{p}_attack"), 0.0, 60.0, 0.0) as f32,
        hold: o.num_or(&format!("{p}_hold"), 0.0, 60.0, 0.0) as f32,
        decay: o.num_or(&format!("{p}_decay"), 0.0, 60.0, 0.0) as f32,
        sustain: (o.num_or(&format!("{p}_sustain"), 0.0, 100.0, 100.0) / 100.0) as f32,
        release: o.num_or(&format!("{p}_release"), 0.0, 60.0, 0.001) as f32,
    };
    for stage in [
        "delay", "attack", "hold", "decay", "sustain", "release", "depth",
    ] {
        o.gap(
            &format!("{p}_vel2{stage}"),
            0.0,
            "velocity-scaled envelope stages",
        );
    }
    o.gap(&format!("{p}_start"), 0.0, "envelope start levels");
    e.release = e.release.max(0.0);
    e
}

fn lfo(o: &mut Op<'_>, freq: &str, delay: &str, fade: &str, shape: LfoShape) -> Option<Lfo> {
    let rate = o.num(freq, 0.0, 100.0)?;
    let delay = o.num_or(delay, 0.0, 60.0, 0.0) as f32;
    let fade = o.num_or(fade, 0.0, 60.0, 0.0) as f32;
    if rate < 0.01 {
        return None;
    }
    Some(Lfo {
        shape,
        rate: rate as f32,
        delay,
        fade,
    })
}

impl Mapper<'_> {
    fn region(&mut self, r: &Region) -> Option<Draft> {
        let ops = &r.ops;
        let curves = self.curves;
        let mut o = Op::new(ops, self.notes);
        let mut routes = Vec::new();
        let mut group = Group::new("");

        // ---- sample
        let Some(sample_value) = o.raw("sample") else {
            o.notes
                .skip("region:no-sample", "a region without a sample was skipped");

            return None;
        };
        let res = self
            .samples
            .resolve(sample_value, &r.default_path, o.notes)?;
        let info = match self.samples.info(&res) {
            Ok(i) => i,
            Err(e) => {
                o.notes.skip(
                    format!("sample:{}", sample_value.trim()),
                    format!("the sample cannot be read ({e}); its regions were skipped"),
                );
                return None;
            }
        };

        // ---- mapping and conditions (§4.1, §4.5)
        if o.raw("lokey") == Some("-1") || o.raw("hikey") == Some("-1") {
            o.notes.skip(
                "region:lokey=-1",
                "regions reached only through controller triggers were skipped",
            );
            return None;
        }
        let key = |o: &mut Op<'_>, n: &str, d: i64| {
            o.raw(n)
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(d)
                .clamp(0, 127) as u8
        };
        let (klo, khi) = (key(&mut o, "lokey", 0), key(&mut o, "hikey", 127));
        let vlo = o.int("lovel", 0.0, 127.0).unwrap_or(1);
        let vhi = o.int("hivel", 0.0, 127.0).unwrap_or(127);
        if klo > khi || vlo > vhi {
            o.notes.skip(
                "region:empty-range",
                "a region with an empty key or velocity range was skipped",
            );
            return None;
        }
        let (clo, chi) = (
            o.int("lochan", 1.0, 16.0).unwrap_or(1),
            o.int("hichan", 1.0, 16.0).unwrap_or(16),
        );
        if !(clo..=chi).contains(&1) {
            o.notes.skip(
                "opcode:lochan",
                "regions for MIDI channels other than 1 were skipped (the sampler plays channel 1)",
            );
            return None;
        }
        if !o.numbered("on_locc").is_empty()
            || !o.numbered("on_hicc").is_empty()
            || !o.numbered("on_lohdcc").is_empty()
            || !o.numbered("on_hihdcc").is_empty()
        {
            o.notes.skip(
                "opcode:on_loccN",
                "controller-triggered regions were skipped: the sampler triggers zones from notes only",
            );
            return None;
        }

        let mut zone = Zone::new(
            GroupId(0),
            SampleRef::File(String::new()),
            KeyRange { lo: klo, hi: khi },
            60,
        );
        zone.velocity = VelRange {
            lo: if vlo <= 1 { 0 } else { vlo as u8 },
            hi: vhi as u8,
        };

        // ---- trigger (§4.2)
        let trig = o.kw("trigger");
        match trig.as_deref() {
            None | Some("attack") => {}
            Some("release") | Some("release_key") => zone.trigger = ZoneTrigger::Release,
            Some(t @ ("first" | "legato")) => o.notes.approx(
                format!("opcode:trigger={t}"),
                "played as an attack trigger: the sampler has no first/legato note conditions",
            ),
            Some(t) => o.notes.skip(
                format!("opcode:trigger={t}"),
                "unknown trigger mode; played as attack",
            ),
        }
        zone.release_decay_db = o.num_or("rt_decay", 0.0, 200.0, 0.0) as f32;
        let _ = o.kw("rt_dead");
        if o.has("rt_dead") {
            o.notes.skip(
                "opcode:rt_dead",
                "accepted and ignored: a release region plays on its own",
            );
        }

        // ---- round robin and random (§4.3)
        let seq_length = o.int("seq_length", 1.0, 100.0).map(|v| v as u32);
        let seq_pos = o.int("seq_position", 1.0, 100.0).map(|v| v as u32);
        let lr = o.num("lorand", 0.0, 1.0);
        let hr = o.num("hirand", 0.0, 1.0);
        let rand = match (lr, hr) {
            (None, None) => None,
            (a, b) => Some((a.unwrap_or(0.0) as f32, b.unwrap_or(1.0) as f32)),
        };
        let rand = rand.filter(|(a, b)| !(*a <= 0.0 && *b >= 1.0));
        let (seq_length, rand) = match (seq_length.filter(|l| *l > 1), rand) {
            (Some(l), Some(_)) => {
                o.notes.skip(
                    "opcode:lorand",
                    "random layers in a region that also has a sequence; the sequence wins",
                );
                (Some(l), None)
            }
            (l, r) => (l, r),
        };
        if let Some(l) = seq_length {
            let pos = seq_pos.unwrap_or(1);
            if pos > l {
                o.notes.approx(
                    "opcode:seq_position",
                    "a position beyond seq_length was kept; the cycle grows to include it",
                );
            }
            zone.round_robin = Some(pos - 1);
        }

        // ---- keyswitches (§4.4)
        let _ = (o.raw("sw_lokey"), o.raw("sw_hikey"), o.raw("sw_label"));
        let mut art_keys = Vec::new();
        if let Some(k) = o.raw("sw_last").and_then(|v| v.parse::<u8>().ok()) {
            art_keys.push(k);
        }
        let lolast = o.raw("sw_lolast").and_then(|v| v.parse::<u8>().ok());
        let hilast = o.raw("sw_hilast").and_then(|v| v.parse::<u8>().ok());
        if art_keys.is_empty()
            && let (Some(a), Some(b)) = (lolast.or(hilast), hilast.or(lolast))
        {
            art_keys.extend(a.min(b)..=a.max(b));
        }
        let sw_default = o.raw("sw_default").and_then(|v| v.parse::<u8>().ok());
        for n in ["sw_down", "sw_up", "sw_previous"] {
            if o.raw(n).is_some() {
                o.notes.skip(
                    format!("opcode:{n}"),
                    "the sampler has no held-key or previous-note conditions yet; the region plays without it",
                );
            }
        }

        // ---- CC conditions (§4.5)
        let mut ccs: BTreeMap<u32, (f32, f32)> = BTreeMap::new();
        for n in o.numbered("locc").into_iter().chain(o.numbered("hicc")) {
            let lo = o.num(&format!("locc{n}"), 0.0, 127.0).unwrap_or(0.0) / 127.0;
            let hi = o.num(&format!("hicc{n}"), 0.0, 127.0).unwrap_or(127.0) / 127.0;
            ccs.insert(n, (lo as f32, hi as f32));
        }
        for n in o.numbered("lohdcc").into_iter().chain(o.numbered("hihdcc")) {
            let lo = o.num(&format!("lohdcc{n}"), 0.0, 1.0).unwrap_or(0.0);
            let hi = o.num(&format!("hihdcc{n}"), 0.0, 1.0).unwrap_or(1.0);
            ccs.insert(n, (lo as f32, hi as f32));
        }
        for (n, (lo, hi)) in ccs {
            if lo <= 0.0 && hi >= 1.0 {
                continue;
            }
            if n > 127 {
                o.notes.skip(
                    format!("opcode:locc{n}"),
                    "conditions on extended controllers are not supported; the region plays without it",
                );
            } else if lo > hi {
                o.notes.skip(
                    "region:empty-cc-range",
                    "a region with an empty controller range was skipped",
                );
                return None;
            } else {
                zone.cc_conditions.push(CcCondition {
                    controller: n as u8,
                    lo,
                    hi,
                });
            }
        }
        o.gap("lobend", -8192.0, "pitch-bend conditions");
        o.gap("hibend", 8192.0, "pitch-bend conditions");
        o.gap("lochanaft", 0.0, "aftertouch conditions");
        o.gap("hichanaft", 127.0, "aftertouch conditions");
        if let Some(v) = o.kw("sustain_sw")
            && v != "on"
        {
            o.notes.skip(
                "opcode:sustain_sw",
                "the sampler has no per-region sustain pedal switch yet",
            );
        }
        o.gap("sustain_cc", 64.0, "choice of sustain controller");
        o.gap("sustain_lo", 1.0, "sustain pedal threshold");

        // ---- sample playback (§5)
        let start = o.int("offset", 0.0, 4_294_967_295.0).unwrap_or(0).max(0) as u64;
        if start >= info.frames {
            o.notes.skip(
                "opcode:offset",
                "a region starting past its sample's end was skipped",
            );
            return None;
        }
        zone.start = start;
        if let Some(end) = o.int("end", -1.0, 4_294_967_295.0) {
            if end <= 0 {
                o.notes.skip(
                    "opcode:end",
                    "regions with end=0 or end=-1 are silent and were skipped",
                );
                return None;
            }
            let e = (end as u64 + 1).min(info.frames);
            if e <= start {
                o.notes
                    .skip("opcode:end", "a region ending before it starts was skipped");
                return None;
            }
            if e < info.frames {
                zone.end = Some(e);
            }
        }
        let play_end = zone.end.unwrap_or(info.frames);
        let rand_off = o.num_or("offset_random", 0.0, 4_294_967_295.0, 0.0);
        route(
            &mut routes,
            ModSource::Random,
            ModTarget::SampleStart,
            rand_off,
            ModCurve::Linear,
        );
        cc_routes(
            &mut o,
            curves,
            &mut routes,
            "offset_oncc",
            "offset_curvecc",
            ModTarget::SampleStart,
            1.0,
            4_294_967_295.0,
        );
        if let Some(d) = o.num("delay", 0.0, 60.0)
            && d > 0.0
        {
            group.amp_env.delay = d as f32;
            o.notes.approx(
                "opcode:delay",
                "the start delay is played as the amp envelope's delay",
            );
        }
        o.gap("delay_random", 0.0, "random start delays");
        if let Some(d) = o.kw("direction") {
            match d.as_str() {
                "reverse" => zone.reverse = true,
                "forward" => {}
                other => o.notes.skip(
                    format!("opcode:direction={other}"),
                    "unknown direction; played forward",
                ),
            }
        }
        if o.int("count", 0.0, 4_294_967_295.0).is_some_and(|c| c > 0) {
            zone.one_shot = true;
            o.notes
                .approx("opcode:count", "played once as a one-shot (no repeats)");
        }

        // ---- loops (§5.1)
        let mode = o.kw("loop_mode");
        let mode = match mode.as_deref() {
            Some(m @ ("no_loop" | "one_shot" | "loop_continuous" | "loop_sustain")) => {
                m.to_string()
            }
            Some(other) => {
                o.notes.skip(
                    format!("opcode:loop_mode={other}"),
                    "unknown loop mode; the file's own loop setting is used",
                );
                default_loop_mode(&zone, &info)
            }
            None => default_loop_mode(&zone, &info),
        };
        match mode.as_str() {
            "one_shot" => zone.one_shot = true,
            "loop_continuous" | "loop_sustain" => {
                let ls = o
                    .int("loop_start", 0.0, 4_294_967_295.0)
                    .map(|v| v as u64)
                    .or(info.loop_range.map(|l| l.0))
                    .unwrap_or(0);
                let le = o
                    .int("loop_end", 0.0, 4_294_967_295.0)
                    .map(|v| v as u64 + 1)
                    .or(info.loop_range.map(|l| l.1))
                    .unwrap_or(info.frames);
                let (ls2, le2) = (ls.max(start), le.min(play_end));
                if (ls2, le2) != (ls, le) {
                    o.notes.approx(
                        "opcode:loop_start",
                        "a loop reaching outside the played range was cut to fit",
                    );
                }
                if le2 <= ls2 {
                    o.notes.skip(
                        "opcode:loop_end",
                        "an empty loop was dropped; the region plays unlooped",
                    );
                } else {
                    let mode = match o.kw("loop_type").as_deref() {
                        None | Some("forward") => LoopMode::Forward,
                        Some("alternate") => LoopMode::PingPong,
                        Some(t) => {
                            o.notes.approx(
                                format!("opcode:loop_type={t}"),
                                "played as a forward loop",
                            );
                            LoopMode::Forward
                        }
                    };
                    let xf = o.num_or("loop_crossfade", 0.0, 60.0, 0.0);
                    let xf = ((xf * info.sample_rate as f64).round() as u64).min(le2 - ls2);
                    zone.looping = Some(ZoneLoop {
                        mode,
                        start: ls2,
                        end: le2,
                        crossfade: xf,
                        on_release: LoopRelease::Continue,
                    });
                }
            }
            _ => {}
        }
        // (loop_mode resolved above; mark the loop opcodes read even when unused)
        let _ = (
            o.raw("loop_start"),
            o.raw("loop_end"),
            o.raw("loop_type"),
            o.raw("loop_crossfade"),
        );
        if mode == "loop_sustain"
            && let Some(l) = &mut zone.looping
        {
            l.on_release = LoopRelease::Exit;
        }

        // ---- amplifier (§6)
        let mut vol = 0.0f64;
        for n in ["volume", "global_volume", "master_volume", "group_volume"] {
            vol += o.num_or(n, -144.0, 48.0, 0.0);
        }
        for n in [
            "amplitude",
            "global_amplitude",
            "master_amplitude",
            "group_amplitude",
        ] {
            let a = o.num_or(n, 0.0, 100.0, 100.0);
            vol += if a <= 0.0 {
                -144.0
            } else {
                20.0 * (a / 100.0).log10()
            };
        }
        zone.pan = (o.num_or("pan", -100.0, 100.0, 0.0) / 100.0) as f32;
        o.gap("width", 100.0, "stereo width control");
        o.gap("position", 0.0, "stereo position control");
        let akc = o
            .raw("amp_keycenter")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(60.0);
        let akt = o.num_or("amp_keytrack", -96.0, 12.0, 0.0);
        if akt != 0.0 {
            vol += akt * (60.0 - akc);
            route(
                &mut routes,
                ModSource::Key,
                ModTarget::Volume,
                akt * 64.0,
                ModCurve::Linear,
            );
        }
        let vt = o.num("amp_veltrack", -100.0, 100.0);
        let has_curve = !o.numbered("amp_velcurve_").is_empty();
        for n in o.numbered("amp_velcurve_") {
            let _ = o.raw(&format!("amp_velcurve_{n}"));
        }
        if vt.is_some_and(|v| v != 100.0) || has_curve {
            o.notes.approx(
                "opcode:amp_veltrack",
                "velocity sensitivity and custom velocity curves follow the sampler's own velocity response",
            );
        }
        let ar = o.num_or("amp_random", 0.0, 24.0, 0.0);
        route(
            &mut routes,
            ModSource::Random,
            ModTarget::Volume,
            -ar,
            ModCurve::Linear,
        );
        cc_routes(
            &mut o,
            curves,
            &mut routes,
            "gain_oncc",
            "gain_curvecc",
            ModTarget::Volume,
            1.0,
            144.0,
        );
        cc_routes(
            &mut o,
            curves,
            &mut routes,
            "pan_oncc",
            "pan_curvecc",
            ModTarget::Pan,
            0.01,
            200.0,
        );
        for n in o.numbered("amplitude_oncc") {
            let _ = o.raw(&format!("amplitude_oncc{n}"));
            let _ = o.raw(&format!("amplitude_curvecc{n}"));
            o.notes.skip(
                format!("opcode:amplitude_oncc{n}"),
                "the sampler has no linear-gain controller modulation yet (use gain_onccN)",
            );
        }
        if vol < -144.0 || vol > 24.0 {
            o.notes.approx(
                "opcode:volume",
                "a total level outside -144..24 dB was clamped",
            );
        }
        zone.volume_db = vol.clamp(-144.0, 24.0) as f32;
        if let Some(out) = o.int("output", 0.0, 1024.0) {
            if out < MAX_OUTPUT_BUSES as i64 {
                group.output = out as u32;
            } else {
                o.notes.approx(
                    "opcode:output",
                    format!(
                        "outputs above {} play on the main output",
                        MAX_OUTPUT_BUSES - 1
                    ),
                );
            }
        }

        // ---- crossfades (§6.1)
        let curve = |o: &mut Op<'_>, n: &str| match o.kw(n).as_deref() {
            Some("gain") => CrossfadeCurve::Gain,
            _ => CrossfadeCurve::Power,
        };
        let vin = (
            o.num_or("xfin_lovel", 0.0, 127.0, 0.0),
            o.num_or("xfin_hivel", 0.0, 127.0, 0.0),
        );
        let vout = (
            o.num_or("xfout_lovel", 0.0, 127.0, 127.0),
            o.num_or("xfout_hivel", 0.0, 127.0, 127.0),
        );
        let vc = curve(&mut o, "xf_velcurve");
        if let Some(x) = crossfade(CrossfadeSource::Velocity, vin, vout, vc, o.notes) {
            zone.crossfades.push(x);
        }
        let mut xf_ccs: BTreeSet<u32> = BTreeSet::new();
        for p in ["xfin_locc", "xfin_hicc", "xfout_locc", "xfout_hicc"] {
            xf_ccs.extend(o.numbered(p));
        }
        let cc_curve = curve(&mut o, "xf_cccurve");
        for n in xf_ccs {
            let i = (
                o.num_or(&format!("xfin_locc{n}"), 0.0, 127.0, 0.0),
                o.num_or(&format!("xfin_hicc{n}"), 0.0, 127.0, 0.0),
            );
            let out = (
                o.num_or(&format!("xfout_locc{n}"), 0.0, 127.0, 127.0),
                o.num_or(&format!("xfout_hicc{n}"), 0.0, 127.0, 127.0),
            );
            if n > 127 {
                o.notes.skip(
                    format!("opcode:xfin_locc{n}"),
                    "crossfades on extended controllers are not supported",
                );
                continue;
            }
            if let Some(x) = crossfade(
                CrossfadeSource::Cc {
                    controller: n as u8,
                },
                i,
                out,
                cc_curve,
                o.notes,
            ) {
                zone.crossfades.push(x);
            }
        }
        let _ = curve(&mut o, "xf_keycurve");
        o.gap("xfin_lokey", 0.0, "key crossfades");
        o.gap("xfin_hikey", 0.0, "key crossfades");
        o.gap("xfout_lokey", 127.0, "key crossfades");
        o.gap("xfout_hikey", 127.0, "key crossfades");

        // ---- pitch (§7)
        let root = match o.raw("pitch_keycenter") {
            Some("sample") => info.root_key.map(i64::from).unwrap_or(60),
            Some(v) => v.parse::<i64>().unwrap_or(60),
            None => 60,
        };
        let root = root.clamp(0, 127);
        zone.root_key = root as u8;
        let mut cents = o.int("transpose", -127.0, 127.0).unwrap_or(0) as f64 * 100.0;
        for n in ["tune", "global_tune", "master_tune", "group_tune"] {
            cents += o.int(n, -9600.0, 9600.0).unwrap_or(0) as f64;
        }
        if let Res::Gen(Generated::Sine) = res {
            cents += Generated::sine_correction_cents();
        }
        let kt = o.num_or("pitch_keytrack", -1200.0, 1200.0, 100.0);
        if kt != 100.0 {
            cents += (kt - 100.0) * (60.0 - root as f64);
            route(
                &mut routes,
                ModSource::Key,
                ModTarget::Pitch,
                (kt - 100.0) * 64.0,
                ModCurve::Linear,
            );
        }
        let pv = o.num_or("pitch_veltrack", -9600.0, 9600.0, 0.0);
        route(
            &mut routes,
            ModSource::Velocity,
            ModTarget::Pitch,
            pv,
            ModCurve::Linear,
        );
        let pr = o.num_or("pitch_random", 0.0, 9600.0, 0.0);
        if pr > 0.0 {
            cents -= pr / 2.0;
            route(
                &mut routes,
                ModSource::Random,
                ModTarget::Pitch,
                pr,
                ModCurve::Linear,
            );
        }
        cc_routes(
            &mut o,
            curves,
            &mut routes,
            "pitch_oncc",
            "pitch_curvecc",
            ModTarget::Pitch,
            1.0,
            9600.0,
        );
        let semis = (cents / 100.0).round();
        let tune = semis.clamp(-48.0, 48.0);
        if tune != semis {
            o.notes.approx(
                "opcode:transpose",
                "a total transpose beyond ±48 semitones was clamped",
            );
        }
        zone.tune = tune as i32;
        zone.fine = (cents - semis * 100.0).clamp(-100.0, 100.0) as f32;
        let bu = o.num("bend_up", -9600.0, 9600.0);
        let bd = o.num("bend_down", -9600.0, 9600.0);
        let bend = (bu.is_some() || bd.is_some()).then(|| {
            (
                (bu.unwrap_or(200.0).abs() / 100.0).min(48.0) as f32,
                (bd.unwrap_or(-200.0).abs() / 100.0).min(48.0) as f32,
            )
        });

        // ---- envelopes (§8)
        let delay = group.amp_env.delay;
        group.amp_env = envelope(&mut o, "ampeg");
        group.amp_env.delay = (group.amp_env.delay + delay).min(60.0);

        // ---- filter (§10)
        let fil_type = o.kw("fil_type");
        if let Some(cutoff) = o.num("cutoff", 0.0, 1.0e6) {
            let kind = match fil_type.as_deref().unwrap_or("lpf_2p") {
                "lpf_2p" | "lpf_2p_sv" => Some((FilterKind::LowPass12, false)),
                "lpf_1p" => Some((FilterKind::LowPass12, true)),
                "hpf_2p" | "hpf_2p_sv" => Some((FilterKind::HighPass12, false)),
                "hpf_1p" => Some((FilterKind::HighPass12, true)),
                "bpf_2p" | "bpf_2p_sv" => Some((FilterKind::BandPass, false)),
                "bpf_1p" => Some((FilterKind::BandPass, true)),
                "brf_2p" | "brf_2p_sv" => Some((FilterKind::Notch, false)),
                "brf_1p" => Some((FilterKind::Notch, true)),
                "lpf_4p" => Some((FilterKind::LowPass24, false)),
                "hpf_4p" => Some((FilterKind::HighPass24, false)),
                "lpf_6p" => Some((FilterKind::LowPass24, true)),
                "hpf_6p" => Some((FilterKind::HighPass24, true)),
                _ => None,
            };
            let t = fil_type.clone().unwrap_or_else(|| "lpf_2p".into());
            match kind {
                None => o.notes.skip(
                    format!("opcode:fil_type={t}"),
                    "a filter type the sampler does not have; the filter is bypassed",
                ),
                Some((kind, approx)) => {
                    if approx {
                        o.notes.approx(
                            format!("opcode:fil_type={t}"),
                            "played with the nearest slope the sampler has (12 or 24 dB/octave)",
                        );
                    }
                    let fkt = o.num_or("fil_keytrack", 0.0, 1200.0, 0.0);
                    let fkc = o
                        .raw("fil_keycenter")
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(60.0);
                    let cutoff = cutoff * 2f64.powf(fkt * (60.0 - fkc) / 1200.0);
                    if !(10.0..=22_000.0).contains(&cutoff) {
                        o.notes
                            .approx("opcode:cutoff", "cutoff outside 10..22000 Hz was clamped");
                    }
                    let mut f = Filter {
                        kind,
                        cutoff: cutoff.clamp(10.0, 22_000.0) as f32,
                        resonance: o.num_or("resonance", 0.0, 40.0, 0.0) as f32,
                        key_track: fkt as f32,
                        env_depth: 0.0,
                    };
                    let fv = o.num_or("fil_veltrack", -9600.0, 9600.0, 0.0);
                    route(
                        &mut routes,
                        ModSource::Velocity,
                        ModTarget::Cutoff,
                        fv,
                        ModCurve::Linear,
                    );
                    let fr = o.num_or("fil_random", 0.0, 9600.0, 0.0);
                    // SPEC-GAP: §10 does not say whether fil_random is bipolar; read as 0..value.
                    route(
                        &mut routes,
                        ModSource::Random,
                        ModTarget::Cutoff,
                        fr,
                        ModCurve::Linear,
                    );
                    cc_routes(
                        &mut o,
                        curves,
                        &mut routes,
                        "cutoff_oncc",
                        "cutoff_curvecc",
                        ModTarget::Cutoff,
                        1.0,
                        9600.0,
                    );
                    cc_routes(
                        &mut o,
                        curves,
                        &mut routes,
                        "resonance_oncc",
                        "resonance_curvecc",
                        ModTarget::Resonance,
                        1.0,
                        40.0,
                    );
                    let fdepth = o.num_or("fileg_depth", -12000.0, 12000.0, 0.0);
                    let any_fileg = ops.keys().any(|k| k.starts_with("fileg_"));
                    if any_fileg {
                        group.filter_env = Some(envelope(&mut o, "fileg"));
                        f.env_depth = fdepth.clamp(-9600.0, 9600.0) as f32;
                    }
                    group.filter = Some(f);
                }
            }
        }
        if o.has("cutoff2") || o.has("fil2_type") || o.has("resonance2") {
            for n in [
                "cutoff2",
                "fil2_type",
                "resonance2",
                "fil2_keytrack",
                "fil2_keycenter",
                "fil2_veltrack",
            ] {
                let _ = o.raw(n);
            }
            o.notes.skip(
                "opcode:cutoff2",
                "the sampler has one filter per group; the second filter is bypassed",
            );
        }
        // pitch envelope: through the second envelope when the filter does not use it
        let pdepth = o.num_or("pitcheg_depth", -12000.0, 12000.0, 0.0);
        if pdepth != 0.0 {
            if group.filter_env.is_none() {
                group.filter_env = Some(envelope(&mut o, "pitcheg"));
                route(
                    &mut routes,
                    ModSource::Envelope { index: 1 },
                    ModTarget::Pitch,
                    pdepth,
                    ModCurve::Linear,
                );
                o.notes.approx(
                    "opcode:pitcheg_depth",
                    "the pitch envelope plays through the group's second envelope",
                );
            } else {
                o.notes.skip(
                    "opcode:pitcheg_depth",
                    "the group's second envelope already drives the filter; the pitch envelope is not played",
                );
            }
        }

        // ---- LFOs (§9)
        let mut lfos: Vec<(Lfo, Vec<(ModTarget, f64)>)> = Vec::new();
        for (fam, target, range) in [
            ("amplfo", ModTarget::Volume, 10.0),
            ("fillfo", ModTarget::Cutoff, 1200.0),
            ("pitchlfo", ModTarget::Pitch, 1200.0),
        ] {
            let depth = o.num_or(&format!("{fam}_depth"), -range, range, 0.0);
            let l = lfo(
                &mut o,
                &format!("{fam}_freq"),
                &format!("{fam}_delay"),
                &format!("{fam}_fade"),
                LfoShape::Sine,
            );
            if let Some(l) = l
                && depth != 0.0
            {
                lfos.push((l, vec![(target, depth)]));
            }
        }
        let mut lfo_ns: BTreeSet<u32> = BTreeSet::new();
        for k in ops.keys() {
            if let Some((n, _)) = lfo_rest(k) {
                lfo_ns.insert(n);
            }
        }
        for n in lfo_ns {
            let p = format!("lfo{n}_");
            let wave = o.int(&format!("{p}wave"), -1.0, 255.0).unwrap_or(1);
            let shape = match wave {
                0 => LfoShape::Triangle,
                1 => LfoShape::Sine,
                3 => LfoShape::Square,
                6 => LfoShape::SawUp,
                7 => LfoShape::SawDown,
                12 => LfoShape::Random,
                2 | 4 | 5 => {
                    o.notes.approx(
                        format!("opcode:{p}wave={wave}"),
                        "pulse waves are played as a square wave",
                    );
                    LfoShape::Square
                }
                _ => {
                    o.notes.approx(
                        format!("opcode:{p}wave={wave}"),
                        "an LFO wave the sampler does not have; played as a sine",
                    );
                    LfoShape::Sine
                }
            };
            o.gap(&format!("{p}phase"), 0.0, "LFO start phases");
            let mut targets = Vec::new();
            for (t, target, scale, range) in [
                ("pitch", ModTarget::Pitch, 1.0, 9600.0),
                ("volume", ModTarget::Volume, 1.0, 144.0),
                ("pan", ModTarget::Pan, 0.01, 200.0),
                ("cutoff", ModTarget::Cutoff, 1.0, 9600.0),
                ("resonance", ModTarget::Resonance, 1.0, 40.0),
            ] {
                let d = o.num_or(&format!("{p}{t}"), -range, range, 0.0);
                if d != 0.0 {
                    targets.push((target, d * scale));
                }
                for m in o.numbered(&format!("{p}{t}_oncc")) {
                    let _ = o.raw(&format!("{p}{t}_oncc{m}"));
                    o.notes.skip(
                        format!("opcode:{p}{t}_onccN"),
                        "the sampler cannot scale an LFO's depth by a controller yet",
                    );
                }
            }
            o.gap(&format!("{p}amplitude"), 0.0, "linear-gain LFO depth");
            o.gap(&format!("{p}width"), 0.0, "LFO width modulation");
            let l = lfo(
                &mut o,
                &format!("{p}freq"),
                &format!("{p}delay"),
                &format!("{p}fade"),
                shape,
            );
            if let Some(l) = l
                && !targets.is_empty()
            {
                lfos.push((l, targets));
            }
        }
        if lfos.len() > 4 {
            o.notes.skip(
                "lfo:more-than-4",
                "the sampler has four LFOs per group; the rest were dropped",
            );
            lfos.truncate(4);
        }
        for (i, (l, targets)) in lfos.into_iter().enumerate() {
            group.lfos.push(l);
            for (t, d) in targets {
                route(
                    &mut routes,
                    ModSource::Lfo { index: i as u8 },
                    t,
                    d,
                    ModCurve::Linear,
                );
            }
        }

        // ---- voices (§11)
        if let Some(p) = o.int("polyphony", 0.0, 1024.0)
            && p > 0
        {
            group.voice_limit = Some(p as u32);
        }
        o.gap("note_polyphony", 0.0, "per-key voice limits");
        let excl = o.int("group", -1e9, 1e9).unwrap_or(0);
        let off_by = o.int("off_by", -1e9, 1e9);
        let _ = (o.kw("off_mode"), o.num("off_time", 0.0, 60.0));
        let choke = match off_by {
            Some(b) => Some(b.min(excl)),
            None => self.chokers.contains(&excl).then_some(excl),
        };

        // ---- effect sends (§12)
        for e in self.effects {
            match e.bus {
                None => group.effects.push(GroupEffect {
                    name: e.name.into(),
                    params: BTreeMap::new(),
                    bypass: false,
                    mix: 1.0,
                }),
                Some(n) => {
                    let send = o.num_or(&format!("effect{n}"), 0.0, 100.0, 0.0);
                    if send > 0.0 {
                        group.effects.push(GroupEffect {
                            name: e.name.into(),
                            params: BTreeMap::new(),
                            bypass: false,
                            mix: (send / 100.0) as f32,
                        });
                    }
                }
            }
        }

        // ---- labels
        let label = ["group_label", "master_label", "global_label"]
            .into_iter()
            .find_map(|n| o.raw(n).map(str::to_string));

        // remember the choke class through the voice limit and a marker in `extra`
        if let Some(c) = choke {
            group.voice_limit = Some(1);
            group.extra.insert("sfz.choke".into(), c.to_string());
            o.notes.approx(
                "opcode:off_by",
                "exclusive groups (hi-hat chokes) play one voice at a time per group",
            );
        }

        finish_unused(&mut o);
        Some(Draft {
            group_ix: r.group_ix,
            label,
            g: GroupDraft {
                group,
                routes,
                seq_length,
                random: rand.is_some(),
                art_keys,
            },
            zone,
            sample: res,
            rand,
            sw_default,
            bend,
        })
    }
}

fn default_loop_mode(zone: &Zone, info: &SampleInfo) -> String {
    if zone.trigger == ZoneTrigger::Release {
        "one_shot".into()
    } else if info.loop_range.is_some() {
        "loop_continuous".into()
    } else {
        "no_loop".into()
    }
}

/// A crossfade from SFZ in/out ranges (0..=127), or `None` when it has no effect.
fn crossfade(
    source: CrossfadeSource,
    (ilo, ihi): (f64, f64),
    (olo, ohi): (f64, f64),
    curve: CrossfadeCurve,
    notes: &mut Notes,
) -> Option<Crossfade> {
    if ilo <= 0.0 && ihi <= 0.0 && olo >= 127.0 && ohi >= 127.0 {
        return None;
    }
    let a = ilo.min(ihi);
    let b = ihi.max(a);
    let c = olo.max(b);
    let d = ohi.max(c);
    if (a, b, c, d) != (ilo, ihi, olo, ohi) {
        notes.approx(
            "opcode:xfin_lo",
            "overlapping crossfade ranges were put in order",
        );
    }
    Some(Crossfade {
        source,
        in_lo: (a / 127.0) as f32,
        in_hi: (b / 127.0) as f32,
        out_lo: (c / 127.0) as f32,
        out_hi: (d / 127.0) as f32,
        curve,
    })
}

/// Reports every opcode of a region the mapper did not read (§15).
fn finish_unused(o: &mut Op<'_>) {
    let unused: Vec<String> = o
        .ops
        .keys()
        .filter(|k| !o.used.contains(*k))
        .cloned()
        .collect();
    for k in unused {
        match leftover(&k) {
            Leftover::Skip(why) => o.notes.skip(format!("opcode:{}", family(&k)), why),
            Leftover::Data => o.notes.kept(
                format!("opcode:{}", family(&k)),
                "kept as data in the instrument",
            ),
            Leftover::Unknown => o.notes.unknown(format!("opcode:{k}")),
        }
    }
}

/// `gain_oncc7` -> `gain_onccN`, for grouping report lines.
fn family(name: &str) -> String {
    let t = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if t.len() < name.len() && (t.ends_with("cc") || t.ends_with('_')) {
        format!("{t}N")
    } else {
        name.to_string()
    }
}

// ------------------------------------------------------------------------- instrument

/// Everything [`map`] produces.
pub struct Mapped {
    pub instrument: HausInstrument,
    pub samples: Vec<ImportedSample>,
}

fn effect_name(t: &str) -> Option<&'static str> {
    Some(match t {
        "reverb" | "fverb" => "reverb",
        "delay" => "delay",
        "comp" => "compressor",
        "eq" => "eq",
        "filter" => "filter",
        "phaser" => "phaser",
        "disto" => "saturator",
        _ => return None,
    })
}

/// Common ancestor of two absolute paths.
fn common(a: &Path, b: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for (x, y) in a.components().zip(b.components()) {
        if x != y {
            break;
        }
        out.push(x.as_os_str());
    }
    out
}

fn rel_name(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| {
            r.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

/// Builds the instrument from parsed regions.
pub fn map(parsed: &Parsed, name: &str, main_dir: Option<&Path>, notes: &mut Notes) -> Mapped {
    let mut inst = HausInstrument::new(name);

    // effects (§12): kept as data, mapped where a built-in is near
    let mut mapped_fx = Vec::new();
    let mut fx_text = Vec::new();
    for e in &parsed.effects {
        fx_text.push(
            e.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        let t = e
            .get("type")
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let bus = e
            .get("bus")
            .map(|b| b.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "main".into());
        let bus = if bus == "main" {
            Some(None)
        } else {
            bus.strip_prefix("fx")
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=4).contains(n))
                .map(Some)
        };
        match (effect_name(&t), bus) {
            (Some(n), Some(bus)) => {
                notes.approx(
                    format!("effect:{t}"),
                    format!("played by the built-in {n} effect with its default settings"),
                );
                mapped_fx.push(MappedEffect { name: n, bus });
            }
            (None, _) => notes.skip(
                format!("effect:{}", if t.is_empty() { "?" } else { &t }),
                "an effect type the sampler does not have; kept as data",
            ),
            (_, None) => notes.skip(
                format!("effect:{t}"),
                "effects on aux buses are not played; kept as data",
            ),
        }
    }
    if !fx_text.is_empty() {
        inst.extra.insert("sfz.effects".into(), fx_text.join("\n"));
        notes.kept("header:<effect>", "every <effect> block is kept as data");
    }

    // control data (§4.5, §13)
    let mut control_text = Vec::new();
    for (k, v) in &parsed.control {
        control_text.push(format!("{k}={v}"));
        let fam = family(k);
        if k.starts_with("set_cc") || k.starts_with("set_hdcc") {
            notes.kept(
                format!("opcode:{fam}"),
                "initial controller values are kept as data; the sampler starts controllers at its defaults",
            );
        } else if k.starts_with("label_") {
            notes.kept(
                format!("opcode:{fam}"),
                "kept as data for the instrument's UI",
            );
        } else {
            match leftover(k) {
                Leftover::Unknown => notes.unknown(format!("opcode:{k}")),
                _ => notes.kept(format!("opcode:{fam}"), "kept as data"),
            }
        }
    }
    if !control_text.is_empty() {
        inst.extra
            .insert("sfz.control".into(), control_text.join("\n"));
    }

    let chokers = parsed
        .regions
        .iter()
        .filter_map(|r| r.ops.get("off_by")?.trim().parse::<f64>().ok())
        .map(|v| v as i64)
        .collect();
    let mut m = Mapper {
        chokers,
        notes,
        curves: &parsed.curves,
        effects: &mapped_fx,
        samples: Samples {
            main_dir,
            probes: BTreeMap::new(),
            dirs: BTreeMap::new(),
        },
    };
    let drafts: Vec<Draft> = parsed.regions.iter().filter_map(|r| m.region(r)).collect();
    let notes = m.notes;

    // articulations (§4.4): one per keyswitch key, in key order
    let mut keys: BTreeSet<u8> = drafts.iter().flat_map(|d| d.g.art_keys.clone()).collect();
    let sw_default = drafts.iter().rev().find_map(|d| d.sw_default);
    if let Some(k) = sw_default
        && !keys.is_empty()
        && !keys.contains(&k)
    {
        notes.skip(
            "opcode:sw_default",
            "the default keyswitch selects no region; the first articulation is the default",
        );
    }
    let labels: BTreeMap<u8, String> = parsed
        .regions
        .iter()
        .filter_map(|r| {
            let k = r.ops.get("sw_last")?.parse::<u8>().ok()?;
            Some((k, r.ops.get("sw_label")?.trim().to_string()))
        })
        .collect();
    let art_of: BTreeMap<u8, ArticulationId> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, ArticulationId(i as u32)))
        .collect();
    for (k, id) in &art_of {
        inst.articulations.push(Articulation {
            id: *id,
            name: labels
                .get(k)
                .cloned()
                .unwrap_or_else(|| format!("Keyswitch {}", note_name(*k))),
            trigger: ArticulationTrigger::Key { key: *k },
            color: None,
        });
    }
    if !keys.is_empty() {
        match sw_default.and_then(|k| art_of.get(&k)) {
            Some(id) => inst.settings.default_articulation = Some(*id),
            None => notes.approx(
                "opcode:sw_default",
                "without sw_default the first articulation plays until a keyswitch is pressed (SFZ plays nothing)",
            ),
        }
    }
    keys.clear();

    // instrument settings
    if let Some((up, down)) = drafts.iter().find_map(|d| d.bend) {
        inst.settings.bend_up = up;
        inst.settings.bend_down = down;
        if drafts
            .iter()
            .any(|d| d.bend.is_some_and(|b| b != (up, down)))
        {
            notes.approx(
                "opcode:bend_up",
                "regions with different pitch-bend ranges share the first one",
            );
        }
    }

    // groups: by <group> header and group-level settings
    let mut buckets: Vec<(usize, GroupDraft, Option<String>, Vec<usize>)> = Vec::new();
    for (i, d) in drafts.iter().enumerate() {
        match buckets.iter_mut().find(|b| b.0 == d.group_ix && b.1 == d.g) {
            Some(b) => b.3.push(i),
            None => buckets.push((d.group_ix, d.g.clone(), d.label.clone(), vec![i])),
        }
    }
    // sample names relative to the common folder
    let files: BTreeSet<PathBuf> = drafts
        .iter()
        .filter_map(|d| match &d.sample {
            Res::File(p) => Some(p.clone()),
            Res::Gen(_) => None,
        })
        .collect();
    let mut root = main_dir.map(normalise);
    for f in &files {
        let parent = f.parent().map(Path::to_path_buf).unwrap_or_default();
        root = Some(match root {
            None => parent,
            Some(r) => common(&r, &parent),
        });
    }
    let root = root.unwrap_or_default();
    let mut samples: Vec<ImportedSample> = Vec::new();
    let mut name_of = |res: &Res| -> String {
        let (name, source) = match res {
            Res::File(p) => (rel_name(&root, p), SampleSource::File(p.clone())),
            Res::Gen(g) => (g.name().to_string(), SampleSource::Generated(*g)),
        };
        if !samples.iter().any(|s| s.name == name) {
            samples.push(ImportedSample {
                name: name.clone(),
                source,
            });
        }
        name
    };

    let mut used_names: BTreeMap<String, u32> = BTreeMap::new();
    let mut zone_id = 0u32;
    let mut group_routes = Vec::new();
    for (gi, (gix, gd, label, members)) in buckets.into_iter().enumerate() {
        let gid = GroupId(gi as u32);
        let base = label.unwrap_or_else(|| format!("Group {}", gix.max(1)));
        let n = used_names.entry(base.clone()).or_insert(0);
        *n += 1;
        let mut g = gd.group.clone();
        g.id = gid;
        g.name = if *n == 1 {
            base
        } else {
            format!("{base} ({n})")
        };
        g.articulations = gd
            .art_keys
            .iter()
            .filter_map(|k| art_of.get(k).copied())
            .collect();
        // random layers (§4.3): positions by range
        let mut rand_pos: Vec<(f32, f32)> = Vec::new();
        if gd.random {
            g.round_robin = RoundRobinMode::Random;
            for &i in &members {
                if let Some(r) = drafts[i].rand
                    && !rand_pos.contains(&r)
                {
                    rand_pos.push(r);
                }
            }
            rand_pos.sort_by(|a, b| a.0.total_cmp(&b.0));
            notes.approx(
                "opcode:lorand",
                "random layers are chosen by the sampler's random round robin (equal chances, never the same layer twice in a row)",
            );
        }
        for r in &gd.routes {
            let mut r = r.clone();
            r.group = Some(gid);
            group_routes.push(r);
        }
        inst.groups.insert(gid, g);
        for i in members {
            let d = &drafts[i];
            let mut z = d.zone.clone();
            z.id = ZoneId(zone_id);
            zone_id += 1;
            z.group = gid;
            z.sample = SampleRef::File(name_of(&d.sample));
            if let Some(r) = d.rand {
                z.round_robin = rand_pos.iter().position(|p| *p == r).map(|p| p as u32);
            }
            inst.zones.insert(z.id, z);
        }
    }
    inst.modulation = group_routes;
    Mapped {
        instrument: inst,
        samples,
    }
}

/// `C4`-style name for a key (C4 = 60).
pub fn note_name(k: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(k % 12) as usize], k as i32 / 12 - 1)
}
