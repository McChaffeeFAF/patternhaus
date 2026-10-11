# Patternhaus

An open-source, pattern-based digital audio workstation in Rust, in the workflow tradition of FL Studio. AI agents can drive every part of it.

> **Status: pre-alpha.** Milestone 1, "make a beat", is complete (`v0.1.0-m1`, see [CHANGELOG.md](CHANGELOG.md)). You can program drums and a synth, mix with the built-in effects, arrange, play live, save, and export WAV/FLAC, by hand or through an AI agent. Expect rough edges.

## Install and run

**Prebuilt builds.** Download a zip for Windows, Linux or macOS:
- from the [`nightly`](https://github.com/McChaffeeFAF/patternhaus/releases/tag/nightly) pre-release, rebuilt on every push to `main`;
- or from a milestone [release](https://github.com/McChaffeeFAF/patternhaus/releases).

Unzip it anywhere. Each zip holds `ph-app`, `patternhaus-cli`, the factory `assets/`, the
licences, `THIRD_PARTY_NOTICES.md` and a short README.

- **Windows:** run `Patternhaus.bat [project_dir]`, the launcher. Without an argument it opens
  `Documents\Patternhaus Projects\first`.
- **Linux/macOS:** run `./patternhaus.sh [project_dir]`. The macOS binaries are unsigned; allow
  them in System Settings → Privacy & Security.
- **Or run `ph-app` directly:**
  `ph-app [project_dir] [--buffer 256] [--silent] [--demo-agent] [--assets DIR]`.
  A project is a folder holding `project.json`; a new folder is fine.

**From source** (Rust stable):

```bash
cargo run --release -p ph-app -- path/to/project
```

**Let an agent drive it (MCP).** `patternhaus-cli` speaks MCP over stdio:

```bash
patternhaus-cli mcp --headless --project path/to/project   # its own studio, no audio device
patternhaus-cli mcp --connect                              # attach to the running ph-app
claude mcp add patternhaus -- /path/to/patternhaus-cli mcp --connect   # e.g. for Claude Code
```

The agent sees the same commands as the UI (`list_commands`, `run`, `get_state`, ...). It can
`render` and `analyze` the result, and `snapshot` returns PNG images. `patternhaus-cli --help`
lists the shell commands (`run`, `render`, `analyze`, `snapshot`, `commands`).

## Developing

Start with [AGENTS.md](AGENTS.md). Before a PR, run:

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace && cargo xtask check && cargo deny check
```

Other repository tasks:
- `cargo xtask golden [--update]`: the golden renders;
- `cargo xtask perf`: the performance gate on the reference machine;
- `cargo xtask notices`: regenerates the third-party notices.

## What it will be

- **Pattern workflow:** channel rack, step sequencer, piano roll, playlist and mixer, plus modular routing, an audio editor, live performance mode, and slicing and time-stretch.
- **Agent-drivable:** every action in the app is a command. The UI, a CLI, a JSON-RPC channel and an MCP server all use the same commands. Agents can edit live alongside you, with per-author undo. They can also "hear" the result through loudness, spectrum, key and chord analysis, and spectrogram images.
- **Haus Sampler:** a deep multi-sample instrument. It opens SFZ, SF2, DecentSampler, EXS and unencrypted Kontakt instruments, and runs Kontakt scripts through a clean-room KSP compatibility layer. It does not circumvent encrypted or licence-locked libraries.
- **Plugins:** CLAP and VST3 (later AU and LV2), with crash isolation and a 32-bit bridge.
- **Real-time safe engine:** multicore, with 64-bit summing on the master, and an offline renderer that serves as the test oracle.

See [docs/DESIGN.md](docs/DESIGN.md) for the full design and milestones.

## Clean-room policy

Patternhaus is implemented from public specifications, documentation and observed behaviour. It contains no code, samples, skins or presets from FL Studio, Kontakt or any other proprietary product, and no disassembly of their binaries. "FL Studio" and "Kontakt" are trademarks of their owners, mentioned only to describe compatibility and workflow. Patternhaus is not affiliated with Image-Line or Native Instruments.

## Contributing

Issues are open now. Pull requests will be accepted after milestone 1, once the architecture has settled. Commits must be signed off under the [DCO](https://developercertificate.org/).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Optional builds with ASIO support are distributed under GPLv3 because of the ASIO SDK's licence (see the design doc, section 11).

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this work is dual-licensed as above, without any additional terms or conditions.
