# Brief M2/T2: Effects, M2 part

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m2/t2-effects`.
**Read first:** `AGENTS.md` ("The audio thread is sacred", "How to add a processor"); DESIGN §7; `crates/ph-effects` (the seven M1 effects, their tests in `tests/rt.rs` and `tests/known_answer.rs`, and the shared harness in `tests/common`); `crates/ph-dsp`; `crates/ph-node` (including the M2 `load_state(&dyn AssetResolver)`).

## Scope

Nine new built-in effects as `ph_node::Processor`, registered in `ph_effects::create`, `BUILTIN` and `ph_engine::catalog`.

| id | Spec |
|---|---|
| `ph.fx.multiband` | 3-band compressor with Linkwitz-Riley crossovers (phase-coherent sum when flat); per band: threshold, ratio, attack, release, gain; solo/bypass per band |
| `ph.fx.gate` | Gate/expander: threshold, ratio (expander) or range (gate), attack, hold, release, sidechain input port, lookahead |
| `ph.fx.convolution` | Partitioned FFT convolution reverb: impulse response from `AssetResolver` (`factory:ir/<name>`) or generated; pre-delay, dry/wet, low/high cut, length trim. Ship 4 **generated** IRs (room, hall, plate, spring) via `tools/soundgen` with provenance |
| `ph.fx.chorus` | Multi-voice chorus: voices, rate, depth, delay, spread, mix |
| `ph.fx.flanger` | Flanger: rate, depth, feedback (±), manual, stereo phase, mix |
| `ph.fx.phaser` | Phaser: stages (2–12), rate, depth, feedback, centre, stereo phase, mix |
| `ph.fx.bitcrush` | Bit depth, sample-rate reduction (with optional anti-alias), dither, mix |
| `ph.fx.stereo` | Width (M/S), balance, mono-below crossover, phase invert L/R, swap |
| `ph.fx.analyzer` | Pass-through analyser: spectrum, loudness (short-term LUFS) and phase correlation, published lock-free for the UI. Its effect window draws a live spectrum and meters with the existing widgets |

Effect windows already come from catalog descriptors, so they appear automatically. Add a custom draw only for the analyser.

## Quality bar

Same as M1 effects (see M1/T2):
- **Real-time safety:** every effect passes the shared `assert_no_alloc` stress test.
- **Determinism:** identical output for the same input and seed.
- **Block-size independence:** 64 ≡ 512 ≡ 37.
- **No denormal slowdown.**
- **Known-answer tests:**
  - the crossover sums flat within 0.1 dB;
  - the gate's static curve;
  - convolution with a unit impulse is identity, and with a known IR matches direct convolution;
  - the chorus, flanger and phaser modulate at their set rates;
  - bitcrush gives exact quantisation;
  - stereo width 0 = mono and 1 = identity;
  - the analyser's LUFS matches `ph-analysis` within 0.2 LU.
- **Benchmarks:** add criterion benches next to the existing ones and report µs per 256-frame block.

## Rules

- **No visible windows**, and never open the real audio device.
- **Clean room:** use DSP literature and your own code; no GPL code.
- **IRs:** generated only, with provenance entries. `cargo run -p soundgen -- check` must stay byte-identical.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge.

## Report back

Summarise:
- the effects with their parameter counts;
- the known-answer results;
- the benchmarks;
- test counts;
- the commit list.
