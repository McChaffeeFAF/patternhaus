# Brief M3/T5: Haus Sampler UI

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t5` on branch `m3/t5-ui`, with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\m3-t5`. That is your own target dir; never use the shared `target/`. See AGENTS.md "Local layout".

**Read first:**
- `AGENTS.md`: the contract log (all M3 rows) and the local layout;
- `docs/DESIGN.md` §9 (UI and the FL-style window handling);
- `docs/theming.md`;
- `docs/milestones/M3.md`: the UI scope and criterion 7;
- `crates/ph-core/src/sampler.rs`;
- the `sampler.*` commands in `crates/ph-commands/src/builtin/sampler.rs` and `sampler_files.rs`;
- `crates/ph-ui`: `windows.rs`, `plugin_ui.rs`, `catalogue_ui.rs`, `browser_model.rs`, `dnd.rs`, `widgets.rs` and the `panels/`. Also look at how the offscreen UI tests in `crates/ph-ui/tests/` work.

**Goal:** a person can see and edit a Haus Sampler instrument the way the agent can through `sampler.*`. **Every UI edit goes through the existing commands**, so undo, co-editing and the agent's view stay consistent. Add no UI-only model changes.

## Scope
1. **Sampler window**, opened from a Haus Sampler channel in the FL window style. It has:
   - **Zone map:** keys across, velocity up. Each zone is a rectangle coloured by group.
     - Click selects a zone. Shift-click and a rubber band select several.
     - Dragging moves a zone (key and velocity); dragging its edges resizes it.
     - Overlapping zones stay readable: round robins stacked, release triggers outlined.
     - Zoom and scroll.
     - Every drag is one undo step (`sampler.zone_move` / `zone_set`).
   - **Group list:** name, colour, voice limit, round-robin mode, mute and solo (if the model has them; if not, leave them out), output.
   - **Zone inspector:** the selected zone's fields, editable. Several selected zones can be edited together.
   - **Keyboard strip** under the map: zone coverage, keyswitch keys coloured by articulation, the current articulation highlighted, and click to audition (through the existing audition path).
   - **Articulations panel:** names, triggers and default (`sampler.keyswitch_set`).
   - **Modulation matrix table:** source, target, depth, curve, scope (`sampler.modulation_set`).
   - **Group envelope, filter and LFO editors:** knobs following the existing widget style.
   - **Import report:** a view of the instrument's `import_notes` and report, so the user sees what was approximated or skipped.
2. **Generic panel:** the instrument's global settings, with no skins (M3b).
3. **Drop to create:**
   - Dropping an `.sfz`, `.sf2`, `.sf3`, `.dspreset`, `.dsbundle` or `.exs` file, or a folder of samples, onto the channel rack creates a Haus Sampler channel through `sampler.import`.
   - A folder shows the auto-map **preview** (`ph_sampler_formats::automap`) before it imports.
   - SF2 files with several presets ask which preset to import.
   - The browser lists these files, and its preview plays a few notes.
4. **Save and collect:** "Save as Haus instrument…" (`sampler.save_native`), "Collect into project" (`sampler.collect`), and "Load Haus instrument…" (`sampler.load`).

## Tests
- Offscreen UI tests, following the existing ones: open the window, drag a zone (check the model and that undo restores it), resize, edit in the inspector, switch articulation from the keyboard strip, drop a generated SFZ and a sample folder (check the preview, then the import), the SF2 preset choice, and save/collect.
- Snapshot images, if the UI tests take them, for the zone map at a few sizes, in dark and light themes.
- Add a manual checklist, `docs/manual-tests/M3-sampler.md` (criterion 7): load your own SFZ (or other supported) library, play it from a MIDI keyboard, switch articulations, edit a zone, save and reopen. If the owner's private Kontakt reader is installed, include it as an optional step.

## Rules
- Use commands for all edits; make no direct model writes from the UI.
- Never open visible windows in tests, and never open real devices.
- Use at most 4 compile jobs and `--test-threads 2`. Another agent is building in parallel, with its own target dir.
- **Checks:** fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- what is in the window;
- the test names;
- screenshots: offscreen captures, saved under `local/reports/m3-ui/` (git-ignored), not committed;
- the contract changes;
- the commit list.
