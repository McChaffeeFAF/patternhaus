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

Expected: a dark window (Neon Slate) with the top bar and toolbar above, the browser docked on the left and the hint bar along the bottom. The
playlist fills the rest of the window as the background, and the channel rack floats over it at the
upper left. The piano roll, mixer, history and agents are hidden until you press their keys.

## Part 1: build the beat

1. **Browser (F8).** On the SAMPLES tab open `drums`, then `haus808`. Click `kick`: you hear it
   (the browser plays it through a temporary channel; nothing is added to the project).
2. **Drag `kick` onto the channel rack.** A "Kick" channel appears. Drag `snare` and
   `hat_closed` the same way (double-click works too). Expected: 3 sampler channels.
3. **Haus Synth.** On the PRESETS tab open `bass` and double-click `sub`. A "Sub"
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
   the selected channel as a piano. `-` and `=` shift the octave (shown beside the typing-keyboard button in the transport row).
   `Ctrl+T` turns the typing keyboard off.
7. **Mixer (F9).** Click insert 4 (the synth's insert, shown in the rack's number box). In the
   panel on the right choose "+ Add effect": **eq**, then **compressor**. Click an effect to open its
   window of knobs. Drag a knob, release: the value is set (double-click resets it). Click the power icon on a slot (hover: "Bypass") to bypass it.
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
    - **Ctrl+S** saves into the project folder. The "UNSAVED" mark at the right of the hint bar goes away.
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
- View, Theme: Neon Slate or Red Line, switched live. Your own themes are TOML files in the
  `themes` folder of the config folder, and show up in the same menu: `docs/theming.md`.
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

## Part 4: the T5c rough-edge fixes

1. **Scrolling.** Open a Haus Synth instrument window and drag its bottom edge up until it is
   small. Expected: a scroll bar appears and the mouse wheel scrolls; knob rows wrap onto new lines
   instead of running off the right edge. The mixer, History and Agents windows scroll in both
   directions. The piano roll, playlist and channel rack keep their own panning and zooming, with
   their toolbars in place.
2. **Effect windows.** In the mixer, open two effects of one insert (click each slot). The title
   bars read like "Insert 4 - EQ". Drag a window over the other's title: the text slides to the
   part of the bar that is still visible. Use the up and down arrow buttons on the slots (or have an agent
   run `mixer.move_effect`, or press Undo): each window follows its effect and keeps its place.
   Removing an effect closes only its window.
3. **Agents open windows.** From any MCP client run `ui.open_panel` with `instrument:<channel id>`
   or `effect:<insert>:<slot>`; `ui.select` a channel; `ui.show_piano_roll`. The windows open and
   the channel and pattern are selected.
4. **Escape.** Open two instrument windows, click the channel rack, press Escape: the front-most
   instrument window closes (then the next one). Escape in a text box does not close anything.
5. **Solo.** Have an agent run `channel.solo` or `mixer.solo`. The S button lights up without
   touching the mouse.
6. **Agents window.** Connect an agent. It is listed at the top with its scopes, how long it has
   been connected, how many calls it has made and how long it has been idle. Disconnect it: it
   moves to "Remembered, not connected", greyed out. While a grant prompt is open, other clients
   can still connect and work.
7. **Piano roll tools.** Alt+drag in empty space draws a loop; the notes inside it are selected
   (Ctrl+drag is still a box). Slice tool: click a note, it splits at the snapped position (one
   Undo puts it back). Mute tool: click or drag across notes; muted notes are drawn washed out and
   are silent; click again to unmute.
8. **Quit with changes.** Make an edit and close the window (or File, Quit, New project, Open
   project). Expected: "Save / Don't save / Cancel". Cancel keeps everything as it was; Save
   writes the project and then goes ahead; Don't save goes ahead and discards.

## Part 5: the visual pass (T8)

Neon, themes and polish. Everything here has an offscreen test or screenshot
(`docs/screenshots/M1/neon-*.png`, `red-*.png`); this is for the things only eyes can judge.

1. **First impression.** Start the app. A loading screen with the mark and wordmark shows for
   about a second (click or press a key to skip it), then fades to the workspace. The window
   title reads `<project> - Patternhaus` with a dot while there are unsaved changes, and the
   window and taskbar icon is the pad-grid mark.
2. **Themes.** View, Theme, Red Line: the whole app recolours at once (wine-black, red accent,
   cyan for agents). Switch back to Neon Slate. Make `themes/mint.toml` as in `docs/theming.md`
   and check it appears in the menu.
3. **Type.** Labels, menus, tabs and buttons are uppercase condensed; names you typed (channels,
   patterns, presets) keep their case; tempo, position, dB and timings are in the mono face.
4. **Hint bar.** Hover knobs, faders, buttons, tabs, the browser tree, menu entries: the bar
   at the bottom shows the control name, its current value and its gestures. With an agent
   connected, `get_state { view: "ui" }` returns the same hint.
5. **Right-click.** Right-click a knob or fader: Reset to default, Automate (disabled), MIDI
   learn (disabled), Help (shows the hint as a toast). Buttons and toggles have the last three.
6. **Window chrome.** The front window has an accent edge and a soft glow that fades in over
   120 ms; windows behind it have neither and a dim title. The `...` button in a title bar (or
   right-click the title) opens that window options: maximize, detach, reset size and
   position, help and close, plus the window own settings (the roll ghost notes, snap and
   zoom, the rack velocity view, the playlist snap).
7. **Toolbar.** Right-click the toolbar (or View, Customise toolbar...): hide items, move them
   earlier or later, reset. Save the layout (View, Layout, Save for me), restart: it is as you
   left it.
8. **Controls.** Drag a knob: its arc eases, glows and shows its value above it. The meters
   fall off smoothly and hold their peaks for about 1.5 s; a clip lights the red LED until you
   click the meter. Step pads: alternate groups of four are shaded, velocity sets the opacity,
   a hit flashes bright with a glow, the playhead column is outlined, and pads an agent edited
   tint with the agent colour for 2 s.
9. **Reduce motion.** View, Reduce motion: flashes, easing, glow fades, the playhead trail and
   toast slides all become jumps. Nothing else changes. Leave the app idle for a minute with
   it open: CPU stays near zero (nothing animates, nothing repaints but a slow poll).
10. **1/64.** In the piano roll set Snap to 1/64 and draw a run of notes. Zoom out and in with
    the wheel: grid lines fade in per tier (bar, beat, 1/16, 1/32, 1/64) once they are about
    6 px apart. A 1/64 note at the normal zoom is a thin sliver you can still grab (8 px
    target). Zoom presets: the magnifier in the toolbar, or Ctrl+0 (fit), Ctrl+1 (1 bar),
    Ctrl+2 (1 beat), Ctrl+3 (1/64 detail); Ctrl+= and Ctrl+- step through them.
11. **About.** Help, About Patternhaus: the mark, the version and the licence text, including
    the three bundled fonts (SIL Open Font Licence 1.1).

12. **Drag and drop.** From Windows Explorer, with the app open (and a project that has a few channels):
    - Drag three .wav files over the channel rack: the rack lights up with the accent edge and glow, the hint bar says "Drop to add 3 samples as channels". Drop: three Sampler channels named after the files, in order. Undo (Ctrl+Z) removes all three at once. Check `samples/` in the project folder has the copies.
    - Drag one .wav over a channel row: only that row lights, "Drop to replace Kick's sample". Drop: the sample changes, the name does not.
    - Drop a file with the same name but different content: it is copied as `name (2).wav`. Drop the very same file twice: no second copy.
    - Drag audio over the playlist: the hint says audio clips arrive later and that it adds channels.
    - Drag a .mid over the playlist or piano roll: "MIDI: drop to import as a new pattern".
    - Drag a folder over the browser: it lights; drop adds it on the Library tab. Restart the app: the folder is still there.
    - With unsaved changes, drag a project folder (or its project.json) anywhere: Save / Don't save / Cancel appears; with none, it opens.
    - Drop a .txt or .m4a: a warning toast names the file and why it was skipped. A corrupt .wav is skipped and named, the good ones still import.
    - Windows may not report the pointer position during a drag from Explorer. If so, the row replace cannot be aimed; a single file then adds a channel instead. Tell us which you see.

## What to look for (the things tests cannot judge)

- The 10 000-note piano roll (make one with the scripted agent or MIDI import) scrolls and zooms
  smoothly; frame time measured offscreen is about 1.3 ms on the CPU (release build, under the 8 ms budget).
- Meters and playhead move smoothly and the audio does not stutter while you drag things.
- Clicking in the browser plays the sample at once; typing-keyboard notes have no noticeable delay.
- Anything that looks off in either theme (Neon Slate or Red Line).

Report anything unexpected with the steps that led to it and the contents of the History window.
