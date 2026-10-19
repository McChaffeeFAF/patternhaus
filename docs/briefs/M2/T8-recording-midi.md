# Brief M2/T8: Recording and MIDI

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t8` on branch `m2/t8-recording-midi` (`git worktree add E:\patternhaus-wt\m2t8 -b m2/t8-recording-midi main`).
**Read first:**
- `AGENTS.md`;
- DESIGN §3, §5 and §9;
- `docs/milestones/M2.md`: "Recording", "MIDI controllers", acceptance criteria 2 and 6;
- `crates/ph-engine/src/record.rs` (the T0 recording contract and simulated input);
- the `record.*` and `midi.*` commands (`midi.learn`, `midi.map`, `midi.unmap`, `midi.list_inputs`, `midi.list_mappings`; check which are stubs);
- `ph-node` CC, bend and expression events;
- the Haus Synth (mod wheel, bend);
- T7's automation recording (`automation.record`) and knob right-click menu;
- T6's audio editor (its disabled record button), if T6 is merged by the time you get there.

## Scope

1. **Audio recording** (cpal input):
   - list inputs; arm a channel or playlist track;
   - input monitoring with a latency readout;
   - count-in and punch in/out over the loop region;
   - each take is saved as a WAV in the project's `samples/` (never overwriting) and placed as an audio clip at the record position, compensating input latency, as one undo step;
   - the recorder writes from the audio callback through a lock-free ring to a writer thread; nothing on the audio thread allocates or does I/O.

   Also:
   - **Monitor path:** live ≡ offline when monitoring is off. Monitoring is excluded from offline renders.
   - **WASAPI exclusive mode** on Windows only if cpal exposes it cleanly; otherwise record why and leave it as a follow-up.
2. **MIDI input** (`midir`; check its licence with `cargo deny`):
   - list inputs and hot-plug refresh;
   - notes go to the selected channel and sound immediately via the preview path;
   - while recording in pattern mode, notes are written into the pattern (quantise optional) as one undo step per take;
   - CC, pitch bend and channel pressure become `ph-node` events. The Haus Synth's mod wheel and bend respond.
3. **MIDI learn:**
   - "MIDI learn" on any knob's right-click menu (next to T7's "Create automation clip");
   - the next CC moved maps to that control;
   - mappings are saved **per user** in the config folder, not in the project;
   - `midi.list_mappings`, `midi.map` and `midi.unmap` work.

   With automation recording armed, mapped CC moves record through `automation.record`.
4. **Agents:** every new command gets schema and behaviour tests, a contract-log row and a parity row.

## Testing without devices

- **Never open the real audio or MIDI device.** Use the simulated input from T0 and a fake MIDI source behind a small trait.
- The real cpal input and midir paths compile and are exercised only through those traits in tests.
- Write a manual-test checklist for the owner (`docs/manual-tests/M2-recording-midi.md`): record from the interface, monitor, punch, play a MIDI keyboard, learn a knob.

## Rules

- **No visible windows.** UI tests run headless; they settle on the session snapshot, never on fixed frame counts.
- **Real-time:** `assert_no_alloc` stress tests for recording and MIDI input. Keep Miri and loom green; add a loom model for any new lock-free structure.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check` and `cargo deny check` all pass.
- **At most 2 local agents at once:** T6 (audio UI, editor, slicer) may still be running in `E:\patternhaus-wt\m2t6`. Keep shared-file edits small and additive.
- **Commits:** commit working increments as you go with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- what records and how (the threads and buffers);
- latency compensation;
- the MIDI paths;
- MIDI learn;
- the test approach without devices;
- test counts and the commit list;
- known gaps, including what only the owner's hardware can prove.
