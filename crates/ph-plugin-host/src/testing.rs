//! The repo's test plugins, for tests in any crate (never used by the app itself).
//!
//! [`test_plugins`] builds `test-plugins/ph-test-plugins` (a cdylib) and the scan helper
//! (`ph-plugin-scan`) once per test process with a nested `cargo build` into
//! `<target>/test-plugins`, a target directory of its own (so it never waits on, or churns, the
//! build that runs the tests), and returns where they are. CI therefore needs no third-party
//! plugin, and nothing here is shipped.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub use crate::fake::FAKE_ID;

/// CLAP ids of the test plugins (`test-plugins/ph-test-plugins`).
pub const SYNTH_ID: &str = "dev.patternhaus.test.synth";
pub const GAIN_ID: &str = "dev.patternhaus.test.gain";
pub const CRASH_ID: &str = "dev.patternhaus.test.crash";
/// Latency the test gain reports (and has).
pub const GAIN_LATENCY: u32 = 32;

/// Built test plugins.
#[derive(Clone, Debug)]
pub struct TestPlugins {
    /// A folder holding only `ph-test-plugins.clap` (a folder to scan).
    pub dir: PathBuf,
    /// The `.clap` file (a bundle folder on macOS).
    pub clap: PathBuf,
    /// The scan helper executable.
    pub scan_helper: PathBuf,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(t) => {
            let t = PathBuf::from(t);
            if t.is_absolute() {
                t
            } else {
                workspace_root().join(t)
            }
        }
        None => workspace_root().join("target"),
    }
}

fn dylib_name() -> &'static str {
    if cfg!(windows) {
        "ph_test_plugins.dll"
    } else if cfg!(target_os = "macos") {
        "libph_test_plugins.dylib"
    } else {
        "libph_test_plugins.so"
    }
}

/// Builds the test plugins and the scan helper (once per process) and returns their paths.
/// Panics with cargo's output when the build fails.
pub fn test_plugins() -> &'static TestPlugins {
    static BUILT: OnceLock<TestPlugins> = OnceLock::new();
    BUILT.get_or_init(build)
}

fn build() -> TestPlugins {
    let target = target_dir().join("test-plugins");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(workspace_root())
        .args([
            "build",
            "--quiet",
            "-p",
            "ph-test-plugins",
            "-p",
            "ph-plugin-host",
            "--lib",
            "--bin",
            "ph-plugin-scan",
            "--target-dir",
        ])
        .arg(&target)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("cannot run cargo to build the test plugins");
    assert!(
        out.status.success(),
        "building the test plugins failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bin = target.join("debug");
    let dylib = bin.join(dylib_name());
    let dir = target.join("plugins");
    let _ = std::fs::create_dir_all(&dir);
    let clap = dir.join("ph-test-plugins.clap");
    if cfg!(target_os = "macos") {
        // a CLAP on macOS is a bundle: Contents/MacOS/<executable> plus Info.plist
        let macos = clap.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).expect("bundle folder");
        copy_if_changed(&dylib, &macos.join("ph-test-plugins"));
        let plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
            <key>CFBundleExecutable</key><string>ph-test-plugins</string>\
            <key>CFBundleIdentifier</key><string>dev.patternhaus.test-plugins</string>\
            <key>CFBundlePackageType</key><string>BNDL</string></dict></plist>\n";
        std::fs::write(clap.join("Contents").join("Info.plist"), plist).expect("Info.plist");
    } else {
        copy_if_changed(&dylib, &clap);
    }
    let exe = if cfg!(windows) {
        "ph-plugin-scan.exe"
    } else {
        "ph-plugin-scan"
    };
    TestPlugins {
        dir,
        clap,
        scan_helper: bin.join(exe),
    }
}

/// Copies unless the target already has the same bytes (a loaded plugin file is locked on
/// Windows; another test process may hold it).
fn copy_if_changed(from: &Path, to: &Path) {
    let new = std::fs::read(from).unwrap_or_else(|e| panic!("{}: {e}", from.display()));
    if std::fs::read(to).is_ok_and(|old| old == new) {
        return;
    }
    let tmp = to.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, &new).expect("write test plugin");
    if std::fs::rename(&tmp, to).is_err() {
        let _ = std::fs::remove_file(&tmp);
        assert!(
            std::fs::read(to).is_ok_and(|old| old == new),
            "cannot replace {} (in use?)",
            to.display()
        );
    }
}
