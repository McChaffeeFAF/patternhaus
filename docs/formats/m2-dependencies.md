# M2 dependencies: licence checks (M2/T0)

Checked 2026-10-11 against crates.io metadata and the upstream repositories. M2/T0 adds
**no** new third-party dependency (`raw-window-handle` 0.6, used by `ph-plugin-host`, was
already in the tree through winit/wgpu and is MIT OR Apache-2.0 OR Zlib). The crates below
are for later tracks. Each must still pass `cargo deny check` when it is added, and
`cargo xtask notices` must be re-run.

| Use | Crate | Version | Licence | Upstream | Verdict |
|---|---|---|---|---|---|
| Time-stretch (T1) | `signalsmith-stretch` | 0.1.3 (2025-09-18) | MIT | Signalsmith Stretch: MIT; Signalsmith Linear (its FFT helper): MIT | Licence OK. Build cost: see below |
| Time-stretch (T1), alternative | `ssstretch` | 0.1.0 | MIT | the same upstream (MIT) | Licence OK; younger and much less used |
| CLAP host (T3) | `clack-host` (+ `clack-extensions`) | 0.2.0 | MIT OR Apache-2.0 | CLAP headers: MIT | OK |
| VST3 host (T3) | `vst3` (coupler-rs/vst3-rs) | 0.3.0 | MIT OR Apache-2.0 | VST 3 SDK: MIT since 3.8 (DESIGN §6) | OK |
| CLAP ABI (T3: host constants, the CLAP test plugins) | `clap-sys` | 0.5.0 | MIT OR Apache-2.0 | CLAP headers: MIT | OK (added in T3) |
| VST3 COM helpers (T3, through `vst3`) | `com-scrape-types` | 0.1.1 | MIT OR Apache-2.0 | | OK (added in T3) |
| Loading VST3 modules (T3) | `libloading` | 0.8 | ISC | | OK (already in the tree through clack-host) |
| Building the crash-guard shim (T3) | `cc` | 1 | MIT OR Apache-2.0 | | OK (already in the tree) |
| Win32 message pump, editor frames (T3) | `windows-sys` | 0.61 | MIT OR Apache-2.0 | | OK (already in the tree; T3 enables more features) |

Rubber Band is GPL and stays out (M2 milestone, "Stretch engine").

## Notes for the tracks

**T1, Signalsmith Stretch.**
- `signalsmith-stretch` 0.1.3 pulls the C++ sources in as git submodules (Stretch and
  Linear, both MIT), compiles a small C wrapper with `cc` as C++14, and generates the FFI
  with `bindgen`. So every build machine (Windows, Linux and macOS CI) needs a C++14
  compiler **and libclang** for bindgen.
- If libclang on CI is a burden, T1 writes a **thin binding of its own**: the same kind of
  `extern "C"` wrapper around `signalsmith-stretch.h`, compiled with `cc`, and
  hand-written `extern` declarations (a handful of functions), with no bindgen. The
  upstream licence (MIT) allows vendoring the header with its licence text; list it in
  `THIRD_PARTY_NOTICES.md`.
- Either way the engine sees only `ph_engine::audio_clips::Stretcher`, whose shape follows
  the Signalsmith API (`configure` ≈ `presetDefault`, `reset`, `input_latency`,
  `output_latency`, `set_transpose` ≈ `setTransposeSemitones`, `process`).

**T3, plugins.**
- Both host crates are permissively licensed; nothing GPL is pulled in.
- ASIO stays a separate GPLv3 build (M1); plugin hosting does not change that.

## Sources

- https://crates.io/crates/signalsmith-stretch and https://github.com/colinmarc/signalsmith-stretch-rs (its `build.rs`)
- https://crates.io/crates/ssstretch and https://github.com/bmisiak/ssstretch
- https://github.com/Signalsmith-Audio/signalsmith-stretch ("Released under the MIT License")
- https://github.com/Signalsmith-Audio/linear (MIT)
- https://crates.io/crates/clack-host and https://github.com/prokopyl/clack (dual MIT / Apache-2.0)
- https://crates.io/crates/vst3 and https://github.com/coupler-rs/vst3-rs
