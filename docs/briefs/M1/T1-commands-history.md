# Brief M1/T1: Commands and history

**Repo:** `E:\patternhaus`. Work on branch `m1/t1-commands`.
**Read first:** `AGENTS.md`; DESIGN §3 and §5; `docs/milestones/M1.md`.
**Depends on:** T0 contracts, which are on `main`. Don't change them. If you believe a contract is wrong, stop and report rather than editing it.

## Goal

1. Every M1 command exists in the `ph-commands` registry.
2. `Session` has the per-author undo that DESIGN §3 decided:
   - field-level, three-way revert;
   - conflict stops, with skip / force / cancel;
   - order-list merging.
3. The co-editing queue exists.
4. Projects save and load as a folder.
5. MIDI files can be imported and exported.

## Scope

### 1. Restructure `builtin.rs` into `src/builtin/<area>.rs` modules

Areas: `project`, `channel`, `pattern`, `note`, `step`, `playlist`, `mixer`, `transport`, `file`, `ui`, `history`, `query`. Keep the existing reference commands, moved into these modules.

### 2. The M1 command set (~90 commands)

Cover every row of `docs/parity/fl-studio.md` § M1 that isn't audio DSP, and fill in its *Wired* column. At least:

- **project:** `new` (an empty project plus one pattern, recorded as an edit), `get_settings`, `set_tempo`, `set_time_signature`, `set_swing`, `rename`.
- **channel:** `add`, `remove`, `rename`, `set_color`, `set_volume`, `set_pan`, `set_mute`, `solo` (session-local view state: solo is *not* a model field, so keep it in the session), `set_insert`, `move`, `set_instrument`, `set_param` (instrument params map), `clone`.
- **pattern:** `add`, `remove`, `rename`, `set_color`, `set_length`, `clone`, `move`, `clear_channel`.
- **note:** `add` (batch), `remove` (by ids), `update` (batch partial fields), `move` (time/key delta), `resize`, `set_velocity`, `transpose`, `quantize` (grid and strength), `copy` (to another time, pattern or channel), `select_range` (a query returning ids in a time/key box).
- **step** (step-sequencer view over notes; a step is a `STEP`-long note on the grid):
  - `toggle`, `set` (a whole row from a bool array), `set_velocity`, `set_pitch`;
  - `get_row`, a query that returns the grid as booleans plus velocities.
- **playlist:** `add_clip`, `remove_clip`, `move_clip`, `resize_clip`, `set_track_mute`, `rename_track`, `fill` (repeat a pattern N times).
- **mixer:** `set_volume`, `set_pan`, `set_mute`, `rename`, `add_effect`, `remove_effect`, `move_effect`, `set_effect_param`, `set_effect_bypass`, `set_effect_mix`, `add_send`, `remove_send`, `set_send_level`, `set_sidechain`, `set_route_to_master`.
- **transport** (through `Host`): `play`, `stop`, `toggle`, `seek`, `set_mode`, `set_loop`, `set_metronome`, `get_state`.
- **file** (through `Host::resolve_path`): `save`, `save_as`, `open` (destructive when there are unsaved changes), `export_audio` (calls `Host::render`), `import_midi`, `export_midi`.
- **ui** (through `Host::ui`): `open_panel`, `close_panel`, `select`, `show_piano_roll`, `notify`.
- **history:** `undo`, `redo`, `undo_author` (reverts all of an author's live transactions, newest first), `undo_txn`, `list`. These need the `Session`; design a clean way for history commands to reach it. One option is a `SessionCommand` path the session handles itself. Every action must still be a registry command with a schema.
- **query** (read-only, `Scope::Read`): `project` (the full model as JSON), `channels`, `patterns`, `pattern_notes`, `mixer`, `playlist`, `object` (any `ObjectPath`).

### 3. Per-author undo (DESIGN §3), built test-first

- Replace the strict body of `Session::undo_txn` with the field-level three-way revert:
  - For each patch, compare its `after` with the object's current value field by field, recursing into JSON objects. Arrays count as one value.
  - Revert the fields this transaction changed and nobody changed since.
  - A field changed since by someone else is a conflict.
- Order lists (`is_order_list`) merge by membership:
  - undoing an insertion removes the id if it's present;
  - undoing a removal re-inserts the id at its old index, clamped.
- Deleted objects: if the object was later modified or deleted by someone else, that's a conflict.
- Conflict resolution: `UndoMode { Stop, Skip, Force }`. `Stop` returns `CmdError::UndoConflict` listing each path, its fields, and who changed it. `Skip` reverts everything except the conflicts. `Force` reverts everything.
- **Property test** (proptest): two authors make random interleaved edits; undo all of one author's work with `Skip`; then the other author's changes must be untouched. Also: undo then redo returns the exact prior state when nobody else intervened.
- Keep `undo` / `redo` / `last_undoable` / `last_redoable` semantics. Any change goes in `AGENTS.md` → Contract log.

### 4. Co-editing queue

`SessionHandle`, a cloneable `Send` handle, owns a `Session` on a dedicated control thread:
- **Requests** (`execute`, `batch`, `undo` and the rest) arrive from many threads, are processed in arrival order, and get replies on per-request oneshot channels.
- **Snapshots:** `subscribe()` returns a receiver of `Change { txn, author, paths, summary, project: Arc<Project> }`. Subscribers are the engine, the UI and agents (DESIGN §5 live events).
- **Not real-time:** nothing in the queue runs on an audio thread. Use `std::sync::mpsc` or crossbeam; no async runtime in `ph-commands`.

### 5. Project folder I/O

A new crate, `ph-formats` (layer: `ph-core`, `ph-node`):
- **Save/load** the project folder from DESIGN §10: `project.json` pretty-printed with stable key order, plus `samples/`, `renders/`, and `history/` (optional patch log). Saving twice must be byte-identical.
- **Migrations:** a scaffold keyed by `format_version`.
- **MIDI:** import and export with `midly`. Import makes one channel per track or MIDI channel, as Synth placeholders, plus a pattern and its notes. Export writes the patterns or the song.
- **No audio decoding:** T2 owns that.

## Done when

- Every listed command is registered, schema-tested, and has at least one behaviour test.
- The property tests pass.
- `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass.
- The parity tracker's *Wired* column is filled for your rows.
- `AGENTS.md` is updated if any convention changed.

## Out of scope

- The engine, DSP, UI, MCP/JSON-RPC transport (T4) and analysis.
- Real file rendering (`export_audio` only calls `Host::render`; tests use a fake host).

## Report back

Summarise:
- the commands added, grouped by area;
- how history commands reach the session, and why;
- any contract friction you hit;
- test counts.

Commit on your branch with sign-off (`git commit -s`) plus a `Co-Authored-By` line. Don't merge to `main`; the orchestrator reviews and merges.
