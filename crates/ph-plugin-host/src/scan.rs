//! Finding and scanning plugins: search paths, the scan cache, the blocklist and the child
//! process that does the dangerous part.
//!
//! - [`candidates`] walks folders for `.clap` and `.vst3` files and bundles.
//! - [`ScanCache`] remembers every scanned file by path, modification time and size, the
//!   plugins it held, and each plugin's [`PluginStatus`] (ok, isolate, blocked). A file that
//!   crashed, hung or failed during a scan is blocked until it changes on disk.
//! - [`scan_files`] scans files one at a time in a **child process** ([`run_child`], the
//!   `ph-plugin-scan` helper), with a timeout, so a plugin that crashes or hangs while loading
//!   cannot take the app (or the rest of the scan) down. The child prints one line,
//!   `PH-SCAN-RESULT:{json}`, and exits.

use crate::*;
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Marker before the child's JSON result (plugins may print to stdout too).
pub const RESULT_MARKER: &str = "PH-SCAN-RESULT:";
/// Default time one file may take to scan.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The plugin file extension of a format.
pub fn extension(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
    }
}

fn format_of(path: &Path) -> Option<PluginFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "clap" => Some(PluginFormat::Clap),
        "vst3" => Some(PluginFormat::Vst3),
        _ => None,
    }
}

/// Plugin files and bundles of `formats` under `roots` (a root may itself be a plugin),
/// sorted. A bundle (a folder named `*.clap` / `*.vst3`) is one candidate; nothing inside it
/// is searched.
pub fn candidates(roots: &[PathBuf], formats: &[PluginFormat]) -> Vec<(PluginFormat, PathBuf)> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, u32)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((p, depth)) = stack.pop() {
        if let Some(f) = format_of(&p).filter(|f| formats.contains(f))
            && p.exists()
        {
            out.push((f, p));
            continue;
        }
        if depth > 8 || !p.is_dir() {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&p) {
            for e in rd.flatten() {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup();
    out
}

/// Modification time (ns since the epoch) and size of a file, or of the newest file and the
/// total size inside a bundle.
pub fn fingerprint(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let stamp = |m: &std::fs::Metadata| {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64)
    };
    if m.is_file() {
        return Some((stamp(&m), m.len()));
    }
    let (mut newest, mut size) = (stamp(&m), 0u64);
    let mut stack = vec![path.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                newest = newest.max(stamp(&m));
                size += m.len();
            }
        }
    }
    Some((newest, size))
}

/// How the last scan of a file went.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanOutcome {
    Ok,
    /// The format refused the file (not a plugin, no factory, cannot be created).
    Failed,
    /// The helper process died while loading it.
    Crashed,
    TimedOut,
}

/// One scanned file in the cache.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedFile {
    pub format: PluginFormat,
    pub mtime: u64,
    pub size: u64,
    pub outcome: ScanOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub plugins: Vec<PluginInfo>,
}

/// A plugin's status and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusEntry {
    pub status: PluginStatus,
    #[serde(default)]
    pub reason: String,
}

/// The scan cache and blocklist, persisted as JSON by [`crate::manager::PluginHost`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanCache {
    pub version: u32,
    /// By path.
    pub files: BTreeMap<PathBuf, CachedFile>,
    /// By [`plugin_key`]: plugins that are not `ok` (crashed: `isolate`; user-blocked).
    pub status: BTreeMap<String, StatusEntry>,
}

/// `clap:<id>` / `vst3:<id>`: how the cache and the blocklist name a plugin.
pub fn plugin_key(format: PluginFormat, id: &str) -> String {
    format!("{}:{id}", extension(format))
}

impl Default for ScanCache {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            files: BTreeMap::new(),
            status: BTreeMap::new(),
        }
    }
}

impl ScanCache {
    pub const VERSION: u32 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<ScanCache>(&b).ok())
            .filter(|c| c.version == Self::VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut c = self.clone();
        c.version = Self::VERSION;
        let json = serde_json::to_vec_pretty(&c).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    }

    /// The cached result for `path` if the file is unchanged since.
    pub fn fresh(&self, path: &Path) -> Option<&CachedFile> {
        let c = self.files.get(path)?;
        let (mtime, size) = fingerprint(path)?;
        (c.mtime == mtime && c.size == size).then_some(c)
    }

    /// Every plugin found, with its status, sorted by name.
    pub fn plugins(&self) -> Vec<(PluginInfo, PluginStatus)> {
        let mut out: Vec<(PluginInfo, PluginStatus)> = self
            .files
            .values()
            .flat_map(|f| f.plugins.iter())
            .map(|p| (p.clone(), self.status_of(p.format, &p.id)))
            .collect();
        out.sort_by(|a, b| {
            (a.0.name.to_lowercase(), &a.0.id).cmp(&(b.0.name.to_lowercase(), &b.0.id))
        });
        out
    }

    pub fn status_of(&self, format: PluginFormat, id: &str) -> PluginStatus {
        self.status
            .get(&plugin_key(format, id))
            .map_or(PluginStatus::Ok, |s| s.status)
    }

    pub fn set_status(
        &mut self,
        format: PluginFormat,
        id: &str,
        status: PluginStatus,
        reason: &str,
    ) {
        let key = plugin_key(format, id);
        if status == PluginStatus::Ok {
            self.status.remove(&key);
        } else {
            self.status.insert(
                key,
                StatusEntry {
                    status,
                    reason: reason.into(),
                },
            );
        }
    }

    /// Files whose scan failed, crashed or timed out (blocked until they change).
    pub fn blocked_files(&self) -> impl Iterator<Item = (&PathBuf, &CachedFile)> {
        self.files
            .iter()
            .filter(|(_, f)| f.outcome != ScanOutcome::Ok)
    }
}

/// How a scan reaches plugin code.
#[derive(Clone, Debug)]
pub enum ScanMode {
    /// Each file in a `ph-plugin-scan` child process (the default; required for real plugins).
    Child { helper: PathBuf, timeout: Duration },
    /// In this process, through the format hosts. Only for in-repo fakes in tests: a crashing
    /// plugin would take the process with it.
    InProcess,
}

/// What scanning one file produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChildResult {
    #[serde(default)]
    pub found: Vec<PluginInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn failed(error: String) -> ChildResult {
    ChildResult {
        found: Vec::new(),
        error: Some(error),
    }
}

/// Scans one file in a child process: `helper <clap|vst3> <path>`.
pub fn scan_in_child(
    helper: &Path,
    timeout: Duration,
    format: PluginFormat,
    path: &Path,
) -> (ScanOutcome, ChildResult) {
    let mut cmd = Command::new(helper);
    cmd.arg(extension(format))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window when the app (a GUI process) starts the helper
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                ScanOutcome::Failed,
                failed(format!(
                    "cannot start the scan helper {}: {e}",
                    helper.display()
                )),
            );
        }
    };
    // read stdout on a thread so a chatty plugin cannot block on a full pipe
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let out = reader.join().unwrap_or_default();
    let Some(status) = status else {
        return (
            ScanOutcome::TimedOut,
            failed(format!("timed out after {} s", timeout.as_secs())),
        );
    };
    let parsed = out
        .lines()
        .rev()
        .find_map(|l| l.trim_end().strip_prefix(RESULT_MARKER))
        .and_then(|j| serde_json::from_str::<ChildResult>(j).ok());
    match parsed {
        Some(r) if r.error.is_none() => (ScanOutcome::Ok, r),
        Some(r) => (ScanOutcome::Failed, r),
        None => (
            ScanOutcome::Crashed,
            failed(match status.code() {
                Some(c) => format!(
                    "the plugin crashed the scan helper (exit code {:#x})",
                    c as u32
                ),
                None => "the plugin crashed the scan helper (killed by a signal)".into(),
            }),
        ),
    }
}

/// The child side, before loading anything: on Windows the plugin main thread moves to a
/// desktop of its own that is never shown, so a window a plugin opens while it is scanned (a
/// licence prompt, say) does not appear on the owner's screen; a plugin that waits for an
/// answer there hits the scan timeout instead. Windows that plugins open from threads of
/// their own still land on the normal desktop. A no-op elsewhere.
pub fn prepare_child() {
    #[cfg(windows)]
    crate::main_thread::main_thread().run(|| {
        use windows_sys::Win32::Foundation::GENERIC_ALL;
        use windows_sys::Win32::System::StationsAndDesktops::{CreateDesktopW, SetThreadDesktop};
        let name: Vec<u16> = format!("ph-plugin-scan-{}", std::process::id())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: a new desktop for this thread, before it has any window
        unsafe {
            let desk = CreateDesktopW(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                GENERIC_ALL,
                std::ptr::null(),
            );
            if !desk.is_null() {
                SetThreadDesktop(desk);
            }
        }
    });
}

/// The child side: scans `args = [format, path]` with `formats` and prints the result line.
/// Returns the process exit code. Used by the `ph-plugin-scan` helper.
pub fn run_child(args: &[String], formats: &[Arc<dyn PluginFormatHost>]) -> i32 {
    let result = match args {
        [format, path] => {
            let path = PathBuf::from(path);
            match formats
                .iter()
                .find(|f| extension(f.format()) == format.as_str())
            {
                Some(f) => match f.scan(&path) {
                    Ok(found) => ChildResult { found, error: None },
                    Err(e) => failed(e.to_string()),
                },
                None => failed(format!("unknown plugin format {format:?}")),
            }
        }
        _ => failed("usage: ph-plugin-scan <clap|vst3> <path>".into()),
    };
    println!(
        "{RESULT_MARKER}{}",
        serde_json::to_string(&result).unwrap_or_else(|_| "{}".into())
    );
    0
}

/// Progress of a scan, for front ends.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanProgress {
    pub index: usize,
    pub total: usize,
    pub path: PathBuf,
    pub cached: bool,
}

/// Scans `files` into `cache`: unchanged files come from the cache, the rest are scanned one
/// at a time per `mode`. Returns what was found and what failed (including blocked files
/// that were skipped because they did not change).
pub fn scan_files(
    cache: &mut ScanCache,
    files: &[(PluginFormat, PathBuf)],
    mode: &ScanMode,
    formats: &[Arc<dyn PluginFormatHost>],
    mut progress: impl FnMut(&ScanProgress),
) -> ScanReport {
    let mut report = ScanReport::default();
    for (i, (format, path)) in files.iter().enumerate() {
        let cached = cache.fresh(path).cloned();
        progress(&ScanProgress {
            index: i,
            total: files.len(),
            path: path.clone(),
            cached: cached.is_some(),
        });
        let entry = match cached {
            Some(c) => c,
            None => {
                let (outcome, r) = match mode {
                    ScanMode::Child { helper, timeout } => {
                        scan_in_child(helper, *timeout, *format, path)
                    }
                    ScanMode::InProcess => match formats.iter().find(|f| f.format() == *format) {
                        Some(f) => match f.scan(path) {
                            Ok(found) => (ScanOutcome::Ok, ChildResult { found, error: None }),
                            Err(e) => (ScanOutcome::Failed, failed(e.to_string())),
                        },
                        None => (
                            ScanOutcome::Failed,
                            failed("no host for this format".into()),
                        ),
                    },
                };
                let (mtime, size) = fingerprint(path).unwrap_or((0, 0));
                let entry = CachedFile {
                    format: *format,
                    mtime,
                    size,
                    outcome,
                    error: r.error,
                    plugins: r.found,
                };
                cache.files.insert(path.clone(), entry.clone());
                entry
            }
        };
        match entry.outcome {
            ScanOutcome::Ok => report.found.extend(entry.plugins.iter().cloned()),
            _ => report.failed.push(ScanFailure {
                path: path.clone(),
                error: entry.error.clone().unwrap_or_else(|| "scan failed".into()),
            }),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_find_files_and_bundles_but_not_inside_bundles() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("a.clap"), b"x").unwrap();
        std::fs::create_dir_all(root.join("sub/b.vst3/Contents/x86_64-win")).unwrap();
        std::fs::write(root.join("sub/b.vst3/Contents/x86_64-win/b.vst3"), b"y").unwrap();
        std::fs::write(root.join("readme.txt"), b"z").unwrap();
        let all = candidates(
            &[root.to_path_buf()],
            &[PluginFormat::Clap, PluginFormat::Vst3],
        );
        assert_eq!(
            all,
            vec![
                (PluginFormat::Clap, root.join("a.clap")),
                (PluginFormat::Vst3, root.join("sub").join("b.vst3")),
            ]
        );
        let clap_only = candidates(&[root.to_path_buf()], &[PluginFormat::Clap]);
        assert_eq!(clap_only.len(), 1);
        // a missing root is not an error
        assert!(candidates(&[root.join("nope")], &[PluginFormat::Clap]).is_empty());
    }

    #[test]
    fn the_cache_is_keyed_by_path_mtime_and_size_and_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("x.clap");
        std::fs::write(&file, b"one").unwrap();
        let mut cache = ScanCache::new();
        let (mtime, size) = fingerprint(&file).unwrap();
        cache.files.insert(
            file.clone(),
            CachedFile {
                format: PluginFormat::Clap,
                mtime,
                size,
                outcome: ScanOutcome::Ok,
                error: None,
                plugins: vec![crate::fake::FakeFormat::info(&file)],
            },
        );
        assert!(cache.fresh(&file).is_some());
        cache.set_status(
            PluginFormat::Clap,
            crate::fake::FAKE_ID,
            PluginStatus::Isolate,
            "crashed",
        );
        let path = d.path().join("cache/plugins.json");
        cache.save(&path).unwrap();
        let back = ScanCache::load(&path);
        assert_eq!(back.files, cache.files);
        assert_eq!(
            back.status_of(PluginFormat::Clap, crate::fake::FAKE_ID),
            PluginStatus::Isolate
        );
        assert_eq!(back.plugins().len(), 1);
        // a changed file is stale
        std::fs::write(&file, b"longer").unwrap();
        assert!(back.fresh(&file).is_none());
        // a broken cache file starts empty
        std::fs::write(&path, b"{nope").unwrap();
        assert!(ScanCache::load(&path).files.is_empty());
    }

    #[test]
    fn in_process_scans_report_found_and_failed_and_reuse_the_cache() {
        let d = tempfile::tempdir().unwrap();
        let good = d.path().join(crate::fake::FAKE_FILE);
        let bad = d.path().join("other.clap");
        std::fs::write(&good, b"").unwrap();
        std::fs::write(&bad, b"").unwrap();
        let formats: Vec<Arc<dyn PluginFormatHost>> = vec![Arc::new(crate::fake::FakeFormat)];
        let mut cache = ScanCache::new();
        let files = candidates(&[d.path().to_path_buf()], &[PluginFormat::Clap]);
        let mut seen = 0;
        let r = scan_files(&mut cache, &files, &ScanMode::InProcess, &formats, |_| {
            seen += 1
        });
        assert_eq!(seen, 2);
        assert_eq!(r.found.len(), 1);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].path, bad);
        assert_eq!(cache.blocked_files().count(), 1);
        let mut cached = 0;
        let r2 = scan_files(&mut cache, &files, &ScanMode::InProcess, &formats, |p| {
            cached += p.cached as usize
        });
        assert_eq!(cached, 2);
        assert_eq!(r2, r);
    }
}
