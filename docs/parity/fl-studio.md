# FL Studio parity tracker

A feature is **done** only when both columns are ticked (DESIGN §1):
- **Wired:** a Patternhaus command implements it. Write the command ids.
- **Behaves:** its behaviour was checked against FL's observed behaviour. Write the date and how, or "differs: …" for deliberate differences.

Behaviour is observed by using FL as a normal user. Never decompile it.

## M1: Make a Beat

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Channel rack | Add / remove / rename / colour channel | `channel.add` `channel.remove` `channel.rename` `channel.set_color` `channel.clone` | |
| Channel rack | Channel volume, pan, mute, solo | `channel.set_volume` `channel.set_pan` `channel.set_mute` `channel.solo` (solo is session state) | |
| Channel rack | Route channel to mixer insert | `channel.set_insert` | |
| Channel rack | Reorder channels | `channel.move` | |
| Step sequencer | Toggle step | `step.toggle` `step.set` `step.get_row` | |
| Step sequencer | Per-step velocity / pitch | `step.set_velocity` `step.set_pitch` | |
| Step sequencer | Pattern length in steps | `pattern.set_length` | |
| Step sequencer | Swing | `project.set_swing` | |
| Piano roll | Draw / delete / move / resize notes | `note.add` `note.remove` `note.move` `note.resize` `note.update` | |
| Piano roll | Select, copy, paste, duplicate | `note.select_range` `note.copy` | |
| Piano roll | Lasso select (Alt+drag), box select (Ctrl+drag) | UI selection only: no command until you act on the selection | |
| Piano roll | Slice tool | `note.update` (length of the head) + `note.add` (the tail) as one batch, one undo step | Cuts at the grid-snapped click; Alt = no snap |
| Piano roll | Mute tool | `note.update { muted }` | A muted note stays and is not played |
| Piano roll | Quantize | `note.quantize` | |
| Piano roll | Transpose | `note.transpose` | |
| Piano roll | Note velocity / pan | `note.set_velocity` `note.update` | |
| Piano roll | Snap settings | UI view setting: `ph-ui` snap box (`piano_roll::SNAPS`); notes it places go through `note.add` `note.move` `note.resize` | |
| Patterns | Add / clone / rename / delete pattern | `pattern.add` `pattern.clone` `pattern.rename` `pattern.remove` `pattern.move` `pattern.set_color` `pattern.clear_channel` | |
| Playlist | Place / move / resize / delete pattern clips | `playlist.add_clip` `playlist.move_clip` `playlist.resize_clip` `playlist.remove_clip` `playlist.fill` | |
| Playlist | Track mute | `playlist.set_track_mute` `playlist.rename_track` | |
| Playlist | Pattern mode vs song mode | `transport.set_mode` | |
| Playlist | Loop region | `transport.set_loop` | |
| Mixer | Insert volume / pan / mute / solo | `mixer.set_volume` `mixer.set_pan` `mixer.set_mute` `mixer.solo` `mixer.rename` | |
| Mixer | Sends | `mixer.add_send` `mixer.remove_send` `mixer.set_send_level` `mixer.set_route_to_master` | |
| Mixer | Effect slots (add / remove / bypass / mix) | `mixer.add_effect` `mixer.remove_effect` `mixer.move_effect` `mixer.set_effect_bypass` `mixer.set_effect_mix` `mixer.set_effect_param` | |
| Mixer | Sidechain routing | `mixer.set_sidechain` | |
| Transport | Play / stop / position / tempo | `transport.play` `transport.stop` `transport.toggle` `transport.record` `transport.seek` `transport.get_state` `project.set_tempo` `project.set_time_signature` | |
| Transport | Metronome | `transport.set_metronome` | |
| Instruments | Sampler (one-shot, loop) | processor `ph.sampler` (`ph_instruments::create`); `channel.add` `channel.set_instrument` `channel.set_param` | |
| Instruments | Haus Synth | processor `ph.synth` (`ph_instruments::create`); `channel.add` `channel.set_instrument` `channel.set_param` | |
| Effects | EQ, compressor, limiter, reverb, delay, filter, saturator | processors `ph.fx.eq`, `ph.fx.compressor`, `ph.fx.limiter`, `ph.fx.reverb`, `ph.fx.delay`, `ph.fx.filter`, `ph.fx.saturator` (`ph_effects::create`); `mixer.add_effect` `mixer.set_effect_param` | |
| Keyboard | Typing keyboard to piano | `ph-ui` `typing` (two-row FL layout, octave shift `-` `=`) -> `Engine::preview_note` | |
| Keyboard | F5–F9 window toggles, FL default shortcuts | `crates/ph-ui/keymap.default.toml`: `app.toggle_panel` (F5 playlist, F6 rack, F7 piano roll, F8 browser, F9 mixer), `transport.toggle` (Space), `history.undo` `history.redo` (Ctrl+Z, Ctrl+Y), `file.save` (Ctrl+S), `pattern.clone` via `app.clone_pattern` (Ctrl+L), `note.copy` (Ctrl+B), `note.quantize` (Ctrl+Q) | |
| Keyboard | Remappable keymap file | TOML keymap, user file layered on defaults, in-app rebind dialog | |
| Files | New / open / save / save-as project | `project.new` `file.open` `file.save` `file.save_as` | |
| Files | Export WAV / FLAC | `file.export_audio` (renders through `Host::render`) | |
| Files | MIDI import / export | `file.import_midi` `file.export_midi` | |
| History | Undo / redo | `history.undo` `history.redo` `history.undo_author` `history.undo_txn` `history.list` | differs: per-author (DESIGN §3) |
| UI | Floating windows, snapping, maximize, instrument and effect windows, multi-monitor windows, saved layouts | `ph-ui` `workspace` (pure window-manager model) and `windows` (egui rendering): floating, overlapping, snapping windows; maximize; detach to an OS window; layouts per user and per project | |
| UI | Browser: factory kits, presets, project and library files; click to preview; drag to the rack | `ph-ui` `panels::browser`; `channel.add` | |
| UI | Step velocity view | `step.set_velocity` | |
| UI | Dark and light themes, user theme files | TOML tokens, `ph-ui` `theme` | |
| UI | History panel: per-author undo, undo of one transaction, conflict dialog | `history.undo` `history.undo_author` `history.undo_txn` (mode `stop` / `skip` / `force`) | differs: per-author (DESIGN §3) |
| UI | Agent activity: flash and badge on agent edits, `ui.select` highlight, grant prompt hook | `ui.open_panel` `ui.close_panel` `ui.select` `ui.show_piano_roll` `ui.notify`; `AgentDirectory` hook | |
