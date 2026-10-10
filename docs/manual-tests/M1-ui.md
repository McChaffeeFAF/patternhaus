# M1 manual test: make a beat by hand (criteria 2 and 3)

For the owner. About 30 minutes. This is the same story the scripted agent tells in criterion 1,
done with the mouse and FL-compatible shortcuts. Everything below was also exercised headlessly
(`cargo test -p ph-ui`); this script is for what only ears and eyes can judge.

## Start the app

From a source checkout:

```powershell
cd E:\patternhaus
cargo run --release -p ph-app -- C:\Users\you\beats\first   # a new folder is fine
```

On the owner's machine there's a prebuilt copy with a launcher. It finds the factory assets and opens
the checkpoint project folder:

```powershell
E:\patternhaus-bin\Patternhaus.bat                       # or: Patternhaus.bat <project_dir> --demo-agent
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

Expected: a dark window with the transport bar on top and the browser docked on the left. The
playlist fills the rest of the window as the background, and the channel rack floats over it at the
upper left. The piano roll, mixer, history and agents are hidden until you press their keys.

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
   panel on the right choose "+ Add effect": **eq**, then **compressor**. Click an effect to open its
   window of knobs. Drag a knob, release: the value is set (double-click resets it). Click "Byp" to bypass.
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

- **F5 F6 F7 F9** show the Playlist, Channel rack, Piano roll and Mixer, and **F8** the docked
  browser. A hidden window appears in front; one that is open but behind another comes to the front;
  one that is already in front is hidden. F10 opens the selected channel's instrument window,
  Ctrl+H History, Ctrl+Shift+A Agents. **Esc** closes the front instrument or effect window.
- **Ctrl+Z / Ctrl+Y** undo and redo *your own* changes. Ctrl+S saves, Ctrl+N new, Ctrl+O open,
  Ctrl+L clones the selected pattern, Space plays and stops, L switches pattern and song mode,
  Ctrl+M toggles the metronome.
- View, "Keyboard shortcuts…": click a binding's key, press the new key. Save writes
  `keymap.toml`. The default file is `crates/ph-ui/keymap.default.toml`; every binding names a
  registry command id or an `app.*` UI action.
- View, Theme: Dark or Light. User themes are TOML files of tokens (see
  `crates/ph-ui/themes/dark.toml`); put `theme = "C:/path/to/my-theme.toml"` in
  `settings.toml` in the config folder.
- **Windows.** Every panel is a floating window inside the workspace:
  - drag a title bar to move it; drag any edge or corner to resize it (there is a minimum size);
  - a window snaps to the workspace edges and to other windows when you bring it within a few
    pixels, and its title bar can never be dragged out of reach;
  - click anywhere on a window behind another to bring it to the front;
  - double-click a title bar (or use the square button) to maximize it inside the workspace;
    do it again to restore. The cross closes it. The first button sends it to its own OS window you
    can put on another monitor; close that OS window and it returns to the workspace;
  - the browser is docked at the left; drag its right edge to resize it, F8 to hide it.
- **Instrument and effect windows.** Click a channel's name in the rack: its instrument window
  (sampler or Haus Synth) opens. Click another channel and both stay open. In the mixer, click an
  effect slot: that effect's window opens with all its knobs. Each remembers where you put it.
  Deleting the channel (or removing the effect) closes its window.
- **Layouts.** View, Layout: "Save for me" (also saved on exit), "Save with this project"
  (`layout.ron` in the project folder, which takes priority), "Reset to default". A saved layout
  keeps every window's position, size, stacking order and whether it is open. Shrink the app window
  and the windows stay reachable.
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
3. Open the **History** window (Ctrl+H). Rows are tagged `human` (blue) or `agent:demo` (pink), with the
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
5. **Agents window (Ctrl+Shift+A).** Without the T4 endpoint it says "The local agent endpoint is not
   running in this build" and shows activity per author. With T4 merged (see the AGENT HOOK in
   `crates/ph-app/src/main.rs`) it lists connected clients with scopes and shows the grant prompt
   (Allow / Read only / Deny) when a new client connects.

## What to look for (the things tests cannot judge)

- The 10 000-note piano roll (make one with the scripted agent or MIDI import) scrolls and zooms
  smoothly; frame time measured offscreen was about 1.4 ms on the CPU.
- Meters and playhead move smoothly and the audio does not stutter while you drag things.
- Clicking in the browser plays the sample at once; typing-keyboard notes have no noticeable delay.
- Anything that looks off in the dark or light theme.

Report anything unexpected with the steps that led to it and the contents of the History window.
