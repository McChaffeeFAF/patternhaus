//! The VST3 host (M2/T3), on the `vst3` crate (MIT OR Apache-2.0; VST 3 SDK MIT since 3.8).

use crate::*;

/// Standard VST3 folders for this platform.
pub fn vst3_default_paths() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(p) = env("COMMONPROGRAMFILES") {
            out.push(p.join("VST3"));
        }
        if let Some(p) = env("LOCALAPPDATA") {
            out.push(p.join("Programs").join("Common").join("VST3"));
        }
    } else if cfg!(target_os = "macos") {
        out.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(h) = env("HOME") {
            out.push(h.join("Library/Audio/Plug-Ins/VST3"));
        }
    } else {
        if let Some(h) = env("HOME") {
            out.push(h.join(".vst3"));
        }
        out.push(PathBuf::from("/usr/lib/vst3"));
        out.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    out
}

/// The VST3 format host.
#[derive(Default)]
pub struct Vst3Format;

impl Vst3Format {
    pub fn new() -> Self {
        crate::guard::install();
        Self
    }
}

impl PluginFormatHost for Vst3Format {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn default_paths(&self) -> Vec<PathBuf> {
        vst3_default_paths()
    }

    fn scan(&self, path: &Path) -> PluginResult<Vec<PluginInfo>> {
        Err(PluginError::NotAPlugin(path.to_path_buf()))
    }

    fn instantiate(&self, info: &PluginInfo) -> PluginResult<Box<dyn PluginInstance>> {
        Err(PluginError::NotFound(info.id.clone()))
    }
}
