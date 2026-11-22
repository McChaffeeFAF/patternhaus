# Brief M3/T12: Analysis and export gaps found in real use

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/gaps` on branch `m3/t12-gaps` (from `main`), with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\gaps`. Never use the shared `target/`.

**Why:** an agent analysing and remaking real songs through Patternhaus (2026-11) hit three gaps and had to work around them with ad-hoc scripts:
1. `analyze` reports no tempo or beat grid for a song or render. The agent measured 115.000 BPM and a 12 ms first-beat offset by hand; without them, MIDI parts drift against audio stems.
2. `analyze` chords are one per bar, and the agent missed a chord change on beat 3 in every fourth bar of a loop. The orchestra played a wrong chord for about 37 bars.
3. Renders export only as WAV and FLAC, and a 5-minute FLAC (39 MB) is too large to send to the owner's phone (30 MB limit). A lossy format is needed for previews.

**Read first:**
- `AGENTS.md`: the contract log and the local layout;
- `crates/ph-analysis`:
  - `tempo.rs` (an existing estimator used for samples: reuse it, don't write a second one);
  - `onsets.rs`;
  - `music.rs` and `music/` (key and chords);
  - `lib.rs` and how `analyze` assembles its sections;
- `crates/ph-control/src/host.rs` (render, analyze, export) and the `analyze` / `export_audio` tools and their schemas;
- `crates/ph-formats/src/encode.rs` and the render/export paths in the CLI (`--format`, `--bit-depth`);
- `deny.toml` (the licence allow-list and exceptions) and `THIRD_PARTY_NOTICES.md` / `xtask notices`.

## Scope
1. **Tempo section in `analyze`**, added to the `what` options and to the default set if it is cheap:
   - the song's tempo from the existing estimator, with confidence and the half/double alternatives;
   - the **first-beat offset** in seconds;
   - a **downbeat** guess, the bar phase, from where the onsets are strongest;
   - **drift**: the tempo measured in windows of about 30 s, so a live-played or tempo-changing piece shows up as such;
   - for project renders, a comparison against the project tempo ("matches the project tempo" or "differs by …").

   Work on any audio: a project render, and also a single audio clip or sample (a `sample` or `clip` parameter), so an agent can analyse an imported reference track without rendering the whole project.
2. **Chord resolution:**
   - `analyze` chords take a `resolution`: `bar` (today's default, unchanged), `half_bar` or `beat`;
   - the output marks a chord change inside a bar (e.g. `"4.1 Ab", "4.3 Gb"`).

   Keep the chord detector itself; just evaluate it on shorter windows aligned to the beat grid from step 1, or to the project grid for project renders.
3. **Compressed export:**
   - Add **Ogg Vorbis** or **Opus** (Ogg container) export to `export_audio` / `render --format`, with a quality or bitrate option and a sensible default (a 5-minute stereo render around 5–10 MB).
   - The encoder must pass `cargo deny check` as configured. Prefer a pure-Rust encoder, or a binding to the reference library under a permissive licence (libvorbis and libopus are BSD-3-Clause); add any needed deny.toml exception the same way the existing ones are written, with a reason line.
   - **Do not add MP3 encoding** (LAME is LGPL, which is not on the allow-list).
   - If no encoder passes cleanly, stop and report the options instead of adding one.
   - Decoding of the new format already works through symphonia (vorbis); add a round-trip test (encode, then decode with the existing decoder, then compare levels and duration).

## Tests
- tempo on generated click tracks (steady, offset, tempo change);
- tempo on a factory loop with a known BPM;
- half-bar chords on a generated progression with a beat-3 change;
- export round-trip and size bounds;
- schema and behaviour coverage for every changed command.

## Rules
- Never open visible windows or real devices.
- Use at most 4 compile jobs and `--test-threads 2`. Another build may run in parallel.
- Don't touch `ph-plugin-host` or `ph-ui`: other work is in progress there.
- **Checks:** fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- Contract log rows; parity rows if FL has an equivalent.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- what each section reports, with example output;
- the encoder chosen, its licence and the deny result;
- the test names;
- the contract changes;
- the commit list.
