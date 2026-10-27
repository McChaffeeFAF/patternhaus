# Brief M2/T10: Integration and QA

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t10` on branch `m2/t10-qa` (`git worktree add E:\patternhaus-wt\m2t10 -b m2/t10-qa main`).
**Read first:**
- `AGENTS.md`;
- `docs/milestones/M2.md` (all of it, especially the acceptance criteria, status sections and follow-ups);
- `docs/milestones/M1.md` (how M1's acceptance evidence was recorded);
- `crates/ph-cli/tests/acceptance_m1.rs`;
- `crates/ph-engine` oracle, golden and perf (`cargo xtask perf`);
- `docs/parity/fl-studio.md`.

## Scope

1. **M2 acceptance test** (criterion 1), `crates/ph-cli/tests/acceptance_m2.rs`. Through MCP only, a scripted agent builds a 32-bar song:
   - intro, build and drop sections;
   - a filter sweep and a volume automation clip;
   - a generated drum loop imported as audio and fitted to the project tempo;
   - a slice instrument made from that loop;
   - one instance of the repo's test CLAP plugin;
   - at least two M2 effects (for example multiband compressor and convolution reverb).

   Assert:
   - the export matches the offline oracle;
   - loudness sits between -14 and -8 LUFS;
   - no clipping;
   - the analysis reports the section structure.

   Use only commands and tools that exist on `main`. If T9 (working in parallel) adds convenience tools, don't depend on them.
2. **Oracle, real-time safety and perf** (criteria 6 and 7):
   - one project combining audio clips, automation, a monitored recording path (simulated input) and the test plugin renders live ≡ offline under `assert_no_alloc`;
   - `cargo xtask perf` runs the reference project + 8 stretched clips + 4 automation clips and reports DSP load against the 25% target;
   - **perf baseline:** the committed file is from the owner's reference machine, so don't overwrite it. Record this machine's numbers in M2.md and say the owner should re-run on the reference machine.
3. **Parity and coverage** (criterion 8):
   - every M2 row in `docs/parity/fl-studio.md` has both columns filled (owner-check-pending where only the owner can judge, with the item added to `docs/parity/owner-checks.md`);
   - every M2 command has schema and behaviour tests; extend the coverage guard if needed.
4. **Follow-ups to clear:**
   - the audio clip's fade-in handle draws over the clip name in the playlist; move the label clear of it;
   - a CJK fallback font for names (an OFL Noto subset or similar; check the licence and add it to the notices), with a long-names scene using CJK text;
   - the analyser window shows the most recently activated analyser; make it per-node, so each analyser's window shows its own feed;
   - the M1 carried-over follow-ups that are cheap: faint 1/32 and 1/64 grid lines, theme-aware default colours avoiding the agent hue, and the `.exe` icon resource. Skip ASIO.
5. **M2.md:**
   - write the acceptance evidence section, with criterion-by-criterion status like M1.md: met, met by test X, or needs the owner (with what to do);
   - list the owner sign-off items (criteria 2, 3 with the local FLP corpus, 4 with the owner's plugin report, and 5's listening test).
6. **Do not tag or release.** Tagging `v0.2.0-m2` needs the owner's explicit OK; prepare the release notes in `docs/releases/v0.2.0-m2.md` instead.

## Rules

- **No visible windows**, and never open the real audio or MIDI device.
- **Generated audio only.** Never commit anything from the owner's FL projects or plugin report.
- **Test waits:** UI tests settle on the session snapshot, never on fixed frame counts.
- **Checks:** `cargo fmt`, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check`, `cargo deny check` and `cargo xtask notices --check` all pass.
- **At most 2 local agents at once:** T9 (agent abilities) works in `E:\patternhaus-wt\m2t9` at the same time. Keep shared-file edits small and additive.
- **Commits:** commit working increments as you go with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- the acceptance test result with its LUFS/peak numbers;
- the oracle and perf numbers;
- the parity/coverage state;
- each follow-up's outcome;
- the criterion-by-criterion table;
- test counts and the commit list;
- known gaps.
