# M3 manual test: controller lane editor (T11)

For the owner, with the app running (and, for step 9, a real MCP client). The lane editor is the
strip under the piano roll's note grid: CC 0-119 and pitch bend per channel and pattern. Any
channel works for editing; to hear the result use a channel that responds to CCs (a Beast Sampler
instrument with a CC-mapped amplitude, or a hosted plugin such as Kontakt).

1. **Open the lane area.** Open a pattern in the piano roll. Under the note grid is a strip
   headed `Lanes -`, a lane selector and tabs. The default lane is **Velocity** (the old velocity
   editor, unchanged: drag a bar to change a note's velocity).
   - `Lanes -` collapses the area to its header (the note grid grows); `Lanes +` brings it back.
   - Drag the thin divider between the grid and the header up or down: the lane gets taller or
     shorter.
2. **Pick a lane.** Open the selector: Velocity, CC1 Mod Wheel, CC11 Expression, CC7 Volume, CC64
   Sustain, Pitch Bend, and **Other CC...** with a number (try 74, then **Open**). Lanes that
   already hold points are listed first, with their point counts, and show as small tabs
   (`CC1`, `Bend`) next to the selector; one click switches.
3. **Add and move points** in CC1.
   - Click on empty space: a point appears, snapped to the roll's grid (change **Snap** in the
     roll's toolbar and click again; hold **Alt** for no snapping). The header shows
     `bar.beat.tick  value` under the pointer in 0-127.
   - Drag the point: it moves in time and value. Release: one undo step (Ctrl+Z puts it back).
   - The line between points is the evaluated curve. It matches what you hear: play the pattern
     and watch the playhead; ramps are smooth, a Hold segment is a step.
4. **Draw freehand.** Drag over empty space: a curve follows the mouse, and on release it is
   thinned to a handful of points (a straight stroke becomes two). Ctrl+Z removes the whole
   stroke at once. Drawing back over a stroke replaces what is under it.
5. **Line tool.** Hold **Shift** and drag: a straight ramp from where you pressed to where you
   released (two points), replacing any points between them.
6. **Select and delete.**
   - **Ctrl + drag** on empty space draws a rubber band that selects the points inside it
     (add **Shift** to extend the selection); Shift or Ctrl + click toggles one point.
   - **Delete** or **Backspace** removes the selected points; **Ctrl+A** selects all points of
     the lane. These act on the lane's points only while the lane has focus (you clicked in
     it); click in the note grid and they act on notes again.
   - **Right-click a point** deletes it.
7. **Curves.** Right-click on a segment (empty space on the line, not on a point): a menu offers
   **Smooth, Linear, S-curve, Hold** for the segment that starts at the point before the click,
   and **Delete point**. (Shift + right-click on a point opens the same menu for it.) A small
   diamond sits on every Linear and S-curve segment: drag it up or down to bend the segment
   (tension). Each change is one undo step.
8. **Pitch bend.** Switch to **Bend**. Zero is the line in the middle of the lane; the readout is
   `-8192` to `+8191`. A flat lane at zero fills nothing; points above bend up and fill toward
   the middle line.
9. **Other authors.** With the lane for CC1 open, ask the agent: "add a swell on the mod wheel of
   this channel over two bars" (`cc.shape`). The points appear in the lane as it works (the tab
   appears if the lane was empty), ringed in the agent colour for a few seconds. Undo with
   `undo_mine` from the agent or Ctrl+Z from you: the lane follows. While you drag one of your own
   points, an agent change elsewhere on the lane does not interrupt the drag.
10. **Zoom and scroll.** Zoom the note grid (Ctrl + wheel) and scroll sideways (Shift + wheel):
    the lane's time axis moves with it, point for point. Wheel over the lane does the same.
11. **Save and reopen.** Save the project and reopen it: the lanes are as you left them; export
    MIDI and check the CC data in another tool (`docs/formats` has the export rules).
