# Brief M1/T7: Integration, QA and release

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t7-qa`.
**Read first:**
- `AGENTS.md`;
- DESIGN §4, §6 (licensing), §11, §12, §13;
- `docs/milestones/M1.md`: the acceptance criteria, plus every follow-up row still assigned to T7;
- `docs/parity/fl-studio.md`.

**Goal:** close M1. Every acceptance criterion gets either automated evidence or a precise owner-run check, every T7 follow-up is resolved, and the repository is ready to tag `v0.1.0-m1`. The orchestrator creates the tag itself; you prepare everything for it.

## Scope

### 1. Engine audition (follow-up T5)

- **Engine API:** add `Engine::audition(&self, sample: &str, velocity: f32)` and `Engine::stop_audition()`. These are additive; log them in the contract log.
  - A dedicated preview voice outside the project graph is mixed into the master output after the master insert, at a fixed preview level.
  - It is real-time safe: the sample is resolved and decoded off the audio thread, and handed over through the existing command ring or a mailbox.
- **Browser:** switch the browser's click-to-preview to `audition`, and remove the temporary "audition channel" project swap.
- **Tests:**
  - no allocation on the audio thread during an audition;
  - an audition never changes the offline render and never appears in exports;
  - auditioning during playback mixes in without disturbing the project's output.

### 2. Licence policy and notices (follow-ups T2, T5)

- **`cargo-deny`:** add `deny.toml` and a CI job.
  - **Allowed licences:** MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0/Unicode-DFS, CC0-1.0, BSL-1.0.
  - **Allowed for unmodified dependencies only:** MPL-2.0. Document why.
  - **Allowed for bundled fonts:** OFL-1.1 and the Ubuntu Font Licence (`epaint_default_fonts`).
  - **Denied:** GPL/LGPL/AGPL, except behind the `asio` feature (DESIGN §11). Check the policy with and without that feature.
  - Also enable the advisories and bans checks.
- **Third-party notices:** generate `THIRD_PARTY_NOTICES.md` (e.g. with `cargo-about`), including the font licences. Commit it, and add a CI check that it's up to date.
- **About box:** the app shows licences: MIT/Apache, plus a GPLv3 notice when built with `asio`.

### 3. Real-time code verification (follow-up T3)

- **Miri:** run the lock-free pieces under Miri: the plan mailbox (`rt.rs`), the collector hand-off, and the worker-pool dependency counters as far as Miri allows.
  - Skip the inline-asm FTZ/DAZ code with `cfg(miri)`.
  - Add a CI job (nightly toolchain) limited to those tests.
- **loom:** add `loom` models for the mailbox put/take race and the pool's ready-queue/dependency-counter protocol, behind a `loom` cfg. Add a CI job that runs them.
- **Fix whatever they find.** Report every finding, even fixed ones.

### 4. Golden renders and command coverage (DESIGN §12, criteria 6 and 8)

**Golden renders:**
- Add `tests/projects/` with 4–6 small projects that between them exercise every instrument, every M1 effect, sends, sidechain, swing, choke and muted notes.
- A golden test renders each project offline and compares it with a committed reference:
  - **Determinism:** run-to-run and across block sizes, bit-exact on the same machine.
  - **Cross-platform:** allow a small tolerance, since libm can differ (compare short reference WAVs, or per-file stats plus spectral fingerprints).
- `cargo xtask golden --update` regenerates the references. The PR must state why.
- Keep references small, under 5 MB in total.

**Command coverage:**
- **Golden test per command:** CI fails if any registered command lacks a schema test or a golden/behaviour test. A guard already exists for behaviour tests; extend it to cover every command registered from `ph-control` (e.g. `query.node_params`).
- **Oracle on the golden projects:** criterion 6 (real-time output through the simulated device must be bit-identical to the offline render) runs on every golden project, not just the T3 fixture.

### 5. Agent acceptance test (criterion 1)

Add `crates/ph-cli/tests/acceptance_m1.rs` (or promote the T4 e2e test). It maps step by step onto criterion 1 as written in `M1.md`:
- a 4-channel beat built through MCP only;
- 8 bars arranged;
- EQ and compressor on an insert, reverb on a send;
- WAV export;
- the render matches the offline oracle;
- LUFS between -16 and -8, no clipping;
- the agent received an image and an analysis report.

Run it in CI on all three OSes. This also covers the follow-up "unix-socket endpoint untested on Linux/mac": run the `--connect` e2e test there too.

### 6. Criteria 3, 4, 5, 7: evidence

- **Criterion 3 (co-editing):** point to the existing tests, or add any gaps.
- **Criterion 4 (round trip):** save, reopen and save again gives a byte-identical `project.json` on every golden project.
- **Criterion 5 (real-time safety):** the existing `assert_no_alloc` stress tests, also run in CI in release mode (debug-assertions on).
- **Criterion 7 (performance):**
  - Add `cargo xtask perf`, which runs the reference benchmark and compares it with a committed baseline file (`perf/baseline-7800x3d.json`). It fails on a regression of more than 10%.
  - CI runners are noisy, so the CI job only *reports* (non-blocking); the gate runs locally.
  - Record the current numbers as the baseline.

### 7. FL behaviour checks (follow-up T3; DESIGN §1 *behaves* column)

**Clean room:** use FL's public documentation (the Image-Line online manual) and observed behaviour only.
- **Volume and pan curves:** document FL's channel volume, mixer fader and pan laws from the manual. Make Patternhaus's curves match where the manual specifies them (e.g. fader dB scale, default level, pan law). Write `docs/formats/fl-behaviour-notes.md` with citations.
- **Clip looping:** confirm the clip behaviour the same way.
- **Owner checklist:** what the manual doesn't settle goes in `docs/parity/owner-checks.md`, a short checklist the owner can do in their FL 21 (e.g. "set channel volume to 50%, read the mixer peak"). Write each check so the result can be typed straight back.
- **Parity tracker:** fill the *Behaves* column wherever the manual settles it, and mark the rest "owner check pending".

### 8. Audio key estimation (follow-up T4, best effort)

Make the audio-based key estimate robust to tuned percussion. Use harmonic/percussive separation (median-filter HPSS), or down-weight onset frames, before computing chroma. Add the e2e-mix case (written A minor with a tuned 808) as a test. Note-based estimation stays the primary path.

### 9. Release workflow

- **Nightly:** a GitHub Actions workflow builds Windows, Linux and macOS binaries on pushes to `main` and attaches them to a rolling "nightly" pre-release. Each artefact is a zip with `ph-app`, `patternhaus-cli`, `assets/`, `THIRD_PARTY_NOTICES.md`, `LICENSE-*` and a short README.
- **Milestone releases:** a tag-triggered workflow (`v*`) creates a GitHub release with the same artefacts.
- **ASIO:** the official ASIO-enabled (GPLv3) Windows build needs the Steinberg ASIO SDK in CI. If that can be fetched legitimately in CI, add it as a separate GPL artefact. If not, document the manual build steps in `docs/building-asio.md`, and the M1 release ships the non-ASIO MIT/Apache build. Report which.
- **Workflow files only:** do not push tags or create releases yourself.

### 10. Close the milestone documents

- **`M1.md`:**
  - every acceptance criterion gets an "Evidence" line (test names, CI jobs, or "owner check" with a link);
  - every follow-up row is marked done, or moved to M2 with a reason.
- **`CHANGELOG.md`:** create it, with a `v0.1.0-m1` entry.
- **`README.md`:** update it with how to install and run (nightly builds, the launcher, `patternhaus-cli mcp`).

## Rules

- **No visible windows** and **never open the real audio device.** Use offscreen tests and `AudioMode::Silent`. The owner uses the desktop.
- **Contract changes:** additive only, each logged in the AGENTS.md contract log and listed in your report.
- **Dependencies:** must have permissive licences (the policy you're writing applies to you).
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test`, `xtask check`, plus your new `cargo deny check`, Miri and loom runs, all passing locally where the toolchain allows. Install the nightly toolchain and Miri with rustup if needed.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main, and don't push.

## Report back

Summarise:
- each section: done, how, and with which evidence;
- the Miri and loom findings;
- the licence policy result (any crates that needed exceptions);
- the golden reference sizes;
- the perf baseline numbers;
- the FL curve decisions, with citations;
- the owner checklist path;
- the release workflow, and the ASIO decision;
- the acceptance-criteria evidence table;
- test counts;
- the commit list.
