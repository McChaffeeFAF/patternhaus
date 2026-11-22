# Brief M3/T11: Controller lane editor in the piano roll

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/lane-editor` on branch `m3/t11-lane-editor` (from `main`), with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\lane-editor`. Never use the shared `target/`.

**Why:** T10 added controller lanes (CC 0-119 and pitch bend per channel per pattern) with `cc.*` commands, engine playback, views and a snapshot strip. People cannot see or draw them in the app yet. This track adds FL's "event editor" under the piano roll.

**Read first:**
- `AGENTS.md`: the contract log, especially the T10 rows, and the local layout;
- `docs/briefs/M3/T10-controller-lanes.md`;
- `crates/ph-core/src/controllers.rs`;
- `crates/ph-commands/src/builtin/cc.rs` (the commands and their exact parameters);
- `crates/ph-ui`:
  - `panels/piano_roll.rs`, `piano_roll.rs` and `piano_roll/` (widget, draw, actions, tests);
  - `automation.rs` and `auto_editor.rs` (the existing automation point and curve editing: reuse its interaction model and drawing where it fits);
  - `widgets.rs`, `theme.rs` and `keymap.rs`;
- the offscreen UI tests in `crates/ph-ui/tests/` and `piano_roll/tests.rs`.

## Scope
1. **Lane area under the note grid.** Resizable, collapsible, sharing the grid's horizontal zoom and scroll exactly.
   - A selector: Velocity (if the roll already has a velocity editor, fold it in here; if not, leave velocity out), CC1 Mod Wheel, CC11 Expression, CC7 Volume, CC64 Sustain, Pitch Bend, and "Other CC…" with a number. Lanes that already have points are listed first.
   - Lanes with points show as small tabs, so switching between them is one click.
2. **Editing**, all through the `cc.*` commands (never direct model writes), one undo step per gesture:
   - click to add a point; drag to move it in time and value (snapping to the roll's grid, Alt for free);
   - right-click a point to delete it; rubber-band select and delete;
   - a **pencil/draw** gesture that writes a freehand curve, thinned to a sensible point density and sent as one command;
   - a **line tool** (shift-drag) that writes a straight ramp;
   - a per-segment curve choice (smooth, linear, s-curve, hold) from a point's context menu, plus tension by dragging the segment (as automation does).
3. **Display:**
   - the evaluated curve, drawn exactly as the engine plays it (use the shared `value_at` evaluation);
   - points and the value under the mouse in the controller's units (0-127 for CC, -8192..8191 for bend);
   - the bend lane centred at 0;
   - other authors' edits (an agent's) appearing live, like notes do.
4. **Keyboard:** delete the selection; Ctrl+A in the lane selects its points when the lane has focus. Add the bindings to the keymap and to the keyboard help, if there is one.

## Tests
Offscreen UI tests:
- add a point and check the model and undo;
- drag a point;
- draw freehand (the point count is thinned, and it is one undo step);
- the line tool;
- change a curve;
- switch lanes;
- the bend lane's display;
- the lane follows the grid's zoom and scroll;
- an agent edit from another author shows up.

Snapshot images of the lane area, if the UI tests take them, are saved to `local/reports/lane-editor/` (git-ignored) and not committed.

Add a manual checklist section to `docs/manual-tests/M3-sampler.md`, or a new `docs/manual-tests/M3-lanes.md`.

## Rules
- Never open visible windows or real devices.
- Use at most 4 compile jobs and `--test-threads 2`. Another build runs in parallel.
- Don't touch `ph-plugin-host`, `ph-engine` or `ph-cli`: someone else is working there.
- **Checks:** fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- Contract log rows if any public API changes; a parity row update in `docs/parity/fl-studio.md`.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- what is in the editor;
- the test names;
- screenshots saved locally;
- the contract changes;
- the commit list.
