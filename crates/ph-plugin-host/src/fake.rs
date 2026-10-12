//! An in-repo test plugin implementing the whole contract, so tests (here, in the engine
//! and in T3) never depend on a third-party plugin.
//!
//! [`FakeFormat`] "scans" any file named [`FAKE_FILE`] and finds one plugin, [`FAKE_ID`]: a
//! stereo gain effect with a crash-on-demand switch. Setting `crash` to 1 makes the next
//! `process` panic, which is how the crash-catch boundary is tested.

use crate::*;

/// The file name [`FakeFormat`] recognises.
pub const FAKE_FILE: &str = "ph-test-gain.clap";
/// The fake plugin's id.
pub const FAKE_ID: &str = "dev.patternhaus.test.gain";

/// A plugin format whose only plugin is [`FakeGain`].
#[derive(Default)]
pub struct FakeFormat;

impl FakeFormat {
    pub fn info(path: &Path) -> PluginInfo {
        PluginInfo {
            format: PluginFormat::Clap,
            id: FAKE_ID.into(),
            name: "Test Gain".into(),
            vendor: "Patternhaus".into(),
            version: "1.0.0".into(),
            kind: PluginKind::Effect,
            path: path.to_path_buf(),
            has_editor: true,
        }
    }
}

impl PluginFormatHost for FakeFormat {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn default_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn scan(&self, path: &Path) -> PluginResult<Vec<PluginInfo>> {
        if path.file_name().is_some_and(|n| n == FAKE_FILE) {
            Ok(vec![Self::info(path)])
        } else {
            Err(PluginError::NotAPlugin(path.to_path_buf()))
        }
    }

    fn instantiate(&self, info: &PluginInfo) -> PluginResult<Box<dyn PluginInstance>> {
        if info.id != FAKE_ID {
            return Err(PluginError::NotFound(info.id.clone()));
        }
        Ok(Box::new(FakeGain::new(info.clone())))
    }
}

const GAIN: usize = 0;
const CRASH: usize = 1;

/// Stereo gain with a crash switch. State: the gain as decimal text.
pub struct FakeGain {
    info: PluginInfo,
    desc: NodeDescriptor,
    params: [f64; 2],
}

impl FakeGain {
    pub fn new(info: PluginInfo) -> Self {
        let port = |name: &str| PortInfo {
            name: name.into(),
            channels: 2,
            sidechain: false,
        };
        let mut crash = ParamInfo::new("crash", "Crash", 0.0, 1.0, 0.0, ParamUnit::Switch);
        crash.stepped = true;
        crash.automatable = false;
        Self {
            desc: NodeDescriptor {
                id: format!("clap:{}", info.id),
                name: info.name.clone(),
                kind: NodeKind::Effect,
                audio_inputs: vec![port("in")],
                audio_outputs: vec![port("out")],
                note_input: false,
                params: vec![
                    ParamInfo::new("gain", "Gain", 0.0, 2.0, 1.0, ParamUnit::Ratio),
                    crash,
                ],
            },
            info,
            params: [1.0, 0.0],
        }
    }
}

impl Processor for FakeGain {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.desc
    }

    fn activate(&mut self, _: &ActivateInfo) {}

    fn process(&mut self, _: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        if self.params[CRASH] >= 0.5 {
            panic!("test plugin crashed on demand");
        }
        let g = self.params[GAIN] as f32;
        for (c, out) in io.outputs[0].iter_mut().enumerate() {
            match io.inputs.first().and_then(|p| p.get(c)) {
                Some(input) => {
                    for (o, i) in out.iter_mut().zip(input.iter()) {
                        *o = *i * g;
                    }
                }
                None => out.fill(0.0),
            }
        }
        ProcessStatus::Continue
    }

    fn set_param(&mut self, id: ParamId, value: f64) {
        if let Some(p) = self.desc.params.get(id.0 as usize) {
            self.params[id.0 as usize] = p.clamp(value);
        }
    }

    fn param(&self, id: ParamId) -> f64 {
        self.params.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    fn reset(&mut self) {}

    fn save_state(&self) -> Vec<u8> {
        self.params[GAIN].to_string().into_bytes()
    }

    fn load_state(&mut self, state: &[u8], _: &dyn AssetResolver) -> Result<(), String> {
        let g: f64 = std::str::from_utf8(state)
            .map_err(|e| e.to_string())?
            .parse()
            .map_err(|e: std::num::ParseFloatError| e.to_string())?;
        self.set_param(ParamId(GAIN as u32), g);
        Ok(())
    }
}

impl PluginInstance for FakeGain {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        Some(Box::new(FakeEditor::default()))
    }
}

/// An editor that opens no window: it records what the host asked.
#[derive(Debug, Default)]
pub struct FakeEditor {
    size: Option<EditorSize>,
    /// Whether the last `open` was embedded.
    pub embedded: bool,
}

impl PluginEditor for FakeEditor {
    fn can_embed(&self) -> bool {
        true
    }

    fn open(&mut self, parent: Option<RawWindowHandle>) -> PluginResult<EditorSize> {
        self.embedded = parent.is_some();
        let s = EditorSize {
            width: 400,
            height: 300,
        };
        self.size = Some(s);
        Ok(s)
    }

    fn close(&mut self) {
        self.size = None;
    }

    fn is_open(&self) -> bool {
        self.size.is_some()
    }

    fn can_resize(&self) -> bool {
        true
    }

    fn set_size(&mut self, size: EditorSize) -> PluginResult<EditorSize> {
        if self.size.is_none() {
            return Err(PluginError::Call("editor is closed".into()));
        }
        // the fake keeps a minimum size, like real plugins do
        let s = EditorSize {
            width: size.width.max(200),
            height: size.height.max(150),
        };
        self.size = Some(s);
        Ok(s)
    }

    fn size(&self) -> Option<EditorSize> {
        self.size
    }
}
