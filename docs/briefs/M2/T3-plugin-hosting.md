# Brief M2/T3: Plugin hosting

**Repo:** `E:\patternhaus`. Work in the worktree `E:\patternhaus-wt\m2t3` on branch `m2/t3-plugin-hosting` (`git worktree add E:\patternhaus-wt\m2t3 -b m2/t3-plugin-hosting main`).
**Read first:**
- `AGENTS.md`: the rules, the layering, and the contract log;
- DESIGN §6 (plugins) and §10 (project folder, plugin state files);
- `docs/milestones/M2.md`: "Plugin hosting", acceptance criteria 1, 4 and 6;
- `docs/formats/m2-dependencies.md`;
- `crates/ph-plugin-host` (the T0 API: `PluginFormatHost`, `PluginInstance`, `PluginEditor`, `PluginState`, `PluginNode`, `FaultHandle`, `fake.rs`);
- `crates/ph-node`;
- how `ph-engine` instantiates instruments and effects;
- how `ph-ui` opens plugin and effect windows.

## Scope

1. **CLAP host** on `clack-host` and `clack-extensions` (MIT/Apache). Cover:
   - instruments and effects;
   - audio ports;
   - note ports;
   - params (enumerate, get/set, automation and modulation events from `ph-node` events, including CC, bend and expression);
   - state save/load;
   - latency;
   - tail;
   - `gui`.
2. **VST3 host** on the `vst3` crate (MIT/Apache). Cover:
   - `IComponent`/`IAudioProcessor`/`IEditController`;
   - the parameter list and changes;
   - events;
   - state (component plus controller);
   - `IPlugView`.

   **CLAP first.** If VST3 can't be finished, land what works and list the gaps.
3. **Scanner:**
   - standard per-OS search paths plus user paths;
   - one plugin at a time with a timeout;
   - a cache keyed by path, mtime and size;
   - a blocklist;
   - `ScanReport` with failures.

   Scan in a **child process** (`patternhaus-cli plugin-scan` or a small helper binary), so a plugin crashing at load can't kill the app or the scan.
4. **Engine integration:**
   - `InstrumentSpec`/`EffectSpec` plugin variants (from T0; add them if missing) instantiate through `ph-plugin-host`;
   - the render plan treats plugins like builtins;
   - latency is reported;
   - offline render works.

   Third-party plugins are exempt from bit-exactness. **The repo's test plugins are not.**
5. **Crash catch** (DESIGN §6; full isolation is M4):
   - wrap every plugin call at the boundary in whatever the platform allows (Windows SEH via a small `cc` shim or `microseh`-style approach if the licence is permissive; a signal handler or `catch_unwind` elsewhere);
   - on a fault the node outputs silence, `FaultHandle` records it, an engine event is emitted, and the plugin is marked *isolate* in the scan cache/blocklist;
   - the session keeps running.

   Test it with the crash-on-demand plugin.
6. **Test plugins in the repo**, built by cargo so CI never needs third-party plugins:
   - a simple synth;
   - a gain;
   - a crash-on-demand plugin.

   CLAP at minimum, e.g. with `clack-plugin` (MIT/Apache) or a minimal hand-written CLAP entry. A VST3 test plugin too if feasible. Put them under `tools/` or `test-plugins/`, excluded from release artifacts.
7. **State:** plugin state is saved as separate files in the project folder (DESIGN §10, e.g. `plugins/<node>.state`) and survives save and reopen. Test with the test synth.
8. **Commands and agents:**
   - `plugin.scan`, `plugin.list`, `plugin.add` (instrument or mixer effect), `plugin.set_param`, `plugin.get_params`, `plugin.open_editor` and `plugin.close_editor`. Reuse `channel.set_instrument`/`mixer.add_effect` with the plugin spec where that fits better.
   - Agents can drive any plugin by parameter id through `query.node_params`.
   - Every command gets schema and behaviour tests, a contract-log row and a parity row.
9. **UI (minimal):**
   - a plugin picker in the add-instrument and add-effect menus, from the scan cache;
   - the generic parameter window (reuse the catalog-descriptor knob window);
   - an "Open plugin editor" action that embeds the plugin GUI in a floating workspace window via `raw-window-handle` on Windows, or falls back to a floating native window.

   Embedding only has to work on Windows for now; list the macOS/Linux status.
10. **Local compatibility report.** A `patternhaus-cli plugin-scan --report docs/parity/plugin-report.md` mode. **Don't commit the owner's report** (it lists their installed plugins). Write it to `E:\patternhaus-reports\plugin-report.md` instead and summarise it for me (counts by format and status, no vendor list needed).

## Rules

- **No visible windows** in tests or scans. Editor-window code is exercised only by unit tests with fakes. Never open a plugin GUI or the real audio device yourself; the owner uses the desktop.
- **Scanning the owner's installed plugins is allowed** (child process, headless). Never load a plugin GUI.
- **Licences:** nothing GPL. Check every new crate with `cargo deny check` and re-run `cargo xtask notices`. Steinberg's VST3 SDK is MIT since 3.8; don't vendor older GPL SDK headers.
- **Never circumvent copy protection** or licence checks in plugins.
- **Real-time:** the plugin call boundary on the audio thread must not allocate on our side. Plugins themselves are measured, not trusted.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo xtask check` and `cargo deny check` pass. Test plugins must build on Windows, Linux and macOS; note anything only CI can prove.
- **At most 2 local agents run at once.** Another agent (T1 audio engine) works in `E:\patternhaus-wt\m2t1` at the same time. Don't touch `ph-engine/src/audio_clips.rs` or the stretch code. Keep shared-file edits (`Cargo.toml`, `ci.yml`, `AGENTS.md` contract log, `M2.md`) small and additive.
- **Commits:** commit with `git commit -s` and end messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't merge and don't push.

## Report back

- what works for CLAP and for VST3 (a table);
- the scanner and crash-catch design, with the test result of the crash plugin;
- the state round-trip;
- the commands;
- the UI status per OS;
- the local plugin-report summary;
- test counts and the commit list;
- known gaps.
