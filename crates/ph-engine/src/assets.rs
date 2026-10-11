//! Sample loading for processors: [`FileResolver`], an [`AssetResolver`] over
//! `ph_formats::decode`.
//!
//! Reference forms (see `ph_core::InstrumentSpec::Sampler`):
//! - `factory:<path>` → `<assets_dir>/samples/<path>.wav`, e.g. `factory:drums/haus808/kick`
//!   → `assets/samples/drums/haus808/kick.wav`;
//! - a relative path → inside the project folder;
//! - an absolute path → only inside the project folder or an allowed library root.
//!
//! Paths are canonicalised before the root check, so `..` and links cannot escape. Decoded
//! samples are cached by canonical path and shared as `Arc<SampleData>`. Called only off
//! the audio thread (plan compilation).

use ph_node::{AssetResolver, SampleData};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

/// Prefix of factory sample references.
pub const FACTORY_PREFIX: &str = "factory:";

#[derive(Default)]
pub struct FileResolver {
    /// Changes when another project is opened ([`FileResolver::set_project_dir`]).
    project_dir: RwLock<Option<PathBuf>>,
    assets_dir: Option<PathBuf>,
    /// Grows when a library folder is added at runtime ([`FileResolver::add_root`]).
    allowed_roots: RwLock<Vec<PathBuf>>,
    cache: Mutex<HashMap<PathBuf, Arc<SampleData>>>,
}

impl FileResolver {
    /// A resolver with no folders: every reference fails until folders are added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folder that project-relative references resolve against.
    pub fn with_project_dir(self, dir: impl Into<PathBuf>) -> Self {
        self.set_project_dir(dir);
        self
    }

    /// Moves project-relative resolution to another folder (a different project was opened).
    pub fn set_project_dir(&self, dir: impl Into<PathBuf>) {
        *self.project_dir.write().unwrap_or_else(|e| e.into_inner()) = Some(dir.into());
    }

    /// The folder project-relative references resolve against.
    pub fn project_dir(&self) -> Option<PathBuf> {
        self.project_dir
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The factory content folder (the repository's `assets/`); `factory:` references
    /// resolve to `<dir>/samples/<path>.wav`.
    pub fn with_assets_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.assets_dir = Some(dir.into());
        self
    }

    /// Allows absolute references inside a library folder.
    pub fn allow_root(self, dir: impl Into<PathBuf>) -> Self {
        self.add_root(dir);
        self
    }

    /// Allows another library folder while running (no duplicates).
    pub fn add_root(&self, dir: impl Into<PathBuf>) {
        let dir = dir.into();
        let mut roots = self
            .allowed_roots
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }

    /// Maps a reference to the file it names, applying the access rules. The file must
    /// exist (the root check works on canonical paths).
    pub fn resolve_path(&self, reference: &str) -> Result<PathBuf, String> {
        if let Some(rest) = reference.strip_prefix(FACTORY_PREFIX) {
            let assets = self
                .assets_dir
                .as_ref()
                .ok_or("no factory assets folder is configured")?;
            let rel =
                relative(rest).ok_or_else(|| format!("invalid factory sample name `{rest}`"))?;
            let root = assets.join("samples");
            let mut file = root.join(rel);
            let name = file
                .file_name()
                .map(|n| format!("{}.wav", n.to_string_lossy()))
                .ok_or_else(|| format!("invalid factory sample name `{rest}`"))?;
            file.set_file_name(name);
            return inside(&file, &[root]);
        }
        let path = Path::new(reference);
        if path.is_absolute() {
            let mut roots = self
                .allowed_roots
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            roots.extend(self.project_dir());
            return inside(path, &roots);
        }
        let project = self
            .project_dir()
            .ok_or("project-relative sample but the project has no folder")?;
        let rel =
            relative(reference).ok_or_else(|| format!("invalid sample path `{reference}`"))?;
        inside(&project.join(rel), std::slice::from_ref(&project))
    }

    /// Number of decoded samples held in the cache.
    pub fn cached(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }
}

impl AssetResolver for FileResolver {
    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        let path = self.resolve_path(reference)?;
        if let Some(hit) = self.cache.lock().map_err(|e| e.to_string())?.get(&path) {
            return Ok(hit.clone());
        }
        let data = ph_formats::decode::decode_file(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let data = Arc::new(data);
        self.cache
            .lock()
            .map_err(|e| e.to_string())?
            .insert(path, data.clone());
        Ok(data)
    }
}

/// A relative path made only of normal components (no `..`, root or drive prefix).
fn relative(s: &str) -> Option<PathBuf> {
    let p = Path::new(s);
    if s.is_empty() || p.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Canonical `path` if it lies inside one of `roots`.
fn inside(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for r in roots {
        if let Ok(rc) = r.canonicalize()
            && canon.starts_with(&rc)
        {
            return Ok(canon);
        }
    }
    Err(format!(
        "{} is outside the project and library folders",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(path: &Path, frames: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ch = vec![
            (0..frames)
                .map(|i| (i as f32 * 0.1).sin() * 0.5)
                .collect::<Vec<_>>(),
        ];
        ph_formats::encode::write_wav(path, &ch, 48_000, ph_formats::encode::WavFormat::Float32)
            .unwrap();
    }

    #[test]
    fn factory_project_and_absolute_references() {
        let tmp = tempfile::tempdir().unwrap();
        let assets = tmp.path().join("assets");
        let project = tmp.path().join("song");
        let library = tmp.path().join("lib");
        let outside = tmp.path().join("elsewhere");
        wav(&assets.join("samples/drums/haus808/kick.wav"), 100);
        wav(&project.join("samples/vox.wav"), 200);
        wav(&library.join("pad.wav"), 300);
        wav(&outside.join("secret.wav"), 10);
        let r = FileResolver::new()
            .with_assets_dir(&assets)
            .with_project_dir(&project)
            .allow_root(&library);

        let k = r.sample("factory:drums/haus808/kick").unwrap();
        assert_eq!(k.frames(), 100);
        assert!(
            r.resolve_path("factory:drums/haus808/kick")
                .unwrap()
                .ends_with(Path::new("samples/drums/haus808/kick.wav"))
        );
        assert_eq!(r.sample("samples/vox.wav").unwrap().frames(), 200);
        let abs = library.join("pad.wav");
        assert_eq!(r.sample(abs.to_str().unwrap()).unwrap().frames(), 300);

        // escapes are refused
        assert!(r.sample("factory:../song/samples/vox").is_err());
        assert!(r.sample("../elsewhere/secret.wav").is_err());
        assert!(
            r.sample(outside.join("secret.wav").to_str().unwrap())
                .is_err()
        );
        assert!(r.sample("factory:drums/missing").is_err());

        // cached and shared
        let k2 = r.sample("factory:drums/haus808/kick").unwrap();
        assert!(Arc::ptr_eq(&k, &k2));
        assert_eq!(r.cached(), 3);
    }

    #[test]
    fn unconfigured_folders_fail_cleanly() {
        let r = FileResolver::new();
        assert!(r.sample("factory:x").unwrap_err().contains("factory"));
        assert!(r.sample("a.wav").unwrap_err().contains("project"));
    }
}
