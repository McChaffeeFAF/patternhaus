# Brief M3/T6: Importer hook

**Owner decision (2026-11-05):** some formats stay out of the public repository. The public repo gets a generic, documented **importer hook** so separately built importers can add formats. Work in the worktree `local/wt/m3-t6`, branch `m3/t6-hook`.

_TARGET_DIR=E:\patternhaus\target\wt\m3-t6` for both.

**Read first:**
- `AGENTS.md`;
- `docs/milestones/M3.md` (defaults, criterion 3, D phase);
- `crates/ph-sampler-formats/src/lib.rs` (the `Importer` trait, registry, `importer_for`, `extensions`) and `stubs.rs`;
- `crates/ph-commands/src/builtin/sampler_files.rs`;
- `crates/ph-app/src/main.rs` and `crates/ph-cli/src/main.rs`;
- the private spec `E:\patternhaus\local\kontakt\docs\nki.md` (for part B only).

## Scope
1. **Runtime registration** in `ph-sampler-formats`, e.g. `register_importer(Box<dyn Importer>)` or a registry value passed through the host. Pick the design that fits the code: avoid global mutable state if the host/session already carries a context, and document the choice. Registered importers:
   - take part in `importer_for`, `extensions`, `sampler.import`, the browser's file filter and drop-to-create;
   - can declare an `Encrypted` refusal with a plain message.
2. **Entry points** so a separate binary can register importers and then run the normal app or CLI, without copying their code.
   - Example: move the bodies of `ph-app` and `patternhaus-cli` `main` into library functions, `ph_app::run(Options { importers })` and the CLI equivalent.
   - Keep the public binaries' behaviour identical.
3. **Remove the Kontakt stub** from the public registry. `.nki`, `.nkm`, `.nkx`, `.nkc` and `.ncw` are no longer known extensions there. The `SampleSource::Ncw` variant and any Kontakt names in public code or docs go too; this is a logged contract change. Search the code for "Kontakt" and "nki" and leave only neutral mentions: the README trademark line, the DESIGN decision note, and the Kontakt *plugin* in the catalogue docs.
4. **Proof:** a test importer living in the test suite (a made-up text format), registered at runtime. It goes through import, play, save as native and reopen, end to end through MCP, with identical renders. This is M3 criterion 3.
5. **Docs:** `docs/formats/importer-hook.md`, explaining how to write and register an importer: the trait contract, never panicking, the report rules, the refusal rules, and the fuzzing advice.

## Rules
- Public checks: fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check`, `cargo deny check`.
- Use at most 4 compile jobs and `--test-threads 2`. Another agent is building in parallel.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- the design;
- the test names;
- the contract changes;
- the commit list.
