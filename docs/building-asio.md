# Building Patternhaus with ASIO (the GPLv3 build)

DESIGN §11: the source is MIT OR Apache-2.0. ASIO support sits behind the `asio` cargo
feature, which is off by default. A binary built with it links Steinberg's ASIO SDK, which
Steinberg has offered under GPLv3 (alongside its proprietary licence) since October 2025.
That binary as a whole is therefore distributed under GPLv3. Its About box says so
(`ph_engine::ASIO_BUILD`), and the release zip carries `GPL-NOTICE.txt`.

## Decision for the M1 release (v0.1.0-m1)

M1 ships the **non-ASIO, MIT/Apache build** for Windows, Linux and macOS.

- CI can fetch the SDK legitimately. Under its GPLv3 option the SDK may be downloaded and
  redistributed, and `asio-sys` (cpal's ASIO backend) downloads it from Steinberg's official
  link, `https://www.steinberg.net/asiosdk`, when `CPAL_ASIO_DIR` is not set.
- The workflows contain the GPL artefact job (`build-asio` in
  `.github/workflows/build-dist.yml`). It runs only when the repository variable
  `PH_BUILD_ASIO` is `true`.
- It stays off for M1 for three reasons:
  - The owner should accept Steinberg's terms for the GPLv3 option first, including the
    trademark rules: "ASIO" may describe compatibility but not name the product.
  - The CI download and the `bindgen` step have not run yet; the T7 agent may not download
    the SDK itself.
  - The DESIGN §13 plan of an ASIO-enabled installer needs the Windows installer, which comes
    after M1.
- To turn it on: GitHub → Settings → Secrets and variables → Actions → Variables → add
  `PH_BUILD_ASIO = true`. The next nightly then also attaches
  `patternhaus-…-windows-x64-asio-gpl3.zip`.

## Building it yourself (Windows)

1. Install the Rust toolchain (MSVC) and LLVM, which `bindgen` needs:
   `winget install LLVM.LLVM`, then set `LIBCLANG_PATH=C:\Program Files\LLVM\bin`.
2. Get the ASIO SDK from Steinberg's developer site (https://www.steinberg.net/developers/),
   choosing the GPLv3 licence option, and unzip it, e.g. to `C:\sdk\asiosdk`.
3. Point cpal at it, so nothing is downloaded during the build:
   `set CPAL_ASIO_DIR=C:\sdk\asiosdk`
4. Build:
   ```powershell
   cargo build --release -p ph-app -p ph-cli --features ph-app/asio
   ```
5. Package it with the GPL notice:
   ```bash
   packaging/package.sh target/release patternhaus-local-windows-x64-asio-gpl3 gpl
   ```
6. Check: Help → About shows the GPLv3 paragraph, and `EngineConfig::exclusive = true`
   (ASIO or exclusive mode) opens your ASIO driver.

## Licence checks for the ASIO build

`cargo deny --features ph-engine/asio check licenses bans sources` (a CI job) confirms that the
feature adds no GPL *crates*: `asio-sys` itself is Apache-2.0. The only GPL component is the
SDK source compiled into the binary, which is why the binary, not the repository, is GPLv3.
The notices in `THIRD_PARTY_NOTICES.md` describe the default build. The ASIO build adds
`GPL-NOTICE.txt`.
