# Brief M2/P1: Polish (FLP import placement and small follow-ups)

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m2/p1-polish`.
**Read first:** `AGENTS.md`; DESIGN §10 ("Placement (decided 2026-10-11)"); the follow-up lists in `docs/milestones/M2.md`; `crates/ph-commands/src/builtin/file.rs` (`file.import_flp`); `crates/ph-control/src/host.rs`; `crates/ph-ui/src/dnd.rs` and `app.rs`; `crates/ph-ui/src/paint.rs` (the text-fitting helpers).

## Scope

### 1. FLP import placement (DESIGN §10, owner sign-off)

- **Default location:** `file.import_flp` creates the project in `Documents\Patternhaus Projects\<name>`, using the user's Documents folder on each OS, numbered when the name exists and never overwriting.
- **Option:** a param (e.g. `placement: "projects_folder" | "next_to_flp"`, default `projects_folder`) and a choice in the drop/import prompt, "Import into Patternhaus Projects" or "Next to the FL project".
- **Fallback:** if the target isn't writable, fall back to the projects folder and note it in the import report.
- **Agents:** the projects folder is an allowed root for agents. Add it in `ph-app` and headless startup, not by loosening the rules.
- **Tests:** one per case: default, next-to, unwritable fallback, numbering, agent confinement.

### 2. Small follow-ups (from `M2.md`)

- **Project title** in the bottom bar: cap it at about 30% of the bar width.
- **Paths:** middle ellipsis for sample paths and file paths (keep the file name visible: `C:\…\Kick 808.wav`). Add a `paint` helper and use it where paths are shown.
- **Playlist clips:** a hover tooltip showing the full clip name.
- **Test hygiene:** tests write the screenshot PNGs in `docs/screenshots/` **only** when `PH_SCREENSHOTS=1` is set. A plain `cargo test` leaves the working tree clean. Verify with `git status`.
- **UI project folder:** when `file.open` or `file.import_flp` moves to another project folder, the UI's `AppPaths.project_dir` (the browser's project tab and the save prompts) follows it.

## Rules

- **No visible windows**, and never open the real audio device. Never write into the owner's real Documents folder in tests: make the projects-folder location injectable and use temp dirs.
- **Contracts:** additive only, logged in the AGENTS.md contract log.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check` and `cargo deny check` all pass, and `git status` is clean after `cargo test`.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge.

## Report back

Summarise:
- each item and its tests;
- any contract additions;
- test counts;
- the commit list.
