# Brief M3/T2: Haus Sampler engine

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t2` on branch `m3/t2-engine`, with `CARGO_TARGET_DIR=E:\patternhaus\target`.

**Read first:**
- `AGENTS.md`: the contract log, especially the M3/T0 rows, and the local layout;
- `docs/DESIGN.md` §4 (engine and real-time rules) and §8;
- `docs/milestones/M3.md`: the Engine scope and criteria 4, 5 and 6;
- `docs/formats/haus-instrument.md`, the model's meaning;
- `crates/ph-core/src/sampler.rs`;
- `crates/ph-sampler`: all of it. `t2.rs` holds your `todo!` entry points;
- `crates/ph-engine`: `stream.rs`, `plan.rs`, `assets.rs`, and the tests `m3_oracle.rs` and `rt.rs`;
- `crates/ph-dsp`: the interpolators, envelopes, filters and the voice allocator;
- `crates/ph-instruments/src/sampler.rs`, the M1 sampler. Reuse its techniques, but don't change it.

**Goal:** make `ph.haus_sampler` sound and behave as M3 promises, while keeping live output bit-identical to offline and the audio thread free of allocation.

## Scope
1. **Playback:**
   - pitch by resampling: Hermite in Live quality, the windowed sinc in High quality, matching `ph.sampler`;
   - start and end, reverse;
   - loops: forward, ping-pong and release-loop, with **loop crossfades**.
2. **Envelopes and filter per group:** amp and filter envelopes (ADSR plus hold and delay, if the model has them), the group filter (SVF modes), and LFOs.
3. **Modulation matrix:**
   - **sources:** velocity, key, CC, pitch bend, aftertouch, LFO, envelope, random per note;
   - **targets:** pitch, volume, pan, cutoff, resonance, sample start;
   - depth and curve, scoped to a group.
4. **Crossfades:** velocity crossfades and CC crossfades (dynamics on the mod wheel), with equal-power curves.
5. **Triggers:** release triggers with decay over held time, round robins (sequential and the seeded random from T0), legato and portamento for mono and legato groups, and voice limits with stealing and short fades.
6. **Streaming:**
   - Implement T0's `StreamSource` in `ph-engine` on top of the M2 disk cache. Voices start from the instrument's preload, then read the stream.
   - Under `StreamPolicy::Live`, a block that hasn't arrived yet is silence and counts as an underrun. Under `Sync` and `Wait` it behaves exactly as M2's audio clips do.
7. **Per-group effects:** an insert chain of built-in effects per group. `ph-sampler`'s layer doesn't include `ph-effects`. Either propose the layer change (in `xtask` `LAYERS` and DESIGN §2) or have the engine compose the chain. Pick one, document it, and add a contract-log row.
8. **Multi-out:** keep T0's port rule. Real routing of the extra ports is T8; don't do it here.

## Tests (each criterion has a named test)
- **Known-answer tests per feature:** pitch, loop crossfade continuity, envelope stages, release-trigger level, round-robin order, crossfade gains, legato glide time, and stealing fades.
- **Oracle (criterion 5):** extend `m3_oracle.rs` to an instrument that uses every feature above, streamed. Compare live against offline across buffers of 256, 128 and 512 frames, 1–3 threads, a seek, a stop and a zone edit mid-song. Everything runs under `assert_no_alloc`, including stealing and a starved stream.
- **Streaming stress (criterion 4):** a generated library of about 2 GB, written at test time to a temp folder. **Mark it `#[ignore]`** with a note, and add an `xtask` subcommand to run it. A smaller version (about 50 MB) runs in CI. Measure underruns, resident memory against the preload budget, and load time.
- **Performance (criterion 6):** add the 64-voice streamed piano case to `xtask perf`, and report its load against the 30% target.
- Loom or Miri coverage for any new lock-free structure.

## Rules
- Don't change M1 or M2 behaviour or golden renders.
- Work from the Patternhaus code only. Don't read third-party sampler code.
- **No visible windows, and never open the real audio or MIDI device.**
- **Build load:** at most 4 compile jobs, one build at a time, and tests with `--test-threads 2`. Another agent is building in parallel.
- **Checks:** `fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- the features, with their test names;
- the per-group effects decision;
- the streaming numbers (underruns, memory, load time) and the perf numbers;
- the contract changes;
- the commit list.
