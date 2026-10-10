# Brief M1/T5c: UI rough edges

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t5c-polish`.
**Read first:**
- `AGENTS.md`;
- DESIGN §9 (the floating workspace) and §5 (agents);
- the review follow-ups table in `docs/milestones/M1.md`: the rows from T5, T5b and T4/T5 are yours;
- `crates/ph-ui/src` (`workspace.rs`, `windows.rs`, `app.rs`, the panels);
- `crates/ph-app/src/agents.rs`;
- `crates/ph-control/src/endpoint.rs` and `grants.rs`.

**Why:** the owner tested the FL-style workspace, liked it, and asked for its rough edges to be fixed before M1 closes.

## Scope

### 1. Window contents scroll instead of clipping

Every window's content sits in a scroll area, both axes as needed, so nothing is cut off when a window is smaller than its content.
- **Exceptions:** the piano roll, playlist and step grid keep their own panning and zooming; their toolbars stay pinned.
- **Instrument windows:** scroll vertically; Haus Synth sections wrap rows of knobs onto new lines instead of overflowing sideways.

### 2. Effect window titles

The effect window title bar can render blank (see `docs/screenshots/M1/12-busy-workspace.png`, the right-hand window). Find the cause and fix it. Titles read `<Insert name> - <Effect name>` (e.g. "Insert 4 - EQ"). Add a test asserting every window kind gets a non-empty title.

### 3. Effect windows follow their effect when reordered

Today `Effect(InsertId, slot)` windows close when effects are reordered or removed.
- **Track identity per open window:** keep the effect's position, and when a new snapshot arrives, remap the window to wherever that effect moved.
  - For moves the UI performs itself, remap directly.
  - For changes from agents or undo, match by insert plus effect kind, and use the position only when the kind appears more than once in that insert.
- **Close** the window only when the effect is actually gone.
- **Remember the rect** per effect identity, so a moved effect keeps its window position.

### 4. Agents can open any window

- `ui.open_panel` and `ui.close_panel` accept `instrument:<channel_id>` and `effect:<insert>:<slot>` as well as the six panel names. Update the command's param docs and schema description in `ph-commands` (the param stays a string, so there's no contract change).
- `ui.select` on a channel opens that channel's instrument window. `ui.select` on an effect slot path opens its effect window.
- `ui.show_piano_roll` also selects the channel and pattern it names.

### 5. Escape and focus

Escape closes the front-most instrument or effect window *in z-order*, even if keyboard focus is elsewhere, as long as focus isn't in a text field.

### 6. Solo display must not drift

Solo is session state and publishes no `Change`, so the UI's solo display drifts when an agent solos something. The UI should read the session's real solo state.
- **Option A:** poll `Session::solo()` through the handle once per frame, cheaply.
- **Option B:** add an additive solo-changed notification to the `Change` stream. If you do this, log it in the AGENTS.md contract log.

Pick one and say why.

### 7. Agents panel shows live connections

- **In `ph-control`:** give the endpoint a live connection registry. It records each client's name, scopes, connected-at time, last activity, and how many commands it has run. Expose it from `LocalServer` with something like `connections() -> Vec<ConnectionInfo>`. This is an additive public API; log it.
- **In `ph-app/src/agents.rs`:** list live connections first, then remembered-but-disconnected clients, greyed out.
- **Don't block the async runtime:** `GrantPrompt::decide` currently blocks an endpoint thread for up to 120 s. Run it on a blocking thread (`spawn_blocking` or equivalent) so a waiting prompt never stalls other clients. Add a test where two clients connect while a prompt is pending.

### 8. Piano roll and editing

- **Lasso select:** Alt+drag (box select stays Ctrl+drag).
- **Slice tool:** splits notes at the click, snapped to the grid.
- **Mute tool:** toggles a note's muted state. Notes have no `muted` field in `ph_core::Note` today. Check DESIGN and the model first.
  - If adding the field is needed, it's an additive change to the `ph-core` model: `#[serde(default)] muted: bool`, honoured by the engine (a muted note doesn't play) and the analysis.
  - Make that change only as a careful, fully tested contract change, logged in the contract log, with a migration-safe default.
  - If it grows beyond that, skip the mute tool and report it.

### 9. Quit with unsaved changes

Closing the window with unsaved changes asks "Save / Don't save / Cancel" (egui viewport close-request handling). Cancel keeps the app open. Do the same for File > Open and New when there are unsaved changes.

## Out of scope (T7 owns these)

Engine `audition()` for the browser preview (don't touch the browser's preview code), licence auditing, loom/Miri, the agent acceptance test, and FL behaviour verification.

## Rules

- **No visible windows:** never launch the app window. Use offscreen `egui_kittest` for gesture tests and screenshots.
- **Never open the real audio device.** Use `AudioMode::Silent`.
- **Clean room:** match FL's behaviour, not its look.
- **Tests:** every item gets tests. Gesture tests use real egui events for scrolling, lasso, slice, mute, the quit dialog and remapping after reorder. Unit tests cover the remap logic and the connection registry.
- **Screenshots:** regenerate any that changed.
- **Docs:** update the manual test script and mark the follow-up rows done in `M1.md`.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.

## Report back

Summarise:
- each item: done, how, and with which tests;
- the choice made for solo, and why;
- whether the mute tool needed a model change;
- any contract changes;
- test counts;
- the commit list.
