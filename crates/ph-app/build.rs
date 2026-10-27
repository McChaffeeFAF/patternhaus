//! Embeds the application icon (`packaging/icons/patternhaus.ico`) and version information
//! into `ph-app.exe` on Windows, so Explorer, the taskbar and shortcuts show it before the app
//! runs (the window icon is also set at runtime, `ph_ui::icon::viewport_icon`).

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/patternhaus.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icons/patternhaus.ico")
            .set("ProductName", "Patternhaus")
            .set("FileDescription", "Patternhaus")
            .set("CompanyName", "Patternhaus contributors");
        if let Err(e) = res.compile() {
            // a missing resource compiler must not break the build: the runtime icon remains
            println!("cargo:warning=no .exe icon resource: {e}");
        }
    }
}
