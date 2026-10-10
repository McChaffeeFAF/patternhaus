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
