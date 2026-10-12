# Patternhaus — Design

Status: **design complete, 2026-10-10.** All sections were decided with the owner. Changing a decided section needs the owner's sign-off, recorded here with a date.

Patternhaus is a clean-room, open-source (MIT OR Apache-2.0) pattern-based DAW in the workflow tradition of FL Studio, written in Rust. Agents can drive every part of it.

| # | Section | Status |
|---|---------|--------|
| 1 | Product scope and parity target | **Decided** 2026-10-10 |
| 2 | Architecture and crate layering | **Decided** 2026-10-10 |
| 3 | Document model, commands and undo | **Decided** 2026-10-10 |
| 4 | Audio engine and real-time rules | **Decided** 2026-10-10 |
| 5 | Agent interface (MCP / CLI / JSON) | **Decided** 2026-10-10 |
| 6 | Plugin hosting | **Decided** 2026-10-10 |
| 7 | Built-in instruments and effects | **Decided** 2026-10-10 |
| 8 | Haus Sampler (Kontakt-class) and library compatibility | **Decided** 2026-10-10 |
| 9 | UI | **Decided** 2026-10-10 |
| 10 | File formats and interchange | **Decided** 2026-10-10 |
| 11 | Platforms and audio/MIDI I/O | **Decided** 2026-10-10 |
| 12 | Quality: testing, oracles, real-time safety | **Decided** 2026-10-10 |
| 13 | Project process: agents, clean-room policy, releases | **Decided** 2026-10-10 |

---

## 1. Product scope and parity target

**Decided 2026-10-10.**

**1.0 scope:**
- the core FL Studio workflow: channel rack, step sequencer, piano roll, playlist (pattern, audio and automation clips), mixer (inserts, sends, effect slots), browser, and plugin hosting;
- **modular routing** (a Patcher-style node graph for instrument and effect chains);
- an **audio editor** (Edison-style destructive editing, recording and chopping);
- **live performance mode** (launch patterns and clips from a grid or controller);
- **slicing and time-stretch** (Slicex-style beat slicing, tempo-synced stretching).

**Parity tracking.** Parity is tracked feature by feature in `docs/parity/fl-studio.md`, with two columns:
- *wired*: a command exists;
- *behaves*: its behaviour was checked against FL's observed behaviour.

A feature counts as done only when both columns are ticked.

**Out of scope for 1.0:** video, notation, FL's mobile features, cloud services, and any FL-specific branding, skins, factory sounds or plugin clones by name.

**Milestones:**
- **M1, Make a beat:**
  - channel rack, sampler and synth, step sequencer, piano roll, basic mixer, playlist with pattern clips, WAV export, and the full agent interface;
  - the engine's routing is a general graph from day one, because modular routing depends on it.
- **M2, Make a song:** automation clips, audio clips and recording, the audio editor, slicing and time-stretch, the effects suite, CLAP/VST3 hosting, MIDI controllers, and **FLP import** (moved from M4 in section 10).
- **M3, Haus Sampler:** the multi-sample engine, SFZ/SF2/DecentSampler/EXS, then Kontakt import.
- **M4, Pro:** the modular-routing UI, live performance mode, plugin delay compensation (PDC), sandboxed plugins, stems, DAWproject, Linux and macOS.

## 2. Architecture and crate layering

**Decided 2026-10-10.** The owner approved the layering; Claude owns crate-level details and flags anything that affects users. Engine first, with a thin UI on top. The layering is checked in CI, so a crate may only depend on crates below it:

```
ph-app (binary) ─ ph-ui ─────────────┐
ph-cli (binary) ─ ph-control ────────┤   ph-control = JSON-RPC + MCP + CLI, all generated from the command registry
                                     ├─ ph-commands (registry, undo/redo, session)
ph-engine (graph, scheduler, render) ┤
   ├─ ph-instruments, ph-effects ────┤─ ph-core (pure data model)
   ├─ ph-dsp (primitives)            │
   ├─ ph-plugin-host (CLAP, VST3)    │
   └─ ph-audio-io (devices, MIDI)    │
ph-formats (project, WAV/FLAC/MP3/OGG, MIDI, DAWproject, FLP import)
ph-analysis (LUFS, spectrum, onsets, key/chords, mix health, snapshot images: agent perception, section 5)
ph-node (Processor trait: the internal plugin interface every built-in implements; mirrors CLAP)
ph-sampler + ph-sampler-formats (SFZ, SF2, DS, EXS, NKI)  → also shipped as a CLAP/VST3 plugin
ph-script (Luau host + KSP compatibility interpreter, section 8)
ph-plugin-server (out-of-process plugin host binary + protocol: isolation, 32-bit bridge, VST2 add-on, section 6)
```

## 3. Document model, commands and undo

**Decided 2026-10-10.**

**Project model**
- A `Project` is pure data, with copy-on-write `Arc` collections. Taking a whole-project snapshot is cheap, and the engine and UI read snapshots.
- Ids are stable and never reused, so agents can safely refer to "note 1532".

**Command registry**
- One `CommandRegistry` holds every operation. Each command has:
  - an id (e.g. `pattern.add_note`);
  - a summary;
  - a JSON-Schema for its parameters (`schemars`);
  - a flag for whether it changes state;
  - a handler.
- Every route goes through the registry: menus, tools, shortcuts, the CLI, JSON-RPC and MCP. There are no hidden UI-only code paths.

**Live co-editing.** You and any number of agents edit the same project at the same time.
- All changes go through one ordered queue on the control thread.
- Each change is stamped with its author (`human`, `agent:<name>`) and a sequence number.
- Readers see consistent snapshots.

**Edits record their own inverse.** A mutating command doesn't hand back a whole-project snapshot. It returns a list of fine-grained **patches**, each with:
- an object path, such as `pattern/3/notes/channel/2/note/1532`;
- a before value;
- an after value.

The history log is the list of patch sets, each tagged with author, transaction id, command id and parameters. Because it's a full log, a session can be replayed as a script, and the log works as an audit trail.

**Per-author undo**
- **Plain undo** (Ctrl+Z) reverts *your own* most recent transaction.
- **"Undo agent X's work"** reverts that author's transactions in reverse order. The same applies to a single agent transaction, chosen from the history panel.
- **Conflicts:** if a later transaction by someone else touched the same object path, undo stops. The conflict is shown, with three choices: skip that object, force the undo, or cancel. Undo never silently overwrites another author's work.
- Undo is a new transaction that applies the inverse patches. So history only grows, and redo is undoing an undo.

**Transactions.** `run_batch(..., as_one_undo)` groups commands into one transaction, so "write the chorus" undoes as a single step.

**Optional preview mode** (off by default, set per session or per agent):
- An agent's batch goes onto a *proposal layer* instead of being committed. The UI shows it as ghost notes, clips and settings.
- The engine can audition the proposal, which plays project and proposal merged.
- Accepting commits the batch as a normal transaction; rejecting throws it away.
- If the project changes underneath a proposal, its patches are rebased. Any patch that conflicts is flagged.

## 4. Audio engine and real-time rules

**Decided 2026-10-10.**

**Precision**
- 32-bit float buffers through the graph.
- 64-bit summing on the master and buses.

**Heavy projects**
- **Multicore graph (M1):** independent channels and inserts run in parallel, scheduled by dependency order with work-stealing.
- **Anticipative processing (architected in M1, shipped in M4):** nodes that aren't monitored live render ahead of the playhead into a lookahead buffer. Armed and live-played paths run at device latency.
  - The render plan marks every node *live* or *anticipative*.
  - Nodes must not assume the block they're processing is "now".
- **Freeze:** manual channel and insert freeze (render to audio, unload the node) in M2. Automatic smart freeze of idle or unchanged channels comes later.
- **Per-node CPU meter:** time spent in each channel, insert, effect and plugin, available to the UI and to agents through `get_state`.
- **Graceful overload:** when the deadline is at risk, shed non-essential work in a fixed order: meters and visuals, then oversampling, then voice count. Only then does audio drop out. Overloads are logged with the offending node.
- **Denormals:** flush-to-zero and denormals-are-zero are set on every audio worker thread.
- **Voices and streaming:** per-instrument voice limits with voice stealing, and disk streaming with a preload buffer for large samples (shared with Haus Sampler).

**Export quality.** Live playback uses efficient interpolation and oversampling. The HQ export mode switches to sinc resampling and higher oversampling. Export settings are part of the render command, so agents can choose them.

**Latency.** Adaptive: the default buffer is 256 samples. On repeated overloads, the engine proposes or applies a larger buffer and shows a visible notice. The user can always override it.

**Engine design**
- **Control thread:** applies commands, then compiles a new `Arc<Project>` into a *render plan* off the audio thread. The plan has a flattened graph, resolved samples, and sorted events per pattern.
- **Audio thread:** picks up the new plan through a lock-free swap. Old plans are freed on a non-real-time thread, never on the audio thread.
- **Rules on the audio thread:** no allocation, no locks, no I/O, no syscalls. Debug builds enforce this with `assert_no_alloc`.
- **Offline render is the same code** driven by a fake clock. It is the reference oracle, so a real-time render must match it to the sample.
- **Graph:** a general directed acyclic graph of nodes and ports (audio, sidechain, events), not a fixed channel→insert→master chain. FL-style routing (channels to inserts to sends to master) is the default layout built from it, and modular routing exposes the graph directly. Processing follows the dependency order, with a work-stealing thread pool across cores. PDC comes in M4.
- **Events:** sample-accurate notes and automation, plus tempo automation through a tempo map.
- **Internal format:** 32-bit float, any sample rate, block size set by the host.

## 5. Agent interface (MCP / CLI / JSON)

**Decided 2026-10-10.**

**No built-in AI.** Patternhaus ships no AI vendor and no chat panel. It is an excellent MCP server, and users drive it from any MCP client (Claude Code, Claude Desktop and others).

**Perception: how agents perceive the music**
- **M1:**
  - **Numeric analysis:** LUFS (integrated and short-term), true peak, spectrum bands, onsets, key and chord estimates, masking and clash warnings between channels.
  - **Mix health:** clipping, DC offset, phase correlation and mono compatibility, plus the song's energy curve over time.
  - **Images:** spectrogram, waveform, piano-roll and playlist snapshots, returned as MCP image content.
- **Later (M2/M3):** rendered audio clips for audio-capable models; comparison against a reference track (tonal balance, loudness, stereo width).

**Readable state.** `get_state` offers musician-readable views as well as raw JSON:
- note names (C4 = MIDI 60) and `bar.beat.tick` positions;
- chord symbols per bar;
- a section map built from playlist markers and pattern structure.

**Change summaries.** Every batch returns a short musical diff, e.g. "added 64 notes to Bass, bars 9–16, now follows Am–F–C–G".

**Live events.** Agents subscribe to human edits, transport, selection and overloads through MCP notifications/resources, so they react instead of polling.

**Determinism.** Renders use fixed seeds for any randomness (humanize, noise, round-robin start), so analysis is reproducible.

**Access**
- **Local only:** a named pipe or Unix socket, or localhost bound to 127.0.0.1. No LAN access in 1.0.
- **Per-agent scopes** chosen when a client connects (remembered per client name):
  - `read`;
  - `compose` (patterns, notes, channels);
  - `mix` (mixer, effects, automation);
  - `full` (everything, including files and plugins).
- **File access** is limited to the project folder, the user's sample and library folders, and the exports folder.
- Destructive commands still need `confirm`.

**Agent routes** Agents can drive the entire program. Every command is reachable, and agents can *hear* the result. Three routes share one registry:
- **MCP server:** run inside the app (live session) or headless (`patternhaus --headless --mcp`).
  - Hundreds of commands would overwhelm a client as individual tools. So it exposes a small fixed toolset: `list_commands(filter)`, `describe_command(id)`, `run(id, params)`, `run_batch([...], as_one_undo)`, `get_state(path)` (JSON views of the project), and `render(range, format)`.
  - It also exposes `analyze(range)`, which returns loudness (LUFS), peak, spectrum, onset and key/chord estimates, so agents get feedback on what they wrote.
  - About 20 high-traffic commands also get direct tools.
- **JSON-RPC:** over a localhost socket or stdio. Used by scripts and tests.
- **CLI:** `ph-cli run pattern.add_note --pattern 0 ...` and `ph-cli script file.json`.
- **Permissions:** destructive commands (delete, overwrite file, close without saving) take a `confirm` flag. The app can be set to require a human click for those in a live session.
- **UI driving:** commands like `ui.open`, `ui.focus` and `ui.select` let an agent drive what the human sees.

## 6. Plugin hosting

**Decided 2026-10-10.**

**Formats**
- **CLAP** (MIT) and **VST3** (MIT since the VST 3.8 SDK, October 2025), in M2.
- **AU** on macOS and **LV2** on Linux, with those platforms in M4, each in a platform-only crate.

**One plugin-server protocol.** Every out-of-process case uses the same `ph-plugin-server` protocol:
- crash isolation;
- the 32-bit bridge;
- the VST2 add-on.

How it works:
- Audio and events travel through shared-memory ring buffers, with a cross-process wake each block.
- Parameters, state and GUI handles go over a control channel.
- Plugin GUIs embed as child windows where the OS allows it, and otherwise float.

**Crash isolation.** Plugins run in-process by default.
- A plugin that crashes is caught as far as the platform allows, logged, and marked *isolate*. From then on it loads out of process automatically.
- Any plugin can be set to isolated by hand.
- The session survives an isolated plugin crashing: its node outputs silence and can be reloaded.

**Older plugins**
- **32-bit plugins:** a 32-bit `ph-plugin-server` helper process on Windows hosts 32-bit CLAP/VST3 plugins, and VST2 ones too through the add-on.
- **VST2:** none in the core app. A separate optional open-source add-on, the *Patternhaus VST2 Bridge*, in its own repo, uses clean-room headers like Ardour and Carla do. It is a plugin-server implementation, so the core never contains VST2 code.

**Plugin editors and built-ins**
- Each plugin's own window, plus a generic parameter UI that agents can also drive by parameter id.
- Patternhaus's own instruments use the same plugin interface internally, so they can also ship as CLAP/VST3 for other DAWs.

## 7. Built-in instruments and effects

**Decided 2026-10-10.**

**Instruments in 1.0:**
- **Sampler:** one-shot and loop playback (M1).
- **Haus Synth:** analog-style subtractive synth (M1). Three oscillators with sub and noise, two filters (LP/HP/BP/notch, ladder and SVF models), three envelopes, two LFOs, a mod matrix, unison, glide and a preset browser.
- **Slicer** (M2).
- **Haus Sampler** (M3).

**Candidates after 1.0** (the owner is open to adding more as the project grows): wavetable synth, FM synth, drum machine.

**Effects: full mixing suite**
- **M1 essentials:** parametric EQ, compressor (with sidechain input), limiter, reverb (algorithmic), delay, filter, saturation/distortion.
- **M2:** multiband compressor, gate/expander, convolution reverb, chorus/flanger/phaser, bitcrush, stereo tools, spectrum/loudness/phase analyzers.
- **M3–M4:** vocoder, pitch correction.

**Factory content: self-generated, with CC0 to fill gaps**
- Drum kits and multisamples are rendered by Patternhaus's own synthesis. That includes an internal (not shipped) drum-synthesis and physical-model renderer under `tools/soundgen`, so the project owns them outright.
- Gaps are filled from CC0 sources only.
- Every factory file has a provenance entry in `assets/PROVENANCE.toml`: generator and parameters, or source URL plus licence. CI rejects any asset without one.
- Nothing is copied or resampled from FL or other commercial content.

## 8. Haus Sampler (Kontakt-class) and library compatibility

**Decided 2026-10-10.**

**Scripting**
- **Native scripts are Luau:** sandboxed Lua via `mlua` with the `luau` feature. Scripts run on a non-real-time script thread and act on the voice engine through a bounded event queue.
- Script callbacks get a per-callback time budget. The real-time thread never waits on script code.

**KSP compatibility layer**
- A separate interpreter for the Kontakt Script Processor language, implemented clean-room from NI's public KSP reference manual.
- It compiles KSP to an internal instruction set run by the same script host.
- Coverage is tracked callback by callback and built-in function by built-in function in `docs/parity/ksp.md`.
- Unsupported built-ins are logged, never silently ignored.

**Instrument interfaces: declarative skins**
- Each instrument's panel is described in a layout file with image assets: knobs, sliders, menus, buttons, labels, keyswitch keyboard colouring, and image strips for animated controls.
- Imported KSP `ui_*` controls map onto this model.
- Every control has an id that agents can read and drive.

**Test corpus**
- The owner has unencrypted .nki libraries for local testing, plus free unlocked libraries such as Pianobook's.
- Third-party libraries never enter the repo.
- CI uses only generated fixtures and freely redistributable files.
- Local compatibility runs write a report to `docs/parity/kontakt-corpus.md`. The report lists only library names and results, never content.

**Draft details carried over**
- **Engine:** zones (key and velocity ranges), round robins, groups, articulations and keyswitches.
- **Playback and sound:** disk streaming for large libraries, release triggers, crossfades, legato, true multi-out, modulation matrix, and per-group effects.
- **Native format:** an open, documented instrument format (a folder plus TOML/JSON).
- **Import formats** (in priority order):
  1. SFZ: an open spec and the de-facto open standard.
  2. SF2/SF3.
  3. DecentSampler (.dspreset).
  4. EXS24.
  5. **Kontakt .nki/.nkm/.ncw:** unencrypted instruments from Kontakt 1.5 through 7. This builds on the publicly documented format knowledge, as the open-source ConvertWithMoss does. The initial scope covers zones, loops, NCW samples and monoliths. Envelopes, filters and groups come later.
  6. **KSP scripts:** run by the KSP compatibility layer described above.
- **Hard boundary:** no circumvention of encrypted or licence-locked libraries, such as NI "Player" libraries and Native Access–activated content. That would break anti-circumvention law and the libraries' licences. When the sampler meets one, it says so plainly.

## 9. UI

**Decided 2026-10-10. Window layout revised 2026-10-10** with owner sign-off, after hands-on testing of the M1 build. The owner prefers FL Studio's window handling to the docked layout. The original docked/detachable decision is replaced below.

**Toolkit.** egui on wgpu (eframe), like PhotoCraft. Custom widgets: piano roll, playlist, step grid, mixer strips, knobs, waveform and spectrogram views. Waveform and spectrogram rendering are cached on the GPU.

**Window layout: an FL-style floating workspace**

These match the *behaviour* of FL's window system. Its look is not copied.
- **Floating, overlapping windows inside one workspace:** channel rack, piano roll, playlist, mixer, history, agents, and editor windows. You move, resize and overlap them; clicking brings one to the front.
- **Maximize and snap:**
  - double-clicking a title bar maximizes a window within the workspace, and does it again to restore;
  - windows snap magnetically to the workspace edges and to each other;
  - windows are always kept reachable inside the workspace.
- **The playlist** opens maximized as the workspace background by default, like a typical FL setup.
- **F5–F9 toggle windows:**
  - pressing the key shows the window and brings it to the front;
  - if the window is already in front, pressing it again hides it.
- **One window per instrument and per effect:**
  - clicking a channel opens that instrument's own window, and clicking an effect slot opens that effect's window;
  - several can be open at once;
  - each remembers its position per object.
- **The browser is docked** at the left edge, with a resizable width. F8 shows and hides it.
- **Detaching to another monitor** is optional per window (multi-viewport). The default is inside the workspace.
- **Layouts:** window positions, sizes, z-order and open state are saved per user, and optionally per project.

**Shortcuts and mouse.** FL-compatible defaults:
- F5–F9 window toggles;
- piano-roll mouse gestures: left-click to draw, right-click to delete;
- clone and quantize shortcuts.

Shortcuts and gestures are functional conventions, so we match them for muscle memory. FL's visual design, icons and trade dress are not copied. Every binding is remappable, and the keymap is a file. Each binding names a command id from the registry.

**Look: revised 2026-10-10** with owner sign-off on the visual-direction canvas (https://claude.ai/artifact/AJfwGWHTjpk2dH8n8oKf5J).

The aim is a professional first impression, at FL-level density and polish, in our own visual language. FL's trade dress is never copied.
- **Themes:**
  - **Neon Slate** (default): slate surfaces, neon blue `#38c6ff` accent, purple for agents.
  - **Red Line** (alternate): wine-black surfaces, red `#ff3b5c` accent, cyan for agents, yellow for clipping.
- **Neon means something:** playhead, active steps, meters, selection, and the focused window's edge glow. Glow is used only on live elements.
- **Type** (all OFL, bundled, replacing egui's default fonts):
  - labels: uppercase Barlow Condensed with wide tracking;
  - content (names the user types): Barlow, kept in the case it was typed;
  - readouts: Share Tech Mono.
- **Details:**
  - LCD-style transport readout;
  - hardware-style step pads, knobs with value arcs, fader caps, LEDs;
  - vertical mixer-strip names;
  - a hint bar.
- **Motion:** step pads flash on hit, meters hold their peaks, the playhead has a soft trail, the focused window glows, and knob arcs ease.
- **Customisable like FL:** a full theme/skin system (colours, fonts, control styles, density) that users can share, a customisable toolbar, right-click menus everywhere, and an options menu per window.
- **Grid down to 1/64:** musical snap names (1/4 to 1/64, plus triplets); a five-tier grid (bar, beat, 1/16, 1/32, 1/64) that fades in by zoom; and a minimum drawn width and click target for tiny notes.
- **Scaling:** works at any DPI.
- **As built (M1/T8):**
  - Themes are TOML files (`crates/ph-ui/themes/`, user themes in `<config>/themes/`; see `docs/theming.md`). The old Dark and Light themes were removed.
  - Fonts are in `assets/fonts` (OFL, with provenance and licence texts); the icons are self-drawn vectors, so there is nothing to license.
  - The hint bar's text is published through `ph-control`, so an agent can read it with `get_state { view: "ui" }`.
  - The scope in the top bar is an envelope of the master level (the engine publishes levels, not samples).

**Shared with agents.** The UI reads only snapshots, and every change goes through commands. So anything you can see, an agent can query, and anything you can do, an agent can do.

## 10. File formats and interchange

**Decided 2026-10-10.**

**Native project: a folder**
- `project.json`: the `ph-core` model, versioned, with ordered migrations. It's pretty-printed with stable key order, so it reads well to humans, agents and git diffs.
- `samples/` holds collected audio, `renders/` holds freezes and bounces, and `history/` holds the optional persisted per-author patch log.
- **Export as `.phz`:** zips the folder, with samples collected, for sharing.
- Large opaque blobs (plugin states) are stored as separate files referenced from `project.json`, so they don't bloat it.

**Audio and MIDI**
- **Audio import:** symphonia (WAV, AIFF, FLAC, MP3, OGG, M4A where licensing allows).
- **Audio export:** WAV and FLAC in M1; MP3 and OGG later.
- **MIDI:** import and export.

**DAWproject.** Import and export (M4). It's an MIT-licensed exchange format supported by Bitwig, Studio One and Cubase.

**FLP import, moved to M2 (the owner has FL 21 projects to bring over)**
- Built clean-room from publicly documented format notes (e.g. PyFLP's).
- **Imports:** channels, patterns and notes, playlist, mixer routing, automation, and third-party plugin states.
- **Native FL plugins:** mapped to the closest Patternhaus equivalent where one exists, otherwise a labelled placeholder that keeps the original data.
- **Import report:** lists everything that was approximated or skipped.
- **Placement:** `file.import_flp` writes a new folder `<name> (Patternhaus)/` beside the `.flp` (numbered when it exists) and opens it; plugin states go to `plugins/*.flpstate`, the report to `import-report.md`.
- **Corpus:** the owner's projects are a local-only test corpus.

## 11. Platforms and audio/MIDI I/O

**Decided 2026-10-10.**
- **Order:** Windows first, then Linux and macOS (CI builds all three from M1).
- **Audio I/O:** `cpal`, using WASAPI (shared and exclusive), CoreAudio, and ALSA/PipeWire/JACK.
- **ASIO: an optional GPL build.**
  - The source stays MIT/Apache, with ASIO behind a `asio` cargo feature that's off by default.
  - The official Windows installer is built with `asio` on. That binary is distributed under GPLv3, which the release notes and the About box state.
  - Builds without the feature stay MIT/Apache.
- **MIDI:** `midir`, plus MIDI 2.0 later.
- **Web build:** not planned. Browser audio and plugin hosting are too limited for a DAW.

## 12. Quality: testing, oracles, real-time safety

**Decided 2026-10-10** (Claude owns the details). In addition to the list below:
- **Agent acceptance tests:** each milestone ends with a scripted test where an agent completes the milestone's user story *through MCP only*, then the result is checked by render and analysis. For M1: build a 4-channel beat, arrange 8 bars and export a WAV.
- **Performance budgets:** CI tracks DSP cost per voice and per effect, graph overhead per node, and UI frame time on a reference project. A regression of more than 10% fails the build.

**Practices**
- **Unit tests:** in every crate.
- **Golden renders:** each test project in `tests/projects/` renders to a reference WAV. Tests compare the result within a small error tolerance (an epsilon), or hash it for bit-exact paths.
- **Real-time vs offline oracle:** the real-time engine, driven by a simulated device, must match the offline render.
- **Real-time safety:** `assert_no_alloc` in debug and CI, plus a stress test that floods the engine with plan swaps during playback.
- **Fuzzing:** every importer (FLP, NKI, SFZ, MIDI, DAWproject).
- **Command coverage:** every registered command has a schema test and at least one golden test. CI fails on unregistered or untested commands.
- **Benchmarks:** DSP and graph performance, tracked over time.

## 13. Project process

**Decided 2026-10-10.**

**Repository**
- Public at `github.com/McChaffeeFaF/patternhaus`, licensed MIT OR Apache-2.0. It can move to an organization later.
- **Contributions:** issues are open from day one. Outside PRs are accepted after M1, once the architecture has settled.
- Contributors sign off commits under the DCO (no CLA) and follow `AGENTS.md` and the clean-room policy.

**Releases.** Automatic nightly builds from `main`, plus tagged releases at each milestone. The Windows installer is the GPL build with ASIO (section 11).

**Development model**
- **Claude orchestrates:** writes briefs, routes them, reviews diffs, merges.
- Coding goes to Sonnet 5.5 agents (`sonnet-coder`). Opus handles design-heavy or diagnostic work.
- At most 2 main agents run at once.
- **Usage budget:** whatever is left after the owner's other projects. Pause at 99% weekly usage, per the owner's standing rule.
- Each track works in its own git worktree or branch, and merges after review with tests green.

**Further practices**
- **`AGENTS.md`:** the start point for humans and AI agents. It covers layering rules, real-time rules, how to add a command, test expectations and the clean-room policy.
- **Clean-room policy:**
  - Implement from public specs, documentation and observed behaviour.
  - No disassembly or decompilation of FL Studio or Kontakt binaries, and no proprietary code, samples, skins or presets.
  - Notes on file formats cite their public sources.
- **CI:** runs on Windows, Linux and macOS (build, test, clippy, fmt, layering check, asset provenance check).
- **Workflow:** `docs/parity/` and `docs/formats/` are living documents. Work is split into tracks (engine, UI, agent interface, sampler, formats) so parallel agents don't collide.
