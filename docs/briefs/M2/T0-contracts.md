# Brief M2/T0: Contracts for M2

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m2/t0-contracts`.
**Read first:**
- `AGENTS.md`, the whole contract log;
- `docs/DESIGN.md` §3, §4, §6 and §10;
- `docs/milestones/M2.md`, especially "Contracts fixed in T0 (draft)" and the Scope sections;
- `crates/ph-core` (model, patch, path, `Edit`);
- `crates/ph-node`;
- `crates/ph-engine` (`lib.rs`, `plan.rs`, `control.rs`, `assets.rs`);
- `crates/ph-commands` (`host.rs`, `registry.rs`, `builtin/`);
- `crates/ph-formats` (folder migrations).

**Goal:** fix the shapes every other M2 track builds on, so phases B and C can run in parallel without colliding. Behaviour that needs real DSP or UI belongs to later tracks. T0 defines the types, model invariants, patches, migrations, interfaces and stubs, and tests all of them.

## Contracts to define

### 1. `ph-core` model, all additive, with `format_version` going from 1 to 2

**Clips gain a kind.**
- **Pattern clips** keep their current fields and are the default, so v1 files load unchanged.
- **Audio clips** carry: a `sample` reference (into the sample registry); `offset` into the source, in samples or ticks (choose one and document why); `length`; `gain_db`; `fade_in`/`fade_out` (length plus curve: linear, exp, log, S); `pitch_semitones`; `fine_cents`; `reverse`; `stretch: Resample | Stretch | Slice`; and `tempo_sync: bool`.
- `ObjectPath::Clip` covers every kind.

**Automation clips.** A playlist clip kind whose `target` is a `ParamPath`.
- `ParamPath` identifies a channel instrument param, an insert effect param (by insert, slot and param id), a mixer insert volume/pan/mute, a channel volume/pan, or the project tempo.
- It has a string form, like `ObjectPath`. Agents use it.
- **Points:** `[{ tick, value, curve }]`, with curve types `Linear | Hold | Smooth | SCurve` and tension where it applies. Keep the points sorted, with ids or stable indices so patches stay small.
- **Edit helpers:** add, move or remove a point, and set its curve.

**Sample registry.** A project-level map from a sample reference to metadata:
- original file name;
- frames, sample rate, channels, length;
- `bpm: Option<f64>` (detected or set) with its source (`Detected | User`);
- `root_key`;
- analysis cache keys (onsets, key);
- provenance (imported from, at).

It gets its own `ObjectPath` variant. `sample.import` (from M1/T9) should register here; adapt it.

**Recording takes.** A take is a sample registry entry plus the time range it was recorded over and the input it came from. Keep it minimal.

**Migration.** `format_version` 1 to 2 goes in `ph-formats`, with a test that loads the M1 golden projects and an M1 project folder unchanged.

**Requirements carried over from M1:**
- `Edit` helpers for everything above, with patch inversion and conflict paths that stay correct;
- the T1 property tests extended to the new objects (random interleaved edits, per-author undo).

### 2. `ph-node`

**Events.** `EventKind::Cc { controller, value }`, `PitchBend { value }`, `NoteExpression { note_id, kind, value }` (CLAP-style kinds: volume, pan, tuning, vibrato, brightness, pressure), plus `Param` ramps if needed for sample-accurate automation.
- Document real-time rules and units.
- Make the Haus Synth's `mod_wheel` and `pitch_bend` respond to `Cc` and `PitchBend` events (keeping the params for back-compat).

**Load state.** `Processor::load_state` receives `&dyn AssetResolver`. Update every implementor.

**Plugin-node API, as a trait or struct in `ph-node` or a new `ph-plugin-host` crate** (follow the LAYERS table in `xtask`):
- scan results `PluginInfo { format: Clap | Vst3, id, name, vendor, version, kind, path }`;
- an instance wrapper that implements `Processor`;
- a state blob;
- an editor-window handle (platform raw window handle, open/close/resize);
- the crash-catch boundary contract (what the host does when a plugin call panics or faults).

Stub it: no CLAP or VST3 code yet (that's T3), but compile the trait plus a fake in-repo test plugin that implements it.

### 3. `ph-engine` interfaces (stubs with `todo!` or minimal behaviour, documented)

- An audio-clip node that streams from the sample registry: a prefetch thread off the audio thread, and an interface for a stretch engine (`trait Stretcher`). Implementations come in T1.
- An automation evaluator: sample-accurate values for a `ParamPath` at a given block, feeding `Param` events, plus tempo automation through `TempoMap`.
- An input/recording API: `Engine::arm(input, target)`, `record_start/stop`, monitor. The real device input is T8; define the types and a simulated input for tests.

### 4. `ph-commands` families, as registered names with param schemas

Register `audio.*`, `automation.*`, `plugin.*`, `record.*`, `midi.*`, `slicer.*`, `editor.*` and `file.import_flp`.
- **Implement now:** the model-only commands (e.g. `audio.add_clip`, `audio.set_fade`, `automation.add_clip`, `automation.add_point`, `sample.set_bpm`), each with behaviour tests.
- **Stub the rest:** commands that need DSP or devices return a clear "not available until T<n>" error, so schemas and the coverage guard are in place.
- Use the existing scope rules.

### 5. Licence checks for the M2 dependencies

Confirm the licences of:
- the Rust Signalsmith Stretch binding(s), `signalsmith-stretch` or `ssstretch`, upstream MIT;
- `clack` (CLAP host);
- `vst3` (MIT/Apache).

If a binding is unsuitable, record that T1 will write a thin binding of its own. Put the findings in `docs/formats/m2-dependencies.md`. Add **no** heavy dependencies in T0.

## Rules

- **Additive, migration-safe, documented.** Every contract change gets a row in the AGENTS.md contract log. M1 behaviour and golden renders must not change.
- **No visible windows, and never open the real audio device.**
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge.

## Report back

Summarise:
- each contract, with file paths;
- design choices that later tracks must know (units, invariants);
- the migration result;
- the licence findings;
- test counts;
- the commit list.
