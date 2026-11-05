# Brief M3/T9: Plugin instrument catalogue (agent access to hosted instruments)

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t9` on branch `m3/t9-catalogue`, with `CARGO_TARGET_DIR=E:\patternhaus\target`.

**Why (owner, 2026-11-05):** Patternhaus is agent-oriented. The owner wants agents to **use** the sampled instruments they own, including licence-locked ones such as Kontakt Player libraries. The legal way is to host the real plugin (Kontakt 7 VST3 loads fine in the M2 host), which decrypts content under the owner's licence, and to make it agent-friendly.

This track is **generic**: it works for any CLAP or VST3 instrument. Nothing here is Kontakt-specific code, and nothing reverse-engineers any plugin or its state. Plugin state blobs are opaque: saved and restored, never parsed.

**Read first:**
- `AGENTS.md`: the contract log and the local layout;
- `docs/DESIGN.md` §5 (the agent interface) and §6 (plugins);
- `docs/milestones/M3.md`;
- `crates/ph-plugin-host` (scan, instances, state, `main_thread.rs`, `embed.rs`);
- the `plugin.*` commands in `crates/ph-commands` (`add_instrument`, `set_param`, `open_editor`/`close_editor`, `get_params`);
- `crates/ph-ui/src/plugin_ui.rs`;
- `crates/ph-control` (views, resources, `get_state`);
- `crates/ph-analysis`;
- the M2 plugin tests and the test plugins in `test-plugins/`.

## Goal
The owner loads an instrument once in a plugin's own editor (e.g. a Kontakt library), then saves it to a **catalogue**. From then on, an agent can do all of this through MCP:
1. browse or search the catalogue;
2. add an entry to a channel;
3. know how to play it: range, keyswitches, articulations, useful CCs;
4. write parts;
5. render and analyze the result.

## Scope
1. **The catalogue store.** A per-user folder, outside any project: default `<user data dir>/Patternhaus/catalogue/`, configurable, and documented in `docs/formats/catalogue.md`. Each entry has:
   - the plugin id and format, the plugin version at save time, and the opaque state blob;
   - a name, tags (instrument family, library, vendor) and a free-text description;
   - a **playing guide**: playable range, keyswitches (key → articulation name), CCs (cc → meaning, e.g. CC1 dynamics, CC11 expression), velocity behaviour notes, and whether the instrument is mono or legato.

   Entries are plain TOML plus the blob, so the owner can edit them by hand. The store never goes into a project or the repo. Projects using an entry carry their own copy of the state, as plugins already do.
2. **Commands** (`catalogue.*`):
   - `save`: from a channel's live plugin state, with name, tags and guide;
   - `list` and `search`: by text and tags;
   - `get`: full entry, including the guide;
   - `add_to_channel`: create a plugin instrument channel restored from the state;
   - `update_guide`, `remove`, and `rename`.

   All of them have schema and behaviour tests, and the coverage guard covers them.
3. **Agent assistance for guides.** Add a `catalogue.probe` command. It renders short probe notes through the instrument offline (a chromatic sweep at a few velocities; a keyswitch sweep if candidate keyswitch keys are given) and uses `ph-analysis` to propose the playable range (keys that produce sound), and which keys are silent (likely keyswitches). The result is a proposal the agent or owner confirms into the guide. It must work headlessly, without opening the plugin's editor.
4. **UI:**
   - a "Save to catalogue…" action on plugin instrument channels;
   - a catalogue panel in the browser (list, search, drag onto the channel rack);
   - a simple guide editor.

   Follow the existing UI patterns and the offscreen UI tests.
5. **MCP:** a `patternhaus://catalogue` resource, `get_state` showing the guide for catalogue-backed channels, and the server instructions mentioning the catalogue.
6. **Robustness:** an entry whose plugin is missing, or whose version changed, is reported plainly. Restoring state into a different plugin version is attempted, with a warning. Loading never blocks the audio thread.

## Tests
Use the repo's test CLAP and VST3 plugins (their state must round-trip) for: save, list, add-to-channel, render equal to the source channel, and probe proposing the right range (give a test plugin a limited key range if it needs one, with a logged additive change). **Never load third-party plugins in tests.**

**Owner check, written into the manual checklist (`docs/manual-tests/M3-catalogue.md`):** load a Kontakt library in Kontakt's editor, save it to the catalogue, then have an agent add it and play a part.

## Rules
- Never open visible windows in tests, and never open real devices.
- **Build load:** at most 4 compile jobs, one build at a time, and tests with `--test-threads 2`. Other agents are building in parallel.
- **Checks:** `fmt`, `clippy -D warnings`, `test --no-fail-fast`, `xtask check`, `xtask notices --check` and `cargo deny check` all pass.
- **Commits:** commit increments with `git commit -s` and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge or push.

## Report back
- the commands and their test names;
- the store format;
- how probing performs on the test plugins;
- the contract changes;
- the commit list.
