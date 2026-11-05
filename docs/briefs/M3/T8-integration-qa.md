# Brief M3/T8: Integration, QA and release preparation

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t8` on branch `m3/t8-qa`, with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\m3-t8`. That is your own target dir; never use the shared `target/`.

**Read first:**
- `AGENTS.md`: the whole M3 contract log;
- `docs/milestones/M3.md`: every criterion;
- `docs/briefs/M2/T10-integration-qa.md` and the "Acceptance evidence" section of `docs/milestones/M2.md`. Match that format;
- `docs/parity/haus-sampler.md` and `docs/parity/fl-studio.md`;
- `docs/manual-tests/M3-*.md`;
- `xtask` (perf, sampler-stress);
- `.github/workflows/ci.yml` and `release.yml`;
- `CHANGELOG.md`, and `docs/releases/v0.2.0-m2.md` as the model for release notes.

## Scope
1. **Leftover engine work:**
   - **Multi-out routing** (T2 left it to T8): a Haus Sampler group's output `n` reaches the instrument node's port `n`. The mixer can route those ports to inserts, the agent can do it through MCP, and live equals offline. If this turns out to be large, implement the routing in the engine and over MCP, keep the UI minimal, and report it.
   - **Mono/legato return-to-held-note**, which T2 skipped, if it is small.
2. **Model gaps reported by T3 and T4.** List them in `docs/parity/haus-sampler.md` under "Known gaps (after M3)". Implement only the cheap ones that unblock common libraries: initial controller values (`set_ccN`) and a backward loop mode are good candidates. Everything else stays listed.
3. **Performance:**
   - T2's piano perf case had a worst block of 381% against a 26% mean. Find out why: the first block, a stream warm-up, or real spikes. Fix real spikes, for example by moving work off the audio thread or pre-touching preload buffers.
   - Re-run `xtask perf --report` on a quiet machine, with no other builds running. Record the numbers in the evidence.
   - Leave `xtask perf --update` for the owner's reference-machine run.
4. **Acceptance evidence:** an "Acceptance evidence (M3/T8)" section in `docs/milestones/M3.md`, in M2's format. It gives a status per criterion: **met** by named tests, or **needs the owner** with exact steps. The steps cover criterion 7 (the checklists), criterion 6 (the perf re-run), and the catalogue check with Kontakt.
5. **Coverage and parity:** check that every `sampler.*` and `catalogue.*` command has schema and behaviour tests (the coverage guards), and that every M3 row in both parity docs has both columns filled.
6. **Docs:**
   - README: describe the Haus Sampler and catalogue features as they really are;
   - DESIGN §1 and §8: note what shipped and what moved to M3b and M4;
   - `docs/formats/` indexes.

   Keep everything accurate and modest. The public repo must not mention any private component except the importer hook as a generic feature.
7. **Release preparation (don't tag):**
   - write `docs/releases/v0.3.0-m3.md`;
   - add a `[v0.3.0-m3]` section to `CHANGELOG.md`, which `release.yml` reads;
   - bump the workspace version to `0.3.0` if that is how M2 was done (check git history around `v0.2.0-m2`).

   **Never create or push a tag.** The owner hasn't released `v0.2.0-m2` yet either. Note in the release notes that M2's tag comes first.
8. **Full checks**, clean, with your own target dir:
   - fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check`;
   - `cargo xtask sampler-stress` (the 2 GB run);
   - the Miri and loom jobs as CI runs them, if they're feasible locally.

## Rules
- Never open visible windows or real devices.
- Use at most 4 compile jobs and `--test-threads 2`.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- each criterion's status;
- the perf findings and numbers;
- what multi-out shipped;
- the gaps list;
- the release-prep files;
- the commit list.
