# Brief M2/T5: FLP importer (clean room, implementer role)

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m2/t5-flp-import`.

## Your role in the clean room

You are the **implementer**. Build the importer from **`docs/formats/flp.md` only**, plus the Patternhaus code. **You must not read:**
- PyFLP's source code or documentation;
- any other FLP parser or format write-up;
- `docs/formats/flp-corpus-report.md` (the spec writer's notes);
- the spec writer's scratch files.

If the spec is unclear, implement the most conservative reading, mark it `// SPEC-GAP: <section>` in the code, and list it in your report. Never fill a gap from outside sources.

**Read first:** `AGENTS.md`; `docs/formats/flp.md` (all of it); DESIGN §10; `docs/milestones/M2.md` (FLP section and acceptance criterion 3); `crates/ph-core` (model, including M2 audio/automation clips and the sample registry); `crates/ph-formats`; `crates/ph-commands/src/builtin/m2_stubs.rs` (`file.import_flp` is stubbed there, so keep its id and params).

## Scope

1. **Parser** in `ph-formats` (`flp` module). It turns bytes into an event stream into a typed intermediate structure.
   - It must never panic on any input: return errors.
   - Tests use synthetic byte sequences built in the test.
   - Add a cargo-fuzz-style fuzz target, or a proptest over random bytes, that runs in CI (bounded).
2. **Mapping** from the intermediate structure to a Patternhaus `Project`, via `Edit`:
   - **Project:** tempo (spec: BPM × 1000), time signature, title and metadata.
   - **Channels:**
     - Samplers map to `Sampler` with the sample path. Resolve `%…%` placeholders where the spec says how; otherwise keep the path and report it.
     - Generator plugins become placeholders that keep their plugin id and state blob, saved as `plugins/<file>` per DESIGN §10.
     - Audio-clip channels become sample-registry entries plus audio clips.
     - Automation channels become automation clips, mapping targets via `ParamPath` where the spec confirms the target encoding.
   - **Patterns and notes:** convert PPQ from the file's PPQ (96 in the corpus) to 960, exactly. Map key, velocity, pan, fine pitch and length.
   - **Playlist:** tracks (names) and clips (pattern, audio and automation) with positions, lengths and offsets in ticks.
   - **Mixer:** inserts (names, colours, volume and pan via the spec's raw scales), routing, and effect slots. VST2 and VST3 slots become `EffectSpec::Plugin` placeholders that keep their state; native FL effects become labelled placeholders.
   - **Levels:** use the spec's confirmed raw-to-unit mappings. Where a curve is unknown (for example channel volume), use the linear mapping and record an approximation in the report.
3. **Import report** (`ImportReport`):
   - counts per object kind;
   - every approximation, skipped item and unknown event id, with how often each occurred;
   - written as `import-report.md` into the project folder and returned by the command.
4. **Command:** `file.import_flp { path }` creates a new project from the file. It's destructive like `file.open`: confirm, and the unsaved-changes rule applies.
   - The human may import from any path (as with `file.open`); agents only from allowed roots.
   - It also moves the project folder: import into a new folder the user picks, or alongside the file as `<name> (Patternhaus)/`. Choose one, document it, and use `Host::set_project_dir`.
5. **Corpus check (local only, never in CI):** `cargo run -p flp-probe` already exists. Add a local-only test or xtask (`cargo xtask flp-corpus <dir>`) that imports every `.flp` in the owner's folders, read-only:
   - `C:\Users\conno\Documents\Image-Line\FL Studio\Projects`
   - `C:\Users\conno\OneDrive\Documents\Image-Line\FL Studio\Projects`

   It reports the success rate, plus aggregate counts compared against `flp-probe stats`: notes, clips, channels. M2 acceptance criterion 3 needs at least 90% of notes and clips carried over correctly. **Never commit anything from the corpus.** Commit only aggregate numbers, in your report.
6. **Drag and drop:** dropping a `.flp` on the app imports it through `file.import_flp` (extend the M1/T9 drop table).

## Rules

- **Clean room** as above: the spec only.
- **No visible windows**, and never open the real audio device. Never launch FL Studio.
- **Additive only.** Log any contract addition in the AGENTS.md contract log.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge.

## Report back

Summarise:
- the features mapped (full, approximate or skipped);
- the SPEC-GAP list;
- the corpus results (success rate and aggregate diffs);
- the fuzz setup;
- test counts;
- the commit list.
