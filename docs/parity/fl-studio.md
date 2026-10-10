# FL Studio parity tracker

A feature is **done** only when both columns are ticked (DESIGN §1):
- **Wired:** a Patternhaus command implements it. Write the command ids.
- **Behaves:** its behaviour was checked against FL's observed behaviour. Write the date and how, or "differs: …" for deliberate differences.

Behaviour is observed by using FL as a normal user. Never decompile it.

## M1: Make a Beat

| Area | Feature | Wired | Behaves |
|---|---|---|---|
| Channel rack | Add / remove / rename / colour channel | | |
| Channel rack | Channel volume, pan, mute, solo | | |
| Channel rack | Route channel to mixer insert | | |
| Channel rack | Reorder channels | | |
| Step sequencer | Toggle step | | |
| Step sequencer | Per-step velocity / pitch | | |
| Step sequencer | Pattern length in steps | | |
| Step sequencer | Swing | | |
| Piano roll | Draw / delete / move / resize notes | | |
| Piano roll | Select, copy, paste, duplicate | | |
| Piano roll | Quantize | | |
| Piano roll | Transpose | | |
| Piano roll | Note velocity / pan | | |
| Piano roll | Snap settings | | |
| Patterns | Add / clone / rename / delete pattern | | |
| Playlist | Place / move / resize / delete pattern clips | | |
| Playlist | Track mute | | |
| Playlist | Pattern mode vs song mode | | |
| Playlist | Loop region | | |
| Mixer | Insert volume / pan / mute / solo | | |
| Mixer | Sends | | |
| Mixer | Effect slots (add / remove / bypass / mix) | | |
| Mixer | Sidechain routing | | |
| Transport | Play / stop / position / tempo | | |
| Transport | Metronome | | |
| Instruments | Sampler (one-shot, loop) | | |
| Instruments | Haus Synth | | |
| Effects | EQ, compressor, limiter, reverb, delay, filter, saturator | | |
| Keyboard | Typing keyboard to piano | | |
| Keyboard | F5–F9 window toggles, FL default shortcuts | | |
| Files | New / open / save / save-as project | | |
| Files | Export WAV / FLAC | | |
| Files | MIDI import / export | | |
| History | Undo / redo | | differs: per-author (DESIGN §3) |
