//! [`PluginHost`]: the formats, the scan cache and the blocklist behind one handle. The engine
//! instantiates plugins through it; commands scan and list through it.

use crate::scan::*;
use crate::*;
use std::time::Duration;

/// How a [`PluginHost`] is set up.
#[derive(Clone, Debug, Default)]
pub struct HostConfig {
    /// Where the scan cache and blocklist are kept (JSON); `None` keeps them in memory.
    pub cache_file: Option<PathBuf>,
    /// Folders scanned besides each format's standard folders.
    pub extra_paths: Vec<PathBuf>,
    /// The `ph-plugin-scan` helper; `None` looks next to the running executable
    /// ([`find_scan_helper`]).
    pub scan_helper: Option<PathBuf>,
    /// Per-file scan timeout; `None` = [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
}

/// The helper binary next to the running executable (or one folder up, for test binaries in
/// `target/<profile>/deps`), or `$PATTERNHAUS_PLUGIN_SCAN`.
pub fn find_scan_helper() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PATTERNHAUS_PLUGIN_SCAN") {
        return Some(PathBuf::from(p));
    }
    let name = if cfg!(windows) {
        "ph-plugin-scan.exe"
    } else {
        "ph-plugin-scan"
    };
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [Some(dir), dir.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// One plugin as `plugin.list` shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListedPlugin {
    #[serde(flatten)]
    pub info: PluginInfo,
    pub status: PluginStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Formats, cache and blocklist. Thread-safe; nothing here runs on the audio thread.
pub struct PluginHost {
    formats: Vec<Arc<dyn PluginFormatHost>>,
    cache: Mutex<ScanCache>,
    config: HostConfig,
    in_process: bool,
}

impl PluginHost {
    /// CLAP and VST3, with the cache loaded from `config.cache_file`.
    pub fn new(config: HostConfig) -> Self {
        Self::with_formats(
            config,
            vec![
                Arc::new(crate::clap::ClapFormat::new()),
                Arc::new(crate::vst3::Vst3Format::new()),
            ],
        )
    }

    pub fn with_formats(config: HostConfig, formats: Vec<Arc<dyn PluginFormatHost>>) -> Self {
        let cache = config
            .cache_file
            .as_deref()
            .map_or_else(ScanCache::new, ScanCache::load);
        Self {
            formats,
            cache: Mutex::new(cache),
            config,
            in_process: false,
        }
    }

    /// Scans in this process instead of a helper. Only for in-repo fakes in tests.
    pub fn scan_in_process(mut self) -> Self {
        self.in_process = true;
        self
    }

    pub fn formats(&self) -> &[Arc<dyn PluginFormatHost>] {
        &self.formats
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ScanCache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn persist(&self, cache: &ScanCache) {
        if let Some(f) = &self.config.cache_file
            && let Err(e) = cache.save(f)
        {
            eprintln!("cannot write the plugin cache {}: {e}", f.display());
        }
    }

    /// The standard folders of every format plus the configured extra folders.
    pub fn default_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .formats
            .iter()
            .flat_map(|f| f.default_paths())
            .collect();
        v.extend(self.config.extra_paths.iter().cloned());
        v.dedup();
        v
    }

    fn mode(&self) -> Result<ScanMode, String> {
        if self.in_process {
            return Ok(ScanMode::InProcess);
        }
        let helper = self
            .config
            .scan_helper
            .clone()
            .or_else(find_scan_helper)
            .ok_or("the plugin scan helper (ph-plugin-scan) was not found next to Patternhaus")?;
        Ok(ScanMode::Child {
            helper,
            timeout: self.config.timeout.unwrap_or(DEFAULT_TIMEOUT),
        })
    }

    /// Scans `paths` (folders or plugin files; empty = [`Self::default_paths`]) and updates the
    /// cache. Each file runs in the helper process, one at a time.
    pub fn scan(
        &self,
        paths: &[PathBuf],
        progress: impl FnMut(&ScanProgress),
    ) -> Result<ScanReport, String> {
        let roots = if paths.is_empty() {
            self.default_paths()
        } else {
            paths.to_vec()
        };
        let formats: Vec<PluginFormat> = self.formats.iter().map(|f| f.format()).collect();
        let files = candidates(&roots, &formats);
        let mode = self.mode()?;
        // scan without holding the lock (it may take minutes), then merge
        let mut working = self.lock().clone();
        let report = scan_files(&mut working, &files, &mode, &self.formats, progress);
        let mut cache = self.lock();
        for (_, path) in &files {
            if let Some(f) = working.files.get(path) {
                cache.files.insert(path.clone(), f.clone());
            }
        }
        self.persist(&cache);
        Ok(report)
    }

    /// Forgets every scanned file (not the plugin statuses), so the next scan reads all files
    /// again.
    pub fn forget_files(&self) {
        let mut c = self.lock();
        c.files.clear();
        self.persist(&c);
    }

    /// Every known plugin with its status.
    pub fn list(&self) -> Vec<ListedPlugin> {
        let cache = self.lock();
        cache
            .plugins()
            .into_iter()
            .map(|(info, status)| {
                let reason = cache
                    .status
                    .get(&plugin_key(info.format, &info.id))
                    .map(|s| s.reason.clone())
                    .unwrap_or_default();
                ListedPlugin {
                    info,
                    status,
                    reason,
                }
            })
            .collect()
    }

    /// Files the scanner could not read (blocked until they change).
    pub fn failures(&self) -> Vec<ScanFailure> {
        self.lock()
            .blocked_files()
            .map(|(p, f)| ScanFailure {
                path: p.clone(),
                error: f.error.clone().unwrap_or_default(),
            })
            .collect()
    }

    pub fn find(&self, format: PluginFormat, id: &str) -> Option<ListedPlugin> {
        self.list()
            .into_iter()
            .find(|p| p.info.format == format && p.info.id == id)
    }

    pub fn status(&self, format: PluginFormat, id: &str) -> PluginStatus {
        self.lock().status_of(format, id)
    }

    /// Sets a plugin's status by hand (block, isolate, or back to ok) and persists it.
    pub fn set_status(&self, format: PluginFormat, id: &str, status: PluginStatus, reason: &str) {
        let mut c = self.lock();
        c.set_status(format, id, status, reason);
        self.persist(&c);
    }

    /// Records a crash: the plugin is marked *isolate* (DESIGN §6) and no longer loaded
    /// in-process.
    pub fn mark_fault(&self, format: PluginFormat, fault: &PluginFault) {
        self.set_status(
            format,
            &fault.plugin_id,
            PluginStatus::Isolate,
            &format!("crashed in {:?}: {}", fault.call, fault.message),
        );
    }

    /// Creates an instance of a scanned plugin. Refuses isolated and blocked plugins (until
    /// M4 can load them out of process) and plugins the scan has not seen.
    pub fn instantiate(
        &self,
        format: PluginFormat,
        id: &str,
    ) -> PluginResult<Box<dyn PluginInstance>> {
        let found = self.find(format, id).ok_or_else(|| {
            PluginError::NotFound(format!(
                "{} (scan for plugins first)",
                plugin_key(format, id)
            ))
        })?;
        match found.status {
            PluginStatus::Ok => {}
            PluginStatus::Isolate => {
                return Err(PluginError::Call(format!(
                    "{} is isolated after a crash ({}); out-of-process loading arrives in M4",
                    found.info.name, found.reason
                )));
            }
            PluginStatus::Blocked => {
                return Err(PluginError::Call(format!(
                    "{} is blocked ({})",
                    found.info.name, found.reason
                )));
            }
        }
        let host = self
            .formats
            .iter()
            .find(|f| f.format() == format)
            .ok_or_else(|| PluginError::NotFound(plugin_key(format, id)))?;
        host.instantiate(&found.info)
    }

    /// The compatibility report (markdown) of what the cache knows.
    pub fn report_markdown(&self) -> String {
        report_markdown(&self.lock())
    }
}

/// A markdown compatibility report of a scan cache: counts by format and status, then every
/// plugin and every file that failed.
pub fn report_markdown(cache: &ScanCache) -> String {
    use std::fmt::Write;
    let plugins = cache.plugins();
    let mut s = String::new();
    let _ = writeln!(s, "# Plugin compatibility report\n");
    let _ = writeln!(
        s,
        "Generated by `patternhaus-cli plugin-scan --report`. Lists the plugins installed on this \
         machine; keep it local.\n"
    );
    let _ = writeln!(s, "## Summary\n");
    let _ = writeln!(
        s,
        "| Format | Files | Plugins ok | Isolated | Blocked | Instruments | Effects | With editor | Files failed | Crashed | Timed out |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|");
    for format in [PluginFormat::Clap, PluginFormat::Vst3] {
        let files: Vec<&CachedFile> = cache
            .files
            .values()
            .filter(|f| f.format == format)
            .collect();
        let ps: Vec<&(PluginInfo, PluginStatus)> =
            plugins.iter().filter(|p| p.0.format == format).collect();
        let count = |st: PluginStatus| ps.iter().filter(|p| p.1 == st).count();
        let outcome = |o: ScanOutcome| files.iter().filter(|f| f.outcome == o).count();
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            extension(format).to_uppercase(),
            files.len(),
            count(PluginStatus::Ok),
            count(PluginStatus::Isolate),
            count(PluginStatus::Blocked),
            ps.iter()
                .filter(|p| p.0.kind == PluginKind::Instrument)
                .count(),
            ps.iter()
                .filter(|p| p.0.kind != PluginKind::Instrument)
                .count(),
            ps.iter().filter(|p| p.0.has_editor).count(),
            outcome(ScanOutcome::Failed),
            outcome(ScanOutcome::Crashed),
            outcome(ScanOutcome::TimedOut),
        );
    }
    let _ = writeln!(s, "\n## Plugins\n");
    let _ = writeln!(
        s,
        "| Name | Vendor | Version | Format | Kind | Editor | Status | Id |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
    for (p, st) in &plugins {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {:?} | {} | {:?} | `{}` |",
            p.name.replace('|', "/"),
            p.vendor.replace('|', "/"),
            p.version,
            extension(p.format),
            p.kind,
            if p.has_editor { "yes" } else { "no" },
            st,
            p.id
        );
    }
    let failed: Vec<_> = cache.blocked_files().collect();
    if !failed.is_empty() {
        let _ = writeln!(s, "\n## Files that could not be scanned\n");
        let _ = writeln!(s, "| File | Format | Outcome | Error |");
        let _ = writeln!(s, "|---|---|---|---|");
        for (path, f) in failed {
            let _ = writeln!(
                s,
                "| {} | {} | {:?} | {} |",
                path.display(),
                extension(f.format),
                f.outcome,
                f.error
                    .as_deref()
                    .unwrap_or("")
                    .replace('|', "/")
                    .replace('\n', " ")
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::*;

    fn host(dir: &Path) -> PluginHost {
        std::fs::write(dir.join(FAKE_FILE), b"").unwrap();
        PluginHost::with_formats(
            HostConfig {
                cache_file: Some(dir.join("cache.json")),
                extra_paths: vec![dir.to_path_buf()],
                ..Default::default()
            },
            vec![Arc::new(FakeFormat)],
        )
        .scan_in_process()
    }

    #[test]
    fn scan_list_instantiate_and_isolate() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        assert!(matches!(
            h.instantiate(PluginFormat::Clap, FAKE_ID),
            Err(PluginError::NotFound(_))
        ));
        let r = h.scan(&[], |_| {}).unwrap();
        assert_eq!(r.found.len(), 1);
        assert_eq!(h.list().len(), 1);
        assert!(h.instantiate(PluginFormat::Clap, FAKE_ID).is_ok());
        h.mark_fault(
            PluginFormat::Clap,
            &PluginFault {
                plugin_id: FAKE_ID.into(),
                call: PluginCall::Process,
                message: "boom".into(),
            },
        );
        assert_eq!(h.status(PluginFormat::Clap, FAKE_ID), PluginStatus::Isolate);
        let e = h.instantiate(PluginFormat::Clap, FAKE_ID).err().unwrap();
        assert!(e.to_string().contains("isolated"), "{e}");
        // persisted: a new host over the same cache file knows
        let h2 = host(d.path());
        assert_eq!(h2.list()[0].status, PluginStatus::Isolate);
        assert!(h2.list()[0].reason.contains("boom"));
        let md = h2.report_markdown();
        assert!(md.contains("| CLAP | 1 | 0 | 1 | 0 |"), "{md}");
        h2.set_status(PluginFormat::Clap, FAKE_ID, PluginStatus::Ok, "");
        assert!(h2.instantiate(PluginFormat::Clap, FAKE_ID).is_ok());
    }
}
