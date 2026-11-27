# Brief M3/T13: Rename Patternhaus to Beast Studio

**Repo:** `E:\patternhaus` (the local folder keeps its name). GitHub is now `github.com/McChaffeeFAF/beast-studio`; the old URL redirects. Work in the worktree `local/wt/rename` on branch `m3/t13-rename` (from `main`), with `CARGO_TARGET_DIR=E:\patternhaus\target\wt\rename`. Never use the shared `target/`.

## Owner decisions (2026-11-27)
- **Name:**
  - the product is **Beast Studio**, part of the owner's Fluffy Beast Productions family alongside Beast Karaoke and Beast DJ;
  - the built-in sampler is **Beast Sampler** (formerly "Haus Sampler");
  - the mascot (art to come later, not in this track) is **Lyric**, with a Neon Red theme.
- **Model:**
  - it **stays open source** (MIT OR Apache-2.0, unchanged);
  - the owner will also sell a paid, ready-to-run build on Steam, plus paid content add-ons;
  - the name, the mascot and the brand art are **not** covered by the code licence.
- **Internal names stay:** `ph-*` crate names, Rust type names (`HausInstrument`, `HausSampler`, …), module names and test-plugin IDs (`dev.patternhaus.test.*`) all keep their names. Only what people and agents *see or type* changes.
- **MCP:** the server is renamed to `beast-studio`, and URIs become `beast-studio://…`.

## Rules for every occurrence: classify before you change (no blind replace)
Write a small script (a file, not an inline shell one-liner) that lists every case-insensitive hit of `patternhaus` and of `haus` in text files. Classify each hit into one bucket below, and review the diff bucket by bucket. Never run a replacement over binary files.

1. **Display names → change.**
   - The window title, About, splash, menus, dialogs, toasts and error text.
   - CLI help and the binary descriptions.
   - The MCP server instructions and tool descriptions.
   - The README and the forward-facing docs: DESIGN, `docs/formats/*`, `docs/manual-tests/*`, the parity docs' column headers and current rows, `docs/theming.md`, and the CONTRIBUTING/SECURITY files if present.
   - "Haus Sampler" → "Beast Sampler" in all visible text and current docs. The word "Haus" on its own in visible text, where it means the sampler, also changes.
2. **Binaries and commands → change together, in one commit:**
   - the `ph-app` binary → `beast-studio`;
   - `patternhaus-cli` → `beast-studio-cli`;
   - packaging scripts, `.github/workflows/*`, `release.yml`, `xtask` and test helpers that run or find these binaries;
   - `ph-plugin-scan` → `beast-studio-plugin-scan`, *if* it's found by name in exactly one helper (`find_scan_helper`). Otherwise leave it and say so.
3. **Persisted, wire and on-disk identifiers: decide one by one; never just rename.**
   - **Settings and data folders.** Old: `%APPDATA%\Patternhaus\catalogue`, the `patternhaus` config dir (layout/settings, `agents.json`), and `%LOCALAPPDATA%\Patternhaus` (plugin cache); plus the Linux/macOS equivalents, where the two spellings are different folders. New: `Beast Studio` / `beast-studio`.
     - **Copy once:** on first start, if the new folder is absent and the old one exists, copy the old one recursively into the new location.
     - **Never move or delete the old folder.** Other sessions may still run an old build against it.
     - Put this in one function, with tests that use temp dirs only.
     - **Check first that no test or headless test run can reach the real `%APPDATA%`/`%LOCALAPPDATA%`.** If any can, fix that first, in its own commit.
   - **Local endpoint** (`\\.\pipe\patternhaus-<user>`, `$XDG_RUNTIME_DIR/patternhaus.sock`): the new name is `beast-studio`. A new CLI with `--connect`, or the default auto-attach, tries the new name first, then the old one.
   - **Environment variables:**
     - `PATTERNHAUS_*` becomes `BEAST_STUDIO_*`;
     - **the old name is still read as a fallback**, so the owner's scripts keep working;
     - this covers ASSETS, CATALOGUE, CJK_FONT, CRASH_LOG, PLUGIN_SCAN, PLUGIN_SETTLE_SECONDS and any others you find;
     - document the new names.
   - **URIs:** `patternhaus://…` becomes `beast-studio://…`. Reading a resource still accepts the old scheme as an alias. Update the instructions and the tests.
   - **MIDI client name:** "Beast Studio".
   - **Default folder names** (e.g. a default `patternhaus-project`): new defaults use the new name, and existing folders still open.
   - **Keep unchanged:**
     - anything stored in saved projects, catalogue entries or factory paths: `factory:drums/haus808/...`, serialized enum or field names, the `instrument.toml` keys;
     - the test-plugin IDs.

     Check the serde names before touching any type.
4. **Real paths and URLs:**
   - GitHub URLs (the `repository` fields in `Cargo.toml`, badges, links) → `github.com/McChaffeeFAF/beast-studio`;
   - local paths like `E:\patternhaus\...` in docs: leave them (the folder isn't moving).
5. **History stays as written:**
   - AGENTS.md contract-log rows;
   - `docs/briefs/**`;
   - milestone evidence sections;
   - `docs/releases/v0.2.0-m2.md` and any past release notes;
   - CHANGELOG entries for past versions.

   Add a top note to `docs/milestones/M3.md`: "Renamed from Patternhaus to Beast Studio on 2026-11-27; earlier sections use the old name."

## Licence and brand
- **README:**
  - lead with **Beast Studio**, with a "formerly Patternhaus" note;
  - an **Editions** paragraph: the code is free and open source, and anyone can build it; a paid, ready-to-run Beast Studio build and paid content add-ons will be sold on Steam by Fluffy Beast Productions;
  - a **Trademarks and brand assets** section: "Beast Studio", "Beast Sampler", the Lyric character and the Fluffy Beast Productions names and art are not licensed under MIT/Apache; forks must use a different name and must not use the brand art;
  - the existing Kontakt trademark line stays as it is.
- **Brand folder:**
  - create `brand/`, holding only a `README.md` that says brand assets will live here and an `LICENSE-BRAND.md` that reads "© Fluffy Beast Productions. All rights reserved. Not covered by the repository's MIT/Apache licence";
  - make `xtask` provenance/notices and `deny` treat `brand/` correctly (excluded from the code-licence claims);
  - **add no image files** (the art comes later).
- **Licence files:** don't change `LICENSE-MIT` or `LICENSE-APACHE`, except for a copyright holder line if one names Patternhaus.

## Out of scope
- The private component under `E:\patternhaus\local\kontakt`, the launchers in `local/app`, and `.mcp.json`: the orchestrator updates these.
- Don't touch `local/` at all.
- Don't touch the paused T8 branch or worktree.

## Done means
- `git grep -i patternhaus` lists only an allow-list you write in your report: history files, fallback code for the old env vars, folder and URI names, the test-plugin IDs, the M3 note, the README "formerly" note, and local paths.
- `git grep -i "haus sampler"` lists only history files.
- **Checks:** fmt, `clippy -D warnings`, `test --workspace --no-fail-fast`, `xtask check`, `xtask notices --check` (regenerate the notices if needed) and `cargo deny check` all pass.
- Linux/macOS can't be linted locally (the plugin host builds a C file). Look hard for anything left unused or `cfg`-specific by the rename: a Linux-only dead-code error is what broke main last time.
- A contract-log row lists every renamed binary, env var, folder, pipe and URI, with its fallback.
- A `[Unreleased]` CHANGELOG entry: "Renamed from Patternhaus to Beast Studio".

## Rules
- Never open visible windows or real devices.
- Use at most 4 compile jobs and `--test-threads 2`.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`, one commit per bucket where practical. Don't merge or push.

## Report back
- each bucket, with its counts;
- the allow-list;
- the migration design and its test names;
- every changed external name with its fallback;
- the commit list.
