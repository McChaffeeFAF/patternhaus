# Beast Studio

*Formerly Patternhaus.* Beast Studio is part of the Fluffy Beast Productions family, alongside Beast Karaoke and Beast DJ.

An open-source, pattern-based digital audio workstation in Rust, in the workflow tradition of FL Studio. AI agents can drive every part of it.

> **Status: pre-alpha.** Milestone 1, "make a beat", is complete (`v0.1.0-m1`, see [CHANGELOG.md](CHANGELOG.md)). You can program drums and a synth, mix with the built-in effects, arrange, play live, save, and export WAV/FLAC, by hand or through an AI agent. Expect rough edges.

## Editions

The code is free and open source (MIT OR Apache-2.0), and anyone can build it. A paid, ready-to-run Beast Studio build and paid content add-ons will be sold on Steam by Fluffy Beast Productions. The name, the mascot and the brand art are not part of the open-source licence (see [Trademarks and brand assets](#trademarks-and-brand-assets)).

## Install and run

**Prebuilt builds.** Download a zip for Windows, Linux or macOS:
- from the [`nightly`](https://github.com/McChaffeeFAF/beast-studio/releases/tag/nightly) pre-release, rebuilt on every push to `main`;
- or from a milestone [release](https://github.com/McChaffeeFAF/beast-studio/releases).

Unzip it anywhere. Each zip holds `beast-studio`, `beast-studio-cli`, the factory `assets/`, the
licences, `THIRD_PARTY_NOTICES.md` and a short README.

- **Windows:** run `Beast Studio.bat [project_dir]`, the launcher. Without an argument it opens
  `Documents\Beast Studio Projects\first` (an existing `Documents\Patternhaus Projects\first`
  from an earlier version is still opened).
- **Linux/macOS:** run `./beast-studio.sh [project_dir]`. The macOS binaries are unsigned; allow
  them in System Settings → Privacy & Security.
- **Or run `beast-studio` directly:**
  `beast-studio [project_dir] [--buffer 256] [--silent] [--demo-agent] [--assets DIR]`.
  A project is a folder holding `project.json`; a new folder is fine.

**From source** (Rust stable):

```bash
cargo run --release -p ph-app -- path/to/project
```

(`ph-app` is the crate; it builds the `beast-studio` binary.)

**Let an agent drive it (MCP).** `beast-studio-cli` speaks MCP over stdio:

```bash
beast-studio-cli mcp --headless --project path/to/project   # its own studio, no audio device
beast-studio-cli mcp --connect                              # attach to the running Beast Studio
claude mcp add beast-studio -- /path/to/beast-studio-cli mcp --connect   # e.g. for Claude Code
```

The agent sees the same commands as the UI (`list_commands`, `run`, `get_state`, ...). It can
`render` and `analyze` the result, and `snapshot` returns PNG images. `beast-studio-cli --help`
lists the shell commands (`run`, `render`, `analyze`, `snapshot`, `commands`).

## Settings, environment variables and names from before the rename

Beast Studio was called Patternhaus until 2026-11-27. Nothing you set up under the old name is lost:

| What | Now | Still works |
|---|---|---|
| Settings, agent grants, catalogue, plugin cache | `%APPDATA%\Beast Studio` and `%LOCALAPPDATA%\Beast Studio`; on Linux `~/.config/beast-studio`, `~/.local/share/Beast Studio`, `~/.cache/beast-studio`; on macOS the same places under `Library/` | The old `Patternhaus` folders are **copied** once, on the first start, if the new one does not exist. They are never moved or deleted, so an older build keeps working. |
| Environment variables | `BEAST_STUDIO_ASSETS`, `BEAST_STUDIO_CATALOGUE`, `BEAST_STUDIO_CJK_FONT`, `BEAST_STUDIO_CRASH_LOG`, `BEAST_STUDIO_PLUGIN_SCAN`, `BEAST_STUDIO_PLUGIN_SETTLE_SECONDS` | The same names with the `PATTERNHAUS_` prefix are read when the new one is not set. |
| Local endpoint for agents | `\\.\pipe\beast-studio-<user>`, or `$XDG_RUNTIME_DIR/beast-studio.sock` | `beast-studio-cli` tries it first, then the old `patternhaus-<user>` pipe or `patternhaus.sock`. |
| MCP server and resources | server `beast-studio`, resources such as `beast-studio://events` | `patternhaus://...` URIs are read as aliases. |
| Default project folders | `Documents/Beast Studio Projects`, `./beast-studio-project` | Existing folders still open; the launchers and the CLI fall back to the old ones when only those exist. |

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
- **Beast Sampler:** a deep multi-sample instrument. It opens SFZ, SF2/SF3, DecentSampler and EXS instruments, saves them in an open native format, and takes more formats through a documented importer hook. It does not circumvent encrypted or licence-locked libraries.
- **Plugins:** CLAP and VST3 (later AU and LV2), with crash isolation and a 32-bit bridge.
- **Real-time safe engine:** multicore, with 64-bit summing on the master, and an offline renderer that serves as the test oracle.

See [docs/DESIGN.md](docs/DESIGN.md) for the full design and milestones.

## Clean-room policy

Beast Studio is implemented from public specifications, documentation and observed behaviour. It contains no code, samples, skins or presets from FL Studio, Kontakt or any other proprietary product, and no disassembly of their binaries. "FL Studio" and "Kontakt" are trademarks of their owners, mentioned only to describe compatibility and workflow. Beast Studio is not affiliated with Image-Line or Native Instruments.

## Trademarks and brand assets

"Beast Studio", "Beast Sampler", the Lyric character and the Fluffy Beast Productions names and art are not licensed under MIT or Apache-2.0. The licences cover the source code only. A fork must use a different name and must not use the brand art. Brand assets, when they are added, live in [`brand/`](brand/) under [their own licence](brand/LICENSE-BRAND.md).

## Contributing

Issues are open now. Pull requests will be accepted after milestone 1, once the architecture has settled. Commits must be signed off under the [DCO](https://developercertificate.org/).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Optional builds with ASIO support are distributed under GPLv3 because of the ASIO SDK's licence (see the design doc, section 11).

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this work is dual-licensed as above, without any additional terms or conditions.
