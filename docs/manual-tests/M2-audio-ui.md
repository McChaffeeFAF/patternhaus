# M2 manual test: audio clips, the audio editor and the slicer (criterion 2, in part)

For the owner. About 20 minutes. Everything below was also exercised headlessly
(`cargo test -p ph-ui --test audio_ui`); this is for what only ears and eyes can judge.
Use your own loop (a drum loop of one or two bars is ideal).

## 1. Put a loop on the playlist

1. Drag the loop from Explorer onto the playlist, onto track 3 at bar 2.
   *The hint bar says "Drop to place 1 sample as an audio clip", and a clip with a waveform appears
   where you dropped it.* It is copied into the project's `samples/` folder.
2. Drag a sample from the browser's Samples tab to track 4. *A ghost shows where it lands.*
3. Set the row height to Medium or Large in the playlist toolbar if the waveform is hard to see.

## 2. Edit the clip

| Do | Expect |
|---|---|
| Drag the right edge to the left | The clip gets shorter, the sound does not move. One undo step |
| Drag the left edge to the right | The clip starts later and the sound stays where it was on the timeline |
| Alt+drag inside the clip | The sound slides inside the clip (slip edit); the clip stays put |
| Drag the small square at a top corner | A fade; its curve is drawn over the waveform |
| Switch on **Split**, click in the clip | The clip is cut in two at the grid |
| Ctrl+B with the clip selected | A copy right after it, with the same settings |
| Right-click the clip | The settings menu: gain, pitch, reverse, mode, BPM, **Fit to tempo**, fade shapes, split, duplicate, open in editor, make sliced instrument, delete |

Press **Play** (song mode) after each change and listen: edges and fades should be click-free.

## 3. Fit to tempo

Right-click the clip, set the BPM (or **Detect**), choose **Fit to tempo**. Change the project tempo
and listen: the loop follows it. Try the three modes (Resample, Stretch, Slice).

## 4. The audio editor

Double-click the clip (or right-click, Open in audio editor).

1. Drag across the waveform to select. **Audition** plays the selection (or all of it).
2. **Trim**, then **Fade in**, **Fade out**, **Normalise**, **Reverse**, **De-click**. After each, the
   window shows the new file and the clip on the playlist plays it.
3. Press Ctrl+Z (or Undo in the toolbar): the clip goes back and the window shows the previous version.
4. Look in the project's `samples/` folder: every edit is a new file, the original is untouched.
5. **Rec** is greyed out until the recorder arrives (T8).

## 5. Slice a loop

1. In the editor press **Slicer**, then **Detect**. Markers appear on the transients; move the
   sensitivity slider and press Detect again for more or fewer.
2. Click in the strip under the waveform to add a marker, drag a marker to move it, right-click it to
   delete it. The keys under the markers (C2, C#2, ...) are the keys that play each slice.
3. Press **Make instrument**. A new Sampler channel appears, one slice per key, and (with the box
   ticked) a pattern that replays the loop. Put the pattern on the playlist and compare it with the
   original loop: it should sound the same at the loop's own tempo.
