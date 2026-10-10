# Brief M1/T6: Factory content

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t6-content`.
**Read first:** `AGENTS.md` ("Factory assets"); DESIGN §7 ("Factory content"); `crates/ph-instruments/src/synth.rs` (the Haus Synth parameter ids).
**Depends on:** T2, which is on `main`. T3 (engine) is running in parallel. Don't touch `ph-engine` or `ph-formats`.

## Goal

The sounds M1 needs to make a beat out of the box: a generated drum kit and a set of Haus Synth presets. Every file must be one the project owns outright, with provenance recorded.

## Conventions shared with T3

- **Sample references:** `factory:<path>` resolves to `assets/samples/<path>.wav`, e.g. `factory:drums/haus808/kick` → `assets/samples/drums/haus808/kick.wav`. T3's `FileResolver` implements this mapping.
- **Synth presets:** `assets/presets/synth/<category>/<name>.json`. `InstrumentSpec::Synth { preset: Some("bass/sub") }` names the file relative to that folder.

## Scope

### 1. `tools/soundgen`: the drum and sound generator

A workspace member under `tools/soundgen`; add `"tools/*"` to the workspace members. It's not shipped and not a `ph-*` crate.
- **Synthesis:** drum synthesis and simple physical models, written in plain Rust. It may use `ph-dsp` primitives.
- **Recipes:** each sound is a recipe with explicit parameters. `cargo run -p soundgen -- render` writes every WAV (48 kHz, 24-bit, via `hound`) and every PROVENANCE entry.
- **Deterministic:** the same recipe always gives byte-identical output. `cargo run -p soundgen -- check` re-renders into a temporary folder and fails if anything in `assets/` differs. Add that command to CI.

### 2. Drum kits

Three kits of about 12 sounds each:
- **`haus808`:** a classic analog drum machine character.
- **`haus909`:** punchier, with more noise.
- **`acoustic-ish`:** physical-model toms, snare and hats.

Each kit has:
- kick;
- snare, rim and clap;
- closed hat, open hat and pedal hat;
- low, mid and high toms;
- crash or ride;
- one percussion sound (cowbell, shaker or clave).

Add a few long 808-style sub-bass hits, tuned to C, to `drums/haus808/`.

**Quality bar.** The agent can't listen, so verify by measurement and write tests in `tools/soundgen`:
- peaks normalised to -1 dBFS;
- no DC offset (below -60 dB);
- every file starts at a zero crossing or with a 0.5 ms fade-in, and ends with a fade to true silence (no click);
- kicks have a measured fundamental between 40 and 70 Hz;
- the closed hat is shorter than the open hat;
- the tuned 808s measure within 5 cents of C.

The owner listens to them at checkpoint 1.

### 3. Haus Synth presets

About 30 presets as JSON maps of parameter id to value, in the categories `bass/`, `lead/`, `pad/`, `pluck/`, `keys/` and `fx/`. Examples: `bass/sub`, `bass/reese`, `bass/acid`, `bass/pluck`, `lead/supersaw`, `lead/square`, `pad/warm`, `pad/glass`, `pluck/bell`, `keys/epiano-ish`, `fx/riser`.
- **Loading:** update `ph_instruments::create` so a Synth `preset` loads first, then `params` override it.
- **Embedded:** factory presets are compiled into `ph-instruments` with `include_str!`, so there's no file I/O at instrument creation. An unknown preset is an error that lists the available names.
- **Tests:**
  - every preset loads;
  - every id it names exists;
  - every value is in range;
  - every preset renders a C3 note without clipping above 0 dBFS at velocity 1.0.

### 4. Provenance

Write `assets/PROVENANCE.toml` entries for every file:
- **path** relative to `assets/`;
- **license:** `CC0-1.0` for everything generated (the project dedicates its generated sounds to the public domain, so users can use them in any music);
- **generator:** `soundgen <recipe> <params hash>`, or `source` for presets (`hand-authored, Patternhaus contributors`).

`cargo xtask check-provenance` must pass.

### 5. Documentation

`assets/README.md` lists the kits and presets, gives the `factory:` naming, and explains that everything is CC0.

## Done when

- The kits and presets exist.
- The soundgen and preset tests pass.
- `soundgen check` passes and is in CI.
- `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass.
- `assets/` totals no more than 40 MB.

## Out of scope

- The Sampler UI, the browser, and Haus Sampler multisamples (M3).
- Any third-party sample content.

## Report back

Summarise:
- the list of kits and presets;
- the measurements from the quality tests;
- the total asset size;
- any `ph-instruments` changes;
- the commit list.

Commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.
