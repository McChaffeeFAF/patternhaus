# AGENTS.md

Start here, whether you are a human contributor or an AI agent. Read
[docs/DESIGN.md](docs/DESIGN.md) for *why*, and the current milestone plan
([docs/milestones/M1.md](docs/milestones/M1.md)) for *what's next*. If you were given a
brief, it lives in `docs/briefs/`.

## Ground rules

1. **Decided design is binding.** Sections of DESIGN.md marked *Decided* change only with
   the owner's sign-off. If your task seems to need a change, stop and say so.
2. **Clean room.**
   - Implement from public specs, documentation and observed behaviour only.
   - Never decompile or disassemble FL Studio, Kontakt or any other proprietary software.
   - Never copy their code, samples, presets, skins or icons.
   - Format notes in `docs/formats/` cite their public sources.
3. **Contracts are owned by the orchestrator.** These T0 interfaces change only through a
   reviewed PR that updates every user and records the change in the contract log below:
   - `ph-core` model, `ObjectPath`, `Patch`, `Edit`;
   - `ph-commands` `Command`, `Registry`, `Host`, `Session`;
   - the `ph-node` `Processor` trait;
   - the `ph-engine` public API.
4. **Commit sign-off.** Every commit carries `Signed-off-by:` (DCO); use `git commit -s`.
   AI-assisted commits also carry a `Co-Authored-By:` line naming the model.

## Layering

Each crate may depend only on the `ph-*` crates its layer allows (DESIGN §2). The table
lives in `xtask/src/main.rs` and `cargo xtask check-layers` enforces it in CI. A new crate
must be added to that table and to the DESIGN §2 diagram in the same change.

```
pure data / interfaces   ph-core  ph-node  ph-dsp  ph-script  ph-audio-io
building blocks          ph-instruments  ph-effects  ph-sampler  ph-formats  ph-plugin-host
engine                   ph-engine
commands / perception    ph-commands  ph-analysis
front ends               ph-control  ph-ui  ph-cli  ph-app
```

## The two laws

### 1. Everything is a command, every change is a patch

- **Commands only.** Every user-visible action is a command in the `ph-commands` registry.
  That covers menus, shortcuts, tools, the CLI, JSON-RPC and MCP. There are no UI-only
  code paths.
- **Commands never mutate a `Project` directly.** They use `ph_core::Edit`, which records
  `Patch`es, and return `CommandOutput::from_edit`. The `Session` applies the patches,
  stamps them with an author, and keeps them for per-author undo.
- **Ids are never reused.** Undo does not free ids.

### 2. The audio thread is sacred

Inside `Processor::process`, `set_param` and `reset`, and anywhere else on the audio
thread:
- no allocation or freeing (no `Vec::push` past capacity, `Box::new`, `String`, `format!`, `clone` of heap data);
- no locks, channels that block, I/O, syscalls, logging or panics.

Allocate in `activate`. Communicate with lock-free queues. Old render plans are freed off
the audio thread. Tests run the engine under `assert_no_alloc`.

## How to add a command

1. Pick the area module in `crates/ph-commands/src/builtin/` and an id `<area>.<verb>`.
2. Define a params struct with `#[derive(Deserialize, JsonSchema)]` and
   `#[serde(deny_unknown_fields)]`.
   - Put a doc comment on every field: it becomes the schema text agents read.
   - Times are ticks: 960 per quarter note, 240 per step.
   - Use `ph_core` id types.
3. Write `fn(&mut CommandCtx, Params) -> CmdResult<CommandOutput>`. Use `Edit` for changes,
   and return created ids in `result`. Write a one-line summary for a musician, e.g.
   "added 4 notes to Kick", not "inserted 4 Note objects".
4. Register it with `typed(id, summary, scope, flags, f)`:
   - **Scope:** `Read` for queries; `Compose` for notes, patterns, channels, playlist and
     transport; `Mix` for the mixer and effects; `Full` for files, plugins and project-level
     operations.
   - **Flags:** `READ`, `MUTATES` or `DESTRUCTIVE`. *Destructive* means irreversible: it
     overwrites or deletes a file, or discards unsaved work. Undoable deletions are not
     destructive.
5. Tests: the schema test is automatic. Add at least one behaviour test (golden test once
   T7 lands), and add the command to `docs/parity/fl-studio.md` if it maps to an FL feature.

## How to add a processor (instrument or effect)

Implement `ph_node::Processor`:
- **Stable ids.** Give it a stable descriptor `id` (`ph.synth`, `ph.fx.reverb`) and stable
  parameter ids. Parameter ids are never renamed; deprecate them and add new ones instead.
- **Allocate in `activate`**, which may be called again whenever sample rate, block size
  or `Quality` change.
- **Clamp parameters** in `set_param`. Smooth audible parameter changes yourself.
- **Return `Sleep`** when silent with no tail, so the engine can skip the processor.
- **Deterministic.** Use the seed from `ActivateInfo` for all randomness, so output is
  identical for identical input.

## Tests and checks

Run everything before asking for review:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask check
```

CI runs the same commands on Windows, Linux and macOS. Expectations:
- Unit tests in every crate.
- Property tests for anything history- or merge-related.
- Golden renders for anything audible (T7 harness).
- Fuzzing for every importer.
- A DSP change must not move a golden render unless the change is intended. If it is,
  regenerate the render and explain why in the PR.

## Factory assets

Every file under `assets/` needs an entry in `assets/PROVENANCE.toml`, with `path`,
`license`, and either `generator` (tool and parameters) or `source` (URL). Only
self-generated or CC0 content is allowed. `cargo xtask check-provenance` enforces this.

## Contract log

| Date | Contract | Change |
|---|---|---|
| 2026-10-10 | all | T0 initial versions |
