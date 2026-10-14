//! `patternhaus-cli plugin-scan --report` over the repo's test plugins (M2/T3): the scan runs
//! each file in the helper process, fills the cache, blocks a file that is not a plugin, and
//! writes the markdown compatibility report.

use std::process::Command;

#[test]
fn plugin_scan_writes_a_report_and_a_cache() {
    let plugins = ph_plugin_host::testing::test_plugins();
    let tmp = tempfile::tempdir().unwrap();
    // the test plugins and one broken file
    let dir = tmp.path().join("plugins");
    std::fs::create_dir_all(&dir).unwrap();
    let clap = dir.join("ph-test-plugins.clap");
    if plugins.clap.is_dir() {
        // a macOS bundle: point at the original instead of copying the tree
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
    } else {
        std::fs::copy(&plugins.clap, &clap).unwrap();
    }
    std::fs::write(dir.join("broken.clap"), b"not a plugin").unwrap();
    let report = tmp.path().join("out/plugin-report.md");
    let cache = tmp.path().join("cache.json");
    let mut args = vec![
        "plugin-scan".to_string(),
        dir.to_string_lossy().into_owned(),
        "--report".into(),
        report.to_string_lossy().into_owned(),
        "--cache".into(),
        cache.to_string_lossy().into_owned(),
        "--timeout".into(),
        "60".into(),
    ];
    if plugins.clap.is_dir() {
        args.insert(1, plugins.clap.to_string_lossy().into_owned());
    }
    let out = Command::new(env!("CARGO_BIN_EXE_patternhaus-cli"))
        .args(&args)
        .env("PATTERNHAUS_PLUGIN_SCAN", &plugins.scan_helper)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["clap"]["plugins"], 3);
    assert_eq!(summary["clap"]["ok"], 3);
    assert_eq!(summary["clap"]["instruments"], 1);
    assert_eq!(summary["clap"]["effects"], 2);
    assert_eq!(summary["vst3"]["plugins"], 0);
    assert_eq!(summary["files_failed"], 1);
    let md = std::fs::read_to_string(&report).unwrap();
    assert!(md.contains("# Plugin compatibility report"));
    assert!(
        md.contains("| CLAP | 2 | 3 | 0 | 0 | 1 | 2 | 0 | 1 | 0 | 0 |"),
        "{md}"
    );
    assert!(md.contains("PH Test Synth"));
    assert!(md.contains("broken.clap"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[1/2]"), "{stderr}");
    // a second run comes from the cache
    let again = Command::new(env!("CARGO_BIN_EXE_patternhaus-cli"))
        .args(&args)
        .env("PATTERNHAUS_PLUGIN_SCAN", &plugins.scan_helper)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&again.stderr).contains("(cached)"));
    assert!(cache.is_file());
}
