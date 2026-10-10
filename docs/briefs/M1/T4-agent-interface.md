# Brief M1/T4: Agent interface and analysis

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t4-agent`.
**Read first:**
- `AGENTS.md`, including the contract log;
- DESIGN §3, §5 (all of it) and §12;
- `docs/milestones/M1.md`, including the review follow-ups table;
- `crates/ph-control/src/lib.rs` (`Studio`, the core you build on);
- `ph_commands::{Registry, SessionHandle, Change}`;
- `ph_engine::catalog`.

**Depends on:** everything on `main`. T5 (the UI) runs in parallel and also builds on `Studio`.
- Don't change `Studio`, `StudioConfig`, `AudioMode` or `EngineHost` without recording it in the contract log.
- Don't touch `ph-ui` or `ph-app`.

## Goal

An agent can drive Patternhaus entirely through MCP: discover every command, build and arrange a beat, play it, render it, and **perceive** the result through analysis numbers and images. The same functionality is available over JSON-RPC and a CLI.

## Scope

### 1. `ph-analysis` (layer: `ph-core`, `ph-node`, `ph-dsp`)

**Loudness and levels:**
- integrated and short-term LUFS per ITU-R BS.1770-4 (K-weighting and gating);
- true peak (4× oversampled);
- RMS;
- crest factor.

**Spectrum:** per-band energy (sub, bass, low-mid, mid, high-mid, presence, air), plus spectral centroid.

**Mix health:**
- clipping count;
- DC offset;
- phase correlation;
- mono-compatibility (the loss in mono, in dB);
- the energy curve per bar.

**Onsets:** with timestamps in seconds and in `bar.beat.tick`.

**Key and chords:**
- **From notes:** read directly from `Project` notes (exact): the key estimate, plus chord symbols per bar or per beat (triads, sevenths, sus, inversions).
- **From audio:** chroma → template matching.

**Masking and clash warnings:**
- per pair of channels, overlapping spectral bands with level;
- notes from different channels a semitone apart sounding at the same time.

**Images:** spectrogram, waveform, piano roll (one pattern, or all channels coloured) and playlist overview, as PNG via `tiny-skia` and `png`. The look must be legible to a vision model: axes, note names, bar numbers.

**Known-answer tests:**
- render generated MIDI with known keys and chords, and check that both estimators get them right;
- check LUFS against the reference values from EBU Tech 3341 test signals (synthesise them);
- check true peak against analytic cases.

### 2. Readable views (`ph-control`)

`get_state` views for agents, besides the raw `query.*` JSON:
- **Notes:** names (C4 = 60) and `bar.beat.tick`.
- **Pattern summary:** channels, note ranges, and chords per bar.
- **Section map:** built from playlist clips and pattern names, e.g. "intro 1–8: Beat, Pad; drop 9–16: …".
- **Mixer summary:** routing, effects and levels.
- **Change summaries:** after each `run_batch`, return a musical diff: per channel, notes added or removed with bar ranges, and the resulting chord progression where it changed.

### 3. MCP server (`ph-control`)

Use `rmcp`, the official Rust MCP SDK (check its licence is MIT or Apache).

**Fixed toolset** (DESIGN §5):

| Tool | Purpose |
|---|---|
| `list_commands(filter)` | Discover commands |
| `describe_command(id)` | Parameter schema and summary |
| `run(id, params, confirm?)` | Run one command |
| `run_batch(commands, as_one_undo)` | Run several as one undo step |
| `get_state(view, args)` | Readable views (above) or raw JSON |
| `render(range?, format?)` | Writes a file under exports and returns its path plus a quick analysis |
| `analyze(range?, what[])` | Numbers from §1 |
| `snapshot(kind, args)` | MCP image content |
| `node_params(instrument or effect)` | Wraps `ph_engine::catalog` |
| `undo_mine` / `redo_mine` | Undo or redo the agent's own work |
| `history(limit)` | Recent history |

Plus direct tools for about 15 high-traffic commands: add notes, toggle steps, add a channel, add a clip, set tempo, set a mixer volume, add an effect, set an effect param, play, stop, and export audio.

**Agents also need parameter lists as a command.** Register `query.node_params` into the registry from `ph-control` (`Registry::register` is public), so agents and the CLI reach it through normal command calls as well.

**Identity and scopes** (DESIGN §5):
- each MCP client becomes `Caller::agent(<client name>)`;
- its grant comes from configuration: a `--grant compose,mix` flag for headless sessions, or remembered per client name in a settings file for in-app sessions (T5 supplies the in-app prompt; give it a hook);
- `read` is the default for unknown clients in-app; `full` is the default for headless sessions started explicitly with `--grant`.

**Live events:** subscribe to `SessionHandle::subscribe`. Push MCP notifications or resource updates for edits made by other authors (human or other agents), plus transport and overload events.

**Transports:**
- **Headless:** `patternhaus-cli mcp --headless --project <dir>` serves MCP over stdio, using `Studio` with `AudioMode::Silent` by default and `--audio` for the real device.
- **Live:** the app (T5) opens a local endpoint: a named pipe on Windows, a Unix socket elsewhere. `patternhaus-cli mcp --connect` is a stdio proxy to it, so Claude Code registers one stdio server that either attaches to the running app or starts headless. Put the endpoint server in `ph-control`, so T5 only calls `serve_local(&studio)`.

### 4. JSON-RPC and CLI

- **JSON-RPC 2.0:** over the same local endpoint, with methods mirroring the tools.
- **`ph-cli`** binary crate, named `patternhaus-cli`:
  - `run <command> --params '<json>'`
  - `script <file.json>` (a list of requests, optionally as one undo)
  - `render`
  - `analyze`
  - `mcp`

  Headless by default, or `--connect` to a running app.

### 5. Review follow-ups assigned to T4 (from `M1.md`)

- **Sidechain:** `mixer.set_sidechain` also sets the target compressor's `sidechain` param to External, and back when cleared.
- **Engine warnings:** an unloadable sample or unsupported instrument compiles as silence. Surface it: add a warnings list to `EngineStats` (an additive change to the `ph-engine` API, logged in the contract log), and show it to agents in `get_state` and render results.
- **History log on save:** `file.save` should persist the per-author history log in `history/`. The queue owns the session, so do it through the `SessionHandle`.

## Done when

1. An **end-to-end test** passes. It spawns `patternhaus-cli mcp --headless` and talks real MCP over stdio as a client. Using only MCP tools, it:
   - creates a 4-channel beat (kick, snare and hat from `factory:` samples; bass on a Haus Synth preset);
   - arranges 8 bars;
   - adds EQ and a compressor on an insert and reverb on a send;
   - exports a WAV;
   - checks that `analyze` reports LUFS between -16 and -8 and no clipping, that `snapshot` returns a PNG, and that the key estimate matches the written bassline.

   This is the precursor to M1 acceptance criterion 1.
2. The known-answer analysis tests pass.
3. `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass. New crates go in the LAYERS table: `ph-analysis` and `ph-cli` are already listed.

## Out of scope

- The UI (T5): the in-app grant prompt UI and showing an agent's selections. Provide hooks only.
- Reference-track comparison and audio clips for audio-capable models (M2/M3).

## Report back

Summarise:
- the tool list;
- the scope model as implemented;
- the analysis accuracy numbers;
- the e2e test transcript summary;
- any contract changes;
- test counts;
- the commit list.

Commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.
