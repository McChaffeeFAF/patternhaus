# Brief M1/T5: UI

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t5-ui`.
**Read first:**
- `AGENTS.md`;
- DESIGN §9 (all of it), §3 (co-editing, per-author undo) and §1 (scope);
- `docs/milestones/M1.md` (acceptance criteria 2 and 3);
- `docs/parity/fl-studio.md`;
- `crates/ph-control/src/lib.rs` (`Studio`, the only thing the UI talks to);
- `ph_commands` (`Registry::list`, `SessionHandle`, `Change`, `UiRequest`);
- `ph_engine` (`status`, `stats`, `meters`, `preview_note`, `catalog`).

**Depends on:** everything on `main`. T4 (agent interface) runs in parallel and owns `ph-control`'s MCP, JSON-RPC and analysis parts. Don't change `Studio` or `EngineHost` without recording it in the contract log, and don't touch `ph-analysis` or `ph-cli`.

## Goal

A desktop app a person can make a beat in, with FL Studio muscle memory. It works alongside agents that are editing live.

## Ground rules

- **Commands only.** The UI never mutates the project. Every action is a command through `studio.session`, as `Caller::human()`.
- **Snapshots only.** It reads `Arc<Project>` snapshots from the `Change` subscription, plus `engine.status()`, `stats()` and `meters()` for live display.
- **Responsive.** Edits are asynchronous (`submit_*`), so the UI never blocks on the control thread.
- **Clean room.** Match FL's *functional* conventions: shortcuts, mouse gestures, workflow. Don't copy FL's visual design, icons, colours or skins. The look is the DESIGN §9 "modern flat dark".

## Scope

### 1. App shell (`ph-app` binary; `ph-ui` library)

- **Window:** eframe on wgpu, with `egui_dock` (or equivalent, permissive licence) for docked and detachable panels.
  - Multi-viewport, so any panel can float onto another monitor.
  - Layouts are saved per user (and optionally per project).
- **Startup:** `ph-app [project_dir]` starts `Studio` with `AudioMode::Device`, falling back to `Silent` with a visible notice if no device opens.
  - It opens the project folder, or a new project via `project.new`.
  - The factory `assets/` folder is found next to the executable, or in the repo during development.
- **Local agent endpoint:** call T4's `serve_local(&studio)` if it has merged into `main`. Otherwise leave a clearly marked hook.

### 2. Panels

**Transport bar:**
- play/stop, pattern/song mode, tempo (drag and type), position (`bar:beat:tick`), metronome, loop;
- master meter, DSP load and overload count;
- the engine's "use a larger buffer?" suggestion, shown as a non-blocking notice;
- the pattern selector.

**Browser:**
- factory kits (`factory:` samples) with click-to-preview;
- Haus Synth presets;
- files under the project folder and library folders.

Dragging a sample onto the rack creates a Sampler channel; dragging a preset creates a Synth channel.

**Channel rack and step sequencer:**
- channel rows with mute, solo, volume, pan and the mixer-insert number;
- step buttons: 16 by default, extending with pattern length;
- a per-step velocity view;
- right-click menus (rename, colour, clone, delete, route);
- channel reordering.

**Piano roll.** **Build this first, as a spike** (see Performance). It needs:
- draw, select (box or lasso), move, resize and delete;
- duplicate (Ctrl+B), quantize, transpose, and copy/paste;
- a velocity lane, snap settings, horizontal and vertical zoom, and a keyboard with note names;
- ghost notes from other channels;
- preview on click (`engine.preview_note`);
- **agent preview hooks:** notes changed by an agent flash and show an author badge (DESIGN §3).

**Playlist:**
- tracks with mute and name;
- pattern clips: place, move, resize, delete;
- loop region, playhead, and click-to-seek;
- zoom.

**Mixer:**
- insert strips with meters, volume, pan, mute, solo and name;
- 10 effect slots per insert (add, remove, reorder, bypass, mix);
- sends with levels, `route_to_master`, and sidechain source.

**Plugin and effect panels:**
- generic knobs and switches built from `ph_engine::catalog` descriptors (units, choices, stepped);
- double-click resets to default;
- the instrument panel for channels: Sampler with a waveform view; Haus Synth grouped by section.

**History panel:**
- per-author entries showing the author, a summary and the time;
- buttons: "Undo mine", "Undo all by agent X", and undo of a single transaction;
- **the conflict dialog**, which shows the conflicting paths, fields and authors, with Skip / Force / Cancel (DESIGN §3).

**Agents panel:**
- connected clients, their scopes, and their recent actions;
- the grant prompt when a new client connects (the hook comes from T4).
- Selections by agents (`ui.select`) highlight in the relevant panel.

### 3. Keymap, themes and typing keyboard

- **Keymap file** (TOML) mapping keys to command ids. The defaults follow FL's functional shortcuts:
  - F5 playlist, F6 channel rack, F7 piano roll, F8 browser, F9 mixer;
  - space to play/stop, Ctrl+Z / Ctrl+Y for undo/redo (your own, DESIGN §3);
  - Ctrl+S to save, Ctrl+B duplicate, Ctrl+Q quantize, Ctrl+L to clone a pattern;
  - piano-roll tools.

  Every binding is remappable.
- **Typing keyboard to piano:** the computer keyboard plays the selected channel through `preview_note`, with octave shift.
- **Themes:** dark and light theme files (TOML tokens), scaled correctly at any DPI.

## Performance (measure and report)

- **Piano-roll spike first:** 10,000 notes, scroll, zoom and drag. Frame time must be 8 ms or less on the owner's machine (7800X3D, RTX 3090). If egui can't do this, stop and report before building the other panels.
- **Meters and playhead** update at the display refresh rate without stalling the audio thread. They read atomics, never the audio path.
- **Big projects:** opening a project with 100 channels and 50 patterns stays interactive.

## Done when

1. **M1 acceptance criterion 2** can be performed by hand:
   - build a 4-channel beat with factory sounds and a Haus Synth preset;
   - use the step sequencer and piano roll;
   - add EQ, compressor and reverb send;
   - arrange 8 bars;
   - play it live at a 256-sample buffer;
   - save, reopen, and export a WAV.

   Write the steps as a script in `docs/manual-tests/M1-ui.md` for the owner.
2. **Criterion 3 is visible in the UI:** a test agent (scripted through `Studio`) edits while you do, and per-author undo and the conflict dialog behave as DESIGN §3 says.
3. **UI logic is tested** without a window: command mapping, keymap parsing, and view models (piano-roll hit-testing, quantize grid, step mapping).
4. **Checks:** `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass. CI on Linux needs the GUI dev packages, which are already installed in the workflow.
5. **Parity tracker:** the Keyboard and UI rows are filled with *Wired*.

## Out of scope

- The analysis views (T4 provides data; showing it is M2).
- MIDI controllers, recording, audio clips and plugin GUIs (M2).
- The modular routing UI and performance mode (M4).

## Report back

Summarise:
- the panel list and how each maps to commands;
- the piano-roll spike numbers and the other performance measurements;
- the keymap;
- screenshots: save PNGs to `docs/screenshots/M1/` using egui's screenshot facility;
- any contract changes;
- test counts;
- the commit list.

Commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.
