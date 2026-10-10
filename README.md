# Patternhaus

An open-source, pattern-based digital audio workstation in Rust, in the workflow tradition of FL Studio. AI agents can drive every part of it.

> **Status: pre-alpha.** The design phase is complete, and implementation of milestone 1 ("make a beat") is starting. Nothing is usable yet.

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
