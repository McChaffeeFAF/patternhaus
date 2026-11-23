//! `ph_plugin_host::shutdown`: every instance is destroyed before a VST3 module is told to
//! exit (`ExitDll`), the order Kontakt 7 needs (it faulted at process exit otherwise). Its own
//! test binary, because shutdown tears down every plugin in the process.

#![cfg(windows)]

use ph_node::*;
use ph_plugin_host::testing::*;
use ph_plugin_host::vst3::Vst3Format;
use ph_plugin_host::*;
use std::time::{Duration, Instant};

#[test]
fn shutdown_destroys_every_instance_before_the_module_exits() {
    let Some(path) = test_plugins().vst3.clone() else {
        return;
    };
    let log = std::env::temp_dir().join(format!("ph-vst3-exit-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    // the plugin reads this in ExitDll, in this process
    unsafe { std::env::set_var("PH_TEST_VST3_EXIT_LOG", &log) };

    let infos = Vst3Format::new().scan(&path).unwrap();
    let mut kept = Vec::new();
    for info in infos.iter().take(2) {
        let mut i = Vst3Format::new().instantiate(info).unwrap();
        i.activate(&ActivateInfo {
            sample_rate: 48_000.0,
            max_frames: 128,
            quality: Quality::Live,
            seed: 1,
        });
        kept.push(i);
    }
    // at exit the engine has dropped its instances: their destruction is only queued on the
    // plugins' main thread when shutdown starts
    drop(kept);

    let t = Instant::now();
    assert!(
        shutdown(Duration::from_secs(20)),
        "shutdown finished in time"
    );
    let took = t.elapsed();
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        lines.trim(),
        "exit live=0",
        "ExitDll ran once, after every instance was destroyed"
    );
    // young instances are given time to settle, but shutdown is bounded
    assert!(took < Duration::from_secs(15), "took {took:?}");
    // a second call does nothing
    assert!(shutdown(Duration::from_secs(5)));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap_or_default().trim(),
        "exit live=0"
    );
    let _ = std::fs::remove_file(&log);
}
