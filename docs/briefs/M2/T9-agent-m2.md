# Brief M2/T9: Agent abilities, M2

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t9` on branch `m2/t9-agent-m2` (`git worktree add E:\patternhaus-wt\m2t9 -b m2/t9-agent-m2 main`).
**Read first:**
- `AGENTS.md`;
- DESIGN §5 (agents);
- `docs/milestones/M2.md`: "Agent abilities, M2" and acceptance criterion 1;
- `crates/ph-control` (`agent.rs`, `views.rs`, `diff.rs`, the MCP server, the tool table);
- `crates/ph-analysis`;
- `crates/ph-ui` (`core.rs`: how agent edits flash and badge; `panels/agents.rs`).

## Scope

1. **Agent tools cover all of M2.** Every M2 command family is reachable and described well enough for an agent to use it without reading source:
   - audio clips, sample/BPM and fit-to-tempo;
   - the editor and slicer;
   - automation;
   - plugins;
   - recording and MIDI (where headless makes sense).

   Check `list_commands`/`describe_command` output and add the few convenience tools that a song-building agent obviously needs, for example `add_audio_clip` and `add_automation`, mapped onto existing commands like the M1 convenience tools.

   `get_state` views must show:
   - audio clips (file, range, mode, BPM);
   - automation clips (target, point count, range);
   - plugin instances.

   Musical diffs must describe audio and automation edits in words.
2. **Rendered audio for audio-capable models:**
   - `render` can return a short rendered excerpt as audio content (MCP audio content, base64 WAV), bounded in length and size with clear limits;
   - plus the existing analysis.
3. **Reference-track comparison:** `analyze { what: ["reference"], reference: <file or sample> }` compares the mix to a reference the user supplies. Report:
   - tonal balance per band (dB difference);
   - integrated and short-term loudness;
   - stereo width/correlation;
   - a few plain-language suggestions.

   Never copy the reference into the repo; tests use generated audio.
4. **Agent preview mode** (deferred from M1):
   - an agent can submit a batch as a *proposal*. The session applies it to a preview copy, the UI shows it as ghost notes/clips (with the agent's colour) and offers Accept / Reject, and agents get the decision back;
   - accepted proposals land as one undo step attributed to the agent;
   - rejected ones change nothing;
   - proposals made stale by other edits are reported, not force-applied;
   - commands, MCP tools and the UI are all covered, with tests (headless UI tests settle on the session snapshot).

## Rules

- **No visible windows**, and never open the real audio or MIDI device.
- **Generated audio only** in tests and fixtures.
- **Checks:** every new command or tool gets schema and behaviour tests, a contract-log row and a parity/coverage row. `cargo fmt`, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check` and `cargo deny check` all pass.
- **At most 2 local agents at once:** T10 (integration and QA) works in `E:\patternhaus-wt\m2t10` at the same time. Keep shared-file edits small and additive.
- **Commits:** commit working increments as you go with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- the tool coverage table;
- what `render` returns;
- the reference comparison output on an example;
- how preview mode works end to end;
- test counts and the commit list;
- known gaps.
