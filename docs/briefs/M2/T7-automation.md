# Brief M2/T7: Automation

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t7` on branch `m2/t7-automation` (`git worktree add E:\patternhaus-wt\m2t7 -b m2/t7-automation main`).
**Read first:**
- `AGENTS.md`;
- DESIGN §3 and §9;
- `docs/milestones/M2.md`: "Automation", acceptance criteria 1, 6 and 7;
- `crates/ph-core/src/automation.rs` (the model and its rules);
- `crates/ph-engine/src/automation.rs` (the T0 evaluator; T1 already wired **tempo** automation into the plan's tempo map);
- the `automation.*` commands in `ph-commands`;
- `ph-node` param events;
- `crates/ph-ui` (playlist, knobs, the right-click placeholder T8 of M1 left on knobs, the workspace window manager).

## Scope

1. **Engine.** Automation clips drive every automatable target, sample-accurately, through `ph-node` param events (value or ramp) or the mixer's own smoothing:
   - instrument and effect params, including plugin params through `ph-plugin-host` (T3);
   - mixer volume, pan and send levels;
   - channel volume and pan;
   - tempo (already done by T1: keep it).

   Curve types: linear, hold, smooth, S-curve. Overlap rules follow `ph_core::automation`.
   - When automation stops, the param returns to the project value, FL-style.
   - Live ≡ offline bit-exact, extend the oracle with automation on several target kinds at block sizes 64/512/37.
   - Add an `assert_no_alloc` stress test with automation.
2. **Commands.** Finish whatever the `automation.*` family lacks, including `automation.record` from knob moves while playing (thinned to a reasonable point density). Every command gets schema and behaviour tests, a contract-log row and a parity row.
3. **UI:**
   - automation clips draw in the playlist as a curve inside the clip, and can be placed, moved, resized and deleted like pattern clips;
   - an automation-clip editor window (a floating workspace window) to add, move and delete points, change a point's curve type by right-click, and snap to grid;
   - **"Create automation clip"** from any knob's right-click menu: creates a clip on a free playlist track at the loop or selection range, targeting that param;
   - knobs under automation show it (an accent ring) and follow the automated value while playing;
   - recording automation: with record armed and playing, knob drags write points.

   All of this works in both themes and passes the long-names test.
4. **Perf.** Add 4 automation clips to the perf project (on top of T1's 8 stretched clips) and report DSP load against the 25% target.

## Rules

- **No visible windows**, and never open the real audio device. UI tests run headless (egui_kittest offscreen), like the existing ones.
- **Test waits:** settle on the session's snapshot (see `crates/ph-ui/tests/common::settle`), never on a fixed frame count.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check` and `cargo deny check` all pass.
- **At most 2 local agents at once.** T6 (audio clip UI, editor, slicer) works in `E:\patternhaus-wt\m2t6` at the same time and also edits the playlist. Keep your playlist changes to automation-clip drawing and interaction, in their own module where you can; keep shared-file edits small and additive.
- **Commits:** commit with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- what is automatable and how it reaches the audio;
- oracle and real-time results;
- the UI features, with screenshots in the scratch folder (not committed unless `PH_SCREENSHOTS=1` docs shots);
- the perf numbers;
- test counts and the commit list;
- known gaps.
