# Brief M3/T0: Contracts for the Haus Sampler

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t0` on branch `m3/t0-contracts`, with `CARGO_TARGET_DIR` set to `E:\patternhaus\target`, the shared build folder. See AGENTS.md "Local layout".

**Read first:**
- `AGENTS.md`: the whole contract log and the local layout;
- `docs/DESIGN.md` §2 (layers), §3 (model, commands, undo), §4 (engine) and §8 (Haus Sampler);
- `docs/milestones/M3.md`: all of it;
- `crates/ph-core` (model, patch, path, `Edit`, audio.rs, automation.rs);
- `crates/ph-node`, especially `SampleStream`, `AssetResolver` and events;
- `crates/ph-instruments/src/sampler.rs`, the M1 `ph.sampler`, which stays as it is;
- `crates/ph-engine`: `stream.rs`, `assets.rs`, `catalog.rs`, `plan.rs`;
- `crates/ph-commands` (registry, `builtin/`) and `crates/ph-formats` (folder migrations);
- the `LAYERS` table in `xtask/src/main.rs`. It already reserves `ph-sampler`, `ph-sampler-formats` and `ph-script`.

**Goal:** fix the shapes every later M3 track builds on, so phases B–D can run in parallel without colliding. T0 defines types, invariants, patches, migrations, interfaces and stubs, and tests them. The real DSP, importers and UI belong to later tracks.

## Contracts to define

### 1. The instrument model
The zone map must be undoable, patchable and editable by agents through `ph-commands`. `ph-commands` may only depend on `ph-core` and `ph-formats`. So **the instrument data model lives in `ph-core`** (a new `sampler` module), and `ph-sampler` (the engine) consumes it. If you find a better split that keeps undo and the layers intact, propose it in your report and log it in the contract log.

It covers:
- `HausInstrument`: name, groups, zones, articulations, a modulation matrix, global settings, `source` provenance (format, file, importer version), and an `import_notes` list of things approximated or skipped.
- **Zones:**
  - sample reference: into the project's sample registry, or a path relative to the instrument folder;
  - key range, velocity range, root key, tune and fine;
  - volume, pan, start/end, loop (mode, start, end, crossfade), reverse;
  - trigger (attack or release), round-robin position;
  - CC conditions;
  - group id.
- **Groups:**
  - name, colour;
  - amp and filter envelopes, filter, LFOs;
  - voice limit and stealing mode, polyphony mode (poly, mono, legato), portamento;
  - round-robin mode (sequential or random), output bus, effect chain (built-in effect ids and params);
  - keyswitch membership;
  - opaque `extra` data that importers keep for M3b: script text and UI controls.
- **Articulations and keyswitches:** key or CC triggers, a default.
- **Modulation matrix:** source, target, depth, curve, and an optional scope (group).
- Ids that are stable across edits, `ObjectPath` variants (`…/zone/<id>`, `…/group/<id>`), and `Edit` helpers for add, move, remove and set on zones and groups.
- Patch inversion and conflict paths stay correct. Extend the property tests (random interleaved edits, per-author undo) to these objects.

How a channel refers to an instrument: by path (external instrument folder) or embedded in the project. Document the choice and its effect on save, reopen and "collect into project". Then bump `format_version` and add a migration in `ph-formats`, with a test that loads the M1 and M2 golden projects unchanged.

### 2. The native instrument format
- Define the on-disk form: a folder with `instrument.toml`, `samples/`, and reserved `ui/` and `scripts/`.
- Write `docs/formats/haus-instrument.md`: every key, its type, units, default and version.
- Provide serde types and a versioned load/save with migrations. Put them where the layering allows; `ph-formats` is the natural home.
- Round-trip tests.

### 3. `ph-sampler` crate (engine), as a stub
- Create the crate, with its `LAYERS` entry already present, and add it to DESIGN §2 if anything changes.
- It defines `ph.haus_sampler`: a `Processor` that compiles a `HausInstrument` into an immutable real-time plan (zone lookup tables by key and velocity, group data). It also defines how the engine hands it sample streams and preloads, reusing the M2 `SampleStream` and prefetch design in `ph-engine/src/stream.rs`. Decide and document whether instruments get their own preload type or the existing streamer gets a preload API.
- The voice model interface: trigger, release and steal, the round-robin state and the per-render seeded RNG (for bit-exact live = offline). Leave `todo!` where real DSP goes, which is T2's job. A minimal working path (one zone, one shot, no envelope) is enough to wire up the engine and an oracle test.
- Multi-out: say how extra output buses appear on the node, and stub them.

### 4. `ph-sampler-formats` crate, as a stub
- An importer trait: bytes or path in; a `HausInstrument`, its sample list and an import report out; plus an `Encrypted` error. Never panic.
- A registry by extension: `.sfz`, `.sf2`/`.sf3`, `.dspreset`, `.exs`, `.nki`/`.nkm`/`.nkx`/`.nkc`/`.ncw`.
- Every importer stubbed to "not available until T<n>".
- One cargo-fuzz target skeleton per importer, following how the FLP fuzzers are set up.

### 5. `ph-commands`: the `sampler.*` family, registered with param schemas
- Load or import a file into a channel; zone add, move, set, remove; group set; keyswitch set; modulation set; save as native; report.
- **Implement now** the commands that only touch the model, each with behaviour tests.
- **Stub** the ones that need importers or DSP, so the schemas and the coverage guard are in place.
- Add `get_state { view: zones }` (the shape only; T7 completes it).

### 6. Dependency check
List the crates M3 will need (SF2 parsing, if any; zlib or other decompression for NKI; TOML) with their licences, in `docs/formats/m3-dependencies.md`. Check them against `deny.toml`. Prefer writing small parsers ourselves. Add **no** heavy dependencies in T0.

## Rules
- **Additive, migration-safe and documented.** Every contract change gets a row in the AGENTS.md contract log. M1 and M2 behaviour and golden renders must not change.
- **Clean room:** don't read any third-party sampler-format code. T0 doesn't need it.
- **No visible windows, and never open the real audio or MIDI device.**
- **Build load:** at most 4 compile jobs (`-j 4`), one build at a time.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass.
- **Commits:** commit increments as you go, with `git commit -s` and a `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` line. Don't merge or push.

## Report back
- each contract, with file paths;
- the design choices later tracks must know (ids, units, the seeding rule, the instrument reference rule);
- the migration result and test counts;
- the dependency findings;
- the commit list.
