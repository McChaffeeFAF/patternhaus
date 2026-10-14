//! Compiles the crash guard (`src/guard.c`): SEH on MSVC, signal handlers on Unix.

fn main() {
    println!("cargo:rerun-if-changed=src/guard.c");
    let mut build = cc::Build::new();
    build.file("src/guard.c").warnings(true);
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !msvc {
        // Rust panics (from our own adapter code) must unwind through the C frame
        build.flag_if_supported("-fexceptions");
    }
    build.compile("ph_plugin_guard");
}
