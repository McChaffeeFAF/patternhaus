# Brief M1/T9: Drag and drop from the OS

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t9-dnd`.
**Read first:**
- `AGENTS.md` ("How to add a command", the contract log);
- DESIGN §5 (file-access rules) and §10 (project folders);
- `crates/ph-commands/src/builtin/file.rs` and `channel.rs`;
- `crates/ph-commands/src/host.rs` (the `Host` trait);
- `crates/ph-control/src/host.rs` (`EngineHost::resolve_path`);
- `crates/ph-ui` (the browser, channel rack, playlist and piano roll panels, the hint bar and `windows.rs`);
- `crates/ph-formats` (`decode`, MIDI import).

**Why:** the owner asked to drag `.wav`, `.mp3` and other files from Windows Explorer into the app, which is one of FL's most-used gestures. Today only drags from Patternhaus's own browser work.

## Behaviour

| Drop… | onto… | result |
|---|---|---|
| Audio files (WAV, AIFF, FLAC, MP3, OGG; one or many) | Channel rack, empty area | One new Sampler channel per file, named after the file stem, in drop order. A multi-file drop is **one undo step**. |
| One audio file | An existing channel row | Replaces that channel's sample (the instrument becomes or stays a Sampler with the new sample) |
| Audio files | Playlist | Same as the channel rack for now (audio clips are M2). Say so in the hint text. |
| A `.mid` / `.midi` file | Playlist or piano roll | `file.import_midi` |
| A folder | Browser | Added as a library folder for this session and remembered in settings. The browser shows it. |
| A project folder, or a `project.json` | Anywhere | Opens it, asking Save / Don't save / Cancel when there are unsaved changes (reuse the T5c dialog) |
| Anything unsupported | Anywhere | A notice naming the files that were skipped, and why |

**While hovering**, highlight the target panel or row with the accent edge and glow. The hint bar says what will happen ("Drop to add 3 samples as channels", "Drop to replace Kick's sample", "MIDI: drop to import as a new pattern"). Use egui's `hovered_files` and `dropped_files`, and make sure drag-and-drop is enabled on the viewport.

## Samples are copied into the project

Dropped audio is copied into the project's `samples/` folder, so projects stay self-contained and portable (DESIGN §10).
- **Naming:** keep the file name; on a clash, add ` (2)`, ` (3)` and so on. If an identical file (same bytes, by hash) is already there, reuse it instead of copying.
- **Reference:** the channel references `samples/<name>`, relative to the project.

### Command: `sample.import`

A new command with params `{ paths: [String], channel?: ChannelId }`.
- **Without `channel`:** creates channels.
- **With `channel`:** replaces that channel's sample. Only one path is allowed.
- **Returns:** the channel ids and the project-relative references.
- **Mutating and undoable.** Undo removes the channels; the copied files stay. They're harmless, and DESIGN never deletes user data on undo.
- **Scope `Compose`.**

The copy is a filesystem side effect, so it goes through the `Host`.
- **New method:** an additive default method on the `Host` trait, e.g. `fn import_file(&mut self, source: &Path, into: ImportKind, author: &Author) -> CmdResult<String>`, returning the project-relative path. Log it in the AGENTS.md contract log.
- **Who may import what (DESIGN §5):**
  - the **human** may import from any path, since the drop itself is the user's intent;
  - **agents** may import only from paths inside the allowed roots (project folder and library folders), checked through the same confinement rules as `resolve_path`.
- **Implementations:** `EngineHost` performs the copy. `NullHost` returns a clear error.
- **Decoding:** validate each audio file by decoding its header (`ph_formats::decode`) before copying. A file that won't decode is skipped and reported, not copied.

`file.import_midi` and `file.open` already exist. The UI calls them for `.mid` and project drops. Check that `file.import_midi` accepts a path dropped from outside the project for a human caller under the same rule. If it doesn't, route it through the same human-may-import-anything rule.

## Rules

- **Commands only:** the UI turns every drop into commands (`sample.import`, `file.import_midi`, `file.open`) as `Caller::human()`; it never copies files itself.
- **No visible windows**, and **never open the real audio device.** Test offscreen with `egui_kittest`. Simulate drops by injecting `RawInput.dropped_files` and `hovered_files` into the test harness.
- **Tests, at least:**
  - one per table row;
  - name clashes and dedupe;
  - a multi-file drop as one undo step;
  - an agent refused outside the allowed roots;
  - an undecodable file skipped and named;
  - the hover highlight and hint text.

  Use small WAV/FLAC fixtures generated in the test (`ph_formats::encode`), not committed files.
- **Docs:**
  - add a "Drag and drop" part to `docs/manual-tests/M1-ui.md`;
  - add parity rows in `docs/parity/fl-studio.md`;
  - add `CHANGELOG.md` entries.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check`, `cargo deny check` and `cargo xtask notices --check` all pass.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.

## Report back

Summarise:
- the behaviour table, as implemented;
- the Host and command contract additions;
- the access rules;
- the tests;
- test counts;
- the commit list.
