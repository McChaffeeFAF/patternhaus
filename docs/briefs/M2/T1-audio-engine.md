# Brief M2/T1: Audio engine

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t1` on branch `m2/t1-audio-engine` (`git worktree add E:\patternhaus-wt\m2t1 -b m2/t1-audio-engine main`).
**Read first:**
- `AGENTS.md`: the rules, "The audio thread is sacred", and the contract log;
- DESIGN §3 (engine) and §8 (audio);
- `docs/milestones/M2.md`: "Audio clips and tempo", acceptance criteria 1, 5, 6 and 7;
- `docs/formats/m2-dependencies.md`, the T1 notes;
- `crates/ph-core/src/audio.rs` (`SampleEntry`, `AudioClip`, `StretchMode`, `time_ratio`);
- `crates/ph-engine/src/audio_clips.rs` (the T0 stub: streaming node, `trait Stretcher`);
- `crates/ph-engine` render plans, oracle tests and `FileResolver`;
- `crates/ph-analysis`.

## Scope

1. **Audio-clip playback.** Pattern and audio clips play together in song mode. The node honours:
   - `source_offset`, trim and slip;
   - gain, sample-accurate fades with their curve shapes;
   - reverse;
   - pitch;
   - mute.

   Clips that cross the loop point and clips that start mid-way when you seek are handled.
2. **Disk streaming.** A prefetch thread reads off the audio thread into lock-free ring buffers. Short samples (under ~10 s) are preloaded whole. Underruns are counted and exposed (`Engine` telemetry), and the audio thread never blocks. The offline render reads synchronously through the same path so live ≡ offline.
3. **Stretch engine.** Implement `Stretcher` on Signalsmith Stretch (MIT):
   - Prefer the `signalsmith-stretch` crate if it builds on all three CI OSes. CI may need libclang: add it to `ci.yml` (Windows: LLVM via choco or the preinstalled one; Linux: `libclang-dev`; macOS: Xcode's).
   - If that is fragile, write the thin `cc` binding described in m2-dependencies.md: a vendored header, a C wrapper and hand-written externs.
   - Update `THIRD_PARTY_NOTICES` (`cargo xtask notices`) and keep `cargo deny check` green.
4. **The three stretch modes:**
   - *Resample:* varispeed, high-quality interpolation from `ph-dsp`.
   - *Stretch:* keep pitch, follow the tempo.
   - *Slice:* re-time at detected onsets, each slice unstretched.

   Tempo-synced clips follow tempo changes, including a `TempoMap` with more than one tempo.
5. **BPM and onset detection** in `ph-analysis`:
   - onset strength;
   - a tempo estimate in 60–200 BPM with half/double resolution;
   - a confidence score;
   - onset list.

   Results are stored in `SampleEntry` metadata. Use the existing commands from T0 if they exist (check `ph-commands` for `audio.*`); otherwise add:
   - `audio.detect_bpm`;
   - `audio.set_bpm`;
   - `audio.fit_to_tempo` (sets stretch mode and ratio so the clip's bars match the project);
   - the clip-property commands T0 didn't cover.

   Every new command gets schema and behaviour tests, a contract-log row and a parity row.
6. **Oracle and real-time safety.**
   - Extend the oracle so projects with audio clips (every mode, fades, reverse, a tempo change) render bit-identical live vs offline, at block sizes 64/512/37.
   - Add an `assert_no_alloc` stress test with streaming clips.
   - Add a golden render for one audio-clip project.
7. **Objective stretch tests** on generated material (`tools/soundgen` or in-test synthesis), across 0.7×–1.3×:
   - pitch drift of a sine and of a harmonic tone, in cents;
   - transient smear on a click train: onset spread in ms;
   - level stability in dB.

   Record the numbers and the bounds you chose in `docs/milestones/M2.md` under a "Stretch quality (T1)" heading, for the owner's listening sign-off.
8. **Perf.** Extend `cargo xtask perf` with the reference project plus 8 stretched audio clips. Report the DSP load against the 25% target.

Keep the UI out of scope (T6 does the playlist and editor). The only exception: if audio clips already draw in the playlist from T0/T9, they must still draw and play.

## Rules

- **No visible windows**, and never open the real audio device. Tests use the offline and fake-device paths.
- **No GPL:** no Rubber Band, no GPL code read or copied.
- **Generated test material only.** Commit nothing from the owner's files.
- **Real-time:** no allocation, locks or I/O on the audio thread. Keep Miri and loom jobs green; add a loom model if you add a new lock-free structure.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check` and `cargo deny check` pass, and `soundgen check` stays byte-identical. Run the CI-relevant steps locally; you can't see CI, so tell me anything only CI can prove (e.g. the libclang setup on macOS/Linux).
- **At most 2 local agents run at once.** Another agent (T3 plugin hosting) works in `E:\patternhaus-wt\m2t3` at the same time. Don't touch `ph-plugin-host`. If you must change a shared file (`Cargo.toml`, `ci.yml`, `AGENTS.md` contract log, `M2.md`), keep the edit small and additive.
- **Commits:** commit with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- what plays and how (architecture in a few lines);
- which Signalsmith route you took, and its CI needs;
- oracle, real-time and golden results;
- the stretch-quality numbers with your bounds;
- BPM detection accuracy on your generated test set;
- the perf numbers;
- test counts and the commit list;
- known gaps.
