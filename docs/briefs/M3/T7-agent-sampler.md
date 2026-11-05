# Brief M3/T7: Agent features for the Haus Sampler

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t7` on branch `m3/t7-agent`, with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\m3-t7`. That is your own target dir; never use the shared `target/`.

**Read first:**
- `AGENTS.md`: the contract log (all M3 rows) and the local layout;
- `docs/DESIGN.md` §5 (the agent interface: perception, diffs, co-editing);
- `docs/milestones/M3.md`, especially criterion 1 and the Agent features scope;
- `crates/ph-control`: views, `views_m3.rs`, resources, server instructions, snapshot, the musical diff, `agent.rs` and `coverage.rs`;
- the `sampler.*` and `catalogue.*` commands in `crates/ph-commands`;
- `crates/ph-analysis`;
- `crates/ph-cli/tests/acceptance_m2.rs`, the model for the acceptance test.

**Goal:** an agent working through MCP alone can understand, edit, hear and see a Haus Sampler instrument as well as a person in the UI can. Then prove it with the M3 acceptance test.

## Scope
1. **`get_state { view: zones }`**, completed:
   - a compact zone map per instrument: zones grouped by group, key and velocity ranges, roots, round-robin and trigger, articulations and keyswitches;
   - a short **playing guide** derived from the instrument (playable range, keyswitches with names, CCs that do something, velocity layers), in the same shape as the catalogue guide, so an agent plays a Haus instrument and a catalogue plugin the same way;
   - an `instrument` filter, and size limits with paging for big libraries.
2. **`snapshot { kind: zone_map }`:** an image of the zone map, consistent with the UI's colours, and readable at agent image sizes.
3. **Musical diff** for `sampler.*` edits: "moved zone c4_f.wav to C4–D4 vel 81–127", "added articulation Staccato on C0", and so on. Also cover the `while_you_worked` blocks.
4. **Import report over MCP:** the `sampler.import` result already returns it. Add a resource `patternhaus://instrument/<id>/report` and mention it in the server instructions.
5. **Housekeeping:**
   - removing a channel offers to remove its now-unused instrument. That's a command option; don't do it silently;
   - a `sampler.prune` command removes unused instruments, as one undo step;
   - `project.new` with instruments gets a test beyond the empty case.
6. **Server instructions:** short guidance on sampler workflows: import or load, read the zones view or guide, play inside the range and use keyswitches, render and analyze. Keep it tight.
7. **Acceptance test, criterion 1** (`crates/ph-cli/tests/acceptance_m3.rs`): a scripted agent driving `patternhaus-cli mcp --headless` over stdio, MCP only:
   - it loads a generated multi-sample SFZ (velocity layers, round robins, release triggers, two keyswitched articulations) and a generated SF2 drum kit;
   - it writes a 16-bar part that uses both articulations;
   - it inspects zones through `get_state` and edits them (moves a zone, changes a velocity split);
   - it saves the instrument as native, saves and reopens the project, and renders it.

   **Checks:** the export matches the offline oracle; `analyze` finds the intended key and chords; the edit is audible in the render (or checked through the zones view); the renders before and after reopening are identical. Add it as a CI step named "M3 acceptance", like M2's.

## Rules
- Never open visible windows, and never open real devices.
- Use at most 4 compile jobs and `--test-threads 2`. Another agent is building in parallel.
- **Checks:** fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- each feature with its test names;
- the acceptance numbers (LUFS, key and chords found);
- the contract changes;
- the commit list.
