//! Compiles the vendored Signalsmith Stretch (header-only C++) behind `cpp/ph_stretch.cpp`,
//! a plain `extern "C"` wrapper. Needs only a C++14 compiler (MSVC, GCC or Clang); no
//! libclang, because the Rust declarations in `src/lib.rs` are written by hand.

fn main() {
    println!("cargo::rerun-if-changed=cpp/ph_stretch.cpp");
    println!("cargo::rerun-if-changed=vendor/signalsmith-stretch/signalsmith-stretch.h");
    println!("cargo::rerun-if-changed=vendor/signalsmith-linear/fft.h");
    println!("cargo::rerun-if-changed=vendor/signalsmith-linear/stft.h");
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++14")
        .file("cpp/ph_stretch.cpp")
        .include("vendor")
        .include("vendor/signalsmith-stretch")
        // vendored third-party code: its warnings are not ours to fix
        .warnings(false);
    // Same results on every run: no fast-math, no FMA contraction differences between
    // builds of the same target.
    if b.get_compiler().is_like_msvc() {
        b.flag("/fp:precise").flag("/EHsc");
    } else {
        b.flag("-ffp-contract=off");
    }
    b.compile("ph_stretch");
}
