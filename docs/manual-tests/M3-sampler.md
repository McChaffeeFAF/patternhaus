# M3 manual test: the Haus Sampler by hand (criterion 7)

For the owner, with the app running on your own audio device and MIDI keyboard. You need one
of your own instrument libraries in a supported format: SFZ, SF2/SF3, DecentSampler
(`.dspreset` or `.dsbundle`) or EXS24 (`.exs`), or a folder of samples whose file names
carry note names (`Piano_C4_f.wav`). Use a library with velocity layers and, if you have one,
keyswitched articulations.

1. **Import it.** Drag the instrument file from Explorer onto the **channel rack**. The hint
   bar says "Drop to import … as a Haus Sampler channel" while you hover.
   - A new channel appears and its **sampler window** opens.
   - An SF2/SF3 with several presets first asks which preset to import: pick one.
   - A folder of samples dropped on the rack first shows the **auto-map preview** (each
     file, its root key, keys and velocities). Check it, then **Import**. (A folder dropped on
     the browser is still added to the library, as before.)
   - Alternatively, add the library folder to the browser (drop it on the browser), find the
     file in the **Library** tab, click it to hear a few notes, and double-click it to import.
2. **Read the report.** Open the window's **Import report** tab. It lists what was
   approximated, skipped, or kept for later (scripts and skins, M3b). Check nothing you rely
   on is in the skipped list.
3. **Play it from your MIDI keyboard.** Select the channel in the rack (the keyboard plays the
   selected channel) and play across the range and the velocities. Listen for the velocity
   layers and the round robins (repeated notes vary).
4. **Switch articulations.** Play the library's keyswitch keys on your keyboard; the sound
   changes. In the window's **keyboard strip**, keyswitch keys are coloured by articulation;
   clicking one auditions it and highlights it as the current articulation (shown under the
   strip). In the **Articulations** tab, change which articulation is the default and check
   the instrument starts with it after the project is reopened.
5. **Edit a zone.** In the **zone map**, click a zone (Shift-click or drag a box for several).
   - Drag it one or two keys sideways; play: it moved, at the same pitch mapping.
   - Drag its right edge in: the zone covers fewer keys, the root key stays.
   - In the **Zone** tab, change its volume or tune; play to hear it.
   - Press **Ctrl+Z** after each edit: one press undoes the whole drag or edit.
   - Ctrl+wheel zooms the map, the wheel scrolls it; **Fit** shows the whole instrument.
6. **Save and reopen.**
   - **Collect into project** copies the library samples into the project (the "linked"
     badge disappears), so the project no longer needs the library folder.
   - **Save as Haus instrument…** writes `instrument.toml` and `samples/` to a folder you
     choose (relative to the project). Open the `instrument.toml` in a text editor: it is
     readable.
   - Save the project (Ctrl+S), close Patternhaus, start it again and open the project. The
     channel plays exactly as before, with your zone edit and default articulation.
   - **Load Haus instrument…** in the window loads the saved folder back onto the channel.
7. **Optional: your private Kontakt reader.** If your separately built Kontakt importer is
   installed (M3/T6 hook), drop an unencrypted `.nki` on the rack and repeat steps 2 to 6
   with it. Encrypted or licence-locked libraries must be refused with a plain message,
   never opened.

Report anything that sounds different from the library's own player, with the Import report
tab's contents, so the parity table (`docs/parity/haus-sampler.md`) can be updated.
