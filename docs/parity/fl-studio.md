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
| Piano roll | Quantize | `note.quantize` | |
| Piano roll | Transpose | `note.transpose` | |
| Piano roll | Note velocity / pan | `note.set_velocity` `note.update` | |
| Piano roll | Snap settings | | |
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
| Keyboard | Typing keyboard to piano | | |
| Keyboard | F5–F9 window toggles, FL default shortcuts | | |
| Files | New / open / save / save-as project | `project.new` `file.open` `file.save` `file.save_as` | |
| Files | Export WAV / FLAC | `file.export_audio` (renders through `Host::render`) | |
| Files | MIDI import / export | `file.import_midi` `file.export_midi` | |
| History | Undo / redo | `history.undo` `history.redo` `history.undo_author` `history.undo_txn` `history.list` | differs: per-author (DESIGN §3) |
