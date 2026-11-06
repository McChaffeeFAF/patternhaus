# Brief M3/T10: Controller lanes (CC and pitch bend in patterns)

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/cc-lanes` on branch `m3/t10-cc-lanes` (from `main`), with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\cc-lanes`. That is your own target dir; never use the shared `target/`. See AGENTS.md "Local layout".

**Why (owner, 2026-11-05):** an agent composing for a hosted orchestral library (Metropolis Ark through Kontakt) could not shape dynamics: Ark's long articulations take their dynamics from the mod wheel (CC1), and song data has no way to carry controller curves to an instrument. Today `EventKind::Cc` / `PitchBend` reach instruments only from live MIDI input. Every consumer already handles them: VST3 through `IMidiMapping`, CLAP as MIDI events, the Haus Sampler (modulation sources), the synth (pitch bend). What is missing is the song side.

**Goal:** patterns carry **controller lanes** per channel (like FL's piano-roll event editor), the engine plays them sample-accurately and identically live and offline, and agents can write them as easily as notes.

**Read first:**
- `AGENTS.md`: the contract log (all rows), the local layout and the rules;
- `docs/DESIGN.md` §2 (model), §4 (engine, real time) and §5 (the agent interface: diffs, co-editing);
- `crates/ph-core/src/model.rs` (`Pattern`, `Note`, patches/`ObjectPath`, `FORMAT_VERSION`) and `automation.rs` (`AutomationPoint`, `CurveKind` and its evaluation: reuse it, don't copy it);
- `crates/ph-engine`: how pattern notes become events (`player.rs`, `runtime.rs`, `plan.rs`), seeking, looping, and the offline oracle tests;
- `crates/ph-node/src/lib.rs`: `EventKind::Cc`, `PitchBend`, event ordering;
- the `note.*` and `automation.*` commands in `crates/ph-commands`, their behaviour/schema tests and the coverage guards;
- `crates/ph-control`: views (`get_state`), the musical diff, snapshot `piano_roll`, server instructions;
- `crates/ph-formats/src/midi.rs` (MIDI import/export).

## Scope
1. **Model (ph-core).**
   - A pattern holds, per channel, lanes keyed by a `Controller`: `Cc(n)` for n 0..=119, and `PitchBend`. (Channel pressure is optional: add it only if it costs nothing extra.)
   - A lane is points `{ tick (pattern-relative), value, curve, tension }`, with the same point/curve semantics as automation. Values are normalised: CC 0..=1, pitch bend -1..=1.
   - Serde-default and omitted when empty, so old projects load unchanged. Follow AGENTS.md on whether this needs a `FORMAT_VERSION` bump, and log the contract change either way.
   - Patches, undo and co-editing work like note edits: one undo step per command, three-way merge, `while_you_worked` reports.
   - Copying or duplicating a pattern, and the existing pattern/channel operations (delete channel, clone channel, split/merge if present), keep lanes consistent.
2. **Engine.**
   - While a pattern clip plays, each lane sends its value to the channel's instrument as `Cc` / `PitchBend` events.
   - Points are exact. Ramps between points are sent at a fixed control rate. Pick one, document it (for example every 64 frames, or about 1.3 ms), and skip a send when the 7-bit value (14-bit for pitch bend) has not changed.
   - **Chase:** starting playback, seeking, a loop wrap and a clip start all send each lane's current value first, so playing from the middle of a swell sounds right. At a clip end the last value stays; the next clip's chase sets its own.
   - Real time: no allocation (`assert_no_alloc`), with lane data precompiled in the plan like notes. **Live output stays bit-exact with the offline oracle.**
   - Clips that trim or offset a pattern, pattern loops and tempo changes behave like notes do.
3. **Commands** (with schema + behaviour tests and coverage):
   - `cc.set`: replace a lane's points in a tick range, or the whole lane;
   - `cc.add_points` / `cc.remove_points` / `cc.clear`;
   - `cc.list`: the lanes of a pattern/channel, with values;
   - `cc.shape`: a musical helper that writes a ramp/swell/hold over a range from a short spec (e.g. `{ "controller": 1, "from": 0.2, "to": 0.9, "start": .., "end": .., "curve": "smooth" }`, and a swell `0.3→0.9→0.4`). It returns the points it wrote.

   Use controller names in results (CC1 "Mod Wheel", CC11 "Expression", CC64 "Sustain", CC7 "Volume", "Pitch Bend") and accept them as input as well as numbers.
4. **Agent interface.**
   - The musical diff for lane edits, e.g. "CC1 Mod Wheel on Strings, bars 25-28: swell 0.3→0.9".
   - `get_state` views show lanes compactly per pattern/channel (ranges and shapes, not every point).
   - The `piano_roll` snapshot draws a lane strip under the notes when the channel has lanes, or when asked.
   - Server instructions get one short line: dynamics for sampled orchestral instruments usually live on CC1/CC11; write them with `cc.shape`; the catalogue guide's CC list says which controllers an instrument uses.
5. **MIDI files.** `file.import_midi` brings CC and pitch-bend data in as lanes, and `file.export_midi` writes lanes out, both round-trip tested.

**Out of scope (later track):** the piano-roll lane *editor* UI. The UI must still compile and must not lose lanes: e.g. the piano roll's copy/paste of notes leaves lanes alone, and pattern duplicate copies them. Automation clips targeting CCs are also out of scope.

## Tests
- Model: round-trip, an old project without lanes loads identically, patches, three-way merge.
- Engine:
  - exact frames of point events;
  - ramp density and dedupe;
  - chase on play, seek, loop and clip start;
  - trimmed clips and tempo changes;
  - `assert_no_alloc`;
  - live == offline (oracle) with lanes on a plugin channel (the fake/test plugins), a Haus Sampler channel (a CC1 → amplitude modulation, so the effect is audible and measurable) and a synth channel (pitch bend).
- **Acceptance (MCP only):** a scripted agent writes a sustained note with a `cc.shape` swell on CC1 driving a Haus Sampler instrument's amplitude. The render's level rises and falls with the swell (measure it), and live and offline renders are identical. Add it to the M3 acceptance CI step, or as its own step named "M3 controller lanes".

## Rules
- Never open visible windows or real devices.
- Use at most 4 compile jobs and `--test-threads 2`.
- **Checks:** fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- Add contract log rows in AGENTS.md, and parity rows ("piano roll event editor / controller lanes": FL vs Patternhaus).
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- the design: model shape, control rate, chase rules;
- the commands with their test names;
- the acceptance numbers;
- the contract changes;
- anything left out;
- the commit list.
