//! Maps the typed FLP data ([`super::read`]) to a Patternhaus [`Project`] through [`Edit`],
//! and records everything it approximated or skipped in the [`ImportReport`].
//!
//! Scales used (spec section in brackets):
//! - ticks: `ticks * 960 / file PPQ`, rounded (exact for PPQ 96) [1.1, 5.2, 7.2];
//! - channel volume `raw / 12800` and pan `(raw - 6400) / 6400` [4.2]; the volume curve is
//!   not documented, so the linear reading is reported as an approximation;
//! - insert volume `raw / 12800` (12800 = 0 dB), pan `raw / 6400` [6.5];
//! - note velocity `v / 128`, pan `(p - 64) / 64`, fine pitch `(f - 120) * 10` cents [5.2].

use super::parse::*;
use super::read::*;
use super::report::ImportReport;
use ph_core::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// An audio file found for a path stored in the FLP, already copied into the new project
/// folder by the caller (`reference` is how the project names it, e.g. `samples/kick.wav`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSample {
    pub reference: String,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Finds the audio files an FLP names. The importer asks once per distinct path.
pub trait SampleResolver {
    /// `flp_path` is the path as stored (it may start with a `%...%` placeholder).
    fn resolve(&mut self, flp_path: &str) -> Option<ResolvedSample>;
}

/// Resolves nothing: every sample path is kept as stored and reported.
pub struct NoSamples;

impl SampleResolver for NoSamples {
    fn resolve(&mut self, _: &str) -> Option<ResolvedSample> {
        None
    }
}

/// A file the import wants written into the project folder (`plugins/...`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFile {
    /// Project-relative, forward slashes.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// The result of an import.
#[derive(Debug)]
pub struct FlpImport {
    pub project: Project,
    /// Plugin state blobs to save as `plugins/<file>` (DESIGN §10).
    pub files: Vec<ImportFile>,
    pub report: ImportReport,
}

/// Parses `bytes` and maps them. `fallback_title` names a project whose file has no title.
pub fn import_flp(
    bytes: &[u8],
    fallback_title: &str,
    resolver: &mut dyn SampleResolver,
) -> Result<FlpImport, FlpError> {
    let raw = parse(bytes)?;
    let file = read(&raw);
    map(&file, fallback_title, resolver)
}

const FL_DEFAULT_COLOR: u32 = 0x0056_5148;
/// Plugin id prefixes of placeholders (the original plugin cannot be loaded by id).
pub const VST2_PREFIX: &str = "vst2:";
pub const NATIVE_PREFIX: &str = "fl-native:";
pub const WRAPPER_PREFIX: &str = "fl-wrapper:";

fn rgb(v: u32) -> Color {
    // bytes R, G, B, 0 (spec 4.1)
    ((v & 0xff) << 16) | (v & 0xff00) | ((v >> 16) & 0xff)
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn stem(path: &str) -> String {
    let n = file_name(path);
    match n.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_string(),
        _ => n,
    }
}

/// An identified plugin: how the project names it, and a label for people.
struct PluginRef {
    format: PluginFormat,
    id: String,
    label: Option<String>,
    native: bool,
    vst2: bool,
}

fn plugin_ref(internal: &str, state: Option<&[u8]>) -> PluginRef {
    if internal == "Fruity Wrapper" {
        let info = parse_wrapper(state.unwrap_or(&[]));
        let (id, vst2) = match info.id {
            // SPEC-GAP: 8.2 (record 52): the byte order of the class id relative to the
            // textual VST3 id is not stated; the bytes are written in file order.
            WrapperId::Vst3(c) => (c.iter().map(|b| format!("{b:02x}")).collect(), false),
            WrapperId::Vst2(f) => {
                let text: String = if f.iter().all(|b| b.is_ascii_graphic()) {
                    f.iter().map(|b| *b as char).collect()
                } else {
                    f.iter().map(|b| format!("{b:02x}")).collect()
                };
                (format!("{VST2_PREFIX}{text}"), true)
            }
            WrapperId::Unknown => (
                format!(
                    "{WRAPPER_PREFIX}{}",
                    info.name.as_deref().unwrap_or("unknown")
                ),
                false,
            ),
        };
        PluginRef {
            // The model has no VST2 format; VST2 placeholders use Vst3 with a `vst2:` id.
            format: PluginFormat::Vst3,
            id,
            label: info.name,
            native: false,
            vst2,
        }
    } else {
        PluginRef {
            format: PluginFormat::Vst3,
            id: format!("{NATIVE_PREFIX}{internal}"),
            label: Some(internal.to_string()),
            native: true,
            vst2: false,
        }
    }
}

/// The part of the model scale a mixer automation target maps to.
fn automation_target(target: u32) -> Option<(ParamPath, f64, f64)> {
    let (param, insert, _slot) = mixer_target(target)?;
    if insert as usize > MIXER_INSERTS {
        return None;
    }
    let id = InsertId(u32::from(insert));
    match param {
        // SPEC-GAP: 9.2: the normalised 0..1 range of a mixer parameter is not stated;
        // taken as linear over the raw range (volume 0..16000, pan -6400..6400).
        192 => Some((ParamPath::InsertVolume(id), 0.0, 16000.0 / 12800.0)),
        193 => Some((ParamPath::InsertPan(id), -1.0, 2.0)),
        _ => None,
    }
}

struct Point {
    tick: Tick,
    value: f64,
    tension: f32,
}

/// Event 234 (spec 9.1): a 21-byte header with the point count at offset 17, then 24-byte
/// points (f64 delta in quarter notes, f64 value, f32 tension, u32 curve code), then a trailer.
fn automation_points(b: &[u8]) -> Vec<Point> {
    let count = u32_at(b, 17).unwrap_or(0) as usize;
    let fit = b.len().saturating_sub(21) / 24;
    let n = count.min(fit);
    let mut quarters = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = 21 + i * 24;
        let delta = f64_at(b, at)
            .filter(|d| d.is_finite() && *d >= 0.0)
            .unwrap_or(0.0);
        let value = f64_at(b, at + 8).filter(|v| v.is_finite()).unwrap_or(0.0);
        let tension = f32_at(b, at + 16).filter(|t| t.is_finite()).unwrap_or(0.0);
        quarters += delta;
        out.push(Point {
            tick: (quarters * PPQ as f64).round().clamp(0.0, 1e15) as Tick,
            value: value.clamp(0.0, 1.0),
            tension: tension.clamp(-1.0, 1.0),
        });
    }
    out
}

struct Importer<'a> {
    file: &'a FlpFile,
    ed: Edit,
    rep: ImportReport,
    files: Vec<ImportFile>,
    resolver: &'a mut dyn SampleResolver,
    ppq: u64,
    ch_map: BTreeMap<u16, ChannelId>,
    pat_map: BTreeMap<u16, PatternId>,
    audio_src: BTreeMap<u16, &'a FlpChannel>,
    auto_src: BTreeMap<u16, &'a FlpChannel>,
    resolved: HashMap<String, Option<ResolvedSample>>,
    sample_ids: HashMap<String, SampleId>,
}

/// Maps a read file to a project.
pub fn map(
    file: &FlpFile,
    fallback_title: &str,
    resolver: &mut dyn SampleResolver,
) -> Result<FlpImport, FlpError> {
    let project = Project::empty(fallback_title);
    let mut im = Importer {
        file,
        ed: Edit::new(&project),
        rep: ImportReport::new(file.version.clone(), file.header.ppq),
        files: Vec::new(),
        resolver,
        ppq: u64::from(file.header.ppq.max(1)),
        ch_map: BTreeMap::new(),
        pat_map: BTreeMap::new(),
        audio_src: BTreeMap::new(),
        auto_src: BTreeMap::new(),
        resolved: HashMap::new(),
        sample_ids: HashMap::new(),
    };
    im.settings(fallback_title)?;
    im.channels()?;
    im.patterns()?;
    im.mixer()?;
    im.playlist()?;
    im.rep.unread_events = file.unread.clone();
    let rep = im.rep;
    let (project, _) = im.ed.finish();
    Ok(FlpImport {
        project,
        files: im.files,
        report: rep,
    })
}

impl<'a> Importer<'a> {
    fn ticks(&self, v: u64) -> Tick {
        (v * PPQ + self.ppq / 2) / self.ppq
    }

    fn settings(&mut self, fallback_title: &str) -> Result<(), FlpError> {
        let f = self.file;
        let bpm = match (f.tempo_milli, f.tempo_coarse) {
            (Some(m), _) => f64::from(m) / 1000.0,
            (None, Some(c)) => {
                // SPEC-GAP: 3: events 66/93 (old tempo) are documented only; the coarse one
                // is read as whole BPM.
                self.rep
                    .approx("tempo read from the old coarse tempo event", 1);
                f64::from(c)
            }
            (None, None) => {
                self.rep.approx("no tempo in the file: assumed 140 BPM", 1);
                140.0
            }
        };
        let clamped = bpm.clamp(10.0, 999.0);
        if clamped != bpm {
            self.rep.approx("tempo outside 10-999 BPM clamped", 1);
        }
        let ts = match (f.ts_numerator, f.ts_denominator) {
            (Some(n), Some(d)) if (1..=64).contains(&n) && [1, 2, 4, 8, 16, 32].contains(&d) => {
                // SPEC-GAP: 3 / 11.5: events 17/18 as numerator/denominator is inferred and
                // only 4/4 was seen.
                TimeSignature {
                    numerator: n,
                    denominator: d,
                }
            }
            (None, None) => TimeSignature::default(),
            _ => {
                self.rep
                    .approx("time signature not understood: set to 4/4", 1);
                TimeSignature::default()
            }
        };
        let title = if f.title.trim().is_empty() {
            fallback_title.to_string()
        } else {
            f.title.clone()
        };
        self.ed.update_settings(|s| {
            s.title = title;
            s.bpm = clamped;
            s.time_signature = ts;
        })?;
        for (k, v) in [
            ("Genre", &f.genre),
            ("Author", &f.author),
            ("Comments", &f.comments),
            ("URL", &f.url),
        ] {
            if !v.trim().is_empty() {
                self.rep.metadata.push((k.to_string(), v.clone()));
                self.rep.skip(
                    "project info (genre, author, comments, URL): kept in this report only",
                    1,
                );
            }
        }
        if f.filter_groups > 0 {
            self.rep.skip("channel filter groups", f.filter_groups);
        }
        Ok(())
    }

    fn resolve(&mut self, path: &str) -> Option<ResolvedSample> {
        if let Some(r) = self.resolved.get(path) {
            return r.clone();
        }
        let r = self.resolver.resolve(path);
        self.rep.add(
            if r.is_some() {
                "samples_found"
            } else {
                "samples_missing"
            },
            1,
        );
        self.resolved.insert(path.to_string(), r.clone());
        r
    }

    /// The reference a sampler channel uses for `path`.
    fn sampler_ref(&mut self, path: &str) -> String {
        match self.resolve(path) {
            Some(r) => r.reference,
            None => {
                self.rep
                    .approx("sample file not found: path kept as stored", 1);
                path.to_string()
            }
        }
    }

    /// The registry entry for an audio clip's sample, created on first use.
    fn sample_entry(&mut self, path: &str) -> Result<SampleId, FlpError> {
        if let Some(id) = self.sample_ids.get(path) {
            return Ok(*id);
        }
        let r = self.resolve(path);
        if r.is_none() {
            self.rep
                .approx("sample file not found: audio clip registered with the stored path and unknown length", 1);
        }
        let (reference, frames, sample_rate, channels) = match r {
            Some(r) => (
                r.reference,
                r.frames,
                r.sample_rate.max(1),
                r.channels.max(1),
            ),
            None => (path.to_string(), 0, 44_100, 2),
        };
        let id = match self.ed.project().sample_by_path(&reference) {
            Some(e) => e.id,
            None => {
                self.rep.add("samples", 1);
                self.ed.add_sample(SampleEntry {
                    id: SampleId(0),
                    path: reference,
                    name: file_name(path),
                    frames,
                    sample_rate,
                    channels,
                    bpm: None,
                    root_key: None,
                    analysis: SampleAnalysis::default(),
                    provenance: Provenance {
                        origin: Origin::Imported {
                            from: path.to_string(),
                        },
                        at: None,
                    },
                })?
            }
        };
        self.sample_ids.insert(path.to_string(), id);
        Ok(id)
    }

    fn state_file(&mut self, name: String, state: &Option<Vec<u8>>) -> Option<String> {
        let bytes = state.as_ref().filter(|b| !b.is_empty())?;
        let path = format!("plugins/{name}.flpstate");
        self.files.push(ImportFile {
            path: path.clone(),
            bytes: bytes.clone(),
        });
        self.rep.add("plugin_files", 1);
        Some(path)
    }

    fn channels(&mut self) -> Result<(), FlpError> {
        let file = self.file;
        for ch in &file.channels {
            self.rep.add("channels_in_file", 1);
            let kind = ch.kind.unwrap_or(if ch.plugin_name.is_empty() {
                CH_SAMPLER
            } else {
                CH_GENERATOR
            });
            if ch.kind.is_none() {
                self.rep.approx(
                    "channel without a type event: type guessed from its plugin name",
                    1,
                );
            }
            match kind {
                CH_SAMPLER | CH_GENERATOR => self.rack_channel(ch, kind)?,
                CH_AUDIO => {
                    self.rep.add("channels.audio_sources", 1);
                    self.audio_src.insert(ch.iid, ch);
                }
                CH_AUTOMATION => {
                    self.rep.add("channels.automation_sources", 1);
                    self.auto_src.insert(ch.iid, ch);
                }
                CH_LAYER => self.rep.skip("layer channel (no layer in Patternhaus)", 1),
                _ => self.rep.skip("channel of an unknown type", 1),
            }
        }
        Ok(())
    }

    fn rack_channel(&mut self, ch: &FlpChannel, kind: u8) -> Result<(), FlpError> {
        let (instrument, label) = if kind == CH_SAMPLER {
            self.rep.add("channels.sampler", 1);
            let sample = ch.sample_path.as_deref().map(|p| self.sampler_ref(p));
            let label = ch.sample_path.as_deref().map(stem);
            (
                InstrumentSpec::Sampler {
                    sample,
                    root_key: 60,
                    params: BTreeMap::new(),
                },
                label,
            )
        } else {
            self.rep.add("channels.plugin", 1);
            let p = plugin_ref(&ch.plugin_name, ch.state.as_deref());
            let state = self.state_file(format!("flp-channel-{}", ch.iid), &ch.state);
            self.plugin_note(&p, "generator");
            (
                InstrumentSpec::Plugin {
                    format: p.format,
                    plugin_id: p.id,
                    state,
                    params: BTreeMap::new(),
                },
                p.label,
            )
        };
        let name = ch
            .name
            .clone()
            .or(label)
            .unwrap_or_else(|| format!("Channel {}", u32::from(ch.iid) + 1));
        let id = self.ed.add_channel(&name, instrument)?;
        self.ch_map.insert(ch.iid, id);
        self.rep.add("channels", 1);

        // SPEC-GAP: 4.2: the channel volume curve is not documented; linear is used.
        let [pan_raw, vol_raw, pitch] = ch.levels.unwrap_or([6400, 10_000, 0]);
        if ch.levels.is_some() {
            self.rep.approx(
                "channel volume: linear reading of the raw 0-12800 scale (FL's curve is not documented)",
                1,
            );
        }
        if pitch != 0 {
            self.rep.skip("channel pitch offset", 1);
        }
        let mut insert = u32::from(ch.insert.unwrap_or(0));
        if insert as usize > MIXER_INSERTS {
            self.rep.approx(
                "channel routed to an insert above 32: sent to the master",
                1,
            );
            insert = 0;
        }
        let color = ch.color.map(rgb);
        let muted = ch.enabled == Some(false);
        self.ed.update_channel(id, |c| {
            if let Some(col) = color {
                c.color = col;
            }
            c.volume = (vol_raw as f32 / 12800.0).clamp(0.0, 1.0);
            c.pan = ((pan_raw as f32 - 6400.0) / 6400.0).clamp(-1.0, 1.0);
            c.muted = muted;
            c.insert = InsertId(insert);
        })?;
        Ok(())
    }

    fn plugin_note(&mut self, p: &PluginRef, what: &str) {
        if p.native {
            self.rep.approx(
                &format!("native FL {what} kept as a labelled placeholder with its state (state is opaque, spec 8.1)"),
                1,
            );
        } else if p.vst2 {
            self.rep.approx(
                &format!(
                    "VST2 {what}: no VST2 host, kept as a placeholder with its FL wrapper state"
                ),
                1,
            );
        } else {
            self.rep.approx(
                &format!("VST {what} kept as a placeholder with its FL wrapper state (the inner plugin state is not unpacked)"),
                1,
            );
        }
    }

    fn patterns(&mut self) -> Result<(), FlpError> {
        let file = self.file;
        let mut ids: BTreeSet<u16> = file.patterns.keys().copied().collect();
        if let Some(a) = file.arrangements.first() {
            ids.extend(a.items.iter().filter_map(FlpItem::pattern));
        }
        for fid in ids {
            let pat = file.patterns.get(&fid);
            let name = pat
                .and_then(|p| p.name.clone())
                .unwrap_or_else(|| format!("Pattern {fid}"));
            let id = self.ed.add_pattern(&name)?;
            self.pat_map.insert(fid, id);
            self.rep.add("patterns", 1);
            let Some(pat) = pat else { continue };
            let color = pat.color.map(rgb);
            let length = (pat.length > 0).then(|| self.ticks(u64::from(pat.length)));
            self.ed.update_pattern(id, |h| {
                if let Some(c) = color {
                    h.color = c;
                }
                h.length = length;
            })?;
            if pat.controller_bytes > 0 {
                // SPEC-GAP: 5.3: controller (223) record layout is not confirmed.
                self.rep.skip(
                    "pattern controller events (layout unconfirmed)",
                    (pat.controller_bytes / 12) as u64,
                );
            }
            for n in &pat.notes {
                self.note(id, n)?;
            }
        }
        Ok(())
    }

    fn note(&mut self, pattern: PatternId, n: &FlpNote) -> Result<(), FlpError> {
        self.rep.add("notes_in_file", 1);
        let Some(&channel) = self.ch_map.get(&n.channel) else {
            self.rep.skip("note on a channel that was not imported", 1);
            return Ok(());
        };
        if n.key > 127 {
            self.rep.skip("note above MIDI key 127", 1);
            return Ok(());
        }
        let mut length = self.ticks(u64::from(n.length)).max(1);
        if n.length == 0 {
            // SPEC-GAP: 5.2: length 0 is marked "step-sequencer note [D]" without a length;
            // given one step.
            length = STEP;
            self.rep
                .approx("note of length 0 (step-sequencer note) given one step", 1);
        }
        let spec = NoteSpec {
            key: n.key as u8,
            start: self.ticks(u64::from(n.position)),
            length,
            velocity: (f32::from(n.velocity) / 128.0).clamp(0.0, 1.0),
            pan: ((f32::from(n.pan) - 64.0) / 64.0).clamp(-1.0, 1.0),
            fine_pitch: (f32::from(n.fine_pitch) - 120.0) * 10.0,
            muted: false,
        };
        self.ed.add_note(pattern, channel, spec)?;
        self.rep.add("notes", 1);
        if n.mod_x != 128 || n.mod_y != 128 {
            self.rep.skip("note mod X / mod Y", 1);
        }
        if n.flags != 0x4000 && n.flags != 0 {
            // SPEC-GAP: 5.2: flag bits other than 0x4000 (slide, portamento) are untested.
            self.rep
                .skip("note flags other than the default (slide, portamento)", 1);
        }
        Ok(())
    }

    fn mixer(&mut self) -> Result<(), FlpError> {
        let file = self.file;
        let params: HashMap<(u8, u8, u8), i32> = file
            .mixer_params
            .iter()
            .map(|p| ((p.insert, p.slot, p.param), p.value))
            .collect();
        // FL writes the "current" slot last (index 126); it is not a real insert (spec 6.1).
        let real = file.inserts.len().min(126);
        self.rep.add("inserts_in_file", real as u64);
        if real > MIXER_INSERTS + 1 {
            let used = file.inserts[MIXER_INSERTS + 1..real]
                .iter()
                .filter(|i| i.name.is_some() || i.color.is_some() || !i.slots.is_empty())
                .count();
            self.rep.skip(
                "mixer inserts above 32 that are named, coloured or hold effects",
                used as u64,
            );
        }
        if real > 0 {
            // SPEC-GAP: 6.2: the flag bits (mute, solo, bypass, polarity) are unassigned.
            self.rep.skip(
                "mixer insert flags (mute, solo, polarity: bit meanings unknown)",
                1,
            );
        }
        // Sends added so far, for the cycle check.
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); MIXER_INSERTS + 1];
        for i in 0..real.min(MIXER_INSERTS + 1) {
            let fi = &file.inserts[i];
            let idx = i as u8;
            // SPEC-GAP: 6.5: the insert volume curve (raw to dB) is not documented; linear.
            let volume = params.get(&(idx, 0, 192)).copied();
            let pan = params.get(&(idx, 0, 193)).copied();
            if volume.is_some() {
                self.rep.approx(
                    "mixer insert volume: linear reading of the raw scale, 12800 = unity (FL's curve is not documented)",
                    1,
                );
            }
            let to_master;
            let mut sends: Vec<Send> = Vec::new();
            if i == 0 {
                to_master = false;
            } else {
                to_master = fi.routing.first().is_none_or(|b| *b != 0);
                // SPEC-GAP: 6.3: bytes 100-103 of every non-master insert's routing are set in
                // the corpus files, which the spec's reading ("non-zero = sends to that insert")
                // turns into routes to inserts 100-103. All are above 32, so none is imported
                // either way; they show up in the "route to an insert above 32" count.
                for (j, b) in fi.routing.iter().enumerate().take(126).skip(1) {
                    if *b == 0 || j == i {
                        continue;
                    }
                    self.rep.add("routes_in_file", 1);
                    if j > MIXER_INSERTS {
                        self.rep.skip("route to an insert above 32", 1);
                    } else if reaches(&adj, j, i) {
                        self.rep.skip("route that would form a loop", 1);
                    } else {
                        adj[i].push(j as u32);
                        sends.push(Send {
                            to: InsertId(j as u32),
                            level: 1.0,
                        });
                        self.rep.add("routes", 1);
                        // SPEC-GAP: 6.3 / 11.1: the send level of a route has not been found.
                        self.rep
                            .approx("route send level not in the file: set to unity", 1);
                    }
                }
                if to_master {
                    self.rep.add("routes", 1);
                }
            }
            let mut slots: Vec<(u16, &FlpSlot)> = fi
                .slots
                .iter()
                .enumerate()
                // SPEC-GAP: 6.4: without slot indices (FL 12) the position is taken from the
                // order of appearance; the spec says it should come from the 225 records.
                .map(|(n, s)| (s.index.unwrap_or(n as u16), s))
                .collect();
            slots.sort_by_key(|(k, _)| *k);
            let mut effects = Vec::new();
            for (slot_idx, s) in slots {
                if s.internal_name.is_empty() {
                    continue;
                }
                let p = plugin_ref(&s.internal_name, s.state.as_deref());
                let state = self.state_file(format!("flp-insert{i}-slot{slot_idx}"), &s.state);
                self.rep.add("effects", 1);
                self.rep.add(
                    if p.native {
                        "effects.native"
                    } else {
                        "effects.vst"
                    },
                    1,
                );
                self.plugin_note(&p, "effect");
                let enabled = params
                    .get(&(idx, slot_idx as u8, 0))
                    .is_none_or(|v| *v != 0);
                let mix = params
                    .get(&(idx, slot_idx as u8, 1))
                    .map_or(1.0, |v| (*v as f32 / 12800.0).clamp(0.0, 1.0));
                effects.push(EffectSlot {
                    effect: EffectSpec::Plugin {
                        format: p.format,
                        plugin_id: p.id,
                        state,
                        params: BTreeMap::new(),
                    },
                    bypass: !enabled,
                    mix,
                });
            }
            let color = fi.color.filter(|c| *c != FL_DEFAULT_COLOR).map(rgb);
            let name = fi.name.clone();
            self.ed.update_insert(InsertId(i as u32), |ins| {
                if let Some(n) = name {
                    ins.name = n;
                }
                ins.color = color;
                ins.volume = volume.map_or(1.0, |v| (v as f32 / 12800.0).clamp(0.0, 2.0));
                ins.pan = pan.map_or(0.0, |v| (v as f32 / 6400.0).clamp(-1.0, 1.0));
                ins.effects = effects;
                ins.sends = sends;
                ins.route_to_master = to_master;
            })?;
            self.rep.add("inserts", 1);
        }
        Ok(())
    }

    fn playlist(&mut self) -> Result<(), FlpError> {
        let file = self.file;
        let Some(arr) = file.arrangements.first() else {
            return Ok(());
        };
        for other in file.arrangements.iter().skip(1) {
            self.rep.skip(
                "playlist items of further arrangements (Patternhaus has one playlist)",
                other.items.len() as u64,
            );
        }
        if arr.stray_item_bytes > 0 {
            self.rep
                .skip("playlist bytes that do not fill a whole record", 1);
        }
        for t in &arr.tracks {
            // SPEC-GAP: 7.3: the track a 238 record describes is taken as `iid - 1`.
            let Some(idx) = t
                .iid
                .checked_sub(1)
                .filter(|i| (*i as usize) < PLAYLIST_TRACKS)
            else {
                continue;
            };
            let name = t.name.clone();
            let color = (t.color != FL_DEFAULT_COLOR).then(|| rgb(t.color));
            if name.is_some() {
                self.rep.add("tracks_named", 1);
            }
            // SPEC-GAP: 7.3: "track enabled = 0 is mute" is presumed, never seen.
            let muted = !t.enabled;
            self.ed.update_track(idx, |tr| {
                if let Some(n) = name {
                    tr.name = n;
                }
                tr.color = color;
                tr.muted = muted;
            })?;
        }
        for it in &arr.items {
            self.rep.add("clips_in_file", 1);
            self.item(it)?;
        }
        Ok(())
    }

    fn item(&mut self, it: &FlpItem) -> Result<(), FlpError> {
        if it.length == 0 {
            self.rep.skip("playlist item of length 0", 1);
            return Ok(());
        }
        // SPEC-GAP: 7.2 / 7.3: FL 12 numbers tracks differently (unknown); the FL 20/21 rule
        // is applied to every file.
        // Track number = 499 - value (spec 7.2, FL 20/21).
        let Some(track) = 499u32.checked_sub(it.track_raw) else {
            self.rep
                .skip("playlist item with a track number out of range", 1);
            return Ok(());
        };
        if track as usize >= PLAYLIST_TRACKS {
            self.rep
                .skip("playlist item on a track above 64 (Patternhaus has 64)", 1);
            return Ok(());
        }
        let start = self.ticks(u64::from(it.position));
        let length = self.ticks(u64::from(it.length)).max(1);
        if let Some(p) = it.pattern() {
            let Some(&pid) = self.pat_map.get(&p) else {
                self.rep
                    .skip("playlist item of a pattern that does not exist", 1);
                return Ok(());
            };
            let off = i32::from_le_bytes(it.start_raw);
            let offset = if off > 0 { self.ticks(off as u64) } else { 0 };
            let id = self.ed.add_clip(track, pid, start)?;
            self.ed.update_clip(id, |c| {
                c.length = length;
                c.offset = offset;
            })?;
            self.rep.add("clips", 1);
            self.rep.add("clips.pattern", 1);
            return Ok(());
        }
        let file = self.file;
        if let Some(src) = self.audio_src.get(&it.index).copied() {
            let Some(path) = src.sample_path.as_deref() else {
                self.rep.skip("audio clip whose channel has no sample", 1);
                return Ok(());
            };
            let sid = self.sample_entry(path)?;
            if src.levels.is_some_and(|l| l[0] != 6400 || l[1] != 10_000) {
                self.rep
                    .approx("audio clip channel volume/pan not carried over", 1);
            }
            let off = f32::from_le_bytes(it.start_raw);
            if off.is_finite() && off > 0.0 {
                // SPEC-GAP: 7.2 / 11.3: the unit of a channel clip's start offset is unknown.
                self.rep.approx(
                    "audio clip start offset (unit unknown): played from the start of the sample",
                    1,
                );
            }
            self.ed
                .add_audio_clip(track, start, length, AudioClip::new(sid))?;
            self.rep.add("clips", 1);
            self.rep.add("clips.audio", 1);
            return Ok(());
        }
        if let Some(src) = self.auto_src.get(&it.index).copied() {
            let Some(target) = file.auto_targets.get(&it.index).copied() else {
                self.rep.skip("automation clip without a target event", 1);
                return Ok(());
            };
            let Some((path, base, scale)) = automation_target(target) else {
                let class = if target >> 28 == 4 {
                    "automation of a target in the 0x4 space (unknown)"
                } else if mixer_target(target).is_some() {
                    "automation of a mixer parameter other than insert volume and pan"
                } else {
                    "automation of an unrecognised target"
                };
                self.rep.skip(class, 1);
                return Ok(());
            };
            let points = src
                .points
                .as_deref()
                .map(automation_points)
                .unwrap_or_default();
            if points.is_empty() {
                self.rep.skip("automation clip without points", 1);
                return Ok(());
            }
            let range = path.fixed_range().unwrap_or((0.0, 1.0));
            let clip = self.ed.add_automation_clip(track, start, length, path)?;
            for p in &points {
                self.ed.add_point(
                    clip,
                    p.tick,
                    (p.value * scale + base).clamp(range.0, range.1),
                    CurveKind::Linear,
                    p.tension,
                )?;
            }
            self.rep.add("automation_points", points.len() as u64);
            self.rep.add("clips", 1);
            self.rep.add("clips.automation", 1);
            // SPEC-GAP: 9.1: the last field of a point (curve type?) is not read and the f32
            // is taken as tension of a plain curve.
            self.rep.approx(
                "automation curves: point tension applied to plain curves (curve-type field not read)",
                1,
            );
            self.rep.approx(
                "automation values: 0-1 scaled linearly over the raw range of the mixer parameter",
                1,
            );
            return Ok(());
        }
        self.rep.skip(
            "playlist item of a channel that is missing or not a clip source",
            1,
        );
        Ok(())
    }
}

/// Whether `from` already reaches `to` through the sends in `adj`.
fn reaches(adj: &[Vec<u32>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; adj.len()];
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if std::mem::replace(&mut seen[n], true) {
            continue;
        }
        stack.extend(adj[n].iter().map(|x| *x as usize));
    }
    false
}
