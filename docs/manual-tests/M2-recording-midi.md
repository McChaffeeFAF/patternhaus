# M2 manual test: recording and MIDI (criterion 2, by hand)

For the owner. About 40 minutes, with your audio interface and a MIDI keyboard. Everything
below was also exercised headlessly on **simulated inputs and a fake MIDI source**
(`cargo test -p ph-engine -p ph-commands -p ph-control -p ph-ui`); no test ever opens a real
audio or MIDI device. This script is for what only your hardware can prove: that the device
paths work, and that the latency compensation lands on the beat.

## Start the app

```powershell
cd E:\patternhaus
cargo run --release -p ph-app -- C:\Users\you\beats\recording   # a new folder is fine
```

Plug the interface and the keyboard in first. Keep the buffer at 256 frames (the default).
Mappings you learn are saved in `%APPDATA%\patternhaus\midi-mappings.json` (or the `--config`
folder), not in the project.

## Part 1: audio inputs, monitoring and the latency readout

1. In the transport row click **IN**. The popup lists your interface's inputs as pairs
   (`<device>: In 1-2`, `In 3-4`, ...). Expected: your interface appears within a second;
   **Refresh** lists it again; unplugging it and pressing Refresh removes it.
2. Under "Record onto" keep "a playlist track", Track 1. Tick your interface's input to arm it.
   Expected: a **Monitor** box, a level bar and `-> track 1` appear. Make a noise: the bar moves.
   If the bar is dead, check the Windows sound settings for the device's sample rate: it must be
   48 kHz (the popup's error says so).
3. Tick **Monitor** and play or sing. Expected: you hear yourself through the master. Untick it
   and you do not. With headphones on, no feedback.
4. Read the **Latency** line: input, output and "hearing yourself" in milliseconds. Expected: a
   few ms each at a 256-frame buffer (around 5.3 ms of buffer per direction at 48 kHz, plus
   whatever the driver adds). Write the numbers down.
5. Put a **reverb on insert 0** (master) and monitor again. Expected: the monitored input goes
   through the effect (it enters the target's insert: the master for a track, the channel's
   insert for a Sampler channel).

## Part 2: record a take, count-in, punch

1. Add a pattern with a four-on-the-floor kick and a loop clip for four bars. Turn the
   metronome on. In IN choose **Count-in: 1 bar**.
2. Click **REC**. Expected: the metronome counts one bar (the song waits), then the song plays
   and the REC button stays lit. Clap on every beat for two bars.
3. Click **Stop** (or REC again). Expected: a toast `recorded 1 take`; an audio clip on track 1
   starting at the position where recording began; the file `Take 1.wav` in
   `<project>\samples`. **Ctrl+Z** removes the clip and the registry entry as one step (the
   WAV stays on disk); **Ctrl+Y** brings them back.
4. **Latency check (the one only you can do).** Zoom the playlist on the clip and open it in a
   sample editor or look at the waveform: the claps should sit on the beat lines, within about
   a millisecond or two. If they are consistently early or late, note by how much (ms). With a
   loopback cable (an output looped into the armed input), record the metronome and measure the
   offset; if it is not near zero, the measured compensation is off for your driver. The
   override is `record.start { "latency_ms": <value> }` (Agents window, MCP or CLI); tell the
   orchestrator the figure so a setting can be added.
5. Record again: the file is `Take 2.wav`. A take is **never** written over an old one, also
   when you record the same input twice in a row or reopen the project.
6. **Punch.** Set a loop region over bars 2 and 3 (shift-drag the playlist ruler), tick
   "Punch in and out over the loop region" in IN, and start recording from bar 1. Expected:
   the song plays from the start, recording begins exactly at bar 2 and ends at bar 4, and the
   new clip covers only that range (monitoring keeps going outside it).
7. **Dropouts.** Record a minute with a busy project at the 256 buffer. Expected: no clicks in
   the recorded file, and the toast does not mention dropped frames. If it does, write down
   what else was running.
8. **Sampler channel target.** Under "Record onto" choose "a Sampler channel" and a sampler
   channel, arm, record a note, stop. Expected: the channel now plays your recording (its
   sample is replaced; its old sample stays in the project's registry).

## Part 3: MIDI keyboard

1. Select a Haus Synth channel in the rack. Play the keyboard. Expected: sound at once, with no
   noticeable lag; keys release cleanly. In IN, the **MIDI devices** list shows your keyboard
   with a filled dot; unplug it and replug it: it reconnects within about a second without
   restarting.
2. Select another channel and play: the new channel sounds ("Keys play: ..." in IN).
3. Move the **mod wheel** and **pitch bend** on a held note. Expected: the Haus Synth's bend
   follows the wheel across its range (2 semitones by default) and returns to centre. The mod
   wheel moves whatever the patch routes to it (try a preset with a mod-wheel route).
   Aftertouch has no listener in the built-in instruments yet.
4. **Record notes.** Switch to **PAT** mode with a pattern selected, click **REC**, play a
   short phrase, click **Stop**. Expected: the notes appear in the piano roll at the beat you
   played them (to within a block), as one undo step. Repeat with `record.stop { quantize: 240
   }` through the Agents window or CLI: the starts snap to the 16th grid.

## Part 4: MIDI learn

1. Right-click any knob (a Haus Synth cutoff, the channel volume). Expected: the menu shows
   **MIDI learn…** next to "Create automation clip". Click it: a toast asks you to move a
   control.
2. Move a knob or slider on your controller. Expected: a toast `CC <n> on <device> now moves
   <control>`, and moving it now moves the knob on screen. The audio follows each step.
3. Right-click the same knob: **Remove MIDI mapping (CC n)** is offered; **Cancel MIDI learn**
   appears while a learn is waiting.
4. Close the app and reopen it: the mapping is still there (it lives in
   `%APPDATA%\patternhaus\midi-mappings.json`, not in the project). Note the limit: a mapping
   names a project object (`channel/3/volume`), so in another project it moves whatever has
   that id (or nothing, if there is none).
5. Click the red **automation record** button (the circle next to the metronome), play the
   song, and move the mapped controller. Stop. Expected: an automation clip for that control
   appears on the playlist, thinned to a sensible number of points.

## Part 5: things to try to break

- Arm an input, then unplug the interface. Expected: nothing crashes; the next record or
  Refresh reports the input is gone.
- Start recording with nothing armed in song mode. Expected: a toast explaining what to arm.
- Switch the device's sample rate to 44.1 kHz in Windows and arm it. Expected: a message that
  names the rate mismatch (the app records at the engine rate only).
- Hold the Space bar during a recording. Expected: the transport pauses, the take stays open;
  **Stop** ends it and the take is kept.

## What this does not cover yet

- **WASAPI exclusive mode.** cpal 0.18 has no exclusive-mode switch for WASAPI. Exclusive input
  exists only in the `asio` build. Adding WASAPI exclusive needs either cpal support or a small
  binding of our own: a follow-up.
- Input rate conversion: the interface must run at the engine rate (48 kHz).
- Recording during overload shedding is not specially handled; the take flags dropped frames.

Record the results in `docs/parity/fl-studio.md` (the "M2: recording and MIDI" rows) once you
have run this.
