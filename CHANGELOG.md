# Changelog

All notable changes to Patternhaus. Versions follow the milestones in `docs/milestones/`.

## Unreleased

### Added
- **Drag and drop from the OS (M1/T9):** drop audio (WAV, AIFF, FLAC, MP3, OGG) on the channel rack or playlist to get one Sampler channel per file (one undo step), or on a channel row to replace its sample; drop .mid on the playlist or piano roll to import it; drop a folder on the browser to add a library folder (remembered); drop a project folder or project.json to open it (asking first when there are unsaved changes). The target lights up and the hint bar says what will happen; unsupported files are skipped with a notice naming them.
- `sample.import` command and `Host::import_file`: dropped audio is validated by decoding and copied into the project's `samples/` folder (name kept, " (2)" on a clash, identical files reused). The human may import from any path; agents only from the project and library folders.
- `file.import_midi` and `file.open` accept any absolute path from the human (agents stay confined).

### Changed
- **The visual pass (M1/T8):** a neon-cyberpunk look in two themes, Neon Slate (default) and
  Red Line, switchable live; user themes load from the config folder (`docs/theming.md`).
  The old Dark and Light themes are removed.
  - Bundled OFL fonts (Barlow Condensed, Barlow, Share Tech Mono) replace egui's default
    fonts; one self-drawn vector icon set; knobs with value arcs and glow, recessed faders,
    gradient meters with held peaks and clip LEDs, hardware-style step pads, LCD readouts.
  - A new top bar (wordmark, menus, PAT/SONG, transport, tempo and bar-beat-tick LCDs, a
    scope, DSP and RAM, the agent badge), a customisable toolbar, browser tabs and search with
    a sample preview, new mixer strips, a hint bar, right-click menus on every control and an
    options menu on every window.
  - Motion on UI time only (pad flashes, meter fall-off, playhead trail, focus glow, knob
    easing, toasts), with a reduce-motion setting.
  - The piano roll and playlist snap and draw down to 1/64 (musical snap names, five grid
    tiers, a minimum drawn size and click target for tiny notes, zoom presets with keys).
  - A loading screen, an app icon (`packaging/icons`), a new window title and About box.
- `get_state` has a `ui` view: the hint the UI is showing.

## [v0.1.0-m1] - Milestone 1: Make a Beat

First milestone release, pre-alpha. A person in the UI, or an agent through MCP alone, can
start a project, load drums and a synth, program steps and piano-roll notes, mix with
built-in effects, arrange patterns in the playlist, hear it live, save, reopen, and export
WAV or FLAC.

### Added
- **Model and commands:**
  - the project model with patches, per-author history and transactions, and a co-editing queue;
  - about 90 commands shared by the UI, CLI, JSON-RPC and MCP;
  - the project folder format (`project.json`, v1);
  - MIDI import and export.
- **Engine:**
  - a general node graph on a multicore work-stealing pool;
  - lock-free plan swaps, with retired plans freed off the audio thread;
  - an offline render that is the test oracle;
  - a per-node CPU meter, overload shedding, and FTZ/DAZ;
  - WASAPI, CoreAudio and ALSA through cpal;
  - a browser preview voice, `Engine::audition`.
- **Instruments:** Sampler (one-shot and loop, choke groups) and Haus Synth with factory presets.
- **Effects:** EQ, compressor with sidechain, limiter, reverb, delay, filter, saturator.
- **Agent interface:**
  - an MCP server (`patternhaus-cli mcp`), JSON-RPC, and a CLI;
  - scopes and grants, and a live connection list;
  - analysis: LUFS, peak, spectrum, onsets, key and chords, clashes, masking, mix health;
  - PNG snapshots: spectrogram, waveform, piano roll, playlist.
- **UI:**
  - an FL-style floating workspace (egui): browser, channel rack, step sequencer, piano roll, playlist, mixer, history, agents;
  - FL-compatible shortcuts and a remappable keymap;
  - dark and light themes (replaced in T8 by Neon Slate and Red Line);
  - Help → About with licence notices.
- **Factory content:** a generated drum kit and Haus Synth presets, with provenance.
- **Quality:**
  - golden renders (`tests/projects`, `tests/golden`, `cargo xtask golden`);
  - real-time vs offline oracle tests and `assert_no_alloc` stress tests;
  - Miri and loom checks of the lock-free engine code;
  - the M1 acceptance test, an agent over MCP only;
  - command coverage guards;
  - a perf baseline and `cargo xtask perf`.
- **Release:** nightly builds from `main` and tagged releases for Windows, Linux and macOS
  (zip with `ph-app`, `patternhaus-cli`, assets, notices and a launcher).

### Behaviour matched to FL Studio (public manual)
- Channel volume: 100% = 0 dB, and the 78% default = -5.2 dB.
- Channel pan uses FL's default Circular law, -3 dB at centre.
- What the manual leaves open is in `docs/parity/owner-checks.md`.

### Licensing
- MIT OR Apache-2.0. The dependency policy is in `deny.toml` and the notices in `THIRD_PARTY_NOTICES.md`.
- Exceptions:
  - MPL-2.0 for the unmodified symphonia decoders;
  - OFL-1.1 and the Ubuntu Font Licence for egui's bundled fonts;
  - Unlicense for midly;
  - BSD-1-Clause for assert_no_alloc.
- This release ships the non-ASIO build. The GPLv3 ASIO build is documented in `docs/building-asio.md`.

### Known limitations
- Pre-alpha: expect breakage, and the project format may still change, with migrations.
- No plugin hosting yet (CLAP/VST3 in M2), no MIDI controller input (M2), and no agent preview mode (M2).
- The macOS binaries are not signed.
