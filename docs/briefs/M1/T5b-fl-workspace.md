# Brief M1/T5b: FL-style floating workspace

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t5b-workspace`.
**Read first:**
- DESIGN §9, especially the **window layout revised 2026-10-10** section, which is the spec;
- `AGENTS.md`;
- `crates/ph-ui` (the current docked shell, panels and keymap);
- `crates/ph-app/src/main.rs`.

**Why:** the owner tested the M1 build and prefers FL Studio's window handling to the docked `egui_dock` layout. The panels' *contents* stay as they are. Only the window system around them changes.

## Goal

Replace the dock shell with a floating-window workspace that behaves like FL Studio's.

## Scope

### 1. Window manager model

A pure Rust model with no egui types, so it's unit-testable: `ph_ui::workspace`.

**Windows:**
- **Window ids:** `Playlist`, `ChannelRack`, `PianoRoll`, `Mixer`, `History`, `Agents`, `Instrument(ChannelId)`, `Effect(InsertId, slot)`.
- **State per window:** rect, visible, maximized (with the rect to restore), z-order, and detached (multi-viewport, optional).

**Operations:**
- **`focus` (bring to front)**, **`move`**, **`resize`** (with minimum sizes).
- **`toggle(id)`**, the F-key semantics: hidden → show and front; visible but not front → front; front → hide.
- **`maximize_toggle`.**
- **`clamp_to_workspace`:** the title bar always stays reachable; apply it when the workspace resizes too.
- **Snapping:** magnetic snap to the workspace edges and to other windows' edges within about 8 px while dragging or resizing.

**Defaults:** the playlist opens maximized as the background; the channel rack floats at upper left; the mixer, piano roll and others start hidden.

**Persistence:** the layout serialises to `layout.ron` (reuse the existing RON settings path), per user, with an optional per-project override. Instrument and effect windows remember their rects by object.

**Unit tests:**
- toggle semantics;
- z-order;
- snapping maths;
- clamping after the workspace shrinks;
- maximize and restore;
- round-trip persistence;
- closing a channel's window when the channel is deleted (driven by snapshots).

### 2. egui shell

- **Browser:** a docked left `SidePanel` with a resizable width. F8 toggles it.
- **Workspace:** the central area hosts the windows, drawn as `egui::Area`s or custom-decorated windows. The title bar has the title, a detach button, maximize and close, in our own flat style. Double-clicking the title maximizes.
- **Driven by the model:** the window manager is the source of truth. egui only renders it and reports drags.
- **Window contents:** existing panels render inside their windows unchanged. The playlist window keeps working when maximized as the background.
- **Opening instrument windows:**
  - clicking a channel's name or button in the rack opens `Instrument(id)`, replacing the Inspector tab;
  - several can be open at once;
  - Instrument windows for the Sampler and Haus Synth reuse the inspector UIs.
- **Opening effect windows:** clicking an effect slot in the mixer opens `Effect(insert, slot)` with that effect's knobs. The mixer keeps only its strip and routing view, which also fixes the knob clipping noted in the M1 follow-ups.
- **Agent requests:** agent `ui.open_panel`, `ui.close_panel`, `ui.show_piano_roll` and `ui.select` map onto the window manager. For example, `ui.show_piano_roll` shows the piano roll and brings it to the front.
- **Remove `egui_dock`.**

### 3. Keymap

- **F-keys:** F5 playlist, F6 channel rack, F7 piano roll, F9 mixer, F8 browser, with the toggle semantics above.
- **History and Agents:** keep Ctrl+H and Ctrl+Shift+A.
- **Escape:** closes the focused instrument or effect window.
- Update `keymap.default.toml` and its test.

## Rules

- **No visible windows:** never launch the app window. The owner uses the desktop. Use offscreen `egui_kittest` for gesture tests and screenshots.
- **Never open the real audio device.** Use `AudioMode::Silent`.
- **Clean room:** match FL's *behaviour* only. Don't copy its title-bar styling, icons, colours or layout pixel-for-pixel.
- **Gesture tests through real egui pointer events:**
  - dragging a window by its title moves it;
  - dragging an edge resizes it;
  - clicking a back window raises it;
  - double-clicking a title maximizes;
  - pressing F7 twice: the first press shows the window, the second hides it once it's in front;
  - clicking a channel opens its instrument window.
- **Performance:** keep the 10,000-note piano-roll spike under 8 ms per frame with the piano roll inside a floating window.
- **Screenshots:** regenerate `docs/screenshots/M1/` for the new shell. Include a busy workspace with overlapping windows and two instrument windows open.
- **Manual test script:** update `docs/manual-tests/M1-ui.md` for the new window behaviour.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass.

## Report back

Summarise:
- the window manager design;
- the behaviours implemented;
- the gesture tests;
- the performance numbers;
- the screenshot paths;
- test counts;
- the commit list.

Commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.
