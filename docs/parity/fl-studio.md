# FL Studio parity tracker

A feature is **done** only when both columns are ticked (DESIGN §1):
- **Wired:** a Patternhaus command implements it. Write the command ids.
- **Behaves:** its behaviour was checked against FL's observed behaviour. Write the date and how, or "differs: …" for deliberate differences.

Behaviour is observed by using FL as a normal user. Never decompile it.

Sources for the *Behaves* column:
- **manual:** Image-Line's public FL Studio manual, with the citations in [`docs/formats/fl-behaviour-notes.md`][notes].
- **owner check pending:** what the manual does not settle. The checklist items OC-n are in [`owner-checks.md`](owner-checks.md); hands-on UI behaviour is in [`M1-ui.md`](../manual-tests/M1-ui.md). Fill in the date once checked.
- **differs:** a deliberate difference, with the reason.

## M1: Make a Beat

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Channel rack | Add / remove / rename / colour channel | `channel.add` `channel.remove` `channel.rename` `channel.set_color` `channel.clone` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Channel rack | Channel volume, pan, mute, solo | `channel.set_volume` `channel.set_pan` `channel.set_mute` `channel.solo` (solo is session state) | 2026-10-10, manual: 100% = 0 dB, default 78% = -5.2 dB, Circular pan law (-3 dB at centre); implemented as `v^2.409` + sin/cos [notes] §1-2. Curve between anchors and hard-pan far side: owner check pending (OC-1..OC-4) |
| Channel rack | Route channel to mixer insert | `channel.set_insert` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Channel rack | Reorder channels | `channel.move` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Step sequencer | Toggle step | `step.toggle` `step.set` `step.get_row` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Step sequencer | Per-step velocity / pitch | `step.set_velocity` `step.set_pitch` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Step sequencer | Pattern length in steps | `pattern.set_length` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Step sequencer | Swing | `project.set_swing` | 2026-10-10, manual: even 16ths are delayed, odd ones hold [notes] §6. Maximum offset: owner check pending (OC-11, OC-12). Per-channel swing: M2 |
| Piano roll | Draw / delete / move / resize notes | `note.add` `note.remove` `note.move` `note.resize` `note.update` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Piano roll | Select, copy, paste, duplicate | `note.select_range` `note.copy` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Piano roll | Lasso select (Alt+drag), box select (Ctrl+drag) | UI selection only: no command until you act on the selection | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Piano roll | Slice tool | `note.update` (length of the head) + `note.add` (the tail) as one batch, one undo step | Cuts at the grid-snapped click; Alt = no snap |
| Piano roll | Mute tool | `note.update { muted }` | A muted note stays and is not played |
| Piano roll | Quantize | `note.quantize` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Piano roll | Transpose | `note.transpose` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Piano roll | Note velocity / pan | `note.set_velocity` `note.update` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Piano roll | Snap settings | UI view setting: `ph-ui` snap box (`piano_roll::SNAPS`); notes it places go through `note.add` `note.move` `note.resize` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Patterns | Add / clone / rename / delete pattern | `pattern.add` `pattern.clone` `pattern.rename` `pattern.remove` `pattern.move` `pattern.set_color` `pattern.clear_channel` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Playlist | Place / move / resize / delete pattern clips | `playlist.add_clip` `playlist.move_clip` `playlist.resize_clip` `playlist.remove_clip` `playlist.fill` | manual: a resized clip is a window over its pattern [notes] §5; whether a clip longer than its pattern loops: owner check pending (OC-10). Patternhaus plays silence past the end |
| Playlist | Track mute | `playlist.set_track_mute` `playlist.rename_track` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Playlist | Pattern mode vs song mode | `transport.set_mode` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Playlist | Loop region | `transport.set_loop` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Mixer | Insert volume / pan / mute / solo | `mixer.set_volume` `mixer.set_pan` `mixer.set_mute` `mixer.solo` `mixer.rename` | manual: fader is post-effects, mixer pan merges the far side into the near one (not balance) [notes] §3-4. Fader dB scale, default and pan coefficients: owner check pending (OC-6..OC-9); Patternhaus keeps linear gain and balance until then |
| Mixer | Sends | `mixer.add_send` `mixer.remove_send` `mixer.set_send_level` `mixer.set_route_to_master` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Mixer | Effect slots (add / remove / bypass / mix) | `mixer.add_effect` `mixer.remove_effect` `mixer.move_effect` `mixer.set_effect_bypass` `mixer.set_effect_mix` `mixer.set_effect_param` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Mixer | Sidechain routing | `mixer.set_sidechain` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Transport | Play / stop / position / tempo | `transport.play` `transport.stop` `transport.toggle` `transport.record` `transport.seek` `transport.get_state` `project.set_tempo` `project.set_time_signature` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Transport | Metronome | `transport.set_metronome` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Instruments | Sampler (one-shot, loop) | processor `ph.sampler` (`ph_instruments::create`); `channel.add` `channel.set_instrument` `channel.set_param` | manual: Sampler channels follow the Circular pan law; default level -8.2 dB (78% volume + centre pan) [notes] §2, matched exactly. Sound: owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Instruments | Haus Synth | processor `ph.synth` (`ph_instruments::create`); `channel.add` `channel.set_instrument` `channel.set_param` | differs: Patternhaus's own synth, no FL counterpart (DESIGN §7) |
| Effects | EQ, compressor, limiter, reverb, delay, filter, saturator | processors `ph.fx.eq`, `ph.fx.compressor`, `ph.fx.limiter`, `ph.fx.reverb`, `ph.fx.delay`, `ph.fx.filter`, `ph.fx.saturator` (`ph_effects::create`); `mixer.add_effect` `mixer.set_effect_param` | differs: Patternhaus's own effects (DESIGN §7); the sidechain and mix behaviour are pinned by golden renders (`tests/golden/`) |
| Keyboard | Typing keyboard to piano | `ph-ui` `typing` (two-row FL layout, octave shift `-` `=`) -> `Engine::preview_note` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Keyboard | F5–F9 window toggles, FL default shortcuts | `crates/ph-ui/keymap.default.toml`: `app.toggle_panel` (F5 playlist, F6 rack, F7 piano roll, F8 browser, F9 mixer), `transport.toggle` (Space), `history.undo` `history.redo` (Ctrl+Z, Ctrl+Y), `file.save` (Ctrl+S), `pattern.clone` via `app.clone_pattern` (Ctrl+L), `note.copy` (Ctrl+B), `note.quantize` (Ctrl+Q) | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Keyboard | Remappable keymap file | TOML keymap, user file layered on defaults, in-app rebind dialog | differs: TOML keymap file (FL has no keymap file) |
| Files | New / open / save / save-as project | `project.new` `file.open` `file.save` `file.save_as` | differs: project folder with `project.json`, not `.flp` (DESIGN §10) |
| Files | Export WAV / FLAC | `file.export_audio` (renders through `Host::render`) | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Files | MIDI import / export | `file.import_midi` `file.export_midi` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| Files | Open an FL Studio project (`.flp`) as a new project, with an import report | `file.import_flp` (also by dropping a `.flp`); `ph_formats::flp` | 2026-10-11: synthetic tests, fuzzed, local corpus diff (see M2 status); import goes to `<name> (Patternhaus)/` beside the file |
| History | Undo / redo | `history.undo` `history.redo` `history.undo_author` `history.undo_txn` `history.list` | differs: per-author (DESIGN §3) |
| UI | Floating windows, snapping, maximize, instrument and effect windows, multi-monitor windows, saved layouts | `ph-ui` `workspace` (pure window-manager model) and `windows` (egui rendering): floating, overlapping, snapping windows; maximize; detach to an OS window; layouts per user and per project | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| UI | Drag and drop files from the OS: audio onto the channel rack or playlist makes Sampler channels, onto a channel row replaces its sample; .mid onto the playlist or piano roll imports it; folders onto the browser become library folders; project folders open | `ph-ui` `dnd`; `sample.import`, `file.import_midi`, `file.open`; `Host::import_file` | 2026-10-10: offscreen tests; owner check pending ([M1-ui](../manual-tests/M1-ui.md) item 12) |
| UI | Dropped audio is copied into the project's `samples/` folder (FL keeps references to the original location) | `sample.import`; `EngineHost::import_file` | portable-project choice, DESIGN section 10 |
| UI | Browser: factory kits, presets, project and library files; click to preview; drag to the rack | `ph-ui` `panels::browser`; `channel.add` | 2026-10-10: preview plays outside the project, after the master (`Engine::audition`); feel: owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| UI | Step velocity view | `step.set_velocity` | owner check pending ([M1-ui](../manual-tests/M1-ui.md)) |
| UI | Dark and light themes, user theme files | TOML tokens, `ph-ui` `theme` | differs: Patternhaus themes (DESIGN §9) |
| UI | History panel: per-author undo, undo of one transaction, conflict dialog | `history.undo` `history.undo_author` `history.undo_txn` (mode `stop` / `skip` / `force`) | differs: per-author (DESIGN §3) |
| UI | Agent activity: flash and badge on agent edits, `ui.select` highlight, grant prompt hook | `ui.open_panel` `ui.close_panel` `ui.select` `ui.show_piano_roll` `ui.notify`; `AgentDirectory` hook | differs: no FL counterpart (agent interface, DESIGN §5) |

[notes]: ../formats/fl-behaviour-notes.md
