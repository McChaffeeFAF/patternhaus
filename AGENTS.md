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
5. Commands that must act on the `Session` itself (history, session-local view state such as
   solo, replacing the whole project) are registered the same way, with a params struct,
   scope and flags, but the session runs them: add the id to `SESSION_COMMANDS` in
   `session_cmd.rs` and a handler in `session_cmd::run`. They cannot be batched.
6. Tests: the schema test is automatic. Add at least one behaviour test that calls the
   command by its id, asserting exact results. `ph-control`'s `coverage.rs` fails CI for any
   served command without one. If it changes the sound, extend a golden project
   (`cargo xtask golden --update`, and say why in the PR). Add the command to
   `docs/parity/fl-studio.md` if it maps to an FL feature.

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
cargo deny check          # licence policy (deny.toml), advisories, bans
```

When dependencies change, also run `cargo xtask notices` (it regenerates
`THIRD_PARTY_NOTICES.md`; CI checks it is current). The engine's lock-free code has Miri
(`cargo +nightly miri test -p ph-engine --lib rt_miri`) and loom
(`RUSTFLAGS="--cfg loom" cargo test -p ph-engine --lib --release loom_models`) checks. Run them
when you touch `rt.rs`, `sched.rs`, `pool.rs` or `audition.rs`.

CI runs the same commands on Windows, Linux and macOS. Expectations:
- Unit tests in every crate.
- Property tests for anything history- or merge-related.
- Golden renders for anything audible (`tests/projects/`, `cargo xtask golden`).
- Fuzzing for every importer.
- A DSP change must not move a golden render unless the change is intended. If it is,
  regenerate the render and explain why in the PR.

## Factory assets

Every file under `assets/` needs an entry in `assets/PROVENANCE.toml`, with `path`,
`license`, and either `generator` (tool and parameters) or `source` (URL). Only
self-generated or CC0 content is allowed, with one standing exception: the UI fonts in
`assets/fonts` (Barlow, Barlow Condensed, Share Tech Mono: SIL OFL 1.1, unmodified, with their
licence texts, listed in `THIRD_PARTY_NOTICES.md`). Their entries live in `FONTS` in
`tools/soundgen/src/lib.rs`, which writes the table. `cargo xtask check-provenance` enforces this.
Drum samples come from `tools/soundgen` (`cargo run -p soundgen -- render` regenerates them and
`PROVENANCE.toml`; `-- check` fails if `assets/` differs). Synth presets are hand-authored JSON.

## Contract log

| Date | Contract | Change |
|---|---|---|
| 2026-10-10 | all | T0 initial versions |
| 2026-10-10 | `Session` (T1) | Added, no break: `UndoMode`; `undo_txn_with`, `undo_with`, `redo_with`, `undo_author`, `reset`, `solo()`; `undo_txn` keeps its signature but is now the field-level three-way revert (conflicts name fields and authors). `Session::batch` routes session-handled commands (`history.*`, `channel.solo`, `mixer.solo`, `file.open`) to the session. Added `SessionHandle` (co-editing queue) and the `ph-formats` crate. |
| 2026-10-10 | ph-node `EventKind::NoteOn.fine_pitch` | Unit documented as **cents**, matching `ph_core::Note::fine_pitch`; Sampler and Synth convert (found in T2 review) |
| 2026-10-10 | `ph-engine` API (T3) | Implemented; added, no break: public modules `assets` (`FileResolver`: `factory:<path>` → `<assets>/samples/<path>.wav`, project-relative, allowed roots, cache) and `tempo` (`TempoMap`). Semantics fixed: `meters()` peak is "since last read"; `EngineConfig::exclusive` needs the `asio` build (cpal has no WASAPI exclusive mode) and is otherwise a device error; the simulated device caps overload shedding at level 1 (meters) so it stays bit-identical to offline. Added `ph_formats::encode` (WAV/FLAC export). |
| 2026-10-10 | `ph-commands` `TransportCmd::SetMode` | Now carries `pattern: Option<PatternId>`, resolved by `transport.set_mode` from the project (param `pattern`, default first pattern), so hosts never act on a stale snapshot. |
| 2026-10-10 | `ph-control` (orchestrator) | New contract for phase D: `Studio` (SessionHandle + Engine + snapshot bridge + `ui_requests`), `StudioConfig`, `AudioMode { Device, Silent }` (Silent = simulated device pumped in real time), `EngineHost` (Host over the engine, offline render + encode for export, DESIGN §5 path confinement). T4 and T5 build on it. |
| 2026-10-10 | `ph-engine` API (T4) | Added, no break: `EngineStats.warnings: Vec<String>` and `ph_engine::project_warnings(&Project, &dyn AssetResolver)`. An unloadable sample or unsupported instrument/effect still compiles to silence/pass-through, but `Engine::stats()` now says so (agents see it in `get_state`, `render` and `analyze` results). Anything building `EngineStats` by struct literal needs the new field (nothing in the workspace did). |
| 2026-10-10 | `ph-commands` `mixer.set_sidechain` (T4) | Behaviour, no signature change: also sets every built-in compressor's `sidechain` param on the target insert to External (1) when a source is given, and back to Internal (0) when cleared, so the route and the param change as one undoable edit. |
| 2026-10-10 | `ph-control` (T4) | Added, no break to `Studio` / `StudioConfig` / `AudioMode` / `EngineHost`: `Studio::handle() -> StudioHandle` (cloneable, `Send + Sync`: session, engine, project snapshot, config, registry, event hub); `Studio::run` now goes through it, so a successful `file.save`/`file.save_as` also writes `history/log.jsonl`; `Studio::start` registers `query.node_params` in the registry (`Registry::register`); `Studio` gained private fields. New public API: `Agent` (the fixed MCP toolset, shared by MCP, JSON-RPC and the CLI), `PatternhausServer` (rmcp), `serve_local(&Studio)` / `serve_local_with` (named pipe or Unix socket speaking MCP and JSON-RPC), `serve_stdio`, `proxy_stdio`, `Grants` + `GrantPrompt` (scope grants per client name, hook for the app's prompt), `EventHub`, `start_headless`. T5 only needs `serve_local(&studio)`, and optionally `serve_local_with(.., Grants::per_client(file, Some(prompt)))`. |
| 2026-10-10 | new crates (T4) | `ph-analysis` (perception: LUFS, spectrum, mix health, onsets, key/chords, masking, PNG snapshots) and `ph-cli` (`patternhaus-cli` binary). New third-party dependencies: `rmcp` 3.5 (Apache-2.0), `tokio` (MIT), `tiny-skia` (BSD-3-Clause), `png` and `base64` (MIT/Apache-2.0), `clap` (MIT/Apache-2.0). |
| 2026-10-10 | `ph-control` (T8) | Added, no break: `UiHint { name, value, gestures }`, `Studio::set_ui_hint` / `ui_hint` and `StudioHandle::ui_hint`, and the `ui` view of `get_state` (`{ hint: UiHint or null }`). The UI publishes what its hint bar shows; headless runs have no hint (`null`). |
| 2026-10-10 | `ph-control` `Grants` | Added, no break: `Grants::clients()` (remembered grants by client), used by ph-app's Agents panel adapter (`ph-app/src/agents.rs`) that wires `serve_local_with` + `GrantPrompt` to `ph_ui::AgentDirectory`. |
| 2026-10-10 | `ph-control` connection registry (T5c) | Added, no break: `LocalServer::connections() -> Vec<ConnectionInfo>` (client name, scopes, protocol, `connected_at`, `last_activity`, `commands`), `Agent::connections()`, and the types `Connections`, `ConnGuard`, `ConnectionInfo`, `Protocol`. Every MCP and JSON-RPC connection registers itself and leaves the list when it ends. `Grants::peek` (the grant without prompting); `Grants::grant_for` now asks at most one prompt per client at a time. `Agent::new`, `serve_local_with` and `serve_jsonrpc` keep their signatures. |
| 2026-10-10 | `ph-core` `Note.muted` (T5c) | Added, no break: `Note.muted: bool` (`#[serde(default, skip_serializing_if = "Not::not")]`: older projects load unmuted and unmuted notes serialise exactly as before, so `format_version` stays 1), `NoteSpec.muted` (default false) and `note.update { muted }`. A muted note stays in the pattern and in undo but is not played by the engine plan, not analysed by `ph-analysis`, and not exported to MIDI. `note.copy`, channel and pattern clones keep it. Code that builds `ph_core::Note` by struct literal needs the new field (only `Edit::add_note` did). Solo needed no contract change (the UI polls `Session::solo()`). |
| 2026-10-10 | `ph-engine` API (T7) | Added, no break: `Engine::audition(&self, sample: &str, velocity: f32) -> EngineResult<()>`, `Engine::audition_instrument(&self, &InstrumentSpec, key, velocity, seconds) -> EngineResult<()>` (renders a preset note offline on the calling thread) and `Engine::stop_audition()`. One preview voice outside the project graph, mixed into the device output after the master insert at a fixed level (0.5 × velocity); it never reaches meters, `render_offline` or exports. Decoding and resampling happen on the calling thread; voices travel to and from the audio thread through two single-slot mailboxes, so nothing is allocated or freed there. `ph-ui`: the browser preview uses it, and the temporary "audition channel" project swap is gone (`EngineLink::spawn` lost its unused `project` argument, `EngineLink::set_latest` and `live::audition_project` were removed; `EngineLink::audition` now takes a `live::Audition`). |
| 2026-10-10 | `ph-control` (T7) | Added, no break: `ph_control::studio_registry() -> Registry` (the built-ins plus `query.node_params`, exactly what `Studio::start` serves). The criterion 8 guard (`coverage.rs`) checks every command in it for a schema test and a behaviour test. |
| 2026-10-10 | `ph-engine` API (T7) | Added, no break: `pub const ASIO_BUILD: bool` (true when built with the `asio` feature; the About box shows the GPLv3 notice). `ph-ui` and `ph-app` gained an `asio` feature that forwards to `ph-engine/asio`. |
| 2026-10-10 | `ph-commands` `Host` / `sample.import` (M1/T9) | Added, no break: `Host::import_file(&mut self, source, ImportKind, &Author) -> CmdResult<String>` with a default that refuses (so `NullHost` and every existing impl compile unchanged), and `ImportKind::Sample`. `EngineHost` validates by decoding, then copies into `<project>/samples/` (name kept, ` (2)`, ` (3)` on a clash, identical bytes reuse the existing file) and returns `samples/<name>`. New command `sample.import { paths, channel? }` (Compose, mutating): creates Sampler channels or replaces one channel's sample, returns `{ channels, references, skipped }`. Behaviour change: for `Author::Human`, `file.import_midi`, `file.open` and `Host::import_file` accept any absolute path (dropping a file is the intent); agents and relative paths keep `resolve_path` confinement (DESIGN §5). `builtin::open_project` gained an `&Author` argument (crate-internal). |
| 2026-10-11 | `Host::set_project_dir`, `Engine::replace_project`, `FileResolver` (orchestrator) | Added, no break. `file.open` calls `host.set_project_dir(dir)` before switching projects (default no-op), and `EngineHost` moves its project folder and the engine's `FileResolver` (now runtime-updatable: `set_project_dir`, `add_root`). The bridge applies whole-project replacements (`Change.txn == 0`) with `Engine::replace_project`, which rebuilds every node. **Fixes data loss:** a save after opening another folder overwrote the project the app started with. |
