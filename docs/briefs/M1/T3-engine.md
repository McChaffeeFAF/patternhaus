# Brief M1/T3: Audio engine

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t3-engine`.
**Read first:**
- `AGENTS.md` ("The audio thread is sacred");
- DESIGN §4 (all of it), §11, §12;
- `crates/ph-engine/src/lib.rs`, the API contract you implement;
- `crates/ph-node`, the Processor contract;
- `ph_instruments::create` and `ph_effects::create`, which T2 merged on `main`.

**Depends on:** T0 and T2 (both on `main`). T1 (commands) is running in parallel. Don't depend on it; the engine sits below `ph-commands`.

## Goal

The real engine behind the `ph-engine` API: project snapshot → render plan → glitch-free real-time playback, plus a bit-identical offline render (the oracle) and WAV/FLAC export. It ends with a demo that renders a programmed beat to WAV, which is the owner's first checkpoint.

## Scope

### 1. Plan compiler (off the audio thread)

**Graph.** It turns `Arc<Project>` into a general DAG of nodes and ports (DESIGN §4: not a fixed chain). The M1 layout:
- each channel: instrument, then a channel strip (volume, pan, mute; *solo* comes later from the UI, so leave a hook), then its mixer insert;
- each insert: effect slots (bypass, dry/wet mix), insert volume and pan, meters, then sends (with levels) and `route_to_master`;
- sidechain edges from `sidechain_from` into the sidechain port of the compressor;
- the master insert, then the device output.

**Live or anticipative.** Mark every node live or anticipative (M1 renders everything live, but the flag must exist and flow to `ProcessContext::live`).

**Reuse across swaps.** Unchanged channels and effects keep their processor *instances* and sounding voices:
- key nodes by object identity (channel id; insert and slot index plus spec);
- a params-only change becomes `set_param` calls queued for the audio thread, not a rebuild;
- a changed instrument or effect kind gets a new instance;
- `activate` always runs before the swap, never on the audio thread.

**Events.** Pre-sort per pattern and channel into sample-stamped streams:
- pattern mode loops one pattern; song mode walks the playlist clips (start, length, offset, track mute);
- apply swing (off-beat 16ths);
- note ids are unique per sounding note;
- a seek or stop sends `AllNotesOff`.

**Tempo.** M1 has a constant tempo, but go through a `TempoMap` type so tempo automation can slot in later. Tick→sample conversion must be exact and drift-free over long songs (integer or rational arithmetic, never accumulated floats).

**Choke groups across channels** (T2 report): when a sampler channel with choke group *g* starts a note, the other channels in *g* get a choke. Choke groups are a channel-level grouping.

### 2. Real-time runtime

**Audio output.** `cpal`, with WASAPI shared mode by default and exclusive mode via config. ASIO goes behind a cargo feature `asio`, off by default (DESIGN §11).

**Lock-free communication** in both directions:
- **Plans:** swap them in with a single-slot lock-free exchange. Retired plans and processors go to a collector thread that frees them, so nothing is dropped on the audio thread.
- **Commands:** transport, preview notes and param changes go to the audio thread over an SPSC ring buffer (e.g. `rtrb`).
- **Status:** position, meters, stats and overloads come back the same way, or through atomics / triple buffers.

**Multicore graph.** A custom real-time-safe worker pool: pre-spawned threads, atomic dependency counters, no allocation, no blocking locks. Workers spin briefly, then wait on a cheap primitive.
- **Summation must be deterministic,** in a fixed order regardless of which thread finished first. The oracle depends on it.
- Use 64-bit accumulation on buses and the master (DESIGN §4).

**Audio-thread setup:** FTZ/DAZ on every audio and worker thread.

**Measurement and overload:**
- **Per-node timing** feeds `EngineStats`, plus the DSP load (smoothed processing time ÷ buffer time).
- **Overload detection:**
  - shedding level 1 skips meters;
  - level 2 drops processors to `Quality::Live` oversampling (if any are High);
  - level 3 lowers voice limits.
  - Repeated overloads set `suggest_buffer_frames`.
- **Meters:** peak and RMS per insert.

**Live input and transport:**
- `preview_note` for typing-keyboard and piano-roll previews;
- a metronome click (generated, no sample files);
- the transport: play, stop, seek, pattern/song mode, loop region.

### 3. Offline render and the oracle

**Offline render.** `render_offline` uses the same plan and node code, driven by a fake clock, single-threaded or multi-threaded (results must match).
- `RenderSettings::block_frames` must not change the output.
- `include_tail` renders until tails decay below -96 dBFS, capped at 10 s.

**Simulated device.** `Engine::start_simulated` and `SimulatedDevice::pull` give real-time output *bit-identical* to the offline render for the same project and transport actions. This is the DESIGN §12 oracle.

**Sample loading.** `assets::FileResolver`, an `AssetResolver` implementation over `ph_formats::decode`:
- project-relative and absolute paths within allowed roots;
- a `factory:` prefix mapped to an assets directory;
- caching of decoded samples (`Arc<SampleData>`).

### 4. Export

Add `ph_formats::encode`:
- WAV (16/24/32f, via `hound`) and FLAC (a permissively licensed encoder such as `flacenc`);
- TPDF dither when reducing to 16-bit.

T1 is also working in `ph-formats` (project and MIDI modules). Keep yours in its own `encode` module; the orchestrator merges the two.

### 5. Demo (owner checkpoint 1)

Write `crates/ph-engine/examples/render_beat.rs`. It builds a project with `ph_core::Edit`:
- a 4-on-the-floor kick, a snare and hats (use the Haus Synth or generated samples; factory samples arrive in T6);
- a Haus Synth bassline;
- an EQ and compressor on an insert, and reverb on a send;
- 8 bars in the playlist.

It then renders `beat.wav` and prints its duration and peak.

## Tests (all in CI)

- **Oracle:** simulated real-time equals offline, bit-identical, for several projects, block sizes and thread counts. Include transport changes and plan swaps mid-render.
- **Real-time safety:** the whole audio callback under `assert_no_alloc` while a second thread swaps in new plans every block (adding and removing notes, channels and effects) for thousands of blocks.
- **Timing:** a note at tick *t* starts at exactly `round(t → samples)`. Loops wrap sample-exact. Swing offsets are as specified. No drift after one hour of song time.
- **Voice preservation:** a param-only change during playback keeps voices sounding, with no click.
- **Routing:** sends, `route_to_master`, sidechain ducking, and mute are verified by signal tests.
- **Determinism and seeds:** the same project gives the same output across runs and thread counts.
- **Performance:** a criterion bench on the reference project from M1 acceptance criterion 7 (16 channels, 8-voice synth chords, 8 insert effects, 48 kHz/256). Report its DSP load.

## Out of scope

- PDC and anticipative scheduling (M4; flags only).
- Plugin hosting (M2).
- The UI, commands, the MCP interface and analysis.

## Report back

Summarise:
- the plan/graph design and the node reuse rules;
- the threading model and the real-time-safety measures;
- the oracle results;
- the benchmark DSP load for the reference project;
- any contract friction;
- test counts;
- the commit list.

Commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.
