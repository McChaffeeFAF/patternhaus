# Owner checks (FL Studio 21)

These checks cover what the public FL Studio manual does not settle, plus two
confirmations of what it does (see `docs/formats/fl-behaviour-notes.md`). Run them in
your own FL Studio 21 and type the readings into the Result lines. Values are read
from FL's own hint bar or peak meters; nothing is decompiled.

**Common setup (unless a check says otherwise):** File > New (empty project). Load a
0 dBFS mono sine (e.g. 1 kHz, 2 s) into a Channel Sampler, route it to Insert 1,
leave Insert 1 and Master faders at their defaults, panning law at the default.
Put one step on beat 1 and read the Master peak meter (hover for the hint bar), or
add Fruity dB Meter / Wave Candy to Master for exact numbers.

### Channel volume

- **OC-1 (confirm)** Channel volume knob at default. Read the knob hint value and the
  Master peak level.
  Result: knob ____ %   Master peak L ____ dB   R ____ dB
- **OC-2** Set channel volume to 100%. Read Master peak.
  Result: L ____ dB   R ____ dB
- **OC-3** Set channel volume to 50%, then 25%. Read Master peak each time.
  Result: 50%: ____ dB   25%: ____ dB

### Channel pan

- **OC-4** Channel volume 100%. Pan centre: read L and R. Then pan hard right
  (100% R): read L and R.
  Result: centre L ____ dB R ____ dB   hard right L ____ dB R ____ dB
- **OC-5** Options > Project general settings > Advanced: switch Panning law to
  Triangular, repeat OC-4 (centre and hard right). Switch back afterwards.
  Result: centre L ____ dB R ____ dB   hard right L ____ dB R ____ dB

### Mixer fader

- **OC-6** New project: hover Insert 1 fader and Master fader without moving them;
  nudge each by one step and back if needed to see the hint bar.
  Result: Insert default ____ dB   Master default ____ dB
- **OC-7** Drag Insert 1 fader to the very top. Read the hint bar.
  Result: max ____ dB (hint % ____)
- **OC-8** Set Insert 1 fader to 50% (hint bar), then 25%. Read the dB.
  Result: 50% = ____ dB   25% = ____ dB

### Mixer pan

- **OC-9** Replace the sine with a stereo file: left channel 0 dBFS sine, right
  channel silent. Channel volume 100%, channel pan centre. Pan Insert 1 to 50% R,
  then 100% R. Read Master L and R each time.
  Result: centre L ____ R ____   50% R: L ____ R ____   100% R: L ____ R ____ dB

### Playlist

- **OC-10** Make a 1-bar pattern (one kick on step 1). Place it on the Playlist and
  Shift+drag its right edge to 4 bars. Play from bar 1.
  Result: kick heard on bars: ____ (loop / silence / other: ____ )
  Also: does the clip show the repeated notes drawn inside it? ____

### Swing

- **OC-11** Short click sample on all 16 steps, tempo 120. Global swing 100%, channel
  swing 100%. Render to WAV (or zoom in the Edison recording) and measure the onset
  of step 2 relative to step 1 (a straight 16th at 120 BPM is 125 ms).
  Result: step 2 onset ____ ms after step 1   step 3 onset ____ ms (expect 250)
- **OC-12** Same, global swing 50% (channel 100%), then global 100% with channel 50%.
  Result: global 50%: step 2 at ____ ms   channel 50%: step 2 at ____ ms

## M2: Make a Song

These compare Patternhaus's M2 features with FL Studio 21 where only you can judge (FL's
own behaviour, your hardware, your plugins, your ears). Use generated or your own material;
nothing from these checks is committed. The hands-on Patternhaus side of each is in
[`M2-audio-ui.md`](../manual-tests/M2-audio-ui.md) and
[`M2-recording-midi.md`](../manual-tests/M2-recording-midi.md).

### Audio clips and tempo

- **OC-13** In FL, drop a drum loop on the playlist, drag its fade-in handle to one beat and
  try the fade tension. In Patternhaus do the same with each of the four curves (linear, exp,
  log, S). Is one of the four close enough to FL's default fade, and is a tension knob missed?
  Result: closest curve ____   tension needed? yes / no
- **OC-14** Project at 124 BPM, a 100 BPM loop. In FL set the clip to stretch (keep pitch) and
  to resample; in Patternhaus Fit to tempo (Stretch) and Resample. Listen to both pairs.
  Result: stretch quality vs FL: better / same / worse ____   resample identical? ____
- **OC-15** Add a tempo automation clip 124 → 140 over 4 bars with the loop playing tempo-synced.
  Does FL's clip follow the ramp the same way (stays on the beat, no jumps)?
  Result: ____
- **OC-16** Give FL and Patternhaus the same loops (e.g. 4 of yours: four-on-the-floor,
  breakbeat, half-time, one with swing). Compare the detected tempo.
  Result: loop / FL BPM / Patternhaus BPM: ____

### Slicer and audio editor

- **OC-17** Load the same loop in Fruity Slicer (or Slicex) and in the Patternhaus slicer at the
  default sensitivity. Compare the slice count and where the markers fall.
  Result: FL slices ____   Patternhaus slices ____   markers agree? ____
- **OC-18** In Edison, normalise a quiet file and read its peak; fade in a selection. Do the same in
  the Patternhaus editor.
  Result: Edison normalise peak ____ dB   Patternhaus ____ dB   fade shape alike? ____

### Automation

- **OC-19** Two automation clips on the same knob overlapping in time on two playlist tracks:
  which one does FL play in the overlap (the later-starting one, the lower track, ...)?
  Patternhaus plays the later-starting one.
  Result: ____
- **OC-20** An automation clip that ends before the song does: after its end, does FL hold the
  last value or return the knob to where it was? Patternhaus returns to the knob's value.
  Result: holds / returns ____
- **OC-21** Right-click a knob, "Create automation clip": where does FL put it and how long is
  it (loop region, selection, whole song)? Patternhaus: the loop region, else the selected
  clips, else the bar at the playhead, on the first free track.
  Result: ____

### Recording and MIDI

- **OC-22** Loopback: patch an output of your interface into an input, record a click track, and
  measure how far the recorded click sits from the original (Part 2 step 4 of
  `M2-recording-midi.md`). Do the same in FL.
  Result: Patternhaus offset ____ ms   FL offset ____ ms
- **OC-23** Read the monitoring latency both apps report for the same interface and buffer.
  Result: FL ____ ms   Patternhaus ____ ms
- **OC-24** MIDI keyboard on the selected channel, mod wheel and bend, and "Link to controller"
  in FL vs MIDI learn in Patternhaus on the same knob.
  Result: alike? ____   differences: ____

### Plugins

- **OC-25** Add three of your VST3 plugins (one instrument, two effects) in FL and in Patternhaus.
  Open each editor. Play a note through each. Save, close, reopen: same settings?
  Result: plugin / loads / editor embedded / state kept: ____

### Analysers

- **OC-26** Play a 1 kHz sine at -20 dBFS (and then one of your mixes) through Wave Candy or
  Fruity dB Meter in FL and through the Patternhaus analyser effect. Compare the short-term
  loudness and the spectrum shape.
  Result: FL ____ LUFS   Patternhaus ____ LUFS   spectra alike? ____
