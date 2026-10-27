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

## M2: plugin hosting (T3)

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Plugins | Plugin manager: find installed plugins, a list with status, a blocklist | `plugin.scan` `plugin.list`; `patternhaus-cli plugin-scan --report` (each file in the `ph-plugin-scan` helper process, cache by path, mtime and size) | differs: CLAP and VST3 only (FL also hosts VST2, AU); crashed or hung files are blocked until they change. Owner report: local only |
| Plugins | Add a plugin instrument / a plugin effect to a mixer slot | `plugin.add_instrument` `plugin.add_effect` (also `channel.set_instrument` / `mixer.add_effect` with a plugin spec); UI: "+ Channel > Plugins", "+ Add effect > Plugins" | 2026-10-13: test plugins (`ph-control` plugin tests, M2 acceptance test). Your own VST3s: owner check pending (OC-25) |
| Plugins | Plugin parameters, automation by parameter, the generic parameter view | `plugin.set_param` `plugin.get_params` `query.node_params { plugin }`; the plugin window's knob grid; `Param` events are sample-accurate CLAP param events / VST3 parameter points | differs: VST3 continuous parameters are shown normalized 0..1 (the plugin's own text shows the plain value) |
| Plugins | Open the plugin's own editor | `plugin.open_editor` `plugin.close_editor`; embedded in the floating window on Windows, floating elsewhere | 2026-10-13: the editor request reaches the UI (`ph-control` plugin tests). Embedding with your plugins: owner check pending (OC-25) |
| Plugins | Plugin state saved with the project | `file.save` writes `plugins/<name>.state` (component + controller state for VST3) | 2026-10-13: test synth state survives save and reopen (`ph-control` plugin tests) |
| Plugins | A crashing plugin does not take the session down | crash guard (SEH / signals) in `ph-plugin-host`; the node goes silent, `plugin_fault` event, status *isolate* | differs: FL runs bridged plugins out of process; Patternhaus isolates crashed plugins (out-of-process loading is M4) |

## M2: Make a Song

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Playlist | Audio clips play with pattern clips in song mode: trim either edge, slip, split, clip gain, fade in/out with curve shapes, reverse, pitch (semitones and cents), track mute | `audio.add_clip` `audio.set_range` `audio.split` `audio.set_gain` `audio.set_fade` `audio.set_reverse` `audio.set_pitch` `playlist.*`; engine audio-track node (`ph_engine::player`) | 2026-10-13: engine tests (offsets, fades per curve, reverse, pitch, mute, seek and loop inside clips, live = offline). Differs: fade shapes are four fixed curves (linear, exp, log, S), not FL's tension knob; owner check pending (OC-13) |
| Playlist | Time-stretch modes: resample (pitch follows speed), stretch (pitch kept), slice (re-time at transients) | `audio.set_stretch`; Signalsmith Stretch (MIT) via `ph-stretch` | 2026-10-13: objective stretch tests (M2.md, "Stretch quality (T1)"). Differs: three modes, not FL's list of stretch algorithms; owner listening sign-off pending (OC-14, M2 criterion 5) |
| Playlist | Tempo-synced audio follows the project tempo, including tempo automation | `audio.set_stretch { tempo_sync }`; `tempo` automation clips build the engine's tempo map | 2026-10-13: engine tests (clicks land on the beats through a tempo change); 2026-10-27: M2 acceptance test (a 100 BPM loop fitted into a 124 BPM song). Owner check pending (OC-15) |
| Playlist | Detect a sample's tempo; fit a clip to the project tempo in one step | `sample.detect_bpm` `audio.fit_to_tempo` `sample.set_bpm` | 2026-10-13: generated test set (M2.md, BPM detection). A tempo the user set is never overwritten. Owner check pending against FL's tempo detection on the same loops (OC-16) |
| Slicer | Detect transients (sensitivity) for slicing | `slicer.detect` | 2026-10-13: generated material tests. Owner check pending (OC-17) |
| Slicer | Build a sliced Sampler instrument, one slice per key, with an option to write a pattern that replays the loop | `slicer.make_instrument`; `InstrumentSpec::Sampler.slices` | 2026-10-19: instrument known-answer test, command tests, end-to-end render of the replayed loop matches the original (correlation > 0.9). Differs: slices live on consecutive keys from a root key and play once, FL's Slicex has per-slice envelopes. Owner check pending (OC-17) |
| Audio | Edison-style editor: cut, copy, paste, trim, normalise, fade in and out, reverse, silence, gain, de-click; never overwrites | `editor.open` `editor.apply`; `ph_ui::audio_editor` | 2026-10-19: operation tests with exact results, the original file is byte-identical after every edit, one undo step. Differs: no spectral editing; recording goes through the transport's REC (T8), not the editor. Owner check pending (OC-18) |
| Playlist | Duplicate an audio clip (Ctrl+B) | `audio.duplicate` | 2026-10-19: command test |
| Playlist | Audio clips drawn with a waveform, cached per zoom level; drag in from Explorer or the browser; trim either edge, slip (Alt+drag), split (slice tool), move, delete; fade handles with curve shapes; gain, reverse, pitch, stretch mode, BPM and Fit to tempo from the right-click menu | `ph_ui::playlist::audio`, `ph_ui::waveform`; `audio.add_clip` `audio.set_range` `audio.split` `audio.set_fade` `audio.set_gain` `audio.set_pitch` `audio.set_reverse` `audio.set_stretch` `audio.fit_to_tempo` `sample.set_bpm` `sample.detect_bpm` | 2026-10-19: offscreen pointer tests (every gesture is one command and one undo step), frame-time test with 24 clips. Differs: right-click on an audio clip opens its menu (Delete is in it) instead of deleting; no per-clip colour yet; the tool is a Split toggle, not the C key. Owner check pending ([M2-audio-ui](../manual-tests/M2-audio-ui.md) §1-3, OC-13) |
| Audio | Audio editor window: waveform with zoom and scroll, selection, audition of the selection, edit buttons, a second row for the slicer (sensitivity, markers you add, move and delete, Make instrument) | `ph_ui::panels::audio_editor`; `editor.*`, `slicer.*` | 2026-10-19: offscreen pointer tests (select, trim, undo walks the window back, copy/paste, markers, instrument). Differs: no record button yet (disabled, T8); FL's Edison has a spectrum and loop-tuning tools we do not; the Rec button is still disabled in the editor (record from the transport). Owner check pending ([M2-audio-ui](../manual-tests/M2-audio-ui.md) §4-5, OC-18) |
| Agents | `snapshot` shows an audio clip's waveform | `snapshot { kind: waveform, clip }` | 2026-10-19: ph-control test |
| Audio | Long audio files stream from disk | engine prefetch thread (`ph_engine::stream`), `EngineStats.underruns` | n/a: engine internals (DESIGN §4), no user-facing FL setting is mirrored |

## M2: effects (T2)

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Effects | Multiband compressor (3 bands, crossovers, per-band threshold, ratio, attack, release, gain, solo, bypass) | processor `ph.fx.multiband`; `mixer.add_effect` `mixer.set_effect_param` | differs: Patternhaus's own effects (DESIGN §7), no FL plugin clone; 2026-10-11: known-answer tests (`ph-effects` `known_answer_m2`), in the M2 acceptance song (2026-10-27) |
| Effects | Gate / expander | processor `ph.fx.gate`; `mixer.add_effect` `mixer.set_effect_param` | differs: own effect (DESIGN §7); 2026-10-11: known-answer tests |
| Effects | Convolution reverb with generated impulse responses (room, hall, plate, spring) | processor `ph.fx.convolution` (the IR is processor state); `mixer.add_effect` `mixer.set_effect_param` | differs: own effect, IRs generated by `tools/soundgen` (Fruity Convolver ships recorded IRs); 2026-10-11: known-answer tests, in the M2 acceptance song |
| Effects | Chorus, flanger, phaser | processors `ph.fx.chorus` `ph.fx.flanger` `ph.fx.phaser`; `mixer.add_effect` `mixer.set_effect_param` | differs: own effects (DESIGN §7); 2026-10-11: known-answer tests |
| Effects | Bitcrusher, stereo tools (width, M/S, balance, phase) | processors `ph.fx.bitcrush` `ph.fx.stereo`; `mixer.add_effect` `mixer.set_effect_param` | differs: own effects (DESIGN §7); 2026-10-11: known-answer tests |
| Effects | Spectrum, loudness (momentary, short-term) and phase analysers as an effect window | processor `ph.fx.analyzer` (pass-through, bit-exact); its effect window reads that slot's own feed (`Engine::analyzer_feed`) | differs: one analyser effect instead of Wave Candy and Fruity dB Meter; 2026-10-11: BS.1770 known-answer tests; 2026-10-27: each analyser window shows its own slot. Readings against FL's meters: owner check pending (OC-26) |

## M2: automation (T7)

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Automation | Automation clips in the playlist, with points and curve types (linear, hold, smooth, S-curve) on any instrument, effect, plugin, mixer, channel or send parameter and on tempo | `automation.add_clip` `automation.add_point` `automation.move_point` `automation.set_curve` `automation.remove_points` `automation.set_target`; moving, resizing and deleting are `playlist.*`; the engine plays them sample-accurately (`ph_engine::automation`) | 2026-10-19: engine tests (every target kind, block sizes 64/512/37, live = offline bit-exact, no allocation under swaps). Differs: curves are four fixed shapes with a tension bend, not FL's free tension handles; where clips overlap the latest-starting one wins (FL's rule: owner check pending, OC-19) |
| Automation | A parameter returns to its knob value when automation stops | engine: events and gain buffers fall back to the project's value at a clip's end, on stop and when a clip is deleted | 2026-10-19: engine tests. Owner check pending against FL (does FL jump back at the clip end or keep the last value? OC-20) |
| Automation | "Create automation clip" from a knob's right-click menu | UI on every knob, fader and parameter grid control: `automation.add_clip` over the loop region, else the selected clips, else the bar at the playhead, on the first free track; opens its editor | 2026-10-19: UI tests. Owner check pending (OC-21) |
| Automation | The clip's curve is drawn in the playlist; knobs wear a ring and follow the value while playing | `ph-ui` playlist and widgets | 2026-10-19: UI tests, pictures in both themes. Owner check pending ([M2-audio-ui](../manual-tests/M2-audio-ui.md), OC-21) |
| Automation | Automation clip editor: add, move and delete points, change a curve by right-click, snap | `automation.add_point` `automation.move_point` `automation.remove_points` `automation.set_curve`; a floating window per clip (`WindowId::Automation`) | 2026-10-19: UI tests. Differs: no tension handles or point selection box yet |
| Automation | Record automation from knob moves while playing | transport row record button (automation), `automation.record` writes the thinned samples as one undoable clip | 2026-10-19: UI tests and command tests. Differs: while a knob is dragged the audio follows after release (knobs commit on release), so you record by ear against what already plays |

## M2: recording and MIDI (T8)

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Recording | List audio inputs; arm one for a playlist track or a Sampler channel | `record.list_inputs` `record.arm` `record.disarm`; REC and IN in the transport row; cpal input behind `InputProvider` | 2026-10-19: engine, command and UI tests on simulated inputs; the real device path is checked only on the owner's interface ([M2-recording-midi](../manual-tests/M2-recording-midi.md)). Differs: inputs are listed as channel pairs (`In 1-2`) |
| Recording | Record audio; each take is a new WAV in `samples/` (never overwriting) placed as an audio clip at the record position, one undo step | `record.start` `record.stop`; the writer thread appends the file while recording | 2026-10-19: tests (bit-exact files, numbering past existing files, undo). Differs: FL records into the Edison / playlist depending on mode; Patternhaus always places a clip on the armed track, or replaces the Sampler's sample for a channel target |
| Recording | Input monitoring through the target, with a latency readout | `record.set_monitor`, `record.list_inputs` (latency), the monitor feed into the target's mixer insert | 2026-10-19: tests (audible through the insert, silent when off, never in offline renders, live = offline with monitoring off). Latency figures are measured from the device timestamps: owner check pending (OC-23) |
| Recording | Latency compensation | takes keep the whole file; the clip plays from `source_offset = round-trip latency` | 2026-10-19: tests on a simulated device latency. Differs: FL shifts the recording itself; here nothing is trimmed from the file. `record.start { latency_ms }` overrides the measurement. Owner check pending with a loopback (OC-22) |
| Recording | Count-in and punch in / out over the loop region | `record.start { count_in_bars, punch }`; "Punch in and out over the loop region" under IN | 2026-10-19: tests (the song waits and clicks, capture starts at the first frame, exactly the punch range is kept) |
| Recording | WASAPI exclusive mode | not wired: cpal 0.18 has no exclusive-mode switch for WASAPI; the `asio` build covers exclusive access | follow-up (see M2.md T8 notes) |
| MIDI | MIDI keyboard plays the selected channel, with hot-plug | `midi.list_inputs`; `ph_engine::midi` (midir) routes notes to the preview path | 2026-10-19: tests on a fake MIDI source (notes, release, pitch, hot-plug); owner check pending with a keyboard (OC-24) |
| MIDI | Mod wheel, pitch bend and channel pressure reach the instrument | `ph-node` events `Cc`, `PitchBend`, `NoteExpression::Pressure`; the Haus Synth follows the wheel and the bend | 2026-10-19: tests (bend moves the pitch). Pressure has no listener in the built-in instruments yet |
| MIDI | Record played notes into the pattern (pattern mode), optional quantise, one undo step | `record.start` / `record.stop { quantize }` | 2026-10-19: tests. Differs: notes are placed at the audio thread's last published position (within one block) |
| MIDI | MIDI learn on any knob; mappings per user | knob right-click "MIDI learn…"; `midi.learn` `midi.map` `midi.unmap` `midi.list_mappings`; `<config>/midi-mappings.json` | 2026-10-19: UI and command tests. Differs: the mapping is by project object id, so it applies to whatever has that id in another project. Owner check pending (OC-24) |
| MIDI | Mapped controllers record as automation while armed | the mapped move is written through `automation.record` like a knob drag | 2026-10-19: UI test |

[notes]: ../formats/fl-behaviour-notes.md
