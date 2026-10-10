# M1 manual test: make a beat by hand (criteria 2 and 3)

For the owner. About 30 minutes. This is the same story the scripted agent tells in criterion 1,
done with the mouse and FL-compatible shortcuts. Everything below was also exercised headlessly
(`cargo test -p ph-ui`); this script is for what only ears and eyes can judge.

## Start the app

```powershell
cd E:\patternhaus-wt\t5          # or main, once merged
cargo run --release -p ph-app -- C:\Users\you\beats\first   # a new folder is fine
```

Options:

| Flag | What it does |
|---|---|
| `[project_dir]` | Project folder. If it holds a `project.json` it is opened, otherwise a new project is made with `project.new`. Default `.` |
| `--buffer N` | Audio buffer in frames. Default **256**, which is what criterion 2 wants |
| `--silent` | No audio device (simulated engine). Without this flag, no device also falls back to silent, with a notice in the transport bar |
| `--demo-agent` | A scripted agent, `agent:demo`, edits the project every 4 s. Used in part 3 |
| `--assets DIR` | Factory content folder. Default: `assets/` next to the exe, or found by walking up from it |
| `--library DIR` | An extra sample folder for the browser (repeatable) |
| `--config DIR` | Where keymap, layout and theme are kept. Default `%APPDATA%\patternhaus` |

Expected: a dark window with the transport bar on top, the browser on the left, the channel rack
above the playlist and piano roll tabs, and the mixer, inspector, history and agents tabs below.

## Part 1: build the beat

1. **Browser (F8).** Open "Factory samples", then `drums`, then `haus808`. Click `kick`: you hear it
   (the browser plays it through a temporary channel; nothing is added to the project).
2. **Drag `kick` onto the channel rack.** A "Kick" channel appears. Drag `snare` and
   `hat_closed` the same way (double-click works too). Expected: 3 sampler channels.
3. **Haus Synth.** Open "Haus Synth presets", then `bass`, and double-click `sub`. A "Sub"
   channel appears. Rename a channel with right-click on its name, Rename.
4. **Step sequencer (F6).** Click steps to make a pattern. Click and drag across a row to paint.
   Right-drag erases. Four on the floor is steps 1, 5, 9, 13 on the kick. Press the Play button or
   **Space**: the pattern loops in pattern mode and the playhead outline runs along the steps.
   Turn on "Velocity" in the rack toolbar and drag a lit step up or down to change its velocity.
5. **Piano roll (F7).** Click the synth channel's name in the rack to select it (double-click also
   brings the piano roll forward). In the roll:
   - left-click draws a note, right-click deletes, drag moves, drag the right edge resizes;
   - Alt+E selects (or Ctrl+drag a box in Draw), **Ctrl+B** duplicates, **Ctrl+Q** quantizes,
     Up/Down transposes, Shift+Up/Down moves an octave, Ctrl+C / X / V copy, cut, paste;
   - drag the thin bars in the velocity lane;
   - the Snap box changes the grid; Ctrl+wheel zooms time, Alt+wheel zooms keys, wheel scrolls;
   - click a key on the left to hear it.
   Write a 4-note bass line. Other channels' notes show faintly as ghost notes.
6. **Type notes.** With the roll or rack in front, press `Z S X D C V` and `Q 2 W 3 E R`: they play
   the selected channel as a piano. `-` and `=` shift the octave (shown in the "Keys oct" button).
   `Ctrl+T` turns the typing keyboard off.
7. **Mixer (F9).** Click insert 4 (the synth's insert, shown in the rack's number box). In the
   panel on the right choose "+ Add effect": **eq**, then **compressor**. Click an effect to see its
   knobs. Drag a knob, release: the value is set (double-click resets it). Click "Byp" to bypass.
   On insert 5 add a **reverb**. Then click the snare's insert (2), choose "+ Add send", pick
   insert 5, and drag the send knob to about -8 dB. Optional: set the compressor's sidechain with the
   "Sidechain from" box on insert 4.
8. **Playlist (F5).** Select the beat pattern in the transport's pattern box. In the playlist, click
   track 1 at bar 1 to place a clip; the clip is one pattern long and snaps to bars. Click along to
   place 8 bars, or place one and press **Ctrl+B** to duplicate. Drag to move, drag the right edge to
   resize, right-click to delete. Create a second pattern with the `+` next to the pattern box
   (or Ctrl+Shift+N), write the bass in it, and place it on track 2 from bar 3.
9. **Arrange and play live.** Press **L** (or SONG) for song mode, click the playlist ruler to
   seek, and press Space. Shift-drag on the ruler sets a loop region; the "Loop" button turns the
   loop on or off.
10. **Live check.** Leave it playing at the 256-frame buffer for **5 minutes** while you move
    things around (edit steps, drag clips, turn knobs). In the transport bar:
    - "DSP" should stay well under 50% (budget 25% on the reference project);
    - the overload counter ("N overloads") should not appear. Expected: zero dropouts.
    If a "Use a larger buffer?" notice shows, note it and write down when.
11. **Save, reopen, export.**
    - **Ctrl+S** saves into the project folder. The status bar loses "unsaved changes".
    - File, "Open project…" (Ctrl+O) with the same folder: everything comes back as it was and the
      history starts empty.
    - **Ctrl+R** (File, Export audio…): `exports/mix.wav`, Export. A toast says "Exported". Open the
      WAV in anything and check that it sounds like what you heard. Exporting again to the same name
      is refused until you tick "Replace the file if it exists".

## Part 2: shortcuts, remapping, themes, layouts

- **F5 F6 F7 F8 F9** show the Playlist, Channel rack, Piano roll, Browser and Mixer. When one is
  already in front, the key hides it. F10 Inspector, Ctrl+H History, Ctrl+Shift+A Agents.
- **Ctrl+Z / Ctrl+Y** undo and redo *your own* changes. Ctrl+S saves, Ctrl+N new, Ctrl+O open,
  Ctrl+L clones the selected pattern, Space plays and stops, L switches pattern and song mode,
  Ctrl+M toggles the metronome.
- View, "Keyboard shortcuts…": click a binding's key, press the new key. Save writes
  `keymap.toml`. The default file is `crates/ph-ui/keymap.default.toml`; every binding names a
  registry command id or an `app.*` UI action.
- View, Theme: Dark or Light. User themes are TOML files of tokens (see
  `crates/ph-ui/themes/dark.toml`); put `theme = "C:/path/to/my-theme.toml"` in
  `settings.toml` in the config folder.
- **Layouts.** Drag a tab to dock it elsewhere or float it. Right-click a tab, "Move to its own
  window" makes a real OS window you can put on another monitor; close that window and the panel
  docks again. View, Layout: "Save for me" (also saved on exit), "Save with this project"
  (`layout.ron` in the project folder, which takes priority), "Reset to default".
- **DPI.** Move the window between monitors of different scale: text and controls stay sharp.
  The theme's `scale` token multiplies on top of the OS scale.

## Part 3: co-editing (criterion 3)

Start with the demo agent and open a project that has a pattern and a channel:

```powershell
cargo run --release -p ph-app -- C:\Users\you\beats\first --demo-agent
```

1. Open the **piano roll** on the first channel of the first pattern. Every 4 seconds `agent:demo`
   adds a note. Expected: the new note **flashes pink for 2 seconds with a small "demo" badge**, and
   the roll outlines the note the agent pointed at (`ui.select`). A toast says what it did.
2. Keep working (add your own notes, change velocities). The agent keeps going; neither of you
   blocks the other.
3. Open the **History** tab (Ctrl+H). Rows are tagged `human` (blue) or `agent:demo` (pink), with the
   time. "Undo mine" reverts only your last change. "Undo all by agent:demo" reverts only the
   agent's changes, newest first. The "Undo" button on a row reverts that one transaction.
   Expected after "Undo all by agent:demo": your notes remain, the agent's are gone.
4. **Conflict.** Let the agent add a note, then change *that same note* (drag it, or change its
   velocity). Click "Undo all by agent:demo". Expected: undo **stops** and the "Undo stopped:
   conflicts" dialog lists the note, the fields, and who changed it ("human"). Nothing has changed
   yet.
   - **Cancel**: nothing happens.
   - **Skip these objects**: everything else the agent did is undone; your edited note stays.
   - **Force**: your edit is overwritten too, and the note goes away.
   (Try each; repeat step 4 for each choice.)
5. **Agents tab (Ctrl+Shift+A).** Without the T4 endpoint it says "The local agent endpoint is not
   running in this build" and shows activity per author. With T4 merged (see the AGENT HOOK in
   `crates/ph-app/src/main.rs`) it lists connected clients with scopes and shows the grant prompt
   (Allow / Read only / Deny) when a new client connects.

## What to look for (the things tests cannot judge)

- The 10 000-note piano roll (make one with the scripted agent or MIDI import) scrolls and zooms
  smoothly; frame time measured offscreen was about 1.4 ms on the CPU.
- Meters and playhead move smoothly and the audio does not stutter while you drag things.
- Clicking in the browser plays the sample at once; typing-keyboard notes have no noticeable delay.
- Anything that looks off in the dark or light theme.

Report anything unexpected with the steps that led to it and the contents of the History tab.
