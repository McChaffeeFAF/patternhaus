# Brief M3/T3: Native instruments and the SFZ importer (clean room, implementer role)

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t3` on branch `m3/t3-sfz`, with `CARGO_TARGET_DIR=E:\patternhaus\target`.

## Your role in the clean room
You implement the SFZ importer **from `docs/formats/sfz.md` only**, plus the Patternhaus code. You must not read any SFZ parser or player source (sfizz, LinuxSampler, ConvertWithMoss, or any other), and no other SFZ write-up. If the spec is unclear, implement the most conservative reading, mark it `// SPEC-GAP: <section>`, and list it in your report.

**Read first:**
- `AGENTS.md`: the contract log (the M3/T0 rows) and the local layout;
- `docs/milestones/M3.md`: the Native format and SFZ scope, criteria 1, 2 and 8, and the clean-room rule;
- `docs/formats/haus-instrument.md`;
- `docs/formats/sfz.md`: all of it, including its import-report rules and its fixture plan (§18);
- `docs/formats/sampler-fixtures.md`;
- `docs/parity/haus-sampler.md`;
- `crates/ph-core/src/sampler.rs`;
- `crates/ph-formats/src/instrument.rs`;
- `crates/ph-sampler-formats`: the importer trait, the registry, the stubs and `tests/fuzz.rs`;
- `crates/ph-commands/src/builtin/sampler.rs`. The stubs `load`, `save_native`, `collect` and `import` are yours. Keep their ids and schemas, or log any change.

## Scope
1. **Native instruments.** Implement `sampler.load`, `sampler.save_native` and `sampler.collect` (collect into project), following §2 of the format spec. Library folders are never written. Saving is byte-stable. Undo works across load and collect.
2. **SFZ importer** (`ph-sampler-formats`):
   - headers and inheritance;
   - `#define` and `#include`, with path rules that never escape the library root;
   - note names;
   - every opcode the spec marks *must*: mapped onto the model;
   - *approx* opcodes: mapped, and listed in the import report;
   - *skip* and unknown opcodes: listed, never silently dropped;
   - *data* opcodes: kept in the model's `extra`.

   It never panics, and it loads the samples it references through the existing decoders.
3. **`sampler.import`** for `.sfz`. Formats from later tracks keep their stub errors.
4. **Drop to create.** The command side only: `sampler.import` accepts a folder of samples and auto-maps them by note names in the file names. Make the mapping rule explicit and tested. T5 adds the UI on top.
5. **Fixtures F-SFZ-01 … F-SFZ-17** from the spec, generated at test time. Each renders to a reference (golden conventions) and passes the round trip: import, save as native, reopen, identical render. Mark any fixture that needs T2's DSP and has no sound yet as **pending T2** in the parity tracker, but still check the imported model for it.
6. **Fuzzing:** seed the SFZ fuzz target with the malformed fixtures.
7. **Parity:** fill in the SFZ rows of `docs/parity/haus-sampler.md` (supported, approximated, skipped, test).

## Rules
- Don't touch `ph-sampler` DSP or `ph-engine`. T2 is working there in parallel. If you need an engine change, describe it in your report instead.
- **Build load:** at most 4 compile jobs, one build at a time, and tests with `--test-threads 2`.
- **No visible windows, and never open real devices.**
- **Checks:** `fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- what is supported, approximated and skipped;
- the SPEC-GAPs;
- the fixture and test counts;
- the contract changes;
- the commit list.
