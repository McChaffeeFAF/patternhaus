use crate::StudioConfig;
use ph_commands::*;
use ph_core::Project;
use ph_engine::plugins::{ListedPlugin, PluginSlot};
use ph_engine::{Engine, PlayMode, RenderSettings, TransportCommand, TransportStatus};
use ph_formats::encode::{FlacDepth, WavFormat, write_flac, write_wav};
use ph_node::{AssetResolver, Quality};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// `ph_commands::Host` over the engine and the filesystem.
pub struct EngineHost {
    engine: Arc<Engine>,
    assets: Arc<dyn AssetResolver>,
    project_dir: PathBuf,
    library_roots: Vec<PathBuf>,
    exports_dir: Option<PathBuf>,
    projects_dir: Option<PathBuf>,
    ui: Sender<UiRequest>,
    recording: bool,
    /// The sample resolver the engine uses; moved along with the project folder on open.
    resolver: Option<Arc<ph_engine::assets::FileResolver>>,
    /// Set while a UI takes `ui_requests` (plugin editors open only there).
    ui_attached: Arc<AtomicBool>,
    /// MIDI input and learn (M2/T8); without it the `midi.*` commands refuse.
    midi: Option<Arc<ph_engine::midi::MidiService>>,
    /// MIDI notes are being collected for this pattern and channel (between `record.start`
    /// and `record.stop`).
    note_target: Option<NoteCapture>,
}

impl EngineHost {
    pub fn new(
        engine: Arc<Engine>,
        assets: Arc<dyn AssetResolver>,
        config: &StudioConfig,
        ui: Sender<UiRequest>,
    ) -> Self {
        Self {
            engine,
            assets,
            project_dir: config.project_dir.clone(),
            library_roots: config.library_roots.clone(),
            exports_dir: config.exports_dir.clone(),
            projects_dir: config.projects_dir.clone(),
            ui,
            recording: false,
            resolver: None,
            ui_attached: Arc::new(AtomicBool::new(false)),
            midi: None,
            note_target: None,
        }
    }

    /// Connects the `midi.*` commands and MIDI note recording to the MIDI service.
    pub fn with_midi(mut self, midi: Arc<ph_engine::midi::MidiService>) -> Self {
        self.midi = Some(midi);
        self
    }

    fn midi(&self) -> CmdResult<&Arc<ph_engine::midi::MidiService>> {
        self.midi
            .as_ref()
            .ok_or_else(|| CmdError::Host("this session has no MIDI input".into()))
    }

    fn rec_err(e: ph_engine::EngineError) -> CmdError {
        CmdError::Host(e.to_string())
    }

    /// Shares the flag that says a UI is attached (see `Studio::set_ui_attached`).
    pub fn with_ui_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.ui_attached = flag;
        self
    }

    fn plugin_env(&self) -> CmdResult<ph_engine::plugins::PluginEnv> {
        self.engine
            .plugin_env()
            .ok_or_else(|| CmdError::Host("plugin hosting is off in this session".into()))
    }

    /// Lets [`Host::set_project_dir`] move the engine's project-relative sample lookup too.
    pub fn with_resolver(mut self, resolver: Arc<ph_engine::assets::FileResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    fn state(&self, s: TransportStatus) -> TransportState {
        let (mode, pattern) = match s.mode {
            PlayMode::Song => (TransportMode::Song, None),
            PlayMode::Pattern(p) => (TransportMode::Pattern, Some(p)),
        };
        TransportState {
            playing: s.playing,
            recording: self.recording || self.engine.is_recording() || self.note_target.is_some(),
            mode,
            pattern,
            position: s.position,
            loop_range: s.loop_range,
            metronome: s.metronome,
        }
    }
}

/// `Documents/Patternhaus Projects` for the current user (`%USERPROFILE%` on Windows, `$HOME`
/// elsewhere); `None` when the home folder is unknown. Nothing is created here.
pub fn default_projects_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())?;
    Some(home.join("Documents").join("Patternhaus Projects"))
}

/// Lexically normalises `path` against `base`, refusing `..` that would climb out.
fn normalise(base: &Path, path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Some(out)
}

impl Host for EngineHost {
    fn transport(&mut self, cmd: TransportCmd) -> CmdResult<TransportState> {
        let e = &self.engine;
        let status = match cmd {
            TransportCmd::Play => e.transport(TransportCommand::Play),
            TransportCmd::Stop => e.transport(TransportCommand::Stop),
            TransportCmd::Toggle => {
                let playing = e.status().playing;
                e.transport(if playing {
                    TransportCommand::Stop
                } else {
                    TransportCommand::Play
                })
            }
            TransportCmd::Record => {
                // Recording arrives in M2; the flag is tracked so the UI can show it.
                self.recording = !self.recording;
                e.status()
            }
            TransportCmd::Seek { position } => e.transport(TransportCommand::Seek(position)),
            TransportCmd::SetMode { mode, pattern } => match (mode, pattern) {
                (TransportMode::Song, _) => e.transport(TransportCommand::SetMode(PlayMode::Song)),
                (TransportMode::Pattern, Some(p)) => {
                    e.transport(TransportCommand::SetMode(PlayMode::Pattern(p)))
                }
                (TransportMode::Pattern, None) => {
                    return Err(CmdError::Host("pattern mode needs a pattern".into()));
                }
            },
            TransportCmd::SetLoop { range } => e.transport(TransportCommand::SetLoop(range)),
            TransportCmd::SetMetronome { on } => e.transport(TransportCommand::SetMetronome(on)),
        };
        Ok(self.state(status))
    }

    fn transport_state(&self) -> TransportState {
        self.state(self.engine.status())
    }

    fn render(&mut self, project: &Project, req: &RenderRequest) -> CmdResult<RenderReport> {
        let path = PathBuf::from(&req.path);
        let mode = match self.engine.status().mode {
            PlayMode::Pattern(p) if project.clips.is_empty() => PlayMode::Pattern(p),
            _ => PlayMode::Song,
        };
        let settings = RenderSettings {
            mode,
            range: req.range,
            sample_rate: req.sample_rate,
            quality: if req.high_quality {
                Quality::High
            } else {
                Quality::Live
            },
            include_tail: req.include_tail,
            block_frames: 512,
        };
        // plugins render from fresh instances loaded with the live session's states
        let plugins = self
            .engine
            .plugin_env()
            .map(|env| env.with_states(self.engine.plugin_states()));
        let audio = ph_engine::render_offline_with(
            project,
            &settings,
            self.assets.as_ref(),
            plugins.as_ref(),
        )
        .map_err(|e| CmdError::Host(e.to_string()))?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let written = match (ext.as_str(), req.bit_depth) {
            ("wav", 16) => write_wav(&path, &audio.channels, audio.sample_rate, WavFormat::Pcm16),
            ("wav", 24) => write_wav(&path, &audio.channels, audio.sample_rate, WavFormat::Pcm24),
            ("wav", 32) => write_wav(
                &path,
                &audio.channels,
                audio.sample_rate,
                WavFormat::Float32,
            ),
            ("flac", 16) => {
                write_flac(&path, &audio.channels, audio.sample_rate, FlacDepth::Bits16)
            }
            ("flac", 24) => {
                write_flac(&path, &audio.channels, audio.sample_rate, FlacDepth::Bits24)
            }
            (e, b) => {
                return Err(CmdError::Host(format!(
                    "unsupported export format .{e} at {b}-bit (use .wav 16/24/32 or .flac 16/24)"
                )));
            }
        };
        written.map_err(|e| CmdError::Host(e.to_string()))?;
        let peak = audio
            .channels
            .iter()
            .flatten()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        Ok(RenderReport {
            path,
            seconds: audio.frames() as f64 / audio.sample_rate as f64,
            peak_dbfs: if peak > 0.0 {
                20.0 * (peak as f64).log10()
            } else {
                f64::NEG_INFINITY
            },
        })
    }

    fn resolve_path(&self, path: &str, access: FileAccess) -> CmdResult<PathBuf> {
        let deny = || {
            CmdError::Host(format!(
                "{path}: outside the folders this session may {}",
                match access {
                    FileAccess::Read => "read",
                    FileAccess::Write => "write",
                }
            ))
        };
        let p = normalise(&self.project_dir, path).ok_or_else(deny)?;
        let mut roots: Vec<&PathBuf> = vec![&self.project_dir];
        roots.extend(self.exports_dir.iter());
        roots.extend(self.projects_dir.iter());
        if access == FileAccess::Read {
            roots.extend(self.library_roots.iter());
        }
        let roots: Vec<PathBuf> = roots
            .into_iter()
            .filter_map(|r| normalise(Path::new("/"), &r.to_string_lossy()))
            .collect();
        if roots.iter().any(|r| p.starts_with(r)) {
            Ok(p)
        } else {
            Err(deny())
        }
    }

    fn import_file(
        &mut self,
        source: &Path,
        into: ImportKind,
        author: &Author,
    ) -> CmdResult<String> {
        let ImportKind::Sample = into;
        let source = match author {
            // The drop is the human's intent: any path they can read.
            Author::Human if source.is_absolute() => source.to_path_buf(),
            // Agents (and relative paths) stay inside the allowed roots.
            _ => self.resolve_path(&source.to_string_lossy(), FileAccess::Read)?,
        };
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| CmdError::Host(format!("{}: not a file", source.display())))?;
        let bytes = std::fs::read(&source)
            .map_err(|e| CmdError::Host(format!("{}: {e}", source.display())))?;
        let ext = source.extension().and_then(|e| e.to_str());
        ph_formats::decode::decode_bytes(&bytes, ext)
            .map_err(|e| CmdError::Host(format!("cannot use {name} as a sample: {e}")))?;

        let dir = self.project_dir.join("samples");
        // The same bytes are already there: reuse that file.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut found: Vec<String> = entries
                .flatten()
                .filter(|e| e.metadata().is_ok_and(|m| m.len() == bytes.len() as u64))
                .filter(|e| std::fs::read(e.path()).is_ok_and(|b| b == bytes))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            found.sort();
            if let Some(existing) = found.into_iter().next() {
                return Ok(format!("samples/{existing}"));
            }
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| CmdError::Host(format!("{}: {e}", dir.display())))?;
        // Keep the name; on a clash add " (2)", " (3)" ...
        let path = Path::new(&name);
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
        let mut n = 1u32;
        let (target, final_name) = loop {
            let candidate = match (n, &ext) {
                (1, _) => name.clone(),
                (n, Some(e)) => format!("{stem} ({n}).{e}"),
                (n, None) => format!("{stem} ({n})"),
            };
            let target = dir.join(&candidate);
            if !target.exists() {
                break (target, candidate);
            }
            n += 1;
        };
        std::fs::write(&target, &bytes)
            .map_err(|e| CmdError::Host(format!("{}: {e}", target.display())))?;
        Ok(format!("samples/{final_name}"))
    }

    fn sample_info(&self, reference: &str) -> CmdResult<SampleInfo> {
        let d = self
            .assets
            .sample(reference)
            .map_err(|e| CmdError::Host(format!("cannot read {reference}: {e}")))?;
        Ok(SampleInfo {
            frames: d.frames() as u64,
            sample_rate: d.sample_rate.round() as u32,
            channels: d.channels.len() as u16,
            root_key: d.root_key,
        })
    }

    fn read_audio(&self, reference: &str) -> CmdResult<AudioBuffer> {
        let d = self
            .assets
            .sample(reference)
            .map_err(|e| CmdError::Host(format!("cannot read {reference}: {e}")))?;
        Ok(AudioBuffer {
            channels: d.channels.clone(),
            sample_rate: d.sample_rate.round() as u32,
            root_key: d.root_key,
        })
    }

    fn write_audio(&mut self, name: &str, audio: &AudioBuffer) -> CmdResult<String> {
        let dir = self.project_dir.join("samples");
        std::fs::create_dir_all(&dir)
            .map_err(|e| CmdError::Host(format!("{}: {e}", dir.display())))?;
        // a plain file name: no folders, no extension games
        // only a trailing ".wav" is dropped: dots inside the name stay
        let stem: String = name
            .strip_suffix(".wav")
            .unwrap_or(name)
            .chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        let stem = if stem.trim().is_empty() {
            "edit".to_string()
        } else {
            stem.trim().to_string()
        };
        // never replace a file: add " (2)", " (3)" ... on a clash
        let mut n = 1u32;
        let (target, final_name) = loop {
            let candidate = if n == 1 {
                format!("{stem}.wav")
            } else {
                format!("{stem} ({n}).wav")
            };
            let target = dir.join(&candidate);
            if !target.exists() {
                break (target, candidate);
            }
            n += 1;
        };
        write_wav(
            &target,
            &audio.channels,
            audio.sample_rate,
            WavFormat::Float32,
        )
        .map_err(|e| CmdError::Host(format!("{}: {e}", target.display())))?;
        Ok(format!("samples/{final_name}"))
    }

    fn analyze_audio(&self, reference: &str, sensitivity: f32) -> CmdResult<AudioAnalysis> {
        let d = self
            .assets
            .sample(reference)
            .map_err(|e| CmdError::Host(format!("cannot read {reference}: {e}")))?;
        let t = ph_analysis::tempo::estimate(&d.channels, d.sample_rate);
        let onsets = ph_analysis::tempo::onsets(&d.channels, d.sample_rate, sensitivity as f64);
        Ok(AudioAnalysis {
            bpm: t.as_ref().map(|t| t.bpm),
            confidence: t.as_ref().map_or(0.0, |t| t.confidence),
            loop_beats: t.as_ref().and_then(|t| t.loop_beats),
            alternatives: t
                .as_ref()
                .map(|t| t.alternatives.iter().map(|a| (a.bpm, a.score)).collect())
                .unwrap_or_default(),
            onsets,
            sample_rate: d.sample_rate.round() as u32,
            key: ph_analysis::tempo::ONSETS_KEY.into(),
        })
    }

    fn ui(&mut self, request: UiRequest) -> CmdResult<()> {
        // A closed receiver just means no UI is attached (headless).
        let _ = self.ui.send(request);
        Ok(())
    }

    fn projects_dir(&self) -> Option<PathBuf> {
        self.projects_dir.clone()
    }

    // ---- recording and MIDI (M2/T8)

    fn record_inputs(&mut self) -> CmdResult<RecordInputs> {
        let armed = self.engine.armed_inputs();
        let inputs = self
            .engine
            .inputs()
            .into_iter()
            .map(|i| {
                let a = armed.iter().find(|(info, _, _)| info.id == i.id);
                RecordInput {
                    id: i.id.0,
                    name: i.name,
                    channels: i.channels,
                    sample_rate: i.sample_rate,
                    armed: a.is_some(),
                    monitor: a.is_some_and(|a| a.2),
                    track: a.and_then(|a| match a.1 {
                        ph_engine::record::RecordTarget::Track(t) => Some(t),
                        _ => None,
                    }),
                    channel: a.and_then(|a| match a.1 {
                        ph_engine::record::RecordTarget::Channel(c) => Some(c),
                        _ => None,
                    }),
                }
            })
            .collect();
        let l = self.engine.latency();
        let ms = |frames: u32| frames as f64 * 1000.0 / l.sample_rate.max(1) as f64;
        Ok(RecordInputs {
            inputs,
            latency: LatencyInfo {
                sample_rate: l.sample_rate,
                input_frames: l.input_frames,
                output_frames: l.output_frames,
                monitor_ms: l.monitor_seconds * 1000.0,
                compensation_ms: ms(l.compensation_frames),
            },
            recording: self.engine.is_recording() || self.note_target.is_some(),
        })
    }

    fn record_arm(&mut self, arm: ArmRequest) -> CmdResult<()> {
        let target = match (arm.track, arm.channel) {
            (Some(t), None) => ph_engine::record::RecordTarget::Track(t),
            (None, Some(c)) => ph_engine::record::RecordTarget::Channel(c),
            _ => return Err(CmdError::Host("give a track or a channel".into())),
        };
        self.engine
            .arm_with(ph_engine::record::Arm {
                input: ph_engine::record::InputId(arm.input),
                target,
                monitor: arm.monitor,
            })
            .map_err(Self::rec_err)
    }

    fn record_disarm(&mut self, input: u32) -> CmdResult<()> {
        self.engine.disarm(ph_engine::record::InputId(input));
        Ok(())
    }

    fn record_set_monitor(&mut self, input: u32, on: bool) -> CmdResult<()> {
        self.engine
            .set_monitor(ph_engine::record::InputId(input), on)
            .map_err(Self::rec_err)
    }

    fn record_start(&mut self, start: RecordStart) -> CmdResult<RecordStarted> {
        if self.engine.is_recording() || self.note_target.is_some() {
            return Err(CmdError::Host("already recording".into()));
        }
        let audio = self.engine.armed_inputs().len() as u32;
        if audio == 0 && start.notes.is_none() {
            return Err(CmdError::Host(
                "nothing to record: arm an audio input (record.arm), or switch to pattern mode with a channel selected to record MIDI notes".into(),
            ));
        }
        if audio > 0 {
            let mut opts = ph_engine::record::RecordOptions::new(self.project_dir.join("samples"));
            opts.count_in_bars = start.count_in_bars;
            opts.punch = start.punch;
            opts.latency_ms = start.latency_ms;
            self.engine.record_start(opts).map_err(Self::rec_err)?;
        }
        if let Some(n) = start.notes {
            let midi = self.midi()?.clone();
            midi.set_target_channel(Some(n.channel));
            midi.start_note_recording();
            self.note_target = Some(n);
            if audio == 0 {
                self.engine.transport(TransportCommand::Play);
            }
        }
        Ok(RecordStarted {
            audio_inputs: audio,
            notes: self.note_target.is_some(),
        })
    }

    fn record_stop(&mut self) -> CmdResult<RecordOutcome> {
        let audio = self.engine.is_recording();
        if !audio && self.note_target.is_none() {
            return Err(CmdError::Host("not recording".into()));
        }
        let end = self.engine.song_position();
        let files = if audio {
            self.engine.record_stop().map_err(Self::rec_err)
        } else {
            Ok(Vec::new())
        };
        let note_target = self.note_target.take();
        let notes = match (&note_target, &self.midi) {
            (Some(_), Some(m)) => m
                .finish_note_recording(end)
                .into_iter()
                .map(|n| CapturedNote {
                    key: n.key,
                    velocity: n.velocity,
                    start: n.start,
                    length: n.length,
                })
                .collect(),
            _ => Vec::new(),
        };
        self.engine.transport(TransportCommand::Stop);
        let takes = files?
            .into_iter()
            .map(|f| TakeInfo {
                input: f.input.name,
                track: match f.target {
                    ph_engine::record::RecordTarget::Track(t) => Some(t),
                    _ => None,
                },
                channel: match f.target {
                    ph_engine::record::RecordTarget::Channel(c) => Some(c),
                    _ => None,
                },
                reference: format!("samples/{}", f.file),
                start: f.start,
                length: f.length,
                frames: f.frames,
                sample_rate: f.sample_rate,
                channels: f.channels,
                latency_frames: f.latency_frames,
                overflow: f.overflow,
                error: f.error,
            })
            .collect();
        Ok(RecordOutcome {
            takes,
            notes,
            note_target,
        })
    }

    fn midi_target_channel(&self) -> Option<ph_core::ChannelId> {
        self.midi.as_ref().and_then(|m| m.target_channel())
    }

    fn midi_inputs(&mut self) -> CmdResult<Vec<MidiInputEntry>> {
        Ok(self
            .midi()?
            .inputs()
            .into_iter()
            .map(|i| MidiInputEntry {
                name: i.name,
                connected: i.connected,
            })
            .collect())
    }

    fn midi_learn(&mut self, target: Option<ph_core::ParamPath>) -> CmdResult<()> {
        self.midi()?.learn(target);
        Ok(())
    }

    fn midi_mappings(&mut self) -> CmdResult<Vec<MidiMappingEntry>> {
        Ok(self
            .midi()?
            .mappings()
            .into_iter()
            .map(|m| MidiMappingEntry {
                controller: m.controller,
                device: m.device,
                target: m.target,
            })
            .collect())
    }

    fn midi_map(&mut self, m: MidiMappingEntry) -> CmdResult<Option<MidiMappingEntry>> {
        let old = self
            .midi()?
            .map(ph_engine::midi::Mapping {
                controller: m.controller,
                device: m.device,
                target: m.target,
            })
            .map_err(|e| CmdError::Host(format!("could not save the MIDI mappings: {e}")))?;
        Ok(old.map(|m| MidiMappingEntry {
            controller: m.controller,
            device: m.device,
            target: m.target,
        }))
    }

    fn midi_unmap(
        &mut self,
        controller: u8,
        device: Option<String>,
    ) -> CmdResult<Option<MidiMappingEntry>> {
        let old = self
            .midi()?
            .unmap(controller, device.as_deref())
            .map_err(|e| CmdError::Host(format!("could not save the MIDI mappings: {e}")))?;
        Ok(old.map(|m| MidiMappingEntry {
            controller: m.controller,
            device: m.device,
            target: m.target,
        }))
    }

    fn set_project_dir(&mut self, dir: &Path) {
        self.project_dir = dir.to_path_buf();
        if let Some(r) = &self.resolver {
            r.set_project_dir(dir);
        }
        if let Some(env) = self.engine.plugin_env() {
            env.set_project_dir(dir);
        }
    }

    fn plugin_scan(&mut self, paths: &[PathBuf]) -> CmdResult<PluginScanResult> {
        let env = self.plugin_env()?;
        let report = env.host.scan(paths, |_| {}).map_err(CmdError::Host)?;
        Ok(PluginScanResult {
            plugins: env.host.list().into_iter().map(entry).collect(),
            failed: report
                .failed
                .into_iter()
                .map(|f| PluginScanFailure {
                    path: f.path.display().to_string(),
                    error: f.error,
                })
                .collect(),
        })
    }

    fn plugin_list(&self) -> CmdResult<Vec<PluginEntry>> {
        Ok(self
            .plugin_env()?
            .host
            .list()
            .into_iter()
            .map(entry)
            .collect())
    }

    fn plugin_params(&self, project: &Project, target: PluginTarget) -> CmdResult<PluginParams> {
        let env = self.plugin_env()?;
        let (format, id, spec) = plugin_spec(project, target)?;
        let slot = slot_of(target);
        if let Some(lp) = self.engine.plugin(slot)
            && lp.info.id == id
            && lp.info.format == format
            && !lp.fault.is_dead()
        {
            let values = lp.values();
            let params = lp
                .desc
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let value = values
                        .as_ref()
                        .and_then(|v| v.get(i).copied())
                        .or_else(|| spec.get(&p.id).copied())
                        .unwrap_or(p.default);
                    let text = lp.control.as_ref().and_then(|c| c.param_text(i, value));
                    param(p, value, text)
                })
                .collect();
            return Ok(PluginParams {
                target: Some(target),
                format,
                plugin_id: id,
                name: lp.info.name.clone(),
                live: values.is_some(),
                latency: lp.latency(),
                has_editor: lp.info.has_editor,
                params,
            });
        }
        let mut d = describe(&env, format, &id)?;
        for p in &mut d.params {
            if let Some(v) = spec.get(&p.id) {
                p.value = *v;
            }
        }
        d.target = Some(target);
        Ok(d)
    }

    fn plugin_describe(&self, format: ph_core::PluginFormat, id: &str) -> CmdResult<PluginParams> {
        describe(&self.plugin_env()?, format, id)
    }

    fn save_plugin_states(&mut self, project: &Project, dir: &Path) -> CmdResult<Vec<String>> {
        let refs = state_refs(project);
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        let live = self.engine.plugin_states();
        let mut written = Vec::new();
        for r in refs {
            let rel = Path::new(&r);
            if rel.is_absolute() || rel.components().any(|c| c == Component::ParentDir) {
                continue;
            }
            let target = dir.join(rel);
            let bytes = match live.get(&r) {
                Some(b) => b.clone(),
                // not running (not loaded, isolated, crashed): keep the state it had
                None => match std::fs::read(self.project_dir.join(rel)) {
                    Ok(b) => b,
                    Err(_) => continue,
                },
            };
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CmdError::Host(format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(&target, bytes)
                .map_err(|e| CmdError::Host(format!("{}: {e}", target.display())))?;
            written.push(r);
        }
        Ok(written)
    }

    fn plugin_editor(&mut self, target: PluginTarget, open: bool) -> CmdResult<bool> {
        let _ = self.ui.send(UiRequest::PluginEditor { target, open });
        Ok(self.ui_attached.load(Ordering::Relaxed))
    }
}

fn entry(p: ListedPlugin) -> PluginEntry {
    PluginEntry {
        format: p.info.format,
        id: p.info.id,
        name: p.info.name,
        vendor: p.info.vendor,
        version: p.info.version,
        kind: match p.info.kind {
            ph_engine::plugins::PluginKind::Instrument => "instrument",
            ph_engine::plugins::PluginKind::Effect => "effect",
            ph_engine::plugins::PluginKind::NoteEffect => "note_effect",
        }
        .into(),
        has_editor: p.info.has_editor,
        status: match p.status {
            ph_engine::plugins::PluginStatus::Ok => "ok",
            ph_engine::plugins::PluginStatus::Isolate => "isolate",
            ph_engine::plugins::PluginStatus::Blocked => "blocked",
        }
        .into(),
        reason: p.reason,
        path: p.info.path.display().to_string(),
    }
}

/// The engine's name for a command target.
pub fn slot_of(target: PluginTarget) -> PluginSlot {
    match target {
        PluginTarget::Channel(c) => PluginSlot::Channel(c),
        PluginTarget::Effect { insert, slot } => PluginSlot::Effect { insert, slot },
    }
}

type Spec = (
    ph_core::PluginFormat,
    String,
    std::collections::BTreeMap<String, f64>,
);

fn plugin_spec(project: &Project, target: PluginTarget) -> CmdResult<Spec> {
    let not = || CmdError::Host(format!("no plugin at {target}"));
    match target {
        PluginTarget::Channel(c) => match &project.channel(c).map_err(|_| not())?.instrument {
            ph_core::InstrumentSpec::Plugin {
                format,
                plugin_id,
                params,
                ..
            } => Ok((*format, plugin_id.clone(), params.clone())),
            _ => Err(not()),
        },
        PluginTarget::Effect { insert, slot } => match project
            .mixer
            .get(insert as usize)
            .and_then(|i| i.effects.get(slot as usize))
            .map(|e| &e.effect)
        {
            Some(ph_core::EffectSpec::Plugin {
                format,
                plugin_id,
                params,
                ..
            }) => Ok((*format, plugin_id.clone(), params.clone())),
            _ => Err(not()),
        },
    }
}

fn param(p: &ph_node::ParamInfo, value: f64, text: Option<String>) -> PluginParam {
    PluginParam {
        id: p.id.clone(),
        name: p.name.clone(),
        min: p.min,
        max: p.max,
        default: p.default,
        value,
        text,
        stepped: p.stepped,
        choices: p.choices.clone(),
        automatable: p.automatable,
    }
}

/// A scanned plugin's declared parameters, from a short-lived instance (off the audio thread).
fn describe(
    env: &ph_engine::plugins::PluginEnv,
    format: ph_core::PluginFormat,
    id: &str,
) -> CmdResult<PluginParams> {
    let inst = env
        .host
        .instantiate(format, id)
        .map_err(|e| CmdError::Host(e.to_string()))?;
    let info = inst.info().clone();
    let latency = inst.control().map_or(0, |c| c.latency());
    let params = inst
        .descriptor()
        .params
        .iter()
        .map(|p| param(p, p.default, None))
        .collect();
    Ok(PluginParams {
        target: None,
        format,
        plugin_id: id.to_string(),
        name: info.name,
        live: false,
        latency,
        has_editor: info.has_editor,
        params,
    })
}

/// Every `plugins/<file>` reference of the project.
fn state_refs(p: &Project) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for ch in p.channels.values() {
        if let ph_core::InstrumentSpec::Plugin { state: Some(s), .. } = &ch.instrument {
            out.insert(s.clone());
        }
    }
    for ins in p.mixer.iter() {
        for e in &ins.effects {
            if let ph_core::EffectSpec::Plugin { state: Some(s), .. } = &e.effect {
                out.insert(s.clone());
            }
        }
    }
    out
}
