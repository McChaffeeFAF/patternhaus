# Brief M3/T4: SF2/SF3, DecentSampler and EXS24 importers (clean room, implementer role)

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t4` on branch `m3/t4-sf2-ds-exs`, with `CARGO_TARGET_DIR=E:\patternhaus\target`.

## Your role in the clean room
You implement three importers **from our specs only**: `docs/formats/sf2.md`, `docs/formats/dspreset.md` and `docs/formats/exs.md`, plus the Patternhaus code. You must not read:
- any SoundFont, DecentSampler or EXS parser or player source (FluidSynth, ConvertWithMoss, or any other);
- the SoundFont PDF;
- any other write-up of these formats.

If a spec is unclear, implement the most conservative reading, mark it `// SPEC-GAP: <file> §<n>`, and list it in your report. Where `exs.md` marks a fact as disputed ([D?]), follow its [PH] choice. If it has none, accept both readings when the file makes it unambiguous, and report the case otherwise.

**Read first:**
- `AGENTS.md`: the contract log (the M3 rows) and the local layout;
- `docs/milestones/M3.md`: the importers scope, criteria 2 and 8, and the clean-room rule;
- the three specs, including each one's fixture plan;
- `docs/formats/sampler-fixtures.md`;
- `docs/parity/haus-sampler.md`;
- `docs/formats/haus-instrument.md`;
- `crates/ph-core/src/sampler.rs`;
- `crates/ph-sampler-formats`: all of it. **The SFZ importer (`src/sfz/`) is the pattern to follow:** report classification, probing samples, fixtures with golden references and native round trips, and fuzz seeds;
- `crates/ph-commands/src/builtin/sampler_files.rs` (`sampler.import`);
- `docs/formats/m3-dependencies.md`.

## Scope
1. **SF2/SF3** (`.sf2`, `.sf3`):
   - the RIFF reader;
   - presets, instruments, zones and generators, with the summing rules;
   - modulators: the defaults plus the ones the model can express, the rest reported;
   - stereo links and loops;
   - samples extracted from `sdta` (24-bit with `sm24`).

   SF3 Ogg samples go through the existing Ogg decoder. A multi-preset file imports one preset. `sampler.import` gets an optional `preset` parameter (bank and program, or name), defaulting to the first preset, and the report lists all presets.

   **Where samples go:** an SF2 has no sample files, so the samples must be written out. Write them into the project's `instruments/<name>/samples/` as WAV (collected), and document this.
2. **DecentSampler** (`.dspreset`, plus `.dslibrary` if the spec covers it):
   - XML: write a small reader of our own, as `m3-dependencies.md` recommends, unless you find a strong reason not to, which you must justify;
   - groups, samples, inheritance, round robin, triggers, loops, envelopes, effects (mapped to built-ins or reported);
   - UI, bindings and modulators: kept as data in `extra` for M3b, and reported.
3. **EXS24** (`.exs`): both byte orders, all blocks, zones, groups, sample lookup (the spec's search rules, never leaving allowed folders), and parameters.
4. **`sampler.import`** for all three. Kontakt keeps its stub error.
5. **Fixtures:** every fixture id in the three specs' plans, generated at test time with byte-level builders (`Sf2Builder`, `ExsBuilder`; DecentSampler as strings). Each renders to a reference and passes the native round trip, like the SFZ fixtures. T2's DSP may not be on main when you start. If it isn't, mark fixtures that depend on it **pending T2** in the parity tracker, as T3 did.
6. **Fuzzing:** one target per format, seeded with the malformed fixtures.
7. **Parity:** fill in the SF2/SF3, DecentSampler and EXS rows.

## Rules
- Don't touch `ph-sampler` DSP or `ph-engine`. T2 is working there. Describe any engine or model need in your report instead. Small additive model changes in `ph-core::sampler` are fine if they're logged in the contract log.
- No new third-party crates without a licence check, and a row in `m3-dependencies.md`.
- **Build load:** at most 4 compile jobs, one build at a time, and tests with `--test-threads 2`.
- **No visible windows, and never open real devices.**
- **Checks:** `fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
For each format:
- what is supported, approximated and skipped;
- the SPEC-GAPs and disputed facts met.

Also:
- the fixture and test counts;
- the contract changes;
- the commit list.
