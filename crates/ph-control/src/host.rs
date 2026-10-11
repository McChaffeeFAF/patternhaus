use crate::StudioConfig;
use ph_commands::*;
use ph_core::Project;
use ph_engine::{Engine, PlayMode, RenderSettings, TransportCommand, TransportStatus};
use ph_formats::encode::{FlacDepth, WavFormat, write_flac, write_wav};
use ph_node::{AssetResolver, Quality};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

/// `ph_commands::Host` over the engine and the filesystem.
pub struct EngineHost {
    engine: Arc<Engine>,
    assets: Arc<dyn AssetResolver>,
    project_dir: PathBuf,
    library_roots: Vec<PathBuf>,
    exports_dir: Option<PathBuf>,
    ui: Sender<UiRequest>,
    recording: bool,
    /// The sample resolver the engine uses; moved along with the project folder on open.
    resolver: Option<Arc<ph_engine::assets::FileResolver>>,
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
            ui,
            recording: false,
            resolver: None,
        }
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
            recording: self.recording,
            mode,
            pattern,
            position: s.position,
            loop_range: s.loop_range,
            metronome: s.metronome,
        }
    }
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
        let audio = ph_engine::render_offline(project, &settings, self.assets.as_ref())
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

    fn ui(&mut self, request: UiRequest) -> CmdResult<()> {
        // A closed receiver just means no UI is attached (headless).
        let _ = self.ui.send(request);
        Ok(())
    }

    fn set_project_dir(&mut self, dir: &Path) {
        self.project_dir = dir.to_path_buf();
        if let Some(r) = &self.resolver {
            r.set_project_dir(dir);
        }
    }
}
