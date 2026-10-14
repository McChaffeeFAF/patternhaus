# Brief M2/T6: Playlist audio UI, audio editor, slicer

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t6` on branch `m2/t6-audio-ui` (`git worktree add E:\patternhaus-wt\m2t6 -b m2/t6-audio-ui main`).
**Read first:**
- `AGENTS.md`;
- DESIGN §9 (UI, Look, the floating workspace);
- `docs/milestones/M2.md`: "Audio clips and tempo", "Audio editor and slicer", acceptance criteria 1 and 2;
- `crates/ph-core/src/audio.rs`;
- the T1 audio engine (`ph-engine` `player.rs`, `stream.rs`, `audio_clips.rs`);
- the `audio.*`, `sample.*`, `slicer.*` and `editor.*` commands (`editor.open`, `editor.apply` and `slicer.make_instrument` are still stubs);
- `crates/ph-ui` (playlist, drag and drop from M1/T9, the `paint::text_fit` helpers, themes).

## Scope

1. **Audio clips in the playlist, FL-style:**
   - drawn with a waveform, cached per zoom level off the UI thread (no frame-time regressions: check `cargo xtask perf` and the UI frame-time test);
   - drag in from Explorer or the browser (building on M1/T9);
   - trim either edge, slip-edit (Alt+drag inside the clip), split (slice tool), move, duplicate, delete;
   - fade-in and fade-out handles with curve shapes; gain, reverse and pitch from the clip's right-click menu or properties;
   - the stretch mode per clip (Resample / Stretch / Slice), the detected or set BPM, and **Fit to tempo** in one click;
   - commands only, so every gesture is one undo step and agents see the same diffs.
2. **Audio editor (Edison-style)** as a floating workspace window:
   - waveform with zoom and scroll, selection, playhead, audition of the selection;
   - cut, copy, paste, trim, normalise, fade in and out, reverse, silence, gain, a simple de-click;
   - **never overwrites:** each edit writes a new file into the project's `samples/` and repoints the sample, so the original is never touched and undo works (implement `editor.open`/`editor.apply`);
   - record into the editor is T8's: leave a disabled button.
3. **Slicer:**
   - transient detection with a sensitivity setting (`slicer.detect` exists from T1);
   - slice markers you can add, move and delete by hand;
   - **build a sliced Sampler instrument**, one slice per key (`slicer.make_instrument`), with the option to write a pattern that replays the original loop.
4. **Agents.** Every new command gets schema and behaviour tests, a contract-log row and a parity row. `snapshot` can show an audio clip's waveform.

## Rules

- **No visible windows**, and never open the real audio device. UI tests run headless (egui_kittest offscreen), like the existing ones.
- **Test waits:** settle on the session's snapshot (see `crates/ph-ui/tests/common::settle`), never on a fixed frame count.
- **Files:** generated test audio only. Never delete or overwrite user files.
- **Long names:** new UI must pass the long-names test in both themes.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check` and `cargo deny check` all pass.
- **At most 2 local agents at once.** T7 (automation) works in `E:\patternhaus-wt\m2t7` at the same time and also draws automation clips in the playlist. Keep your playlist changes to audio clips, in their own module where you can; keep shared-file edits small and additive.
- **Commits:** commit with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- the playlist gestures;
- the editor operations and how "never overwrites" works;
- the slicer;
- screenshots in the scratch folder;
- frame-time numbers before and after;
- test counts and the commit list;
- known gaps.
