//! `ph-plugin-scan <clap|vst3> <path>`: scans one plugin file in its own process for the
//! Patternhaus plugin scanner (`ph_plugin_host::scan`), so a plugin that crashes or hangs while
//! loading only takes this helper down. Prints one `PH-SCAN-RESULT:{json}` line.

use ph_plugin_host::{PluginFormatHost, clap::ClapFormat, scan::run_child, vst3::Vst3Format};
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let formats: Vec<Arc<dyn PluginFormatHost>> =
        vec![Arc::new(ClapFormat::new()), Arc::new(Vst3Format::new())];
    std::process::exit(run_child(&args, &formats));
}
