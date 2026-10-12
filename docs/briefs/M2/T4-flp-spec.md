# Brief M2/T4: FLP format specification (clean room, spec role)

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m2/t4-flp-spec`.
**Read first:** `AGENTS.md` (the clean-room policy); DESIGN §10 (FLP import); `docs/milestones/M2.md` (the FLP import section).

## Your role in the clean room

You are the **spec writer**. A *different* agent will later implement the importer from your document alone, never seeing what you saw. So the spec must be complete, precise, and a statement of **facts only**: byte layouts, event ids, value meanings and units, what each feature looks like on disk. It contains no code and no prose copied from elsewhere.

**Allowed sources:**
1. **Your own analysis of the owner's FL Studio project files** (primary). The local corpus:
   - `C:\Users\conno\Documents\Image-Line\FL Studio\Projects` (154 `.flp`)
   - `C:\Users\conno\OneDrive\Documents\Image-Line\FL Studio\Projects` (101 `.flp`)

   Read them, never modify them, never copy them into the repo. Some may be Image-Line demo songs: analysing them locally is fine, committing anything from them is not.
2. **Public documentation pages** (format notes, forum posts, wikis, the PyFLP *reference documentation* on readthedocs) as factual hints only, each cited in the spec.

**Forbidden:**
- reading or reproducing **PyFLP's source code**, or any other GPL or proprietary code;
- decompiling or disassembling FL Studio;
- copying wording from any source.

Where a fact comes from public documentation, confirm it against the corpus where you can, and mark it "confirmed in corpus", "documented, not seen" or "inferred".

## Deliverables

### 1. `docs/formats/flp.md`, the specification

- **Container:** headers and chunks, endianness, the event stream framing (event id ranges and their payload size classes, variable-length encoding), and text encodings (UTF-16 vs ANSI by version).
- **Project-level events:** FL version, tempo (and its unit), time signature, title and other metadata, PPQ.
- **Channels:** type (sampler, generator plugin, audio clip, automation clip, layer), name, colour, volume and pan (raw value ranges and how they map), mixer routing, sample path, plugin identity and state blobs.
- **Patterns:** ids, names, notes (key, position, length, velocity, pan, fine pitch, flags), and the units of every field.
- **Playlist:** tracks, clip items (pattern and audio), positions, lengths and offsets, muted state, track names.
- **Mixer:** inserts, names, routes and levels, effect slots (plugin identity and state), volume and pan raw values.
- **Automation:** how automation clips and points are stored, as far as the corpus shows.
- **Native FL plugins:** how their identity and state appear (we map them to equivalents or placeholders; we don't need to understand their internal state).
- **Version differences seen in the corpus**, especially FL 20, 21 and newer if present.
- **An unknowns section:** everything not yet understood, with the evidence collected so far.

### 2. `tools/flp-probe`

A small Rust tool, a workspace member (follow how `tools/soundgen` is set up). It reads FLP files and prints the event stream and aggregate statistics: event-id frequencies across the corpus, which ids carry text, value ranges per id.
- It's your analysis instrument, and later the implementer's way to check the spec.
- Write it from your own understanding; don't port any parser.
- Its tests use **tiny synthetic FLP byte sequences built in the test**, never corpus files.

### 3. `docs/formats/flp-corpus-report.md`

Aggregate facts only: counts of files per FL version, event-id coverage, and which features appear how often. **No song names, sample names, user names or any content from the files.**

### 4. A controlled-files request for the owner

Add a "Controlled test files" section to the spec. It lists 10–15 tiny projects the owner could save in FL 21 to pin down the remaining unknowns, e.g. "an empty project at 123.456 BPM", "one channel, one note at bar 2, velocity 64". Each comes with the exact steps, so the owner can make them in a few minutes. These are the owner's own files, so they're allowed as fixtures later.

## Rules

- **No visible windows.** Never launch FL Studio or any app.
- **Read-only** on the owner's folders. Never commit corpus files or anything derived from their content beyond aggregate statistics.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass. Add `flp-probe` to any lists it needs, such as the xtask LAYERS table if `tools/` is checked.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge.

## Report back

Summarise:
- the spec's coverage (each feature: confirmed, partial or unknown);
- the sources cited;
- the corpus statistics summary;
- the controlled-files list;
- test counts;
- the commit list.
